use super::carrier::{acquire_carrier_mutation_permit, CarrierMutationPermit};
use super::{CodingAgentBinding, PendingCreateBinding, SessionManager, SessionManagerState};
use uuid::Uuid;

pub(super) fn clear_failed_recipe_for_runtime_binding(
    state: &mut SessionManagerState,
    runtime: Uuid,
) {
    if let Some(id) = state.coding_agent_bindings.get(&runtime).cloned() {
        state.failed_binding_recipes.remove(&id);
    }
}

pub(super) fn retire_coding_agent_binding(state: &mut SessionManagerState, runtime: Uuid) {
    if let Some(binding) = state.coding_agent_bindings.remove(&runtime) {
        if !state
            .coding_agent_bindings
            .values()
            .any(|id| id == &binding)
            && !state.failed_binding_recipes.contains_key(&binding)
        {
            state.binding_seeds.remove(&binding);
            state.binding_origins.remove(&binding);
        }
    }
}

pub(super) fn binding_matches(state: &SessionManagerState, binding: PendingCreateBinding) -> bool {
    state.pending_create.get(&binding.session_id).copied() == Some(binding.nonce)
}

impl SessionManager {
    /// Only pending rows accept a birth binding; capture its seed before insertion.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn install_pending_coding_agent_binding(
        &self,
        pending: PendingCreateBinding,
        binding: Option<CodingAgentBinding>,
    ) -> Result<(), String> {
        let permit = acquire_carrier_mutation_permit().await;
        self.install_pending_coding_agent_binding_with_carrier_permit(&permit, pending, binding)
            .await
    }

    pub(crate) async fn install_pending_coding_agent_binding_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        pending: PendingCreateBinding,
        binding: Option<CodingAgentBinding>,
    ) -> Result<(), String> {
        let mut state = self.carrier_state_write(permit).await;
        if !binding_matches(&state, pending) {
            return Err("pending coding-agent binding capability mismatch".to_string());
        }
        if let Some(binding) = binding {
            Uuid::parse_str(&binding.id).map_err(|_| "invalid_coding_agent_binding".to_string())?;
            if let Some(seed) = binding.seed.as_ref() {
                let valid_seed = ["identityRevision", "originalAgentId"].iter().all(|key| {
                    seed.get(key)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                });
                if !valid_seed {
                    return Err("invalid_coding_agent_binding_seed".to_string());
                }
            }
            if let (Some(before), Some(after)) =
                (state.binding_seeds.get(&binding.id), binding.seed.as_ref())
            {
                if before != after {
                    return Err("coding_agent_binding_seed_conflict".to_string());
                }
            }
            if let Some(seed) = binding.seed {
                state.binding_seeds.insert(binding.id.clone(), seed);
            }
            if let Some(origin) = binding.origin {
                state.binding_origins.insert(binding.id.clone(), origin);
            }
            state
                .coding_agent_bindings
                .insert(pending.session_id, binding.id);
        }
        Ok(())
    }

    pub(crate) async fn coding_agent_binding(&self, id: Uuid) -> Option<CodingAgentBinding> {
        let state = self.state.read().await;
        let binding = state.coding_agent_bindings.get(&id)?;
        Some(CodingAgentBinding {
            id: binding.clone(),
            seed: state.binding_seeds.get(binding).cloned(),
            origin: state.binding_origins.get(binding).cloned(),
        })
    }

    /// D09 enrolls receipt-affected failures; ordinary retention stays unchanged.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn refresh_failed_binding_recipes(
        &self,
        recipes: Vec<(CodingAgentBinding, serde_json::Value)>,
    ) {
        let permit = acquire_carrier_mutation_permit().await;
        self.refresh_failed_binding_recipes_with_carrier_permit(&permit, recipes)
            .await
    }

    pub(crate) async fn refresh_failed_binding_recipes_with_carrier_permit(
        &self,
        permit: &CarrierMutationPermit,
        recipes: Vec<(CodingAgentBinding, serde_json::Value)>,
    ) {
        let mut state = self.carrier_state_write(permit).await;
        for (binding, recipe) in recipes {
            if let Some(seed) = binding.seed {
                state
                    .binding_seeds
                    .entry(binding.id.clone())
                    .or_insert(seed);
            }
            if let Some(origin) = binding.origin {
                state.binding_origins.insert(binding.id.clone(), origin);
            }
            if let Some(affected) = state.failed_binding_recipes.get_mut(&binding.id) {
                *affected = recipe;
            }
        }
    }

    pub(crate) async fn contains_public_or_pending(&self, id: Uuid) -> bool {
        self.state.read().await.sessions.contains_key(&id)
    }
}
