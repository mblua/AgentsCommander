# Preserve Coding Agent + Profile selections with scoped locks

## Problem and approved outcome

Bulk Coding Agent/Profile assignment currently overwrites deliberately customized replicas. Users need to protect the selected pair, manage that protection for one replica, all replicas of the same kind, or an entire room, and give new replicas a protected default with local exceptions.

The user approved the final interactive **v3** prototype, including the post-unlock result, on 2026-09-11 UTC. This issue records that accepted product design and its implementation plan. No production feature has been implemented yet.

## Approved interaction

### Assign a pair, optionally locking it in the same action

- Keep the existing `Apply to` row: `This replica`, `All replicas of this kind`, `Entire room`.
- Immediately below it, add the same choices with `+ lock`: `This replica + lock`, `All replicas of this kind + lock`, `Entire room + lock`.
- These rows represent one mutually exclusive selection. A `+ lock` action assigns the selected Coding Agent and requested Profile letter and protects the resulting pair in the same operation.
- The upper selection-lock bar displays protection state and the saved pair; it must not compete with the lower assignment controls.
- An ordinary bulk assignment without `+ lock` skips protected replicas, including their restarts. Deliberate individual assignment remains possible and preserves an existing lock.

### Resolve conflicts for bulk `+ lock`

If targets are already protected, show a dialog identifying the conflicting replicas and each current pair versus the requested pair before applying. Offer the three approved English actions:

| Action | Effect |
| --- | --- |
| `Cancel` | Change nothing: no assignment, lock mutation, or restart. |
| `Apply only to unlocked` | Assign and lock only previously unlocked eligible replicas; preserve protected replicas and their sessions. |
| `Force all, including locked` | Also overwrite previously protected targets and keep them protected. |

`Cancel` is to the left of the action buttons, matching the existing modal. With no conflicts, apply the assignment and lock directly. A force decision must be explicit, scoped to the reviewed targets, and enforced by the backend rather than only the UI.

### Remove locks independently

- In the upper bar, provide `Remove lock from` with `This replica`, `All replicas of this kind`, and `Entire room`.
- This scope is independent of the lower `Apply to`/`+ lock` scope. Display affected counts and an appropriate action label.
- Unlocking changes protection only: preserve every Coding Agent/Profile pair, do not restart sessions, and do not modify the future-replica default.
- Bulk unlock remains available when the focused replica is unlocked but other replicas in the selected kind/room are protected.
- With zero protected candidates, disable the action and show that there is nothing to remove.

### Compact protected-replica list

The informational kind list shows **only protected rows**. Keep the full `N of M protected` count and complete underlying target data. Example: with room-12 protected and room-15 unprotected, show only room-12 but retain `1 of 2 protected`. Do not remove the unprotected replica from assignment eligibility or unrelated target previews. With zero protected replicas, show the summary and no protected rows.

### Show the completed unlock result

Provide visible feedback after executing unlock, not just the pre-action state. The accepted `Entire room` example removes 3 locks among 4 replicas and shows:

- `Lock removed from 3 replicas · Coding Agent + Profile kept · no restart`;
- `0 of 4 protected`, unlocked states, and disabled `Nothing to remove`;
- preserved pairs and no restarts;
- replicas outside the room and the future default unchanged.

The v3 demo includes a direct `10 · Resultado: room desbloqueada` scenario and the normal before/action/after interaction.

## Scope and inheritance

- Protect the pair **Coding Agent ID + requested Profile letter**, not a frozen copy of the profile's command/environment/model/version. Existing content-drift/fallback behavior remains visible.
- `This replica` targets one replica; `Entire room` targets replicas in that room, not every room of its logical Team.
- `All replicas of this kind` follows the existing kind identity based on the canonical origin Matrix and may include matching replicas across configured roots. Do not group by provider or a similar display name.
- **Confirmed by the user: future replicas inherit the type default, with exceptions per replica.** Materialize the default pair and protection at creation. Later local choices are authoritative; modifying the default does not retroactively overwrite existing replicas. Reopening preserves local state; genuinely recreated replicas use the current creation default.
- Keep individual selection/profile/self-switch behavior consistent with the protection contract; a temporary explicit wake override is distinct from changing persisted selection.

## Investigation and engineering requirements

The initial investigation found these relevant paths (verify symbols on the implementation base):

- `src-tauri/src/commands/config.rs`: assignment scopes, target enumeration, preview/confirmation and apply/restart orchestration.
- `src-tauri/src/config/coding_agent_profiles.rs`: saved pair, legacy profile fields and resolution.
- `src-tauri/src/config/local_config_io.rs`: configuration mutation/publication.
- `src-tauri/src/commands/entity_creation.rs` and `src-tauri/src/cli/team.rs`: replica creation and repeated `team add-member`.
- `src-tauri/src/commands/session.rs`, `src-tauri/src/phone/mailbox.rs`: individual restart/wake/self-switch behavior.
- `src-tauri/src/commands/ac_discovery.rs`, `src-tauri/src/web/commands.rs`: discovery and desktop/web parity.
- `src/sidebar/components/AgentPickerModal.tsx`, `src/shared/types.ts`, `src/shared/ipc.ts`, and sidebar styles: approved UI and contracts.

Persist protection compatibly with older configurations and preserve unrelated configuration keys. Fix the demonstrated repeated-member-add path that can replace a replica configuration and erase `tooling`, custom context, and unknown keys. Handle unreadable/invalid protection state explicitly without silently treating it as unlocked.

Preview, conflict decisions, affected counts and results must agree. Revalidate stale previews and lock-state changes before mutation. Coordinate assignment, unlock, individual writers and optional restarts so a lock/unlock cannot be acknowledged while an earlier bulk operation can still contradict it. Address concurrent app/CLI writes and report partial write/restart failures accurately. Preserve desktop/web behavior and authorization boundaries.

## Acceptance criteria

- [ ] Assignment and `+ lock` rows match the approved placement and are mutually exclusive.
- [ ] Pair assignment plus protection is coherent for replica/kind/room scopes, including replicas without live sessions.
- [ ] Bulk conflict dialog identifies the affected replicas and implements all three actions exactly, with `Cancel` on the left.
- [ ] Ordinary bulk operations skip protected replicas and their restarts.
- [ ] Independent unlock scopes preserve pairs, sessions, outside targets and future defaults; zero candidates are a clear no-op.
- [ ] Compact informational lists omit unprotected rows while preserving full counts and target eligibility.
- [ ] Completed room unlock visibly shows success, removed count, zero remaining protection, and inactive action.
- [ ] New replicas inherit the creation default; per-replica exceptions survive later default changes and reopen/retry flows.
- [ ] Repeated member addition preserves selection, protection, custom context and unknown keys.
- [ ] Legacy/malformed configurations, profile fallback/content changes, stale confirmations, concurrent mutations, and partial failures have explicit behavior and focused tests.
- [ ] UI, desktop/web backend and affected entrypoints agree on effective state and outcomes.

## Design evidence and planning status

Approved reference: final `index-v3.html` (SHA-256 `7901780e9d936c603ba0a704b9cf002197a8bdf5cf687b795b2c0fe37ae7f7b0`). Earlier v1/v2 versions are historical, not the implementation reference.

The last prototype check reported 31/31 assertions passing and valid headless rendering. These verify simulated prototype behavior, **not production behavior**. Product source was not modified. The source baseline synchronized for issue/plan archival is `7aef1d14e6bb0255b1430614c078145335dd54a4`; changes since the prototype reference base are Dockerfile-only.

## Plan package and approved prototype

- [Consolidated implementation plan](https://github.com/mblua/AgentsCommander/blob/feature/1937-selection-lock-plan/plans/issue_1937/PLAN.md)
- [Complete issue_1937 package and navigation](https://github.com/mblua/AgentsCommander/tree/feature/1937-selection-lock-plan/plans/issue_1937)
- [Approved interactive v3 HTML](https://github.com/mblua/AgentsCommander/blob/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/index-v3.html) — download and open locally; the prototype is self-contained.
- [User decisions and approval](https://github.com/mblua/AgentsCommander/blob/feature/1937-selection-lock-plan/plans/issue_1937/evidence/candados-decision-usuario.md)
- [Prototype guide and verification notes](https://github.com/mblua/AgentsCommander/blob/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/README-v3.md)

Identical local package: `D:\0_repos\AgentsCommander_iac\.ac\plans\issue_1937\`. Repository copy: `plans/issue_1937/` on `feature/1937-selection-lock-plan`.

The user approved the UX. The consolidated technical plan is archived for implementation review; no production implementation, merge, or delivery certification is claimed. Earlier prototype versions and historical investigations are preserved as evidence; current user decisions and the accepted v3 take precedence over superseded proposals.

<details>
<summary>Approved v3 screens</summary>

### Assign and lock

![Apply to and + lock](https://raw.githubusercontent.com/mblua/AgentsCommander/refs/heads/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/vistas-v3/apply-lock.png)

### Explicit conflict decisions

![Conflict dialog](https://raw.githubusercontent.com/mblua/AgentsCommander/refs/heads/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/vistas-v3/conflicto.png)

### Independent unlock scopes

![Remove lock scopes](https://raw.githubusercontent.com/mblua/AgentsCommander/refs/heads/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/vistas-v3/remove-scope-barra.png)

### Compact protected-only list

![Only protected rows, full counts](https://raw.githubusercontent.com/mblua/AgentsCommander/refs/heads/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/vistas-v3/tipo-preview-lista.png)

### Result after unlocking Entire room

![Room unlock result](https://raw.githubusercontent.com/mblua/AgentsCommander/refs/heads/feature/1937-selection-lock-plan/plans/issue_1937/prototypes/vistas-v3/resultado-unlock-room.png)

</details>
