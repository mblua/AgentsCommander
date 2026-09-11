# Issue #1937: scoped Coding Agent + Profile locks

Status: **UX_APPROVED / IMPLEMENTATION_PLAN_FOR_REVIEW**. This is a Narrow design consolidation for later review, not implementation authorization or READY_FOR_IMPLEMENTATION. No current dev/grinch consensus, build, product test, PR, merge, or cycle certification is claimed.

Issue: https://github.com/mblua/AgentsCommander/issues/1937

Pinned source base: `7aef1d14e6bb0255b1430614c078145335dd54a4`.
Archive branch: `feature/1937-selection-lock-plan`.
Class: routine application change; concrete risks are overwritten selections, lost configuration updates, and unintended or late session restarts. Host executable attestation and release-provenance controls are not applicable.

## 1. Authoritative package and objective

Preserve deliberate Coding Agent/Profile choices during bulk assignment, allow explicit scoped assignment plus protection and independent unlocking, and initialize future replicas from their origin Matrix default with local exceptions.

Read [ISSUE.md](ISSUE.md), [user decisions](evidence/candados-decision-usuario.md), and the approved [interactive v3](prototypes/index-v3.html). Its SHA-256 is `7901780e9d936c603ba0a704b9cf002197a8bdf5cf687b795b2c0fe37ae7f7b0`. [V3 notes](prototypes/README-v3.md) describe the ten scenarios and reported 31/31 prototype assertions. Those are simulated UI checks, not production evidence. Scenario `resultado-unlock-room` demonstrates the completed unlock result.

[Earlier proposal](evidence/candados-propuesta.md), [feasibility](evidence/candados-viabilidad.md), and [contrast](evidence/candados-contraste.md) supply historical engineering evidence. Their provisional skip-only bulk-lock policy and optional Team+Matrix interpretation are superseded below. Historical prototype notes contain original room-shared locations; package-local entrypoints above are authoritative for this archive.

## 2. Approved behavior

The protected value is **Coding Agent ID + requested Profile letter**. It does not freeze command, environment, model, profile contents, version, or effective fallback. Show requested and effective selection when different; preserve existing drift and unavailable-provider diagnostics.

Keep `Apply to` with `This replica`, `All replicas of this kind`, `Entire room`. Immediately below, repeat these scopes with `+ lock`. All six choices form one radio group: the operation assigns the selected pair, and the second row also protects it. The upper bar reports persisted protection and the saved pair; its controls do not choose assignment scope.

| Operation | Required effect |
| --- | --- |
| Individual assignment | Deliberately change the pair, preserving an existing lock. Individual `+ lock` assigns and locks atomically. |
| Ordinary Kind/room assignment | Skip protected replicas, including every associated restart. |
| Bulk `+ lock`, no protected targets | Assign and lock eligible targets directly through existing scope/restart confirmation rules. |
| Bulk `+ lock`, protected targets | Before mutation, show conflicting replica identities, current pairs and requested pair. Even an already-equal protected pair belongs to this protected-target decision. |
| `Cancel` | No assignment, protection change, or restart. Place it left of the action buttons. |
| `Apply only to unlocked` | Assign and lock previously unlocked eligible replicas; preserve protected pairs, flags, and sessions. |
| `Force all, including locked` | Explicitly overwrite reviewed protected targets as well and leave all successfully assigned targets locked. |
| `Remove lock from` | Independent upper-bar scope: replica, Kind, or entire room. Clear protection only; keep pairs, sessions, and future defaults. |

Unlock is available when the focused replica is unlocked but other selected targets are protected. Show affected counts and a suitable action label. Zero protected candidates means disabled `Nothing to remove`, no write and no restart. Unlock success reports actual removed count and refreshed state. The approved room example is `Lock removed from 3 replicas · Coding Agent + Profile kept · no restart`, `0 of 4 protected`, and disabled `Nothing to remove`; the replica outside that room remains protected.

The compact informational Kind list renders protected rows only, while its denominator and underlying target list remain complete: one visible row can still mean `1 of 2 protected`. Never filter assignment eligibility or conflicts using this presentation list. With zero protection, retain the summary and render no protected rows.

Scope identities retain existing backend meaning: `Replica` is one validated room replica; `Workgroup` is one room, not all rooms of a logical Team; `Kind` compares canonical origin Matrix identity across configured roots. Distinct Matrices with similar names/providers are different kinds. The creation policy is **Default for new replicas of this Matrix**, shared wherever that Matrix is used; no Team+Matrix schema or provider grouping is introduced.

Future defaults are copied once into genuinely new replicas. Explicit creation exceptions win. Subsequent local pair/lock edits are authoritative. Changing the default never rewrites existing replicas. Reopen, archive/restore, or repeated member addition preserves local state; recreation after actual deletion uses the then-current default. Unlocking any existing scope never changes the default.

## 3. Verified implementation surfaces

Paths below are repository-relative, not paths into the archive. They are the expected edit/test surfaces, to be frozen against the eventual implementation base before mutation.

| File | Verified symbol or responsibility; intended change |
| --- | --- |
| `src-tauri/src/commands/config.rs` | `ProfileAssignmentScope`, preview/apply request/result types, `enumerate_profile_assignment_targets`, `profile_assignment_fingerprint`, `preview_coding_agent_profile_selection_inner`, `apply_coding_agent_profile_selection_inner`, `broad_profile_apply_lock`, `set_instance_profile_override_inner`: policies, previews, revalidation, results, unlock/default operations, lifecycle ordering. |
| `src-tauri/src/config/coding_agent_profiles.rs` | `set_replica_coding_agent_selection`, `write_profile_to_launch_path`, `read_replica_profile_result`, `set_agent_default_profile`: strict protection/default readers and coherent scoped mutation; retain legacy profile compatibility. |
| `src-tauri/src/config/local_config_io.rs` | `update_config_json_object`, `local_config_write_lock`: guarded read-modify-publish and cross-process exclusion. |
| `src-tauri/src/commands/entity_creation.rs` | `create_or_update_replica_on_disk`, `create_workgroup_on_disk`, `write_local_config_value`, `TeamConfigMutationGuard`: creation inheritance, safe retry merge; existing OS-lock pattern is a reference, not permission to import commands into persistence. |
| `src-tauri/src/cli/team.rs` | `add_member`: repeated-add path reaches replica creation even when membership was already present. Preserve configuration and report creation failures. |
| `src-tauri/src/commands/session.rs` | `resolve_restart_selected_agent_id`, `restart_session_inner_with_intent`: retain selection precedence and integrate with existing lifecycle ownership. |
| `src-tauri/src/phone/mailbox.rs` | `MailboxPoller::run_self_switch_after_sustained_idle`, `resolve_agent_command`: deliberate persisted self-switch participates in ordering; explicit temporary wake overrides retain current semantics. |
| `src-tauri/src/commands/ac_discovery.rs` | `AcAgentReplica` and its construction sites: expose protection, saved pair and state diagnostics consistently. |
| `src-tauri/src/web/commands.rs`, `src-tauri/src/lib.rs` | Web dispatch/broadcast and Tauri command registration: same inner operations and validation, including unlock/default commands. |
| `src/shared/types.ts`, `src/shared/ipc.ts` | Matching typed requests/results and `SettingsAPI` calls; preserve wire scope names. |
| `src/sidebar/components/AgentPickerModal.tsx`, `src/sidebar/styles/sidebar.css` | Approved rows/bar/dialog/results, independent unlock scope, default controls, accessibility and pending states. |
| `src/sidebar/components/AgentPickerModal.test.tsx` | Extend existing stale-preview and scope-confirmation tests with the acceptance cases below. |

Evidence collected on the pinned base: graph project resolved to this room's authorized repository; index ready, 23,036 nodes and 152,508 edges. Used architecture, symbol search, snippets, inbound trace and exact-path coverage. Coverage reported `metadata_changed` for source files and excluded the frontend test and plan package. Therefore decisive source ranges, registrations, DTOs, UI gates, styles and test anchors were checked directly on disk. Graph results are discovery evidence, not proof of freshness/completeness. Source drift from `f16edd976f9861648d04d137b3bb960189d4548e` to the pinned base was verified as only `crates/session-bridge/Dockerfile`.

Current apply drops its async guard before restarts and passes the request pair into restart; a later lock could otherwise be acknowledged before an earlier bulk restart. Current replica creation builds context from an empty list and replaces the entire config. Current local JSON mutation has a process-local mutex and atomic publication, which alone cannot prevent concurrent app/CLI lost updates. These are implementation requirements, not fixes already delivered.

## 4. Persistence and creation design

Use additive `tooling.selectionLocked: boolean` in the replica's top-level `config.json`. Missing field in valid configuration means false. Unreadable JSON, non-object tooling, or a present non-boolean flag is an explicit invalid state, never unlocked. Invalid candidates receive a diagnostic and no assignment/restart; an unreadable scope anchor fails enumeration. A true flag with an incomplete pair requires explicit repair, not historical inference.

Pair assignment and optional lock share one read-modify-publish: write `currentCodingAgent`, `profile`, matching `instanceProfileOverride` and `instanceProfileOverrideSource="manual"`, and the final lock flag. Preserve unknown keys, `lastCodingAgent`, and the loaded content hash. Unlock writes only the lock field; it does not rematerialize the pair. Ordinary individual edits preserve the flag. A protected profile-only edit validates the new letter and retains Coding Agent; reject clearing it with null until unlocked or explicitly repaired. This is an engineering invariant protecting the materialized pair, not an absolute ban on individual edits.

Proposed Matrix storage: `tooling.replicaSelectionDefault = { codingAgentId, requestedProfile, selectionLocked }`. This is separate from the existing profile-only `defaultProfile`; absence preserves old creation behavior. A dedicated typed default operation validates the canonical Matrix and complete pair, updates only this object, and reports its own result. Disabling Start locked keeps the configured creation pair and uses false. Existing-scope actions do not implicitly save a default. Wire the v3 future-default control to this operation, displaying the actual persisted default rather than merely echoing an unsaved picker selection.

At first creation, read a consistent Matrix policy and materialize pair plus flag, overridden by an explicitly supplied local exception. Invalid protective defaults must fail creation visibly before publishing a misleading replica config. Do not silently select a different provider or mark an incomplete pair protected. Validate requested letters and report effective fallback through the existing resolver.

For existing replicas, merge under the same config guard: update required identity/repositories and normalize mandatory context using existing context as input; preserve tooling, custom context and unknown top-level keys. Never reapply a creation default. Distinguish an absent new config from malformed existing config; preserve malformed bytes and fail. An empty directory left by an interrupted first creation can retry initialization; a committed config is the boundary for local-state authority. Audit every creation entrypoint, including new rooms and member addition, to use this rule. Membership/config/repository work is not one global transaction: report a partial add accurately and make retry safe.

Replica protection is restricted to validated room/wg replicas. Root and origin Matrix selections are not instance-lock targets; a Matrix creation-default operation is a separate capability. Retain existing path and caller authorization for both transports. Do not introduce an unrestricted force flag or arbitrary client-supplied target list.

## 5. Preview, conflict authority and result contract

Extend the current typed operation with assignment mode (ordinary / assign-and-lock), plus a typed conflict decision for the latter (unlocked-only / force-reviewed). An individual scope derives individual intent on the backend. Legacy requests without new fields mean ordinary assignment and cannot force locks.

Preview enumerates a complete canonical candidate snapshot with identity, persisted pair, protection/error state and relevant live session IDs. Return candidate/protected/invalid counts, eligible targets and sessions for each offered decision, and conflict rows. Keep `targetCount` and `liveSessionCount` as actionable counts for the selected policy, add explicitly named candidate counts, and update Rust/TS/UI together. A protected row omitted by policy is `skippedLocked`, not `configWriteFailed`.

Bind confirmation to operation kind, scope and anchor identity, requested pair, restart choice, canonical candidate membership, per-target pair/protection state, eligible membership, and relevant sessions. A fingerprint of paths alone is insufficient: pair or flag changes on the same paths must invalidate it. Use canonical typed serialization and a collision-resistant digest or server-held snapshot; the reviewed conflict decision must be tied to that snapshot. The digest is a staleness check, not caller authentication.

Apply reacquires the operation turn, re-enumerates and compares before any writes. Changed membership, protection, pair or restart targets returns a stale-preview result and requires fresh review; never automatically carry an old force decision forward. Recheck each target's expected state within its config mutation as well. If a later cross-process change is detected after earlier writes, stop that target without force expansion and report partial results. A `Force all` decision overrides valid protected selections only; it cannot override malformed state, failed authorization or target substitution.

The config-layer typed writer distinguishes ordinary bulk, assign-and-lock unlocked-only, reviewed force, and deliberate individual intent. Backend validation constructs that intent; the low-level guard rereads protection before mutation. Already-equal unlocked pairs can still need a lock write, so the old redundant-pair UI gate must not disable `+ lock`. Zero eligible targets produces a successful no-op with zero restarts.

Use separate typed preview/apply operations for unlock, sharing enumeration and state validation but carrying neither assignment pair nor restart option. Return actual cleared paths/count, already-unlocked paths, failures and refreshed remaining protection. Unknown/error targets prevent a false `0 of N` success claim. No optimistic all-success toast on partial failure.

Assignment results separately report written paths, newly protected paths, skipped-locked paths, invalid/stale/write errors, restarted session IDs and `destroyedButNotRecreated` outcomes. Persisted success survives restart failure; retry does not silently repeat successful restarts. Reuse `coding_agent_profile_selection_updated` with operation/affected-path data so all windows and web clients refresh from backend state. Default changes refresh default state without pretending existing replicas changed.

## 6. Concurrency, lifecycle and failure ownership

Extend the existing async selection-operation turn through revalidation, writes and completion/cancellation of all associated restarts. Bulk assignment, unlock, deliberate pair/profile changes and the persisted self-switch sequence must share it. The next lock/unlock is acknowledged only after earlier restarts finish; UI shows pending during this wait. Accept this initial serialization cost instead of a more complex per-target scheduler. Integrate with the existing `SelectionCoordinator` restart path; do not add a second lifecycle owner.

Acquire the operation turn before session/lifecycle work and short-lived config guards. Never hold session/settings locks while waiting for the turn, never hold a synchronous config guard across restart awaits, and never reacquire the same turn from a restart invoked inside it. The backend owns the operation beyond modal close or web disconnect. Cancellation drains or cancels already-started lifecycle work before release; timeout reports actual partial state. A detached restart must not execute later after unlock success. Exact guard placement and the self-switch closure sequence require a lock-order review and executable interleaving tests before readiness.

For app/CLI JSON writes, use a stable sidecar OS file lock keyed by canonical config identity, held from read through atomic publish, with bounded acquisition and OS release on process exit. Do not lock the replaceable config inode itself. The repository's `TeamConfigMutationGuard` already demonstrates `File::try_lock`, timeout and sentinel validation; reuse that established technique within the low-level IO layer without importing command/UI dependencies. All writers of the same replica config must participate, including repeated creation and profile-only writes. Matrix default publication and initial reads must provide a consistent policy snapshot. Avoid nested file guards, or define one deterministic order where unavoidable.

The config guard prevents lost bytes; the async turn prevents late contradictory restarts. Existing CLI member-add remains usable without a live GUI through the shared file guard and preserves existing pair/flag. Persisted self-switch runs through the backend owner. A temporary explicit wake override changes execution, not persisted protection. A new standalone lock CLI is not required by this issue; any future one must use the same authorized backend operation, not bypass it with direct selection writes.

Windows alias/case/verbatim paths must resolve to the same lock identity. Validate the selected sidecar location, filesystem support, timeout and process-death behavior on supported platforms before readiness. Concurrent older binaries or external editors that ignore the protocol cannot be claimed safe; document that compatibility boundary. No bulk transaction or rollback across all replica files is promised: per-file atomicity plus truthful partial results is the intended model.

## 7. Work sequence and acceptance evidence

These are ordered work steps, not published phases/sub-issues or permission to start implementation.

1. **Backend owner, before code:** freeze base and paths; inventory every pair/profile/config writer and creation caller; verify Matrix-default storage, file-lock placement, and SelectionCoordinator lock order. Refresh only evidence affected by relevant base drift. Resolve technical blockers below.
2. **Backend owner:** implement strict state/default readers, typed guarded mutations and lossless creation/retry behavior. Demonstrate persistence and cross-process tests before exposing actions.
3. **Backend owner:** integrate preview, decisions, unlock/default operations, operation lifetime and restart reporting; register desktop/web adapters against the same inner functions.
4. **UI owner:** update DTOs/API/discovery and implement v3 controls, dialog, filtered informational list and completion feedback; extend existing modal tests and transport refresh coverage.
5. **Reviewer/tech lead:** verify behavior, dependency/layering and scoped diff. Derive checks from actual toolchain/workflows, then run the implementation/delivery gates when separately authorized.

| Focused test | Required assertion |
| --- | --- |
| Legacy and malformed configs | Absent flag is false; invalid JSON/tooling/flag is visible and unchanged; legacy profile fields agree after a write; unrelated keys/history/hash survive. |
| Three scopes and mixed state | Kind spans the same canonical Matrix across roots; homonymous Matrices and outside-room replicas stay untouched; offline replicas remain candidates; every protected session is skipped by ordinary bulk. |
| Bulk conflict actions | Cancel has zero side effects; unlocked-only assigns and locks only unlocked targets; force requires the reviewed snapshot and keeps overwritten targets locked. Include already-equal protected pairs and all-protected targets. |
| Independent unlock | Focused unlocked replica does not disable a nonempty room/Kind action; pairs/defaults stay byte-equivalent, no restart requested; zero candidates is a no-op; partial errors do not produce full-success counts. |
| Completed v3 result | Three locks among four room replicas become `0 of 4 protected`; success count and disabled action appear; outside-room lock, saved pairs and default survive. |
| Creation lifecycle | Default copied for new room/member; creation exception wins; later default edits affect neither existing replicas nor exceptions; repeated add/reopen/restore preserves custom context and unknown keys; actual recreation uses current default. |
| Individual/wake behavior | Direct pair edit/self-switch preserves lock; profile-only edit keeps Coding Agent and dual fields; protected null clear is rejected; temporary wake override does not mutate saved pair/flag. |
| Stale/force safety | Flag, pair, candidate or relevant-session changes between preview/apply invalidate confirmation; a forged scope/target/decision cannot expand authority; per-file races produce diagnostics rather than stale overwrite. |
| Ordering and ownership | Pause a bulk between write/restart; subsequent unlock cannot acknowledge early. Repeat with self-switch, client disconnect, cancellation, restart timeout and process failure; no deadlock or late restart. |
| Cross-process IO | Race CLI add-member with app lock/pair/profile writes; both changes survive. Test canonical aliases, acquisition timeout, process death and atomic-publication failure on Windows and supported Unix hosts. |
| UI/transport | Six radios are mutually exclusive; unlock scope is independent; Cancel is left; same-pair `+ lock` works; protected-only list preserves total/eligibility; desktop/web show identical results and refresh. |
| Existing resolution/failures | Requested/effective fallback and content drift remain visible; invalid default fails safely; partial writes/restarts retain accurate counts and destroyed-not-recreated diagnostics. |

## 8. Review prerequisites and delivery limits

The following gates remain **unexecuted for product implementation**. They block readiness, not archival of this useful plan. No Full-plan partition or consensus round is being initiated.

| Gate | Evidence, owner/time, failure behavior |
| --- | --- |
| CI and toolchain parity | Implementer/tech lead before mutation: inspect actual workflow jobs/path filters, required checks, lockfiles and pinned/resolved tools; record exact local commands and versions. Run proportionate reproducible tests; unexplained failures stop acceptance. Reviewer requires triggered and configured-required CI green on the exact eventual PR-head SHA. No guessed commands are presented as verified. |
| Authorized Git and base drift | Tech lead before implementation and before PR update: open issue, authorized issue-numbered implementation branch, base/head/index/tracked/untracked evidence inside repo-*; classify fetched target drift by semantic relevance and refresh affected evidence only. No direct-target push. This archive branch authorizes plan storage only. |
| Scope, process and recovery | Implementer freezes intended paths/commands and relevant inherited configuration; uses explicit repo cwd, scoped artifacts and bounded commands with retained diagnostics. Recheck affected files before writes; restore only own unchanged output on failure, preserving external edits. Final diff/status must match the frozen scope. |
| Concurrency and persistence | Backend owner before readiness: complete writer inventory, OS-lock/platform proof, lock-order and lifecycle tests above. A process-local mutex, atomic rename alone, or one pre-restart reread is insufficient. Failures retain original bytes or report the precise committed subset. |
| Dependency and layering | Architect/reviewer before readiness: enumerate added/removed module arcs with actual call sites. Prefer existing modules/arcs; persistence must not gain AppHandle, Tauri, web, or command-layer dependencies. Obtain clean-base/final-tree measurements with the repository's levelization workflow, or explicit per-arc transitive SCC analysis if the instrument is absent. No cycle-safety assertion has been made here. |

Dependency acceptance is: `coverage.graphShape.cyclicSccs` unchanged, identical cyclic SCC member sets, zero new arcs crossing previously-clean SCC boundaries, regenerated `module-arcs.txt` byte-identical to its committed record, and applicable structural layering guards green. The reviewer must resolve the authorized instrument location and invocation rather than read another agent's private skills. Instrument failure and missing evidence are not a pass. If an intended call fails this gate, revise placement and review the affected design before implementation readiness.

Outstanding technical validation is bounded: complete writer/creation coverage; concrete shared-guard placement and lock order; Matrix-default reader/writer and creation-input plumbing; platform exclusion behavior; exact wire changes and applicable build/CI/cycle commands. Product scope, Matrix identity, conflict choices, independent unlock, and creation-only inheritance are settled.

Archive validation: pinned branch/base and clean tracked/index state inspected; approved v3 hash matched; Dockerfile-only drift checked; package is ignored by Git, so publication requires the tech lead's explicit scoped inclusion. Architect creates only this PLAN.md; tech lead owns identical shared-copy archival, package publication and issue linking. No approved prototypes or product sources are modified by this delivery.
