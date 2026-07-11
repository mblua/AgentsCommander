# AgentsCommander Harness Roadmap

The `agentscommander harness` command is a policy-controlled entry point for coding-agent OS command execution. Phase 1 is intentionally obedient: agents are expected to call it, but it does not prevent direct shell execution.

## Phase 1: Obedient Harness

Current scope:

- CLI surface for argv execution and `--raw-command`.
- Argv execution preserves argument boundaries by passing arguments directly to the child process.
- Raw command execution explicitly uses the platform shell (`cmd.exe /C` on Windows, `sh -c` on Unix). Policy matching for raw strings is best-effort.
- Conservative guardrails for obviously destructive commands, suspicious branch names, and nested shells.
- JSON Lines audit log at `<config_dir>/logs/harness.log` with secret redaction and capped command text.

Non-goal: Phase 1 does not provide strong sandboxing or prevent a shell-capable agent from bypassing the harness.

## Phase 2: Policy Authorization

Planned scope:

- Stronger command classification.
- Task and issue aware authorization.
- Per-agent and per-workgroup policy configuration.
- Clearer escalation paths when a command needs human approval.

## Phase 3: Command Optimization

Planned scope:

- Command rewrites and optimization.
- Safer alternatives for common expensive or fragile command patterns.
- Token and output volume metrics.
- Better policy explanations that help agents choose smaller commands.

## Phase 4: Strong Enforcement

Planned scope:

- Executor control, PATH shims, or sandbox integration.
- Enforcement that does not rely only on agent obedience.
- Tamper-resistant audit flow.
- Compatibility testing across Windows, macOS, Linux, and CI environments.
