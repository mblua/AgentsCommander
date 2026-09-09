# #1854 — Muse Code beta preset with workspace-latest resume

Issue: [#1854 — Add a beta Muse Code catalog preset](https://github.com/mblua/AgentsCommander/issues/1854)
Repository: repo-AgentsCommander
Repository root: /home/mblua/0_repos/AgentsCommander_iac/.ac/room-9-ac-dev-team-v4/repo-AgentsCommander
Planning branch: feature/1854-muse-code-preset
Design evidence base: main at 5c7dd08841a5846f483ba59a0d0dd94ea6410c4d
Workflow: Full, partitioned into native child issues
Technical score: 56/100 (raw 115/200)
Task class: routine application code, tests, catalog data, and documentation
Threat model: trusted workstations/CI/Muse state; producer payloads, execution claims, and receipts are untrusted. Roots are frozen digests, configured CLI/room store, and tech-lead-owned clean-shell execution and exact notifications. No release/signing, untrusted-host, privileged-input, or migration work.

## Objective

Add Muse Code as the eighth beta preset and a first-class kind. For trusted direct local macOS/Linux Muse with empty configured argv, resume intent produces effective `muse resume --last`; fresh stays plain. Preserve manual argv, other providers, and valid catalogs.

The ten-file #1873 head atomically owns Rust serialization and the TypeScript `"muse"` union. Live Root picker selection stays a fresh replacement; dormant Root wake resumes.

Continuity is Muse's newest retained launch-cwd session, not exact AC identity. Failures surface once with no automatic fresh fallback: a spawn `Err` removes its row and requires a later user launch/create, while only a retained post-spawn failure can use fresh `Restart Session` recovery.

## Cause and smallest coherent design

Catalog-only was incomplete once automatic resume was required. Existing callers express intent through skip_auto_resume; Muse lacks a typed profile and trusted injector. The smallest coherent change is:

1. Add the catalog row and count-consumer coverage.
2. Atomically add the Rust/TypeScript kind contract, one runtime profile, and
   one provenance-gated injection seam used by existing lifecycle flows.
3. Mirror the catalog in the frontend and pin existing caller intent.
4. Publish evidence only after all code phases land.

No new generic framework, session UUID store, history parser, caller-specific
resume branch, or fallback retry is needed.

## Verified current architecture

Evidence was verified at the design base with Codebase Memory Tier 2 generation
2026-09-08T01:03:49Z: ready, 28,041 nodes, 172,953 edges, matching branch/head.
Every relied-on source/test/doc/workflow path had no recorded coverage issue;
plans are index-excluded and were read directly. Four unrelated PowerShell
parse gaps do not intersect this scope. A clean coverage result is best-effort,
not proof of completeness.

- CodingAgentKind/profile owns typed detection, serde wire values, resume
  tokens, idle tuning, container credential, and self-clear capability.
- create_session_inner captures configured shell_args before mutating a local
  effective vector, calls provider injectors, then records
  effective_shell_args and sends BackendSpawnSpec.
- AgentSpawnCommand carries resolved shell/args, trusted configured identity,
  and backend. Existing Pi injection demonstrates the fail-closed seam.
- strip_auto_injected_args protects configured/persisted provenance. Muse must
  use a preserve arm, never text-based stripping.
- validate_agent_command_text early-accepts detected Pi only, then scans every
  argv token for legacy provider basenames. Direct Muse therefore needs the
  same detected-kind precedence before a Codex-looking argument can reject it.
- Telegram attach calls derive_reader and returns its error before manager/
  bridge construction; one exact Muse rejection there makes bridge changes
  unnecessary.
- Loop already calls commands::session::create_session_inner, and the recorded
  module arc loops::delivery -> commands::session already exists.
- ProjectPanel, mailbox, and Loop already carry fresh/resume/reuse intent. Root
  differs: a dormant record sends explicit resume, but live agent-picker
  selection omits the flag; omission serializes to null, defaults fresh, tears
  down the old runtime, and spawns a replacement.
- Frontend fallback and Rust embedded catalogs contain seven rows. Rust and
  TypeScript CodingAgentKind both omit muse, while SessionInfo::from copies the
  Rust value directly onto the wire. Existing catalog/lifecycle tests are the
  extension points.
- `PtyBackend::spawn` success is distinct from later child exit. The real local
  backend's SpawnRecord/watch_child seam records exact argv, nonzero liveness,
  cause, and one duplicate-suppressed child-exit diagnostic.

Repository/Git evidence: planning HEAD and origin/main both equal the design
base; no application change exists. The five plan files are ignored
intent-to-add artifacts. Cargo.lock, package-lock.json, module-arcs.txt,
manifests, and workflows are tracked and unchanged.

Vendor feasibility notes report Muse 1.0.3 (1.0.3-R2198.1), Linux,
2026-09-08, parser/no-history outcomes. No qualifying raw successful retained-
session transcript was found in checked authorized sources; prior successful
manual-resume claims are retracted. Workspace-latest resume remains the user
requirement, not an observed vendor result. Vendor continuation and AC live
resume are PENDING. No store/identity/grouping/platform guarantee follows.

## Native issue and branch chain

All four native children are open and linked under #1854. Delivery is strictly
sequential; each branch is created from then-current green main after its
predecessor merges:

| Phase | Child | Branch | Class | Owner | Depends on |
|---:|---|---|---|---|---|
| 1 | [#1860](https://github.com/mblua/AgentsCommander/issues/1860) | feature/1860-muse-embedded-catalog | patterned | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4 | none |
| 2 | [#1873](https://github.com/mblua/AgentsCommander/issues/1873) | feature/1873-muse-auto-resume | design-bearing | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4 | #1860 landed and green |
| 3 | [#1861](https://github.com/mblua/AgentsCommander/issues/1861) | feature/1861-muse-frontend-catalog | patterned | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-webpage-ui-v4 | #1873 landed and green |
| 4 | [#1862](https://github.com/mblua/AgentsCommander/issues/1862) | feature/1862-muse-beta-documentation | patterned | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-technical-writer-v4 | #1861 landed and green |

The planning branch is never an implementation branch. No phase branches from
another feature branch, and every phase leaves main green.

## Frozen catalog contract

Muse is eighth and last in both catalog representations:

~~~json
{
  "key": "muse",
  "label": "Muse Code",
  "description": "Meta terminal coding agent (beta; macOS/Linux host only)",
  "color": "#0668E1",
  "command": "muse",
  "envs": [],
  "isolatedHome": false,
  "removable": true,
  "updateCommands": [],
  "autoUpdate": false
}
~~~

instructionsFilename and configSeed are absent from the built-in row. The runtime resolver supplies
AGENTS.md. Empty update commands and disabled auto-update are contractual.
Fresh/default/fallback catalogs gain Muse; a valid existing user catalog
remains byte-identical and Muse-free.

## Runtime eligibility contract

Add CodingAgentKind::Muse with serde/as_str value muse and MUSE_PROFILE:

- IdleTuning::DEFAULT.
- resume_tokens exactly ["resume", "--last"].
- container_credential None.
- auto_self_clear_supported false.

The same #1873 product commit adds only `"muse"` to the TypeScript
CodingAgentKind union. No one-sided Rust/TypeScript wire head may be handed off
or merged; `SessionInfo::from` and the IPC shape remain unchanged.

Direct shell-stem detection accepts exact case-sensitive muse, including an
absolute path whose file leaf is muse. It rejects prefix names and argument-only
identification through env, shell, PowerShell, cmd, or compound wrappers.

Automatic injection requires all conditions:

- Resolved configured AgentSpawnCommand is present.
- Actual shell/args exactly equal that resolved recipe.
- Backend is LocalProcess.
- Compiled host is macOS or Linux.
- Direct shell file leaf is exact muse.
- Configured argv is empty.
- Detected kind is Muse and resume intent has skip_auto_resume false.

The injector appends only the profile's two tokens to the effective runtime
vector. Any configured argument suppresses injection, including a selector/
UUID, prompt, --no-session-log, root/workspace option, or unknown future flag.
Ad-hoc launches, wrappers, mismatches, Windows, and containers stay unchanged.

Before spawn, settings validation early-accepts only directly detected Pi/Muse
before legacy token scans. Direct
`muse --workspace /tmp/codex resume --last` must validate, remain configured
byte-for-byte, and suppress injection; an `env` wrapper retains the existing
Codex manual-resume rejection. Telegram `derive_reader` returns exactly
`Telegram bridge does not support Muse sessions` for both backends and normal
attach stops before bridge creation; telegram/bridge.rs stays unchanged.

## Lifecycle contract

| Path/state | Intent | Required outcome |
|---|---|---|
| Ordinary create | fresh | configured plain muse |
| Explicit Restart Session | fresh | configured plain muse |
| Startup restore, ordinary retained row | resume | muse resume --last |
| Startup restore, durable fresh marker | fresh | configured plain muse |
| ProjectPanel closed-marker reopen | resume | muse resume --last |
| ProjectPanel first/no marker | fresh | configured plain muse |
| Root with no prior record | fresh | configured plain muse |
| Root agent-picker selection, live record | omitted restart flag defaults fresh | tear down old runtime; one configured plain muse replacement spawn |
| Root dormant | resume | muse resume --last |
| Root missing PTY but non-dormant | same fresh replacement | configured plain muse |
| Mailbox cold creation | fresh | configured plain muse |
| Mailbox known-state wake | resume | muse resume --last |
| Loop cold creation | fresh | configured plain muse |
| Loop exited/missing-PTY wake | resume | muse resume --last |
| Loop live reuse | reuse | no spawn |

Loop cold suppression is Muse-only. All non-Muse providers retain existing
behavior. No new branch is added to lib.rs, phone/mailbox.rs, ProjectPanel.tsx,
or the executable Root action.

## Persistence and failure contract

Session.shell_args and snapshots retain the configured recipe. Only the local
spawn vector and Session.effective_shell_args receive AC-minted resume tokens.
Muse takes an early preserve path in strip_auto_injected_args; manual textual
resume forms are never stripped.

No-history, corrupt/unreadable state, permissions, invalid manual selector,
authentication, parser, and every other process failure follow existing timing.
If `PtyBackend::spawn` returns `Err`, the pending row is reverted/removed and
create returns one diagnostic with no created event. No row remains to restart;
after fixing the cause, the user must launch/create again. If spawn succeeds and
the child later exits nonzero, its created row remains and one child-initiated
`[pty] child-exit` diagnostic surfaces; only that row offers deliberate fresh
`Restart Session`. Each timing attempts one process, with no retry/plain fallback.

## Unsupported boundary

The built-in Muse preset supplies no configSeed/factory seed or automatic Muse
credential flow. A user-configured `configSeed` and generic agent/profile env
still work provider-neutrally, with no Muse/AgentKind gate. No Muse context
watcher, transcript/JSONL scraping, home isolation, Telegram bridge, logical or
privileged PTY behavior, delayed-enter, auto-self-clear, Windows/container
automatic resume, version probe, install/auth/update, platform schema/filter,
release, or packaging is in scope.

`session/manager.rs` and `pty/inject.rs` remain byte-identical. Profile tests
pin default idle tuning and no PtySubmissionAgent, keeping privileged/logical
PTY behavior unsupported. Valid manual configured argv round-trips.

## Exact 31-file union

Five plan artifacts:

1. plans/1854-muse-code-beta-preset/epic.md
2. plans/1854-muse-code-beta-preset/1860-rust-catalog.md
3. plans/1854-muse-code-beta-preset/1873-muse-auto-resume.md
4. plans/1854-muse-code-beta-preset/1861-frontend-catalog-mirror.md
5. plans/1854-muse-code-beta-preset/1862-documentation-evidence.md

Four #1860 product/test files:

6. src-tauri/resources/coding-agents/agents.default.json
7. src-tauri/src/config/coding_agents_catalog.rs
8. src-tauri/src/web/commands.rs
9. src-tauri/tests/cli_project_registration.rs

Ten #1873 runtime/test files:

10. src-tauri/src/session/profile.rs
11. src-tauri/src/commands/session.rs
12. src-tauri/src/config/settings.rs
13. src-tauri/src/config/sessions_persistence.rs
14. src-tauri/src/loops/delivery.rs
15. src-tauri/src/commands/telegram.rs
16. src-tauri/src/config/agent_command.rs
17. src-tauri/src/config/config_seed.rs
18. src-tauri/src/pty/spawn_diagnostics.rs
19. src/shared/types.ts

Six #1861 frontend/test files:

20. src/shared/agent-presets.ts
21. src/shared/agent-presets.test.ts
22. src/sidebar/agent-update-status.test.ts
23. src/sidebar/components/ProjectPanel.reopen-resume.test.tsx
24. src/sidebar/components/root-agent-action.ts
25. src/sidebar/components/root-agent-action.test.ts

Six #1862 documentation files:

26. docs/integrations/coding-agents.md
27. docs/testing/coding-agent-compatibility-muse.md
28. docs/testing/README.md
29. docs/testing/coding-agent-tests-template.md
30. docs/features/agent-auto-update.md
31. docs/faq.md

Each phase's exact file set is exclusive after the #1860 plan bootstrap. Scope
expansion revokes readiness and requires architecture review.

## Ignored-plan bootstrap

The plans directory is ignored. Once the user freezes five recomputed uppercase
SHA-256 digests for the exact revised bytes, #1860 alone transfers and force-adds them from a
then-current main branch. It re-hashes source, destination, staged blobs, and
committed blobs; creates a five-plan-only first commit; then makes its separate
four-file product commit. No plan byte changes thereafter.

After freeze and before any implementation branch, the coordinator replaces
live #1854/#1860/#1873/#1861/#1862 bodies with their exact corresponding plan
bytes, preserves the native parent links/open state, extracts each body back,
and proves byte identity in an immutable sync message. Every phase rechecks its
child body and digest before branching/writing. Current catalog-only bodies are
stale; no Issue mutation is authorized during planning.

A missing/mismatched digest, mixed plan/product commit, extra path, or dirty
state blocks implementation. #1873/#1861/#1862 verify but never recommit plans.

## Dependency-cycle and layering gate

Planned new module arcs: zero. Planned removed module arcs: zero.

The only new cross-module call is:

~~~text
src-tauri/src/loops/delivery.rs -> crate::commands::session::trusted_muse_auto_resume_spawn
~~~

The base arc record already contains agentscommander_lib::loops::delivery ->
agentscommander_lib::commands::session because Loop calls create_session_inner.
Other edits reuse existing/same-file relationships; no lower layer gains UI transport or role inversion.

Every code phase requires byte-identical src-tauri/module-arcs.txt; #1861 also
requires identical frontend dependency output. #1873 runs loops_layering,
instance_gitignore_layering, and project_settings_layering.

The fast path requires a retained exact module-reference diff, #1873's supported
`record:arcs:self` run, equal pre/post arc-record SHA (the self-test does not
regenerate/analyze Rust), identical independently measured phase-base/candidate #1861 dependency output, and green layering.
Any unclassified reference/pair, dependency/scope/arc drift triggers the authorized
`rust-levelization-run` owner. Its clean-base/candidate record must contain pre/post
`coverage.graphShape.cyclicSccs`, sorted SCC member sets, every added/removed and
cross-boundary pair, regenerated arc SHA/byte comparison, and layering exits.
Dirty/missing evidence, exit 3, changed SCCs, a cross-boundary pair, or arc drift blocks.

## Delivery invariants

| Gate | Executable evidence | Failure |
|---|---|---|
| Determinism | Tracked locks; Cargo --locked; Node 22/npm 11.6.2. Code phases provision below AGENTSCOMMANDER_ROOT, prepend its Node bin to PATH for every npm install/ci/run child, and prove direct/PATH Node agree on major 22; cache/tmp/user config stays there | Host Node, version, provision, or unlocked failure blocks |
| Git authority | Exact root, frozen plan digests, synchronized byte-identical open/linked native Issue bodies, issue branch from green main, PHASE_BASE_SHA, clean state, PR to main | Wrong root/branch/base/body/link or dirty state blocks |
| Base drift | Fetch origin/main before mutation/PR update; classify changed paths and semantic relevance; refresh affected evidence | Relevant unreviewed drift blocks |
| Scope | Compare tracked, staged, ordinary-untracked, plan, lock, manifest, workflow, version, generated, and arc paths against phase set | Any unexplained path blocks |
| Execution | Explicit logs/bounds; ordered #1873 gates; lead-owned clean-shell execution; #1861/#1862 each bind every check, CI lookup, and handoff to one captured candidate plus direct lead attestation | Timeout/cancel/zero, HEAD/ref/reflog drift, mixed SHA, fake/producer execution or receipt, private-only/missing evidence fails |
| Recovery | Record pre-write/output hashes; restore only owned bytes that still match phase output | Preserve external bytes; broad reset/checkout/restore/clean forbidden |
| Evidence | Positive effective-argv/wire control, separate spawn-error and real post-spawn nonzero proof, lifecycle/persistence/boundary tests, Grinch fail-then-pass, honest docs ledger | Missing control or false PASS blocks |

Enhanced release/signing/untrusted-host/destructive-migration controls are inapplicable to the accepted threat model. No host installer, credential, TUI input, or live prompt is authorized by an implementation phase.

## Immutable candidate and attestation protocol

For all four phases the producer commits, sends only candidate/base/branch as an
untrusted request, and stops. ac-tech-lead-v4 independently captures one clean
candidate SHA plus symbolic ref and complete HEAD/branch reflog digests, runs the frozen accepted-plan
executor in a fresh shell, and guards each executor and final query before/after.
Every diff names base and candidate, never symbolic HEAD. Normal H1→H2 movement
fails even when H2 is clean with the same owned paths; checkout/detach H1→H2→H1 changes HEAD history even when the original branch never moves. Reflogs must exist and logging must be enabled; every read/hash assignment fails explicitly, including conditional calls. Test read failures at bind, execution, CI, and delivery. Every clean-state check requires successful Git status capture before empty-output comparison, including final/conditional/assignment callers; failed status never proves clean. A new
candidate discards all prior evidence. PR-head CI and handoff must equal it; the
lead repeats the guard before and after both lookup and delivery.

Only a strict canonical attestation sent directly by ac-tech-lead-v4 through the
configured control plane is authoritative. It binds phase/base/candidate/CI,
plan/executor/log/scope/status hashes and derived outcomes. Producer bytes,
plausible summaries, private logs, copied paths, and real producer `Queued:`
receipts fail. #1862 consumes predecessor lead attestations; review consumes the
#1862 lead attestation. Executable negatives cover child/timeout/inner/outer-tee failures, persistent/returning checkout drift in all four phases, failed inventory stages, malformed assigned receipts, fake dependency baseline, escaped scratch, and producer forgery classes. #1861 compares an independently lead-controlled phase-base report with the bound candidate; a candidate never supplies its own baseline.

## Local and exact-head CI ownership

- #1860: 21 exact ordered calls, byte-exact metadata, 315-line manifest, 45-entry sidecar and exact 46-member bundle travel in 144-KiB shards; frozen recipe/verifier/executor plus lead-owned clean-shell run and ACK capsule make receiver trust authoritative.
- #1873: separate precommit feedback ends before commit; a fresh clean-candidate shell runs all three bodies without rebinding/reusing old evidence. Both legacy-named precommit/exact-head gates bind that same HEAD. One EXIT-enforced workflow orders 11 required root checks, harness-owned exact 16-name `--format pretty` enumeration plus 16 independent exact one-pass runs in both target→assert→seal→finalize gates, 11 final HEAD checks, and explicit log destinations. Its 45-member evidence rejects test stdout forgery; the lead independently executes the three frozen bodies.
- #1861: lead-owned four-file Vitest, lifecycle/catalog assertions, dependency/typecheck/build, byte-exact six-path candidate scope, no ref/reflog drift, exact-candidate CI, and `ac.frontend-evidence.v1`.
- #1862: lead-owned six-page positives, repository-Markdown negatives, links/14-row audit, distinct matrix-code versus docs-candidate SHA, six-path/one-new-file scope, exact-candidate CI, and `ac.docs-evidence.v1`.

On every attested exact phase candidate/PR-head SHA, require each triggered/configured-required
check: test-debt; Windows Rust check, clippy, and full tests; Linux Rust check,
clippy, and configured test; macOS Rust check/clippy; rust-fmt; four portable
terminal legs; Windows release CLI smoke; frontend regression; and
validate-branch-name. PR lockfile-drift must pass its detector and skip
regeneration because package inputs do not change. bundle-validation and
version-sync are path-inapplicable. Re-derive after base/workflow/diff drift.
Another SHA, waiver, bypass, or unexplained skip fails.

## Partition

PARTITION: 4 phases

The evidence exceeds the Full-plan partition trigger. Catalog/bootstrap,
atomic runtime/wire behavior, frontend catalog consumers, and documentation
each have one owner and independent verification. #1873's Rust owner owns its
single TypeScript union edit because separating the serialized producer from
the wire consumer would leave a red/inconsistent main boundary. All phases are
sequential; none is parallelizable.

| Phase | Artifact | Child | Class | Owner | Files | Depends on | Parallel | Phase-SHA256 |
|---:|---|---|---|---|---:|---|---|---|
| 1 | 1860-rust-catalog.md | #1860 | patterned | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4 | 9 | none | none | 1B4EC6A2AAEDD2B8E3114181C524D4E2D2DE74B0E0721AB9036AE742067E8C9F |
| 2 | 1873-muse-auto-resume.md | #1873 | design-bearing | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4 | 10 | #1860 | none | 41D8231D093897896076808FF9AEFE52FBC1182214FD78081E029BC89E01F08D |
| 3 | 1861-frontend-catalog-mirror.md | #1861 | patterned | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-webpage-ui-v4 | 6 | #1873 | none | AC9615C0263A7A782E6AA8EA9AFA0263586C2AF8618AE0251F9CF7E62F409D38 |
| 4 | 1862-documentation-evidence.md | #1862 | patterned | AgentsCommander_iac:room-9-ac-dev-team-v4/ac-technical-writer-v4 | 6 | #1861 | none | CC253505664569A80C03BCAF80BEDD60B633D82CE50330D2C1824706A9E7A8F5 |

## Epic acceptance criteria

1. #1860 lands five frozen plans unchanged, then the exact four catalog files;
   Muse is eighth and existing valid catalogs remain byte-identical.
2. #1873 atomically lands both wire-type sides and the fail-closed injection;
   spawn `Err` removes its row for a later user launch/create, post-spawn nonzero
   retains its row for fresh Restart Session, and each proves one diagnostic/no fallback.
3. Only effective argv receives automatic tokens. Direct Muse manual argv,
   including the Codex-looking collision case, validates, suppresses injection,
   and round-trips unchanged; wrappers retain legacy rejection.
4. #1861 mirrors catalog data and proves ProjectPanel/Root intent; live Root
   strictly omits the own skipAutoResume property while dormant Root owns false.
5. Muse adds no built-in seed/factory or automatic Muse credential flow; configured
   `configSeed` and generic agent/profile env remain provider-neutral. Every
   actually unsupported capability and every non-Muse provider is unchanged.
6. #1862 makes all six affected public pages agree without upgrading pending/
   live claims; its exhaustive stale-text gate covers pages outside the old four.
7. Synchronized Issues; independent lead execution; immutable candidates; direct canonical attestations; drift/forgery rejection; ordered #1873 gates; bounded evidence; Grinch; zero arcs; exact scope; and exact-candidate CI pass sequentially.
8. No dependency, lockfile, workflow, version, package, or release changes.

Status: READY_FOR_IMPLEMENTATION
