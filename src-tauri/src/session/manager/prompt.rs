use super::binding::binding_matches;
use super::carrier::{acquire_carrier_mutation_permit, CarrierMutationPermit};
use super::{
    FinalizeMutation, LifecycleMutations, PendingCreateBinding, PendingPrompt, PromptKey,
    RetainedPrompt, SessionManager, SessionManagerState,
};
use crate::session::selection::payload::SelectionCause;
use std::collections::HashMap;
use uuid::Uuid;

pub(super) fn prompt_domain_exclusive(
    state: &SessionManagerState,
    key: &PromptKey,
    owner: Option<Uuid>,
) -> bool {
    state
        .prompt_keys
        .iter()
        .all(|(id, candidate)| Some(*id) == owner || candidate != key)
}

pub(super) fn apply_commit_prompt_domains(
    state: &mut SessionManagerState,
    mutations: &LifecycleMutations,
    cause: SelectionCause,
) {
    let mut predecessors: HashMap<_, _> = mutations
        .prompt_transfers
        .iter()
        .map(|(source, target)| (target.session_id, *source))
        .collect();
    // Fresh restart is also one succession, even though it transfers no text.
    if matches!(cause, SelectionCause::Restart(_)) {
        for mutation in &mutations.finalizations {
            let binding = match mutation {
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding
                }
            };
            if let Some(target) = state.pending_prompts.get(&binding.session_id) {
                if let Some(source) = mutations
                    .removals
                    .iter()
                    .find(|id| state.prompt_keys.get(id) == Some(&target.key))
                {
                    predecessors.insert(binding.session_id, *source);
                }
            }
        }
    }
    for (source, target) in &mutations.prompt_transfers {
        let latest = state
            .sessions
            .get(source)
            .and_then(|row| row.last_prompt.clone());
        state
            .sessions
            .get_mut(&target.session_id)
            .expect("validated pending target")
            .last_prompt = latest;
    }
    for mutation in &mutations.finalizations {
        let binding = match mutation {
            FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                *binding
            }
        };
        finalize_prompt_domain(
            state,
            binding.session_id,
            predecessors.get(&binding.session_id).copied(),
            !matches!(cause, SelectionCause::Restore),
        );
    }
}

pub(super) fn finalize_prompt_domain(
    state: &mut SessionManagerState,
    id: Uuid,
    predecessor: Option<Uuid>,
    allow_fresh_reopen: bool,
) {
    let Some(prepared) = state.pending_prompts.remove(&id) else {
        return;
    };
    let Some(row) = state.sessions.get(&id) else {
        return;
    };
    if !prepared.key.matches(row) {
        return;
    }
    let exclusive = prompt_domain_exclusive(state, &prepared.key, predecessor);
    let entry = state
        .retained_prompts
        .entry(prepared.key.clone())
        .or_default();
    let row = state.sessions.get_mut(&id).expect("pending row exists");
    if !exclusive {
        entry.blocked = true;
        entry.last_prompt = None;
    } else if !prepared.continuation && !prepared.fresh {
        // Admission reserves this pair for configured, unattributed identity.
        // A durable fresh marker cannot establish that external selector.
        entry.blocked = true;
        entry.last_prompt = None;
        row.last_prompt = None;
    } else if prepared.fresh || row.start_fresh_on_restore {
        state.prompt_seed_blocked.insert(id);
        // Startup fresh markers cannot disprove persisted duplicate recipes
        // whose manager rows have not been restored yet.
        if allow_fresh_reopen {
            entry.blocked = false;
        }
        entry.last_prompt = None;
        row.last_prompt = None;
    } else if prepared.continuation {
        if prepared.allow_retained_lookup
            && row.last_prompt.is_none()
            && !entry.blocked
            && state.prompts_reusable
        {
            row.last_prompt = entry.last_prompt.clone();
        }
        if !entry.blocked && row.last_prompt.is_some() {
            entry.last_prompt = row.last_prompt.clone();
        }
    }
    state.prompt_keys.insert(id, prepared.key);
    if let Some(source) = predecessor {
        state.prompt_keys.remove(&source);
    }
}

impl SessionManager {
    pub async fn set_last_prompt(&self, id: Uuid, prompt: String) {
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.last_prompt = Some(prompt.clone());
            state.prompt_seed_blocked.remove(&id);
            if let Some(key) = state.prompt_keys.get(&id).cloned() {
                let exclusive = prompt_domain_exclusive(&state, &key, Some(id));
                let entry = state.retained_prompts.entry(key).or_default();
                if exclusive && !entry.blocked {
                    entry.last_prompt = Some(prompt);
                } else {
                    entry.last_prompt = None;
                    entry.blocked = true;
                }
            }
        }
    }

    pub(crate) async fn install_retained_prompts(
        &self,
        entries: Option<HashMap<PromptKey, RetainedPrompt>>,
    ) {
        let mut state = self.state.write().await;
        state.prompts_writable = entries.is_some();
        state.prompts_reusable = entries.is_some();
        state.retained_prompts = entries.unwrap_or_default();
    }

    pub(crate) async fn retained_prompt_snapshot(
        &self,
    ) -> Option<HashMap<PromptKey, RetainedPrompt>> {
        let state = self.state.read().await;
        state
            .prompts_writable
            .then(|| state.retained_prompts.clone())
    }

    pub(crate) async fn disable_prompt_reuse(&self) {
        self.state.write().await.prompts_reusable = false;
    }

    pub(crate) async fn forget_retained_prompt(&self, key: &PromptKey) {
        self.state.write().await.retained_prompts.remove(key);
    }

    pub(crate) async fn block_retained_prompt_domains(&self, keys: Vec<PromptKey>) {
        let mut state = self.state.write().await;
        for key in keys {
            state.retained_prompts.insert(
                key,
                RetainedPrompt {
                    last_prompt: None,
                    blocked: true,
                },
            );
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 production paths use the explicit-permit initializer"
        )
    )]
    pub(crate) async fn initialize_pending_prompt(
        &self,
        binding: PendingCreateBinding,
        key: Option<PromptKey>,
        initial: Option<String>,
        continuation: bool,
        fresh: bool,
        allow_retained_lookup: bool,
    ) -> Result<(), String> {
        let permit = acquire_carrier_mutation_permit().await;
        self.initialize_pending_prompt_with_carrier_permit(
            &permit,
            binding,
            key,
            initial,
            continuation,
            fresh,
            allow_retained_lookup,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn initialize_pending_prompt_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        key: Option<PromptKey>,
        initial: Option<String>,
        continuation: bool,
        fresh: bool,
        allow_retained_lookup: bool,
    ) -> Result<(), String> {
        let mut state = self.carrier_state_write(permit).await;
        if !binding_matches(&state, binding) {
            return Err("pending prompt capability mismatch".to_string());
        }
        let row = state
            .sessions
            .get_mut(&binding.session_id)
            .ok_or_else(|| "pending prompt row missing".to_string())?;
        row.last_prompt = if continuation && !fresh {
            initial
        } else {
            None
        };
        if let Some(key) = key.filter(|key| key.matches(row)) {
            state.pending_prompts.insert(
                binding.session_id,
                PendingPrompt {
                    key,
                    continuation,
                    fresh,
                    allow_retained_lookup,
                },
            );
        }
        Ok(())
    }

    pub(crate) async fn seed_if_absent_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        id: Uuid,
        key: &PromptKey,
        prompt: String,
    ) -> bool {
        let mut state = self.carrier_state_write(permit).await;
        if state.pending_create.contains_key(&id)
            || state.prompt_seed_blocked.contains(&id)
            || !prompt_domain_exclusive(&state, key, Some(id))
            || state
                .retained_prompts
                .get(key)
                .is_some_and(|entry| entry.blocked)
        {
            return false;
        }
        let Some(row) = state.sessions.get_mut(&id) else {
            return false;
        };
        if !key.matches(row) || row.start_fresh_on_restore || row.last_prompt.is_some() {
            return false;
        }
        row.last_prompt = Some(prompt.clone());
        state.prompt_keys.insert(id, key.clone());
        state.retained_prompts.insert(
            key.clone(),
            RetainedPrompt {
                last_prompt: Some(prompt),
                blocked: false,
            },
        );
        true
    }
}
