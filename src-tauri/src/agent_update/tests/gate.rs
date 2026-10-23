use super::*;

// ---------------------------------------------------------------------
// #1551 - gate
// ---------------------------------------------------------------------

#[test]
fn gate_running_tracks_commands_and_clears_on_finish() {
    let gate = AgentUpdateGate::new();
    gate.mark_started();
    gate.mark_command_started("claude", "Claude", None);
    gate.mark_command_started("claude", "Claude", None);
    assert_eq!(
        gate.snapshot().running,
        vec![command_ref("claude", "Claude")]
    );
    gate.mark_command_finished(ok_result("claude", "Claude"));
    let snapshot = gate.snapshot();
    assert!(snapshot.running.is_empty());
    assert_eq!(snapshot.results.len(), 1);
    gate.mark_command_started("codex", "Codex", None);
    gate.mark_finished(vec![]);
    let snapshot = gate.snapshot();
    assert!(snapshot.running.is_empty());
    assert!(snapshot.results.is_empty());
    assert!(!snapshot.in_progress);
}

#[test]
fn gate_command_finished_upserts_by_command() {
    let gate = AgentUpdateGate::new();
    gate.mark_command_finished(ok_result("claude", "Claude"));
    gate.mark_command_finished(AgentUpdateResult {
        command: "claude".to_string(),
        label: "Claude".to_string(),
        ok: false,
        outcome: AgentUpdateOutcome::Failed,
        error: Some("exit code 1".to_string()),
        install_before: None,
        install_after: None,
        change: AgentUpdateChange::Unknown,
        verification_error: None,
    });
    let results = gate.snapshot().results;
    assert_eq!(results.len(), 1);
    assert!(!results[0].ok);
    assert_eq!(results[0].error.as_deref(), Some("exit code 1"));
}

#[test]
fn gate_is_finished_follows_mark_finished() {
    let gate = AgentUpdateGate::new();
    assert!(!gate.is_finished());
    gate.mark_started();
    assert!(!gate.is_finished());
    gate.mark_finished(vec![]);
    assert!(gate.is_finished());
    gate.mark_started();
    assert!(gate.is_finished());
}

#[tokio::test]
async fn gate_prompt_state_and_claim() {
    let gate = AgentUpdateGate::new();
    assert_eq!(gate.prompt_state("claude"), PromptState::NotPrompted);
    let mut rx = gate.register_prompt("claude", "Claude");
    assert_eq!(gate.prompt_state("claude"), PromptState::Pending);

    let AnswerClaim::Claimed(claimed) = gate.claim_answer("claude", true) else {
        panic!("a pending prompt must be claimed");
    };
    assert_eq!(claimed.closed(), &command_ref("claude", "Claude"));
    assert_eq!(gate.prompt_state("claude"), PromptState::Answered(true));
    let snapshot = gate.snapshot();
    assert!(snapshot.prompt.is_none());
    assert_eq!(
        snapshot.answered,
        BTreeMap::from([("claude".to_string(), true)])
    );
    assert!(
        matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the claim must not release the loop"
    );
    assert!(claimed.deliver(true));
    assert!(rx.await.expect("the loop is released"));

    // `claim_answer` records unconditionally: only `answer_prompt` may call
    // it in production, after classifying under the serial lock.
    assert!(matches!(
        gate.claim_answer("claude", false),
        AnswerClaim::Recorded
    ));
    assert_eq!(gate.prompt_state("claude"), PromptState::Answered(false));

    let codex_rx = gate.register_prompt("codex", "Codex");
    drop(codex_rx);
    let AnswerClaim::Claimed(claimed) = gate.claim_answer("codex", true) else {
        panic!("the entry was present, so the claim must succeed");
    };
    assert!(!claimed.deliver(true), "the receiver is gone");
    assert_eq!(gate.snapshot().answered.get("codex"), Some(&true));

    let _pi_rx = gate.register_prompt("pi", "Pi");
    assert_eq!(gate.drop_pending("pi"), Some(command_ref("pi", "Pi")));
    assert_eq!(gate.drop_pending("pi"), None);
    assert_eq!(gate.prompt_state("pi"), PromptState::Expired);

    let _opencode_rx = gate.register_prompt("opencode", "OpenCode");
    gate.mark_finished(vec![]);
    assert_eq!(gate.prompt_state("opencode"), PromptState::Expired);
    assert_eq!(gate.drop_pending("opencode"), None);
    let answered = gate.snapshot().answered;
    assert_eq!(answered.get("claude"), Some(&false));
    assert_eq!(answered.get("codex"), Some(&true));
}

#[test]
fn gate_mark_started_with_nodes_replaces_nodes_and_snapshot_carries_them() {
    let gate = AgentUpdateGate::new();
    let nodes = vec![
        node("claude", "Claude", vec!["claude --update"]),
        node("codex", "Codex", vec!["codex update"]),
    ];
    gate.mark_started_with_nodes(nodes.clone());
    let snapshot = gate.snapshot();
    assert_eq!(snapshot.nodes, nodes);
    assert!(snapshot.in_progress);
    gate.mark_finished(vec![]);
    assert_eq!(gate.snapshot().nodes, nodes, "the summary keeps the order");
    let replacement = vec![node("pi", "Pi", vec!["pi update"])];
    gate.mark_started_with_nodes(replacement.clone());
    assert_eq!(gate.snapshot().nodes, replacement);
}

#[test]
fn gate_mark_command_skipped_prunes_once_and_returns_the_ref() {
    let gate = AgentUpdateGate::new();
    gate.mark_started_with_nodes(vec![
        node("claude", "Claude", vec!["claude --update"]),
        node("codex", "Codex", vec!["codex update"]),
    ]);
    gate.mark_command_started("claude", "Claude", None);
    gate.mark_command_finished(ok_result("claude", "Claude"));
    assert_eq!(
        gate.mark_command_skipped("codex"),
        Some(command_ref("codex", "Codex"))
    );
    let snapshot = gate.snapshot();
    assert_eq!(
        snapshot.nodes,
        vec![node("claude", "Claude", vec!["claude --update"])]
    );
    assert!(snapshot.running.is_empty());
    assert_eq!(snapshot.results.len(), 1);
    assert_eq!(gate.mark_command_skipped("codex"), None);
    assert_eq!(gate.snapshot().nodes.len(), 1);
}

#[test]
fn gate_mark_command_started_records_install_before_on_its_node() {
    let gate = AgentUpdateGate::new();
    gate.mark_started_with_nodes(vec![node("claude", "Claude", vec!["claude --update"])]);
    let started = gate.mark_command_started(
        "claude",
        "Claude",
        Some(InstallState::missing("x".to_string())),
    );
    assert_eq!(
        started.install_before.as_ref().map(|state| state.status),
        Some(InstallStatus::Missing)
    );
    let snapshot = gate.snapshot();
    assert_eq!(
        snapshot.nodes[0]
            .install_before
            .as_ref()
            .map(|state| state.status),
        Some(InstallStatus::Missing)
    );
    let synthesized = gate.mark_command_started("nope", "Nope", None);
    assert!(synthesized.update_commands.is_empty());
    assert_eq!(synthesized.command, "nope");
    assert_eq!(gate.snapshot().nodes.len(), 1, "nothing was inserted");
}

#[test]
fn pass_nodes_follow_catalog_order_across_updates_and_prompts() {
    let catalog = vec![
        entry("claude", "Claude", vec!["claude --update"]),
        entry("codex", "Codex", vec!["codex update"]),
        entry("pi", "Pi", vec!["pi update"]),
        entry("opencode", "OpenCode", vec!["opencode upgrade"]),
        CodingAgentDefinition {
            key: "pi-alt".to_string(),
            ..entry("pi", "Pi (alt)", vec!["pi update"])
        },
    ];
    let plan = AgentUpdatePlan {
        prompts: vec![UpdateTarget {
            command: "pi".to_string(),
            label: "Pi".to_string(),
            commands: vec!["pi update".to_string()],
            cwd: cwd(),
        }],
        updates: vec![
            UpdateTarget {
                command: "opencode".to_string(),
                label: "OpenCode".to_string(),
                commands: vec!["opencode upgrade".to_string()],
                cwd: cwd(),
            },
            UpdateTarget {
                command: "claude".to_string(),
                label: "Claude".to_string(),
                commands: vec!["claude --update".to_string()],
                cwd: cwd(),
            },
        ],
    };
    let nodes = pass_nodes(&catalog, &plan);
    assert_eq!(
        nodes,
        vec![
            node("claude", "Claude", vec!["claude --update"]),
            node("pi", "Pi", vec!["pi update"]),
            node("opencode", "OpenCode", vec!["opencode upgrade"]),
        ]
    );
}

#[test]
fn pass_nodes_skip_an_empty_effective_winner() {
    // Even a manually built plan cannot surface a node for a command whose
    // first effective catalog entry carries no sequence (#1967).
    let catalog = vec![
        entry("pi", "Pi (empty)", vec![]),
        CodingAgentDefinition {
            key: "pi-alt".to_string(),
            ..entry("pi", "Pi (alt)", vec!["pi --custom"])
        },
    ];
    let plan = AgentUpdatePlan {
        prompts: Vec::new(),
        updates: vec![UpdateTarget {
            command: "pi".to_string(),
            label: "Pi (empty)".to_string(),
            commands: vec!["pi --custom".to_string()],
            cwd: cwd(),
        }],
    };
    assert!(pass_nodes(&catalog, &plan).is_empty());
}
