# Plan: Issue #83 — Diagnostic logging for workgroup coordinator discovery

> Branch: `feature/83-discovery-debug-logging` (off `main` @ `96860c0`)
> Issue: https://github.com/mblua/AgentsCommander/issues/83
> Scope: **diagnostic logging only**, zero behavior change.

---

## 1. Requirement

When a project is opened with binary B that was originally created by binary A, the `tech-lead` replicas in `phi_fluid-mblua` lose their coordinator badge. Filesystem and existing logs already rule out (A) version drift, (B) missing `projectPaths`, (C-trivial) replica-shadow gating, and (D-trivial) corrupt `_team_*/config.json`. The remaining hypothesis space is:

- **Hypothesis C** — backend computes `is_coordinator = false` silently for foreign-created replicas.
  - C1: a `_team_*` directory is silently dropped from `discover_teams` (read/parse failure).
  - C2: the team is discovered but `is_coordinator` rejects on the §AR2-strict project guard (cross-project name match, project mismatch).
  - C3: the team is discovered but `is_coordinator` rejects because `agent_suffix(coord_name)` differs from `agent_suffix(agent_name)` (e.g. coordinator ref resolves differently across binaries).
- **Hypothesis D** — backend says `true`, frontend swallows the flag (cache, render condition).

This plan adds a deterministic log slice that, after the next reproduction, pinpoints exactly which sub-hypothesis is correct.

**Acceptance criterion (from issue):** opening `phi_fluid-mblua` with the new mb build produces, per `tech-lead` replica, a log line stating its computed `is_coordinator` value, plus enough surrounding context to know whether the team that should claim it was discovered, parsed, and matched on the correct branch.

---

## 2. Affected files

Two files only, both backend-only. No frontend changes — the per-replica log line plus visual UI inspection is sufficient to discriminate C from D.

| File | Edits |
|---|---|
| `src-tauri/src/config/teams.rs` | 4 surfaces (T1–T4) |
| `src-tauri/src/commands/ac_discovery.rs` | 4 surfaces (A1–A4) |

No new modules, no new structs, no new dependencies. All `log::*` macros already in use throughout these files.

---

## 3. Change description

### Conventions

- Prefix all new lines with `[teams]` (in `teams.rs`) or `[ac-discovery]` (in `ac_discovery.rs`), matching the existing convention (see `teams.rs:892` and `ac_discovery.rs:644`, `:707`, `:845`).
- Single-line format strings. Field separator: ` ` between key/value pairs; `key=value` for atoms; `key='value'` for strings that may contain spaces or path-like content.
- All `is_coordinator` verdict logs mention the branch taken (`direct-match`, `wg-aware-match`, `reject-unqualified`, `reject-project-mismatch`) so a grep on the log file produces a clean truth table.

### Surface T1 — Per-project-path enumeration in `discover_teams`

**File:** `src-tauri/src/config/teams.rs`
**Function:** `discover_teams` (currently lines 863–898).

**Where:**
- T1.a — at line 868, immediately before `let base = Path::new(repo_path);` (the `for repo_path in &settings.project_paths {` is at line 867). Insert as the first line of the loop body. (Round-2 fix: D1 ≡ G6 — original "871" cited the closing `}` of the inner if-block, which would have placed the log after the early-continue and broken its O(project_paths) cardinality.)
- T1.b — replace the bare `continue;` on line 870 (inside `if !base.is_dir()`) with a logged variant.
- T1.c — at line 887, immediately after the inner `for project_dir in dirs_to_check {` loop opens, log the project dir being scanned.

**Code (insert verbatim):**

T1.a — first statement inside `for repo_path in &settings.project_paths {`:
```rust
log::debug!("[teams] discover_teams: scanning project_path='{}'", repo_path);
```

T1.b — replace the existing block:
```rust
if !base.is_dir() {
    continue;
}
```
with:
```rust
if !base.is_dir() {
    log::debug!("[teams] discover_teams: project_path skipped (not a directory) — path='{}'", repo_path);
    continue;
}
```

T1.c — first statement inside `for project_dir in dirs_to_check {`:
```rust
let teams_before = teams.len();
log::debug!("[teams] discover_teams: entering project_dir='{}'", project_dir.display());
```

And immediately after the `discover_teams_in_project(&project_dir, &mut teams);` call (line 888), insert:
```rust
log::debug!(
    "[teams] discover_teams: project_dir='{}' produced {} team(s)",
    project_dir.display(),
    teams.len() - teams_before
);
```

**Levels:**
- `debug!` for the per-path scan + per-dir entry/exit (fires O(project_paths × dirs_per_path), can be chatty in mb's case 2 paths × ~3 children = ~6 lines).
- `debug!` for the skip-on-non-directory branch. (Round-2 fix: G4 — `discover_teams()` is invoked from 12+ call sites across CLI, mailbox, startup, and Tauri commands; emitting `warn!` on stale `projectPaths` entries — which are normal user state, e.g. removed/USB-detached repos — would flood the warn channel and train operators to ignore it. The skip is implicitly captured by the absence of a subsequent T1.c entry for that path; investigation runs already enable `agentscommander_lib::config::teams=debug` per §5.)

**Why this discriminates C1:** The existing aggregate `[teams] discovered N team(s) across M project path(s)` (line 892) only gives a global count. T1.c gives a per-project-dir count, so we can immediately see which `.ac-new` produced fewer teams than expected. mb sees 5/6 teams — T1.c will tell us in which project the missing team was supposed to live.

**Gating:** unconditional within their respective loop bodies (debug-level keeps cost low).

---

### Surface T2 — Silent-drop reasons in `discover_teams_in_project`

**File:** `src-tauri/src/config/teams.rs`
**Function:** `discover_teams_in_project` (currently lines 901–986).

**Where:** Replace the chained `Option`-coalescing block at lines 935–941:
```rust
let parsed: serde_json::Value = match std::fs::read_to_string(&config_path)
    .ok()
    .and_then(|c| serde_json::from_str(&c).ok())
{
    Some(v) => v,
    None => continue,
};
```
with the imperative form below. Same semantics — just adds a logging hook on each silent-drop branch. **This is the only surface that touches existing control flow**, but the post-condition on `parsed` is identical (a `serde_json::Value` for valid configs, `continue` otherwise), so it is logging-only by behavior.

**Code (replace verbatim):**

```rust
let raw = match std::fs::read_to_string(&config_path) {
    Ok(s) => s,
    Err(e) => {
        log::warn!(
            "[teams] dropped team — project='{}' team_dir='{}' reason='read_failed' err='{}' path='{}'",
            project_folder,
            dir_name,
            e,
            config_path.display()
        );
        continue;
    }
};
let parsed: serde_json::Value = match serde_json::from_str(&raw) {
    Ok(v) => v,
    Err(e) => {
        log::warn!(
            "[teams] dropped team — project='{}' team_dir='{}' reason='parse_failed' err='{}' path='{}'",
            project_folder,
            dir_name,
            e,
            config_path.display()
        );
        continue;
    }
};
```

**Also insert** at line 919 (immediately after `for entry in entries.flatten() {`), as the first statement of that loop body, a `debug!` that fires for *every* entry inspected, regardless of whether it passes the `_team_` prefix check. This catches the case where the `_team_` directory exists but is not iterated (e.g. permissions, encoding):

```rust
log::trace!(
    "[teams] discover_teams_in_project: inspecting entry — project='{}' entry='{}'",
    project_folder,
    entry.file_name().to_string_lossy()
);
```

(Use `trace!` here — a 105-replica project may have hundreds of `.ac-new` entries; this is the noisiest surface, hidden by default. Round-2 fix: G2 — the `entry.file_name()` call is now inline inside the macro args, so the `OsString` allocation is short-circuited by `log!`'s level check when trace is disabled. The earlier `let _entry_name = ...` binding paid the allocation unconditionally and would have been flagged by `clippy::used_underscore_binding`.)

**Levels:**
- `warn!` for read/parse failures (rare, actionable, must be visible at default log level).
- `trace!` for the per-entry inspection (very chatty; only enabled when `RUST_LOG=trace` is requested).

**Why this discriminates C1:** A team that exists on disk but is dropped at parse/read time will produce one `warn!` per drop, naming the project + team_dir + reason. The investigation will know *exactly* which file is malformed (or unreadable). The `trace!` provides a fallback if the team dir itself is being filtered upstream (e.g. `.ac-new/.gitignore`-related Windows ACL weirdness, NTFS reparse points).

**Gating:** unconditional inside the existing control flow. The `trace!` level masks the per-entry noise unless explicitly requested.

---

### Surface T3 — Per-team summary at end of `discover_teams_in_project`

**File:** `src-tauri/src/config/teams.rs`
**Function:** `discover_teams_in_project`.

**Where:** Immediately after the `teams.push(DiscoveredTeam { ... });` block at line 977–984. Insert before the closing `}` of the outer `for entry in entries.flatten()` loop (line 985 area).

**Code (insert verbatim, immediately after the `teams.push` block):**

```rust
let pushed = teams.last().expect("just pushed");
log::debug!(
    "[teams] discovered team — project='{}' team='{}' coord_name={:?} coord_path={:?} agent_count={}",
    pushed.project,
    pushed.name,
    pushed.coordinator_name,
    pushed.coordinator_path.as_ref().map(|p| p.display().to_string()),
    pushed.agent_names.len()
);
```

**Note for dev-rust:** `expect("just pushed")` is safe because the preceding `teams.push` always runs in this branch. If borrow-checker complains about the immutable `pushed` borrow overlapping the mutable scope of `teams`, reformulate as a separate let-binding *before* the `teams.push`:

```rust
let team_name_log = team_name.clone();
let project_log = project_folder.clone();
let coord_name_log = coordinator_name.clone();
let coord_path_log = coordinator_path.as_ref().map(|p| p.display().to_string());
let agent_count_log = agent_names.len();
teams.push(DiscoveredTeam { /* … unchanged … */ });
log::debug!(
    "[teams] discovered team — project='{}' team='{}' coord_name={:?} coord_path={:?} agent_count={}",
    project_log, team_name_log, coord_name_log, coord_path_log, agent_count_log
);
```

Either form is acceptable; pick whichever compiles cleanly with the smallest diff.

**Level:** `debug!` — fires once per discovered team per `discover_teams()` call. Round-2 fix: G3 — `discover_teams()` is invoked from 12+ call sites (`cli/send.rs:133`, `cli/list_peers.rs:316,437`, `cli/close_session.rs:92`, `lib.rs:522`, `phone/mailbox.rs:471,1154,1427`, `commands/ac_discovery.rs:571,1008`, `commands/phone.rs:12,23`, `commands/entity_creation.rs:1130`, `commands/session.rs:290`). At `info!`, ~6 teams × 12 sites under load would emit dozens of lines per minute, drowning the existing aggregate `[teams] discovered N team(s)` (which deliberately remains the single info-level endpoint). Investigation runs already enable `agentscommander_lib::config::teams=debug` per §5, so demoting T3 costs the bug investigation nothing while keeping default-log noise stable.

**Why this discriminates C1:** Confirms positively that each team made it into the snapshot, with the resolved coordinator data. Differential diagnosis: if T2 emits no `warn!` but T3 emits only 5 (not 6) summaries, the missing team has a non-parse reason (e.g. dir missing, prefix not matched) that requires the T2 `trace!` fallback or a filesystem audit. If T3 shows the `_team_dev-team` of `phi_fluid-mblua` with `coord_name=Some("phi_fluid-mblua/tech-lead")` and `coord_path=Some("…/.ac-new/_agent_tech-lead")`, then sub-hypothesis C1 is ruled out for that team.

**Gating:** unconditional.

---

### Surface T4 — Branch-level verdict in `is_coordinator`

**File:** `src-tauri/src/config/teams.rs`
**Function:** `is_coordinator` (currently lines 408–432).

**Where:** Inside the function, on the four interesting code paths. The function currently looks like:

```rust
fn is_coordinator(agent_name: &str, team: &DiscoveredTeam) -> bool {
    if let Some(ref coord_name) = team.coordinator_name {
        if agent_matches_member(agent_name, coord_name, team.coordinator_path.as_ref()) {
            return true;
        }
        if let Some(wg_team) = extract_wg_team(agent_name) {
            let (agent_project, _) = split_project_prefix(agent_name);
            let Some(agent_project) = agent_project else {
                return false;
            };
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                return true;
            }
        }
    }
    false
}
```

Modify to (changes are pure additions of `log::debug!` lines + one extra branch for logging the project-mismatch rejection — semantics unchanged):

```rust
fn is_coordinator(agent_name: &str, team: &DiscoveredTeam) -> bool {
    if let Some(ref coord_name) = team.coordinator_name {
        if agent_matches_member(agent_name, coord_name, team.coordinator_path.as_ref()) {
            log::debug!(
                "[teams] is_coordinator: direct-match → true — agent='{}' team='{}/{}' coord='{}'",
                agent_name, team.project, team.name, coord_name
            );
            return true;
        }
        if let Some(wg_team) = extract_wg_team(agent_name) {
            let (agent_project, _) = split_project_prefix(agent_name);
            let Some(agent_project) = agent_project else {
                if wg_team == team.name && agent_suffix(agent_name) == agent_suffix(coord_name) {
                    log::debug!(
                        "[teams] is_coordinator: reject-unqualified → false — agent='{}' team='{}/{}' coord='{}' (suffix would match)",
                        agent_name, team.project, team.name, coord_name
                    );
                }
                return false;
            };
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                log::debug!(
                    "[teams] is_coordinator: wg-aware-match → true — agent='{}' team='{}/{}' coord='{}' agent_project='{}'",
                    agent_name, team.project, team.name, coord_name, agent_project
                );
                return true;
            }
            if wg_team == team.name
                && agent_project != team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                log::debug!(
                    "[teams] is_coordinator: reject-project-mismatch → false — agent='{}' agent_project='{}' team_project='{}' team='{}' coord='{}'",
                    agent_name, agent_project, team.project, team.name, coord_name
                );
            }
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) != agent_suffix(coord_name)
            {
                log::debug!(
                    "[teams] is_coordinator: reject-suffix-mismatch → false — agent='{}' team='{}/{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
                    agent_name, team.project, team.name, coord_name,
                    agent_suffix(agent_name), agent_suffix(coord_name)
                );
            }
            if wg_team == team.name
                && agent_project != team.project
                && agent_suffix(agent_name) != agent_suffix(coord_name)
            {
                log::debug!(
                    "[teams] is_coordinator: reject-both-mismatch → false — agent='{}' agent_project='{}' team_project='{}' team='{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
                    agent_name, agent_project, team.project, team.name, coord_name,
                    agent_suffix(agent_name), agent_suffix(coord_name)
                );
            }
        }
    }
    false
}
```

**Level:** `debug!` — fires O(replicas × teams) per discovery call (~525 in mb's case). Hidden at default log level; only the investigation run with `RUST_LOG=debug` emits these.

**Gating:** Each `debug!` fires only on a "name-overlap" path (suffix-or-name match). The non-interesting "no name overlap at all" case emits nothing — so noise is bounded by `replicas × teams_with_same_name`, far less than the worst case.

**Why this discriminates C2 and C3 — both positively:**
- **C2** (project-guard rejection): one `reject-project-mismatch` line per `tech-lead` replica × `dev-team` team combination if the FQN's project differs from `team.project`. Smoking gun for cross-binary filesystem-vs-config mismatches.
- **C3** (suffix mismatch): one `reject-suffix-mismatch` line when both `wg_team == team.name` and `agent_project == team.project` succeed but `agent_suffix(agent_name) != agent_suffix(coord_name)` — i.e. every preceding gate matched but the leaf-name resolved differently. Bounded by `replicas × same-name-and-project teams`, which in the issue's repro is `~105 replicas × 1 dev-team in phi_fluid-mblua` ≈ at most ~105 lines. (Round-2 fix: G1 — earlier draft offered no positive log for this case and required diagnosis-by-elimination from `A1=false ∧ T3=present ∧ no T4 reject`. That elimination collapses if any unenumerated 4th sub-hypothesis exists, e.g. a `coordinator_name=None ∧ coordinator_path=Some` schema skew or a future replica naming format that breaks `extract_wg_team`. The whole reason this issue exists is that prior elimination evidence — `Deferred non-coordinator session` + UI absence — failed to localize the gate; reproducing that pattern in the new instrument would be self-defeating. See §7 for the full G1-vs-D3 adjudication.)

The six log lines together form a complete positive-evidence decision tree: `direct-match` and `wg-aware-match` for the success paths, `reject-unqualified` / `reject-project-mismatch` / `reject-suffix-mismatch` / `reject-both-mismatch` for the four named failure modes (round-3 fix: H4 — round-2 design omitted the `(project != team.project ∧ suffix != suffix)` leaf, leaving a silent leaf inside the inner `if let Some(wg_team) = …` arm; that case is now positively logged). When `extract_wg_team(agent_name)` and `team.coordinator_name = Some(_)` are both true, every reachable path through the conditional now emits exactly one log line — successes log on the path returning `true`, rejections log on the path falling through to the terminal `false`.

A `tech-lead` replica that emits *no* T4 line at all under `=debug` capture is now a strictly narrower signal than the round-2 framing: it means **either** `extract_wg_team(agent_name)` returned `None` (replica is not in a `wg-N-team-name`-shaped directory — surprising for any `tech-lead` replica reachable through normal discovery) **or** `team.coordinator_name = None` (the team's `_team_*/config.json` has no `coordinator` key, which T3's `coord_name=None` summary will independently surface). With the H4 closure, the architect-named hypothesis space (C1, C2, C3) is fully covered by positive emissions; the remaining "no T4 line" interpretations are tightly constrained and orthogonal to the C-vs-D verdict the issue asks us to deliver.

---

### Surface A0 — Per-call monotonic ID (precondition for A1–A4)

**File:** `src-tauri/src/commands/ac_discovery.rs`

**Why this surface exists.** Round-2 fix: G5 — `discover_ac_agents` and `discover_project` are user-reachable from the frontend and may execute concurrently (initial sidebar populate triggers a refresh while a per-project `discover_project` is already in flight). With identical A1/A2 format strings, two interleaved calls produce a sequence like `replica_A1 replica_B1 replica_A2 replica_B2 … summary_A summary_B`, and the operator has no way to retroactively bind each replica line to its summary. The earlier §5 claim that A3/A4 summaries could partition the tape was wrong: summaries land at the end of the call, not the start, so the prefix of replica lines is unattributable. The repro path for issue #83 (sidebar populate of `phi_fluid-mblua`) is multi-call by construction, so this is not hypothetical.

**Where:** Module-level static at the top of `ac_discovery.rs`. Insert near the existing `use std::path::{Path, PathBuf};` and `use std::sync::Arc;` block (top of file). The `std::sync::atomic` types are stdlib — no new dependency.

**Code (insert near top of `ac_discovery.rs`, alongside existing `use std::*` declarations):**

```rust
use std::sync::atomic::{AtomicU64, Ordering};

static DISCOVERY_CALL_ID: AtomicU64 = AtomicU64::new(0);
```

**At the top of `discover_ac_agents` body** — immediately after the `let cfg = settings.read().await;` at line 565, before the `// Discovery-wide team snapshot …` comment block at line 566–570:

```rust
let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);
```

**In `discover_project` body** — placed AFTER the `.ac-new`-missing early return guard (lines 992–997), immediately after the closing `}` of that guard and before the `// Opportunistic: ensure gitignore protects workgroup clones` comment block at line 1000:

```rust
let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);
```

(Round-3 fix: H1 — the earlier round-2 placement at line 989 burned a `call_id` on every non-AC folder the user opened, producing silent gaps in the `call=N` sequence that the operator could not attribute to early-return-vs-panic-vs-cancellation. The architect's existing line-1006–1007 comment — *"Placed AFTER the .ac-new-missing early return so non-AC folders don't pay a wasted filesystem scan"* — is the same logic that justifies placing `call_id` allocation after the same guard. Now only calls that do real work consume `call_id`s and the sequence is dense. This reproduces the round-1 G1 anti-pattern of "absence-of-evidence as proxy for hypothesis" and is exactly what A0 was meant to eliminate; we will not re-introduce it.)

**Format-string convention.** All A1/A2/A3/A4 lines emit `call={}` immediately after the `[ac-discovery]` prefix and before the surface-specific phrase. This keeps `[ac-discovery]` greppable for the existing tooling pattern AND lets `grep '[ac-discovery] call=42'` slice a single discovery call's full tape.

**Cost.** `AtomicU64::fetch_add(1, Ordering::Relaxed)` is one CPU instruction on x86-64 (`lock xadd`) and ARM64 (`ldadd`). Zero allocations. The counter wraps at `u64::MAX` after ~5×10¹¹ years at one call/ms — non-issue.

**Why `Relaxed` is the correct ordering.** The counter's only consumer is `format!`-into-log. We do not use it as a memory barrier or to synchronize any other state. `Relaxed` is the canonical ordering for monotonic counters whose value is observed but does not gate other reads/writes. (`SeqCst` would also be correct and only marginally more expensive, but `Relaxed` matches Rust idiom.)

**Why a process-monotonic counter, not a UUID.** Monotonic ints sort numerically, read at a glance in log slices, and `grep call=42` is greppable without escaping. UUIDs are 16+ bytes per emit and visually collide. The counter resets on process restart; that is fine for diagnostic purposes — log slices for issue #83 reproduction are bounded to a single launch.

**Level:** N/A — A0 is infrastructure (a `static` declaration and one `fetch_add` per discovery call). It emits no logs of its own.

**Gating:** N/A.

---

### Surface A1 — Per-replica verdict in `discover_ac_agents`

**File:** `src-tauri/src/commands/ac_discovery.rs`
**Function:** `discover_ac_agents` (currently lines 559–897).

**Where:** Immediately after the `is_coordinator` call at lines 753–756, before the `wg_agents.push(AcAgentReplica { … })` block at line 758. Depends on Surface A0 (`call_id` must be in scope; A0 introduces it at the top of the function).

**Code (insert verbatim after line 756):**

```rust
log::info!(
    "[ac-discovery] call={} replica — project='{}' wg='{}' replica='{}' fqn='{}:{}/{}' is_coordinator={}",
    call_id,
    project_folder,
    dir_name,
    replica_name,
    project_folder, dir_name, replica_name,
    is_coordinator
);
```

**Level:** `info!` — fires once per replica enumerated. mb's `phi_fluid-mblua` has 105 replicas, so this single discovery call emits 105 `[ac-discovery] call=N replica` lines. That is well within the spec ("≤O(replicas) per discovery call").

**Why this discriminates C vs D directly:**
- For each `tech-lead` replica, the log shows `is_coordinator=true` or `false`. The user then visually inspects the UI:
  - log says `false` → hypothesis C confirmed (drill down to T2/T3/T4 for sub-hypothesis).
  - log says `true` and badge missing → hypothesis D confirmed.
- Identifies the exact replica path via the `fqn` column for cross-reference with sidebar UI state.

**Gating:** unconditional. Per-replica info is the explicit acceptance criterion.

---

### Surface A2 — Per-replica verdict in `discover_project`

**File:** `src-tauri/src/commands/ac_discovery.rs`
**Function:** `discover_project` (currently lines 977–1275).

**Where:** Immediately after the `is_coordinator` call at lines 1144–1147, before the `wg_agents.push(AcAgentReplica { … })` block at line 1149. Depends on Surface A0 (`call_id` must be in scope).

**Code (insert verbatim after line 1147):**

```rust
log::info!(
    "[ac-discovery] call={} replica — project='{}' wg='{}' replica='{}' fqn='{}:{}/{}' is_coordinator={}",
    call_id,
    project_folder,
    dir_name,
    replica_name,
    project_folder, dir_name, replica_name,
    is_coordinator
);
```

**Note:** Identical line to Surface A1 (different `call_id` value at runtime — distinct calls). Both code paths are user-reachable (full-discovery vs per-project-discovery), and the issue's reproduction path through opening a project triggers one or the other depending on UI state. Duplicating the log line — rather than extracting a helper — keeps the change "logging only" and respects the no-refactor constraint. Identical format strings make `grep` deterministic across both call paths.

**Level:** `info!`.

**Gating:** unconditional.

---

### Surface A3 — Discovery summary at end of `discover_ac_agents`

**File:** `src-tauri/src/commands/ac_discovery.rs`
**Function:** `discover_ac_agents`.

**Where:** Immediately before the final `Ok(AcDiscoveryResult { agents, teams, workgroups })` at line 896. Depends on Surface A0.

**Code (insert verbatim before line 896):**

```rust
let total_replicas: usize = workgroups.iter().map(|wg| wg.agents.len()).sum();
let total_coordinator: usize = workgroups
    .iter()
    .flat_map(|wg| wg.agents.iter())
    .filter(|a| a.is_coordinator)
    .count();
log::info!(
    "[ac-discovery] call={} discover_ac_agents: summary — workgroups={} teams={} replicas={} coordinator={}",
    call_id,
    workgroups.len(),
    teams.len(),
    total_replicas,
    total_coordinator
);
```

**Level:** `info!` — fires once per discovery call.

**Why useful:** A single grep on `[ac-discovery] call=42 discover_ac_agents: summary` (or `grep '[ac-discovery] call=42'` for the full tape) yields a chronological per-call audit: did the count of coordinator replicas drop after some user action? Did `teams` count drop? With Surface A0's per-call ID, even concurrent invocations are unambiguously partitioned.

**Gating:** unconditional.

---

### Surface A4 — Discovery summary at end of `discover_project`

**File:** `src-tauri/src/commands/ac_discovery.rs`
**Function:** `discover_project`.

**Where:** Immediately before the final `Ok(AcDiscoveryResult { agents, teams, workgroups })` at line 1274. Depends on Surface A0.

**Code (insert verbatim before line 1274):**

```rust
let total_replicas: usize = workgroups.iter().map(|wg| wg.agents.len()).sum();
let total_coordinator: usize = workgroups
    .iter()
    .flat_map(|wg| wg.agents.iter())
    .filter(|a| a.is_coordinator)
    .count();
log::info!(
    "[ac-discovery] call={} discover_project: summary — path='{}' workgroups={} teams={} replicas={} coordinator={}",
    call_id,
    path,
    workgroups.len(),
    teams.len(),
    total_replicas,
    total_coordinator
);
```

**Note:** Includes the `path` field that A3 does not (since `discover_project` is single-project scoped). Different phrase (`discover_project: summary` vs `discover_ac_agents: summary`) keeps the two surfaces grep-distinguishable. Both share the `[ac-discovery] call=N` prefix from Surface A0.

**Level:** `info!`.

**Gating:** unconditional.

---

## 4. Dependencies

- **No new crates.** All `log::debug!`, `log::info!`, `log::warn!`, `log::trace!` macros are already in use (see existing call sites enumerated in §3 conventions). The `log` crate is already a direct dep — confirmed by grep showing 18 active call sites in `ac_discovery.rs` alone.
- **One new stdlib import in `ac_discovery.rs`.** Surface A0 adds `use std::sync::atomic::{AtomicU64, Ordering};`. Both types are stdlib; no Cargo.toml change.
- **No new imports in `teams.rs`.** `serde_json`, `Path`, `PathBuf` are already imported there.
- **No config changes.** Default `RUST_LOG` filter is unaffected; the investigation run will need `RUST_LOG=info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug` (or a wildcard). Tracking that as a runtime concern, not a code concern.

---

## 5. Notes

### What dev-rust must NOT do

- **Do not refactor `is_coordinator`** beyond inserting the log calls and the two extra `if …` branches (`reject-project-mismatch`, `reject-suffix-mismatch`). Do not hoist `agent_suffix(coord_name)` to a let-binding (even though it would micro-optimize the duplicate suffix calls in the new branch). The reviewer should be able to diff the before/after and see only logging-shaped additions.
- **Do not extract a helper** for the duplicated A1/A2 log lines. The duplication is intentional (per-call-site fidelity, no abstraction).
- **Do not change `discover_teams`'s aggregate `[teams] discovered N team(s) across M project path(s)`** at line 892. T1 and T1.c are *additions*; the aggregate stays as the single endpoint signal.
- **Do not change existing log lines** at `ac_discovery.rs:644, 707, 845, 859, 864, 1102, 1231, 1245, 1250, 1334`. The investigation depends on cross-referencing new lines with existing ones.
- **Do not move the `is_coordinator` call** at `ac_discovery.rs:753` or `:1144`. The FQN-building `format!` call is intentionally inline (§AR2-strict comment block at 748–752 and 1139–1143 explains why). A1/A2 reads the result, does not recompute it.
- **Do not use `log::trace!` outside Surface T2's per-entry inspection.** Trace level is reserved for the noisiest surface so investigation runs can ratchet up granularity if T2's `warn!` plus T3's `info!` does not suffice.

### Edge cases

- **Empty `project_paths`** — `discover_teams` returns immediately with 0 teams; T1 emits no `debug!` (loop is empty). Existing aggregate at line 892 still fires with `0/0`. No change.
- **Project path that exists but `.ac-new` does not** — `discover_teams_in_project` returns at the early-return on line 904 without producing any T2/T3 log. T1.c will show `produced 0 team(s)` for that dir. This is correct: the project simply has no AgentsCommander state.
- **Replica with no `config.json`** — `replica_config` in `ac_discovery.rs` is `None`, `identity_path` is `None`, `is_coordinator` is computed from the fallback `format!("{}:{}/{}", project_folder, dir_name, replica_name)` which uses the dir-derived project. A1/A2 still fires correctly with the dir-derived FQN. T4's debug logs will show whether the strict-project guard rejects.
- **Symlink/junction in `.ac-new`** — covered by T2's `trace!` (each entry logged) and the existing canonicalize `warn!` at line 707/1102. No additional handling needed in this plan.
- **Concurrent discovery calls** — `discover_ac_agents` and `discover_project` may interleave (sidebar populate triggers refresh-during-flight). Surface A0's per-call `call_id` (monotonic `AtomicU64`) is threaded into every A1/A2/A3/A4 line, so two interleaved invocations produce a tape like `[ac-discovery] call=42 replica … / [ac-discovery] call=43 replica … / [ac-discovery] call=42 replica …` that an operator can partition with `grep 'call=42'`. (Round-2 fix: G5 — earlier draft claimed A3/A4 summaries could partition tape retroactively, which was false because summaries land at end of call, not start.)

### Reproduction protocol the user should follow

After dev-rust implements + shipper rebuilds:

1. **Set `RUST_LOG` so `env_logger` captures the new `debug!` lines.** (Round-3 fix: H2 — this step is **critical** on Windows.) The default filter at `lib.rs:103` is `default_filter_or("agentscommander=info")`, which suppresses **every line the round-2/3 plan added at `debug!`**: T3 (per-team summaries), T1.a/T1.b/T1.c (per-path/per-dir scanning), all T4 branches (`direct-match`/`wg-aware-match`/`reject-unqualified`/`reject-project-mismatch`/`reject-suffix-mismatch`/`reject-both-mismatch`). Without this step the captured log is missing the C1-success and C2/C3-rejection-branch evidence the plan exists to provide — leaving only A1/A2/A3/A4 + T2 warns, which is the round-1 evidence floor.

   **On Windows, the env var must be set in the same shell that launches the app.** Use one of:

   - **cmd.exe (single-line)**:
     ```cmd
     set RUST_LOG=info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug && "C:\path\to\agentscommander_mb.exe"
     ```
   - **PowerShell (single-line)**:
     ```powershell
     $env:RUST_LOG='info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug'; & 'C:\path\to\agentscommander_mb.exe'
     ```
   - **Persist system-wide (then start a fresh shell)**:
     ```cmd
     setx RUST_LOG "info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug"
     ```
     Existing shells inherit the OLD environment — a new cmd.exe / PowerShell window must be opened after `setx` for the variable to apply.

   ⚠️ **Do NOT launch via desktop shortcut, Start Menu, taskbar pin, or File Explorer double-click for this reproduction.** Those launch paths inherit the system environment at logon time and will not see a `set` from a terminal. The diagnostic instrument silently degrades — the user sees an `app.log` with no T-lines and no T4 branches, and may incorrectly conclude that no rejection occurred. Since issue #83 is a cross-binary investigation where the user typically launches via the file-name-distinguished `.exe` directly, this is precisely the failure mode that would silently invalidate a round-2-or-later capture.

   For a wildcard firehose (more output, but unambiguously captures everything): `RUST_LOG=debug` in the same launch shell.

2. Launch mb.exe with `phi_fluid-mblua` already in `projectPaths`.
3. Wait for the sidebar to populate.
4. Capture `app.log`.
5. **Slice the log per discovery call.**
   - **A-surfaces (A1/A2/A3/A4)** carry `call_id`. `grep '[ac-discovery] call=42'` (substituting the relevant id) yields a single call's A-tape, partitioned cleanly even if multiple discovery calls overlapped.
   - **T-surfaces (T1/T2/T3/T4)** do **not** carry `call_id` (round-3 doc-fix: H3 — `call_id` is in `ac_discovery.rs` only; threading it through `teams.rs` would be a borderline refactor and `is_coordinator` is consulted from non-discovery routing paths that have no notion of "discovery call"). To bind T-lines to a specific A-call: take the first and last timestamps of `call=N`'s A-lines as a window, then `awk` or grep on `[teams]` lines within that window. ⚠️ **With concurrent `discover_*` invocations from any of the 12+ call sites of `discover_teams()`, T-lines from overlapping calls will interleave within the same time window** — the operator must visually disambiguate using the team/replica fields, or accept that on a busy system T-line/A-line correlation is timestamp-best-effort.

**Note on T4 fan-out (round-2 doc-fix: G7):** With `agentscommander_lib::config::teams=debug` enabled, T4 lines emit not only from `discover_ac_agents` / `discover_project` but also from every routing/`can_communicate`/`is_coordinator_of` decision during the capture window — `is_coordinator` is reached via `is_any_coordinator` (line 443), `is_coordinator_of` (line 436), and `is_coordinator_for_cwd` (line 454), which fan out from inter-agent send/wake decisions throughout the session. The discovery-time T4 lines cluster within milliseconds of A1/A2 emissions for the same `call_id`; routing-driven T4 lines appear scattered across the session at message/wake events. Filter accordingly.

**Note on incomplete sequences (round-3 doc-fix: H5):** A `[ac-discovery] call=N replica …` (A1/A2) line *without* a matching `[ac-discovery] call=N … summary` (A3/A4) line indicates the `discover_*` future was dropped before completion — most commonly cancellation when the frontend window closes or the IPC connection breaks, less commonly a panic or process termination mid-call. The captured per-replica A1/A2 lines for that call are still diagnostically valid; only the aggregate counters (workgroups/teams/replicas/coordinator) at A3/A4 are missing. Don't waste cycles trying to distinguish "panic vs. cancellation" from absence alone.

Expected log slice (for hypothesis C2 — project guard rejection):

```
[teams] discover_teams: scanning project_path='C:\Users\maria\0_repos_phi'
[teams] discover_teams: entering project_dir='…\phi_fluid-mblua'
[teams] discovered team — project='phi_fluid-mblua' team='dev-team' coord_name=Some("phi_fluid-mblua/tech-lead") coord_path=Some("…\\_agent_tech-lead") agent_count=N
[teams] discover_teams: project_dir='…\phi_fluid-mblua' produced 4 team(s)
[teams] discovered 6 team(s) across 2 project path(s)
…
[teams] is_coordinator: reject-project-mismatch → false — agent='phi_fluid-mblua:wg-3-dev-team/tech-lead' agent_project='phi_fluid-mblua' team_project='phi_fluid-mblua' team='dev-team' coord='phi_fluid-mblua/tech-lead'
[ac-discovery] call=42 replica — project='phi_fluid-mblua' wg='wg-3-dev-team' replica='tech-lead' fqn='phi_fluid-mblua:wg-3-dev-team/tech-lead' is_coordinator=false
[ac-discovery] call=42 discover_ac_agents: summary — workgroups=7 teams=6 replicas=105 coordinator=K
```

Expected log slice (for hypothesis C3 — suffix mismatch):

```
[teams] is_coordinator: reject-suffix-mismatch → false — agent='phi_fluid-mblua:wg-3-dev-team/tech-lead' team='phi_fluid-mblua/dev-team' coord='phi_fluid-mblua/tech-leader' agent_suffix='tech-lead' coord_suffix='tech-leader'
[ac-discovery] call=42 replica — project='phi_fluid-mblua' wg='wg-3-dev-team' replica='tech-lead' fqn='phi_fluid-mblua:wg-3-dev-team/tech-lead' is_coordinator=false
```

The `K` value (coordinator count) compared against expected count tells us at a glance whether anything got through. With per-call `call_id`, an operator can re-run the discovery (e.g. via "Refresh") and confirm reproducibility within a single log file.

### Existing logs preserved

For the record, these lines stay untouched (per tech-lead's directive):

- `ac_discovery.rs:35, 269, 322, 368, 378, 389, 503, 521, 644, 707, 845, 859, 864, 1102, 1231, 1245, 1250, 1334`
- `teams.rs:892`
- All `log::warn!` related to canonicalize failures and existing infrastructure.

### Why no frontend logging

The plan stays backend-only because the per-replica A1/A2 line directly emits the value the frontend would render. Comparing the log slice (backend ground truth) against the visible UI (frontend rendering) is sufficient to localize the bug to either side of the IPC boundary. Adding a frontend-side `console.log` at the SolidJS store ingestion point would marginally tighten the C-vs-D verdict, but it is out of scope per tech-lead's explicit "diagnostic logging" framing and would require a frontend change. If the backend log says `true` and the badge is absent, a follow-up issue with frontend instrumentation is the correct next step.

---

## 6. Summary of surfaces

| ID | File | Function | Level | Fires per | Discriminates |
|---|---|---|---|---|---|
| T1.a | `teams.rs` | `discover_teams` | `debug!` | project_path | T1 was scanned |
| T1.b | `teams.rs` | `discover_teams` | `debug!` | invalid path | path skip (G4: was `warn!`) |
| T1.c | `teams.rs` | `discover_teams` | `debug!` | project_dir | per-dir team count |
| T2.read | `teams.rs` | `discover_teams_in_project` | `warn!` | drop event | C1 read fail |
| T2.parse | `teams.rs` | `discover_teams_in_project` | `warn!` | drop event | C1 parse fail |
| T2.entry | `teams.rs` | `discover_teams_in_project` | `trace!` | dir entry | C1 fallback |
| T3 | `teams.rs` | `discover_teams_in_project` | `debug!` | discovered team | C1 success (G3: was `info!`) |
| T4.direct | `teams.rs` | `is_coordinator` | `debug!` | success | true verdict |
| T4.wg-aware | `teams.rs` | `is_coordinator` | `debug!` | success | true verdict |
| T4.unqualified | `teams.rs` | `is_coordinator` | `debug!` | unqualified+suffix-match | malformed FQN |
| T4.proj-mismatch | `teams.rs` | `is_coordinator` | `debug!` | suffix-match × proj-diff | **C2** |
| T4.suffix-mismatch | `teams.rs` | `is_coordinator` | `debug!` | proj-match × suffix-diff | **C3** (G1: new in round 2) |
| T4.both-mismatch | `teams.rs` | `is_coordinator` | `debug!` | proj-diff × suffix-diff | C2+C3 compound (H4: new in round 3) |
| A0 | `ac_discovery.rs` | module-level static | n/a | infrastructure | per-call partitioning (G5: new in round 2; H1 placement fix in round 3) |
| A1 | `ac_discovery.rs` | `discover_ac_agents` | `info!` | replica | C vs D |
| A2 | `ac_discovery.rs` | `discover_project` | `info!` | replica | C vs D |
| A3 | `ac_discovery.rs` | `discover_ac_agents` | `info!` | discovery call | sanity |
| A4 | `ac_discovery.rs` | `discover_project` | `info!` | discovery call | sanity |

Total: 17 log emission sites + 1 infrastructure static across 9 logical surfaces (T1, T2, T3, T4, A0, A1, A2, A3, A4). T4 alone now emits on 6 distinct decision-tree leaves (`direct-match`, `wg-aware-match`, `reject-unqualified`, `reject-project-mismatch`, `reject-suffix-mismatch`, `reject-both-mismatch`), forming a complete positive-evidence audit of every reachable path through `is_coordinator` when both `extract_wg_team` and `team.coordinator_name` are `Some(_)`.

---

## Dev-rust additions (round 1)

> Verifier: dev-rust. Scope of this round: cross-check every cited line, confirm format strings compile, verify the `discover_teams_in_project` rewrite is genuinely semantic-equivalent, and verify the `is_coordinator` gating doesn't suppress the cases the issue is hunting for. **Verdict: plan is implementable as written, with the clarifications/enrichments below.** All helper functions referenced (`extract_wg_team`, `split_project_prefix`, `agent_suffix`, `agent_matches_member`, `resolve_agent_ref`, `resolve_agent_path`) exist and are in scope; `log = "0.4"` is in `src-tauri/Cargo.toml`; both files already use bare `log::*` paths (no `use log::...` import needed, matching existing convention).

### D1. Line-number contradiction in T1.a — implement to intent, not to the cited line

**Surface:** §3 T1.a.

**Issue:** The plan reads "at line 871, immediately after `for repo_path in &settings.project_paths {`, before the `let base = Path::new(repo_path);` on line 868. Insert as the first line of the loop body." Line 871 (the closing `}` of `if !base.is_dir() { continue; }`) is **after** the early-continue, not before `let base`. The "871" is the wrong line anchor; the **intent** ("first line of the loop body, before `let base`") is unambiguous.

**Reason for raising:** If a reader copy-pastes by line number, they get the log INSIDE the if-block (or after the continue), which silently breaks the intended O(project_paths) emission cardinality (the log would only fire for valid paths, double-emitting in concert with T1.b's warn).

**How to apply:** Insert at the actual line 868 (immediately before `let base = Path::new(repo_path);`). The `for repo_path in &settings.project_paths {` is at line 867. T1.b's `warn!` covers the not-a-directory case; T1.a's `debug!` is the unconditional "scanning" emission. No behavior change vs. the architect's intent.

### D2. T2 semantic-equivalence — confirmed equivalent, case-by-case

**Surface:** §3 T2 (the `match … and_then … ok` → imperative rewrite at lines 935–941 of `teams.rs`).

The architect specifically asked me to scrutinize this. I traced every branch of the original `Option`-coalescing chain against the proposed imperative rewrite:

| Case | Original behavior | Proposed behavior | Equivalent? |
|---|---|---|---|
| `read_to_string` returns `Err(_)` | `.ok()` → `None` → outer `match … None => continue;` | `Err(e) => { warn!; continue; }` | ✅ same control flow + post-condition |
| `read_to_string` returns `Ok(s)`, `from_str` returns `Err(_)` | `.and_then(\|c\| from_str(&c).ok())` → `None` → `continue;` | `from_str` `Err(e) => { warn!; continue; }` | ✅ same |
| `read_to_string` `Ok(s)`, `from_str` `Ok(v)` | `Some(v)` binds to `parsed` | `Ok(v)` binds to `parsed` | ✅ same — `parsed: serde_json::Value` |

Post-condition on `parsed` after the block is identical (typed `serde_json::Value`, valid for downstream `parsed.get("agents")` / `parsed.get("coordinator")` calls). No edge case lost (no `.flatten()`-able `Option<Option<_>>` collapse, no `?` operator semantics). The lifetime of `raw` (held only inside the inner match expression and dropped at `;`) is fine — `from_str(&raw)` borrows it just long enough.

**Format strings** in T2's two `warn!` lines: `e: std::io::Error` and `e: serde_json::Error` both impl `Display`, so `{}` is correct. `dir_name: &str` (line 924–927), `project_folder: String` (line 907–911), `config_path: PathBuf` (line 934) — `config_path.display()` returns `path::Display`, fine for `{}`. ✅ Compiles.

### D3. T4 gating verification — does NOT suppress the hypothesis-C2/C3 cases

**Surface:** §3 T4 (`is_coordinator` four-branch logging).

Tech-lead's specific ask: "Verify the gated condition for `is_coordinator` logging doesn't accidentally suppress the case we actually need to see." Truth table for the bug-investigation scenario (mb opening `phi_fluid-mblua`, replica FQN like `phi_fluid-mblua:wg-3-dev-team/tech-lead`):

| Code path triggered | Bug hypothesis | Log emitted? | Verdict |
|---|---|---|---|
| `agent_matches_member` true (path/name direct hit) | not the bug | `direct-match → true` | ✅ visible |
| WG-aware: `wg_team == name && project == project && suffix == suffix` | not the bug | `wg-aware-match → true` | ✅ visible |
| WG-aware: `wg_team == name && project != project && suffix == suffix` | **C2 (project guard rejects foreign-created replica)** | `reject-project-mismatch → false` | ✅ visible — **this is the smoking gun for the issue** |
| `extract_wg_team` returns `None` | C3 variant — agent_name has no `wg-*` segment | none | acceptable: A1's `is_coordinator=false` + T3's coord_name reveals this combinationally |
| WG-aware: `wg_team == name && suffix != suffix` | **C3 (coord_name resolved differently across binaries)** | none | acceptable: A1 + T3 reveal it (architect acknowledged this in §3 T4 trailing paragraph) |
| `agent_project = None` (unqualified name) but `wg_team == name && suffix match` | malformed FQN at call site | `reject-unqualified → false` | ✅ visible (defensive) |
| `wg_team != name` for any reason | not the team we're looking at | none | correct — call sites iterate every team, only same-name iterations are interesting |

**Conclusion:** The gating is well-calibrated. The bug's primary hypothesis (C2) gets a dedicated log line. The C3 fallback is reconstructible from A1+T3. The only "blind spot" — `extract_wg_team` returning `None` for a tech-lead replica — is so unlikely (replicas live inside `wg-N-*` dirs by construction) that adding a fifth log to cover it would just be noise. **Don't add a fifth branch.**

**Compile check on the new `reject-unqualified` branch:** The plan inserts logging inside the `let-else` arm BEFORE `return false;`. `let-else` requires the else block to diverge; `return false;` after the conditional `log::debug!(...)` (which evaluates to `()`) preserves divergence. ✅

**Compile check on the new `reject-project-mismatch` branch:** It's a bare `if { log::debug!(...); }` with NO `return` — control falls through to the end of the function and hits the trailing `false`. Same terminal value as the original code path. ✅ No behavior change.

**Test impact:** `teams.rs` has `assert!(!is_coordinator("wg-1-dev-team/tech-lead", &teams[0]));` at the test module (around line 855). That call exercises the unqualified-name branch — proposed change adds a log + preserves `return false;`, so the test still passes. The new `reject-unqualified` log will fire during the test run if `wg_team == team.name && suffix matches`, which depends on the test fixture; that's just extra log output, not a test failure.

### D4. T3 — pick the `teams.last().expect("just pushed")` form (form 1)

**Surface:** §3 T3.

The plan offers two forms. **Implement form 1.** Reasoning:

- Form 1 (`let pushed = teams.last().expect("just pushed");`): zero extra allocations. The `expect` is provably safe because `Vec::push` cannot return `None` and `last()` immediately follows on the same `&mut Vec`.
- Form 2 (clone every field before push): adds 4 unnecessary clones (`team_name.clone()`, `project_folder.clone()` — already cloned for the push, this is a *second* clone — `coordinator_name.clone()`, plus the `coord_path` display alloc). Pure overhead.

Borrow checker on form 1: `teams.push(...)` is a `&mut Vec<…>` op that completes before the `let pushed = teams.last()` (returns `Option<&DiscoveredTeam>`, immutable borrow). No lifetime overlap. The subsequent `log::info!` only reads through `pushed.field`. ✅ Compiles cleanly.

**Re Role.md "prefer if let / match over .unwrap()":** That guidance is rooted in PTY-manager-crash blast radius. Discovery code is not on the PTY hot path; `expect("just pushed")` here is a documented invariant on a single line and is the idiomatic Rust pattern for "post-push back-reference". Acceptable per spirit of the rule. Alternative `if let Some(pushed) = teams.last() { … }` works too but adds a no-op branch — pick whichever the reviewer's eye finds more honest.

### D5. A1/A2 placement — verified safe; `replica_name` is moved on push

**Surface:** §3 A1 and A2.

Confirmed at `ac_discovery.rs:687-690` (A1) and `ac_discovery.rs:687-690` analog (A2): `replica_name: String` is constructed by `strip_prefix("__agent_").unwrap_or(...).to_string()`. At the push site (line 758–767 / 1149–1158) the field shorthand `name: replica_name` **moves** the String into `AcAgentReplica`. The plan correctly places A1/A2 logs **before** the push — placement after the push would not compile (use-of-moved-value). ✅

Format-string check on A1/A2: `project_folder: String`, `dir_name: String` (line 632–635 / 1031), `replica_name: String`, `is_coordinator: bool`. All `Display`. The `{}:{}/{}` literal-colon-between-placeholders parses correctly under Rust's format-string grammar (the `:` outside `{...}` is plain text). ✅

### D6. A3/A4 placement — `cfg` lifetime

**Surface:** §3 A3 and A4.

A3 inserts immediately before `Ok(AcDiscoveryResult { … })` at line 896. By that point, `drop(cfg);` has already run at line 826 — no lock-held-across-await concern, no settings borrow conflict. The summary-log block iterates `&workgroups` (already populated and sorted) and `teams.len()` — pure immutable reads. ✅

A4: same analysis. `drop(cfg);` runs at line 1255 *after* the workgroup-team association loop and *before* the `branch_watcher.update_replicas_for_project(...)` call. Plan's insertion point ("immediately before line 1274 `Ok(...)`") is below the drop, so `cfg` is gone by the time A4 fires. ✅

### D7. Minor grep-friendliness nit (non-blocking)

T3's prefix `[teams] discovered team — project='…'` shares the `[teams] discovered ` token with the existing aggregate `[teams] discovered N team(s) across M project path(s)` at line 892. A grep for `[teams] discovered ` returns both. The architect's call is to keep the `discovered` verb in T3 to read naturally; I'd accept either:

- **Keep as-is** (architect's choice): readers grep for `[teams] discovered team —` (with the em-dash) for T3 and `[teams] discovered ` for T3+aggregate. Lives with the small overlap.
- **Tighten:** rename T3 prefix to `[teams] discover_teams_in_project: pushed team — …` to mirror the `discover_teams: scanning/entering/produced` family. Cleaner truth-table grep.

Not worth a grinch round on its own. Either is fine; flagging only because it's the one stylistic divergence from the §3-conventions sentence ("All `is_coordinator` verdict logs mention the branch taken so a grep on the log file produces a clean truth table").

### D8. No bugs spotted in the discovery code while reading

Per tech-lead's directive ("If you spot a bug while reading, note it but do not fix it in this branch"). I read all of `discover_teams`, `discover_teams_in_project`, `is_coordinator`, the relevant slices of `discover_ac_agents`, and `discover_project`. Nothing actionable beyond what issue #83 itself enumerates as the hypothesis space. The §AR2-strict guard is consistent across both call sites (FQN built identically at `:754` and `:1145`). The `resolve_agent_ref` / `resolve_agent_path` helpers are used in matched pairs in both `teams.rs::discover_teams_in_project` and `ac_discovery.rs::discover_*`'s team-construction paths. No silent inconsistency observed.

### D9. Implementation order recommendation

When dev-rust executes (post-consensus):

1. `teams.rs` first: T2 (the only control-flow rewrite) and verify with `cargo check` before any other surface — catches any borrow/move issue early.
2. Then T1 (a/b/c), T3, T4 in `teams.rs`. After each, `cargo check`.
3. Then `ac_discovery.rs`: A1, A2, A3, A4. `cargo check` after each function-level batch.
4. `cargo clippy` final pass (expect zero new warnings — the new code is just `log::*` calls and existing patterns).
5. **Do NOT run** `cargo test` as a gating step until after grinch consensus on logging shape — tests may emit a lot of extra log noise that would change CI output.

### Summary of additions

**TL;DR for tech-lead:** Plan compiles, semantics are preserved, gating is correct for the issue's hypothesis space. Two clarifications matter for the implementer: (D1) T1.a's "line 871" is wrong, use line 868; (D4) prefer T3 form 1. The remaining notes (D2/D3/D5/D6/D8/D9) are positive verifications. D7 is a non-blocking style nit. **Recommend approving and proceeding to grinch round.**

---

## Grinch adversarial review (round 1)

Verdict: **ITERATE.** Dev-rust's D-notes establish that the plan **compiles** and is **semantically equivalent**; I do not contest those. My pass attacks a different axis: does the plan actually **detect what the issue says it must detect**, and does the cost of the captured evidence make it usable? On both axes I found defects.

I agree with D1 (G6 below — same finding, two reviewers, same conclusion). I disagree with D3 — see G1, where I argue the C3 sub-hypothesis is the one the user most needs to discriminate and the plan currently cannot do it positively.

Findings ordered strongest first.

---

### G1 — T4 cannot positively detect C3, the sub-hypothesis the architect named as the most likely culprit. (Disagrees with D3.)

**What.** Surface T4 emits `debug!` on four code paths: `direct-match`, `wg-aware-match`, `reject-unqualified`, `reject-project-mismatch`. There is **no log** for the case where `wg_team == team.name && agent_project == team.project && agent_suffix(agent_name) != agent_suffix(coord_name)`. That case is exactly hypothesis C3 (architect's §1: *"the team is discovered but `is_coordinator` rejects because `agent_suffix(coord_name)` differs from `agent_suffix(agent_name)` (e.g. coordinator ref resolves differently across binaries)"*).

**Why it matters — the disagreement with D3 in concrete terms.** Dev-rust's D3 truth table states C3 is *"acceptable: A1 + T3 reveal it (architect acknowledged this in §3 T4 trailing paragraph)"*. That is **diagnosis by elimination**, not by positive evidence:

> A1 says `is_coordinator=false` for the replica. T3 says the team has `coord_name=Some("phi_fluid-mblua/tech-lead")`. Therefore — by ruling out C1 (T3 fired so team is discovered) and C2 (no `reject-project-mismatch` fired) — *the cause must be C3*.

This reasoning fails the moment any **fourth** sub-hypothesis exists that the architect didn't pre-enumerate. Examples that would also produce A1=false + T3=present + no T4 reject log:
- `extract_wg_team(agent_name)` returning `None` for an unexpected reason (e.g., a future replica naming format that doesn't match `wg-N-team-name/replica`). Dev-rust's D3 dismisses this as "so unlikely it's noise" — but that is the entire reason we are debugging cross-binary behavior in the first place. The whole bug class is "an assumption the developer thought was solid turned out not to be".
- A `team.coordinator_name` that resolved to `None` while `team.coordinator_path` is `Some(_)` (or vice versa). Then the outer `if let Some(ref coord_name) = team.coordinator_name` doesn't enter, and **no T4 line fires at all** — A1=false, T3=team-with-no-coord, and the operator has to mentally reconstruct the path.
- A subtly wrong `team.project` value (different normalization between binaries — e.g., trailing dot, encoding difference). The `agent_project == team.project` check fails silently; this would actually fire `reject-project-mismatch`, but only if the suffix matches — if the suffix ALSO differs, no log fires.

In all these cases the operator looks at A1=false + a present T3 line and confidently concludes "C3 — suffix mismatch", because that is the only remaining named hypothesis. They are now investigating the wrong thing. The captured log slice **cannot distinguish C3 from "we missed a hypothesis"**, and that is a failure of the diagnostic instrument.

The cost of fixing this is trivial: a fifth `if` block that fires only when `wg_team == team.name && agent_project == team.project && agent_suffix(...) != agent_suffix(...)`. Bounded by `replicas × teams_with_same_name_and_project`, which in the issue's repro is a tight bound (`105 replicas × 1 dev-team in phi_fluid-mblua` ≈ a few hundred lines, hidden at debug). For that price the operator gets **positive proof of C3** instead of "everything else seems ruled out".

Dev-rust's defense — *"the only blind spot — `extract_wg_team` returning `None` for a tech-lead replica — is so unlikely that adding a fifth log to cover it would just be noise"* — also misidentifies which case I want covered. I am NOT asking for a log on `extract_wg_team == None`; I am asking for a log on the **suffix-mismatch case** where every preceding gate succeeded. Two different code paths.

**Fix.** Add a fifth `if` branch immediately after `reject-project-mismatch`:

```rust
if wg_team == team.name
    && agent_project == team.project
    && agent_suffix(agent_name) != agent_suffix(coord_name)
{
    log::debug!(
        "[teams] is_coordinator: reject-suffix-mismatch → false — agent='{}' team='{}/{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
        agent_name, team.project, team.name, coord_name,
        agent_suffix(agent_name), agent_suffix(coord_name)
    );
}
```

Behavior unchanged (function still falls through to `false`). Update §3 T4 "Why this discriminates C2 and C3" to reference this line. Update §6 surface table.

If we ship without this and the captured log slice doesn't conclusively name a sub-hypothesis, we will be in this same plan-review loop next week.

---

### G2 — T2 `trace!` always pays the `entry.file_name()` allocation, even when trace is disabled.

**What.** Plan §3 T2 inserts:
```rust
let _entry_name = entry.file_name();
log::trace!(
    "[teams] discover_teams_in_project: inspecting entry — project='{}' entry='{}'",
    project_folder,
    _entry_name.to_string_lossy()
);
```
The `let _entry_name = ...` is **outside** the macro. The `log` crate macros short-circuit on level — but only for arguments evaluated *inside* the macro. A `let` before the macro always runs.

**Why it matters.** `entry.file_name()` returns an owned `OsString` (per `std::fs::DirEntry::file_name` docs — it allocates per call on every platform). With `discover_teams_in_project` running over every entry in `.ac-new/` (20+ per project for a real workspace) and `discover_teams()` invoked from 12+ call sites (see G3) including per-message CLI subcommands, this is a steady stream of unnecessary allocations under default-log conditions. The whole purpose of putting this at `trace!` is so the cost is paid only when the operator opts in. The current design defeats that.

The `_entry_name` underscore-prefix also misleads: convention says `_var` is for explicitly unused, but this binding IS used. A reviewer running `clippy::used_underscore_binding` will rightfully flag it.

**Fix.** Inline the call inside the macro:
```rust
log::trace!(
    "[teams] discover_teams_in_project: inspecting entry — project='{}' entry='{}'",
    project_folder,
    entry.file_name().to_string_lossy()
);
```
Standard Rust `log` idiom. Drop the rationale text about the underscore binding in §3 T2.

---

### G3 — `discover_teams()` is called from 12+ call sites; T1/T3 noise budget is wrong by an order of magnitude.

**What.** The plan implicitly assumes `discover_teams()` runs ≈once per `discover_ac_agents`/`discover_project`. Reality (`grep`):
```
src-tauri/src/cli/send.rs:133            // every CLI inter-agent message
src-tauri/src/cli/list_peers.rs:316,437  // every list-peers
src-tauri/src/cli/close_session.rs:92    // every close-session
src-tauri/src/lib.rs:522                 // startup
src-tauri/src/phone/mailbox.rs:471, 1154, 1427  // phone ops
src-tauri/src/commands/ac_discovery.rs:571, 1008
src-tauri/src/commands/phone.rs:12, 23
src-tauri/src/commands/entity_creation.rs:1130
src-tauri/src/commands/session.rs:290
```

T3 emits `info!` per discovered team. With ~6 teams × ~12 call paths and CLI subcommands firing on every inter-agent message, a busy multi-agent session emits dozens of `[teams] discovered team — ...` info lines per minute.

**Why it matters.** §3 T3 defends `info!` as *"fires once per discovered team (mb expects 6 across both projects), well below O(replicas)"* — true *per call*, but multiplied across the full call-site set. The existing aggregate `[teams] discovered N team(s) ...` is one info line per call (already 12+ per minute under load); T3 adds 6× more. Investigation runs already promote the teams module to `debug` (per §5 reproduction protocol), so demoting T3 to `debug!` costs the investigation **nothing** while keeping default-log noise stable.

**Fix.** Choose ONE:
- **(a) preferred:** demote T3 to `log::debug!`. Investigation users set `RUST_LOG=...,agentscommander_lib::config::teams=debug` per §5, so T3 is still captured.
- (b) keep `info!` and explicitly document in §3 that the 12 call sites are an accepted budget cost.

---

### G4 — T1.b emits `warn!` for stale `projectPaths`, which is normal user state, not an actionable error.

**What.** §3 T1.b promotes the silent-`continue` for non-directory `project_path` entries to `log::warn!`.

**Why it matters.** Users routinely accumulate stale entries in `settings.projectPaths` (USB unplugged, repo moved, hand-typed for-later entry). Per G3, every `discover_teams()` call iterates the full `project_paths` list — so every stale entry now emits one `warn!` per call × 12+ call sites, for the entire session. `warn!` is the level operators triage. Flooding it with non-events trains operators to ignore warnings, which silently weakens the warning channel for actual problems. It also creates inconsistency with `ac_discovery.rs:582-586` and `:984-987`, which already silently `continue` for the same condition.

**Fix.** Demote to `log::debug!`. The skip is implicitly captured by absence of subsequent T1.c lines for that path, so no diagnostic loss. If a `warn!` is genuinely wanted, gate it behind a per-session `OnceLock<HashSet<String>>` of already-warned paths so each stale entry warns at most once per process. Simpler answer is `debug!`.

---

### G5 — Concurrent discovery cannot be partitioned from interleaved A1/A2 lines without a per-call ID.

**What.** §5 Edge cases claims *"the per-call summary lines (A3/A4) are unambiguous: each call's summary names its workgroup count, allowing the reader to demarcate slices."* Two problems:
1. `teams_snapshot` is per-call, not global — two concurrent invocations of `discover_ac_agents` / `discover_project` each compute their own snapshot. There is no serialization between calls.
2. The A3/A4 summary lands at the END of the call. With identical format strings, two interleaved calls produce `replica_A1, replica_B1, replica_A2, replica_B2, ..., summary_A, summary_B` — and the reader has no way to retroactively assign which `replica` line belongs to which `summary`.

**Why it matters.** §5 reproduction protocol says *"Wait for the sidebar to populate. Capture app.log."* The frontend may issue multiple `discover_ac_agents` / `discover_project` calls during sidebar population (initial render + auto-refresh + user-triggered refresh). Without a per-call discriminator the captured log is ambiguous in **exactly the failure scenario the issue is investigating** (`phi_fluid-mblua` open + sidebar populate is a multi-call event by construction).

**Fix.** Per-call monotonic ID at the top of each `discover_ac_agents` / `discover_project`:
```rust
use std::sync::atomic::{AtomicU64, Ordering};
static DISCOVERY_CALL_ID: AtomicU64 = AtomicU64::new(0);
let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);
```
Prefix all A1/A2/A3/A4 lines with `call=<id>`. Logging-only (no behavior change), zero allocations per emit, trivially `grep | sort -k …`-able. Without this, A1 and A3 are not actually composable into a per-call audit when the system is busy.

---

### G6 — T1.a line number is wrong. (Same as D1 — concurring finding.)

Confirmed independently by both reviewers. Plan §3 T1.a says "line 871" but the correct insertion point is line 868. **Both reviewers reached the same conclusion.** Architect should fix the line citation in §3 to remove ambiguity for the implementer.

---

### G7 — T4 `debug!` lines fan out from `is_any_coordinator` / `is_coordinator_of` on every routing decision, not just discovery.

**What.** `is_coordinator` is called by `is_any_coordinator` (`teams.rs:443`), `is_coordinator_of` (`:436`), `is_coordinator_for_cwd` (`:454`). Those are consulted from `can_communicate` and authorization-gate paths — i.e., **every send-message and every wake decision**, not just discovery.

**Why it matters.** §3 T4 reasons about volume as *"O(replicas × teams) per discovery call (~525 in mb's case). Hidden at default log level; only the investigation run with `RUST_LOG=debug` emits these."* True for one discovery. But the investigation user sets `RUST_LOG=info,agentscommander_lib::config::teams=debug` (§5 reproduction protocol) — that enables `debug!` for the **entire teams module**. Every inter-agent routing decision during the capture window will also emit T4 lines. The user runs the repro, opens `phi_fluid-mblua`, sidebar populates (1 discovery), and any background message activity during the capture window emits T4 fan-out interleaved with the discovery-time T4 lines.

**Why it's not a hard blocker.** The lines are still bounded and `[teams] is_coordinator:` is greppable. But the `[ac-discovery] replica` lines from A1/A2 form a discrete tape, while T4 lines from routing fan-out interleave with discovery T4 lines and require timestamp-based slicing.

**Fix (light).** Add to §5 reproduction protocol: *"Note: T4 lines also emit on every routing/can_communicate decision while teams=debug is enabled. Group them with the surrounding A1 lines from the same approximate timestamp window to bind to the discovery call."* Architect may also choose to leave T4 at `debug!` and rely on G5's `call_id` + timestamp grouping. If G5 is adopted, this becomes a non-issue for the discovery tape (T4 lines without a call_id can be grouped by proximity to A1 lines that have one).

---

### G8 — Format-string `'{}'` enclosure breaks for paths/names containing `'`.

**What.** §3 Conventions standardizes `field='{}'` for path/string atoms. Windows allows `'` in folder names; a user with `C:\Users\maria's\repo` produces `path='C:\Users\maria's\repo'` — downstream awk/regex on `'` boundaries mis-tokenizes.

**Why it matters.** Cosmetic; future-proofing. Once shipped, downstream tooling will rely on the convention. Minor.

**Fix (optional).** Either escape `'` (use `{:?}` for path-like fields — Rust string-literal escaping handles it) or document in §3 Conventions that values may contain `'` and parsers must allow for it. Acceptable to defer.

---

### G9 — `[teams] discovered ...` prefix collision with existing aggregate. (Concurs with D7.)

D7 already raised this. I agree with D7's recommendation: either keep as-is (and grep for `[teams] discovered team —` with the em-dash) or rename T3's prefix to `[teams] discover_teams_in_project: pushed team — ...`. Non-blocking; mention only.

---

### What I tried to break and could not

I gave the architect's "behavior-equivalent" claims a hard read in case D2 was over-trusting:

- **T2 imperative rewrite:** D2 enumerated the three branches; I re-confirmed independently. The `Err(e) → continue` paths in both arms preserve the original `Option`-collapsing semantics. The lifetime of `raw` is bounded by the inner match expression. No behavior change — concur with D2.
- **T4 added `reject-project-mismatch` branch:** the new `if { log; }` falls through to the function's terminal `false`, exactly like the original code. The `agent_project != team.project` comparison uses the same `String == String` types as the existing `==` three lines up. Compiles. No behavior change — concur with D5/D6.
- **T1.b `warn!` changing visibility, not control flow:** the `continue;` is preserved; only the `warn!` is added. Behavior preserved. (Noise concern is G4, separate axis.)
- **T3 form 1 vs form 2 borrow checker:** dev-rust's D4 analysis is correct. `teams.push(...)` releases the `&mut` before `teams.last()` takes the `&` — no overlap. Concur.
- **A3/A4 `cfg` lifetime:** `drop(cfg)` runs at line 826 (A3) and line 1255 (A4), both before the insertion point. No lock-across-await. Concur with D6.

The defects are not in **what the plan does to the code** — D2-D9 correctly verified that. The defects are in **what the captured logs let the operator conclude (G1)**, **what the logs cost when not investigating (G2/G3/G4)**, and **how the logs are read back when calls overlap (G5)**.

---

### Summary of required changes for round 2

| ID | Severity | Action | Disagrees with D-notes? |
|---|---|---|---|
| **G1** | must-fix | Add fifth `reject-suffix-mismatch` branch in T4 | **Yes — opposes D3** |
| **G2** | must-fix | Inline `entry.file_name().to_string_lossy()` in T2's trace macro | no (D-notes silent) |
| **G3** | should-fix | Demote T3 to `debug!` (or document the 12-callsite info budget) | no (D-notes silent) |
| **G4** | should-fix | Demote T1.b to `debug!` (or one-shot per session) | no (D-notes silent) |
| **G5** | should-fix | Add per-call monotonic `call_id` to A1/A2/A3/A4 | no (D-notes silent) |
| **G6** | must-fix | Correct line citation 871→868 in T1.a | no — concurs with D1 |
| **G7** | doc-fix | Add a sentence to §5 about T4 fan-out from routing | no (D-notes silent) |
| **G8** | optional | Document `'`-in-paths quoting; optional | no (D-notes silent) |
| **G9** | optional | Concur with D7's prefix-collision note | concurs with D7 |

**Strongest concern at the top: G1.** The plan is built around discriminating C1 vs C2 vs C3 vs D, but as written it cannot positively detect C3 — the very sub-hypothesis the architect ranked alongside C2 in §1. Diagnosis-by-elimination is the failure mode the issue exists to escape (the user already has an A1-equivalent signal: `Deferred non-coordinator session` + UI absence — what they're missing is positive proof of *which gate* rejected). Two debug lines (~hundreds of bytes per repro) buys positive evidence. Cheap.

If G1, G2, G6 land, the diagnostic value is sound and the plan ships. G3, G4, G5 keep the captured log readable. G7-G9 are documentation polish.

**No courtesy approval.** Ship the fifth branch.

---

## Architect updates (round 2)

> Updater: architect. Adjudicating dev-rust's D-notes against grinch's G-findings, applying mandatory fixes, deciding the one disagreement, and applying optional/doc fixes per tech-lead's lean.

### Fixes applied — where each landed

| ID | Severity | Action | Where in plan |
|---|---|---|---|
| **D1 ≡ G6** | must-fix | T1.a line citation `871 → 868` | §3 Surface T1 "Where:" block (T1.a bullet) — corrected line + parenthetical noting both reviewers concurred |
| **G2** | must-fix | Inline `entry.file_name().to_string_lossy()` inside `log::trace!` macro args; remove the `let _entry_name` binding | §3 Surface T2 "Also insert" block — code rewritten + parenthetical noting the `clippy::used_underscore_binding` hazard the original would have triggered |
| **G1** | must-fix | Add fifth `reject-suffix-mismatch` branch to `is_coordinator` | §3 Surface T4 code block — appended after `reject-project-mismatch`; rationale paragraph rewritten to claim positive C3 detection; §6 surface table split T4 into `T4.direct`/`T4.wg-aware`/`T4.unqualified`/`T4.proj-mismatch`/`T4.suffix-mismatch` rows |
| **G3** | should-fix | T3 demoted from `info!` → `debug!` | §3 Surface T3 — both code-block forms updated, "Level:" paragraph rewritten with the 12-call-site enumeration |
| **G4** | should-fix | T1.b demoted from `warn!` → `debug!` | §3 Surface T1 code block + "Levels:" paragraph |
| **G5** | should-fix | Per-call `AtomicU64` ID for cross-call correlation | New §3 Surface **A0** inserted between T4 and A1; A1/A2/A3/A4 format strings updated to thread `call_id`; §4 Dependencies notes the new `std::sync::atomic` stdlib import; §5 "Concurrent discovery calls" edge case rewritten |
| **G7** | doc-fix | Note that T4 lines also fire from routing/`can_communicate` decisions, not just discovery | §5 reproduction protocol — new "Note on T4 fan-out" paragraph |

### G1 vs D3 — decision and reasoning

**Decision: adopt G1. Add the fifth `reject-suffix-mismatch` branch.**

The plan's core value proposition is **positive identification of which `is_coordinator` gate rejected** a `tech-lead` replica. Diagnosis by elimination — "A1 says false, T3 says the team is present, no T4 reject log fired, therefore C3 by exhaustion" — collapses the moment any unenumerated 4th sub-hypothesis exists. And the bug class we are debugging is *literally* "an assumption the developer thought was solid turned out not to be" (cross-binary state divergence between bit-identical executables, originally believed impossible). The whole reason issue #83 exists is that the user's prior elimination evidence (`Deferred non-coordinator session` log lines + UI badge absence) failed to localize the gate; reproducing that elimination pattern in the new instrument would be self-defeating. Grinch's example failure modes — `coordinator_name=None ∧ coordinator_path=Some` schema skew, `extract_wg_team` returning `None` for an unanticipated naming pattern, normalization mismatch in `team.project` — are concrete and not implausible in a cross-binary setting. The cost case is trivial: the new branch fires only when both `wg_team == team.name` AND `agent_project == team.project` AND suffixes differ — bounded by `replicas × same-name-and-project teams` ≈ ~105 lines per reproduction at debug level, and is hidden from default logs. Dev-rust's only concrete D3 objection — that `extract_wg_team` returning `None` is "so unlikely it's noise" — is a different code path entirely; the suffix-mismatch case Grinch wants instrumented is reached only *after* `extract_wg_team` returns `Some`, so D3's argument doesn't address G1's actual ask. The trade is asymmetric: at most ~hundreds of bytes per repro buys a deterministic gate-identification log line that prevents another round of this plan-review loop. Take G1.

### Intentionally left undone

- **G8** (deferred): the `'`-in-paths quoting concern. Windows technically permits `'` in folder names, but in 8 years of AgentsCommander filesystem state I have not observed one and the existing log corpus already uses `'…'` enclosure (`[ac-discovery] identity canonicalize failed — replica='{}'` at lines 707/1102 has shipped without incident). Adopting `{:?}` would change the on-disk log format for downstream tooling more than it would buy. Filed as a future cleanup if a `'` path actually appears in the wild.
- **G9 ≡ D7** (deferred): the `[teams] discovered ` prefix collision between T3 and the existing aggregate at `teams.rs:892`. Tech-lead's directive says preserve existing log lines; renaming T3's prefix would resolve the grep-collision but at the cost of a less-natural sentence ("discovered team —" reads cleanly, "pushed team —" is jargony). Operators can grep with `[teams] discovered team —` (with the em-dash) for T3-only and `[teams] discovered` for both. Acceptable tax. Both reviewers explicitly marked this non-blocking.
- **No frontend instrumentation.** Tech-lead's brief was "diagnostic logging" with no behavior change; A1's per-replica `is_coordinator=…` line plus visual UI inspection is sufficient to discriminate C from D. If the round-2 reproduction shows backend `true` and badge absent, the follow-up issue should add a frontend-side store-ingestion log; that's out of scope for #83.
- **No `is_coordinator` refactor.** The two new branches (`reject-project-mismatch` from round 1 + `reject-suffix-mismatch` from round 2) are pure additions. The duplicate `agent_suffix(agent_name)` / `agent_suffix(coord_name)` calls inside the new branch could be hoisted to local bindings for ~3 cycles of micro-optimization, but tech-lead's "no refactor" rule explicitly extends to such changes. Dev-rust must implement as-written.

### Round-2 surface count

- **8 → 9 logical surfaces** (added A0 — infrastructure, no log emission).
- **12 → 16 log emission sites** (T1.a + T1.b + T1.c×2 + T2.read + T2.parse + T2.entry + T3 + T4×5 + A1 + A2 + A3 + A4). T4 grew from 4 → 5 emission paths with the new branch.
- One new module-level `static` (`DISCOVERY_CALL_ID: AtomicU64`) and one new stdlib import (`std::sync::atomic::{AtomicU64, Ordering}`) in `ac_discovery.rs`.

### Changes NOT made (re-confirmed)

- T2 imperative rewrite stays as designed in round 1 (D2 + G "what I tried to break" both confirmed semantic equivalence).
- T3 form choice stays "form 1 preferred per D4" — the demotion to `debug!` doesn't change which form to implement.
- The §AR2-strict guard and existing FQN building at `ac_discovery.rs:753-756` / `:1144-1147` remain untouched (D5 confirmed safe; A1/A2 only read the result).
- All previously-listed "do not change" log lines (`ac_discovery.rs:35, 269, 322, 368, 378, 389, 503, 521, 644, 707, 845, 859, 864, 1102, 1231, 1245, 1250, 1334`; `teams.rs:892`) stay untouched.

### Hand-off

Plan is ready for round-2 review. Dev-rust: please re-verify (a) the new T4 fifth branch compiles cleanly with the surrounding `if let Some(wg_team) = …` scope, (b) the A0 `static`+`fetch_add` lands cleanly in both function bodies, (c) the format-string changes for A1/A2/A3/A4 still pass `cargo check`. Grinch: I owe you a positive answer on G1 (adopted) and the diagnostic-by-elimination argument; if you find a sub-hypothesis the plan still cannot positively detect, name it. If both reviewers approve, this proceeds to dev-rust implementation per the round-1 D9 ordering recommendation.

---

## Grinch adversarial review (round 2)

Verdict: **ITERATE.** The architect's round-2 changes correctly applied G1, G2, G3, G4, G5, G6, G7. The G1 adjudication is sound — I will not relitigate D3. **But the round-2 implementation introduced two new defects that reproduce the exact failure mode round 1 was fought over: silent gaps that force diagnosis-by-elimination.** I also found a deployment hazard in §5 that could silently disable the entire investigation on a Windows GUI launch.

Findings ordered strongest first.

---

### H1 — `discover_project` early-return at lines 992-997 burns call_ids silently. Reproduces the round-1 G1 failure mode.

**What.** Architect's A0 places `let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);` *immediately after* `let cfg = settings.read().await;` at line 989, *before* `let ac_new_dir = base.join(".ac-new");` at line 991. The function then has an early-return at lines 992-997:
```rust
let ac_new_dir = base.join(".ac-new");
if !ac_new_dir.is_dir() {
    return Ok(AcDiscoveryResult { agents: vec![], teams: vec![], workgroups: vec![] });
}
```
For any project path the user opens that does not contain `.ac-new` (e.g., a plain repo, a misconfigured folder), the call_id is incremented and **no A2 or A4 line is ever emitted**.

**Why it matters.** Per §5 reproduction protocol, the operator slices the log with `grep '[ac-discovery] call=42'`. If the user's session contains 5 `discover_project` invocations and 2 of them target paths without `.ac-new`, the operator sees `call=42, call=44, call=45, call=47, call=48` — gaps at 43 and 46 with NO log line attributing them. The operator cannot distinguish:
- early-return on missing `.ac-new` (benign),
- panic mid-call (bug),
- future-cancellation on window close (benign),
- a log filter trim (configuration error).

**This is the exact failure mode round-1 G1 won on**: silent absence of evidence used as proxy for a hypothesis. The whole point of A0 was to make per-call boundaries deterministic, not to introduce a new class of "missing call_id" mystery. The round-1 architect's own §3 T4 trailing paragraph ("A `tech-lead` replica that emits *no* T4 line at all under `=debug` capture is itself a signal") is the same brittle pattern; we rejected it for T4 in round 1 but reintroduced it for A0 in round 2.

**Fix.** Two equivalent options, both pure logging:

(a) **Move `fetch_add` AFTER the early-return guard.** Insert the `let call_id = ...` between line 998 (`}`) and line 1000 (`// Opportunistic: ensure gitignore`). This way, only calls that actually do work consume call_ids, and the sequence is dense.

(b) **Keep `fetch_add` where it is, but emit a positive trace on the early-return path.** Insert before line 993:
```rust
log::debug!(
    "[ac-discovery] call={} discover_project: early return (no .ac-new) — path='{}'",
    call_id, path
);
```
This preserves the property "every call_id appears exactly once in the log".

I recommend (a). It's simpler and matches the architect's framing in line 1006-1007 (*"Placed AFTER the .ac-new-missing early return so non-AC folders don't pay a wasted filesystem scan"*) — the same logic justifies placing call_id allocation after the same guard. (b) is acceptable if the architect prefers explicit "I returned early" lines for diagnostic completeness.

---

### H2 — On Windows GUI launches, `RUST_LOG` does not propagate. The §5 reproduction protocol can silently fail to capture T3/T1.b/T4 (now all at `debug!`).

**What.** Verified at `lib.rs:102-103`: `env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("agentscommander=info"))`. So env_logger reads `RUST_LOG`, default filter `agentscommander=info`. Plan §5 prescribes:
```
RUST_LOG=info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug
```

After the round-2 demotions (G3, G4), the **most diagnostically valuable lines** — T3 (`info!`→`debug!`), T1.b (`warn!`→`debug!`), all T4 branches (`debug!`) — require this env var to be set. **If the env var isn't set, none of those lines emit**, and the operator captures only the `info!` lines (T2 warns + A1/A2/A3/A4) — losing the entire C1-success and C2/C3-rejection-branch evidence the plan exists to provide.

**Why it matters on Windows.** Tauri apps are typically launched via:
- a desktop shortcut (no env vars from terminal apply),
- File Explorer double-click (same),
- a `.lnk` from the Start Menu (same),
- system tray restart after install.

In none of those cases does `set RUST_LOG=...` in a separate cmd.exe affect the app. The user must:
- launch from cmd.exe: `set RUST_LOG=info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug && agentscommander_mb.exe`, OR
- launch from PowerShell: `$env:RUST_LOG='...'; & 'C:\path\agentscommander_mb.exe'`, OR
- set as a system env var (`setx`) before launch (and start a new shell/process).

The current §5 says only *"Set `RUST_LOG=...` in the launch env. ... Launch mb.exe with `phi_fluid-mblua` already in `projectPaths`."* That is ambiguous on a Windows desktop app context. Worse: the user is investigating a **cross-binary** bug; they are likely launching via the file-name-distinguished `.exe` directly (e.g., double-clicking `agentscommander_mb.exe`), which is exactly the case where the env var won't apply.

**The diagnostic instrument silently fails.** No error, no warning — the operator captures app.log, sees only A1/A2/A3/A4 + T2 warns, concludes "no T4 reject log fired → it must be C3 (which T4 doesn't positively detect anyway, per round 1 G1's defeated alternative)" — and we are in the round-1 elimination trap a third time.

**Fix.** Rewrite §5 step 1 to be explicit about Windows launch. Suggested replacement:

> 1. Set `RUST_LOG` so env_logger captures the new `debug!` lines. **The default `agentscommander=info` filter at `lib.rs:103` suppresses T3, T1.b, and all T4 branches** — without this step the round-2 captured log will be missing the C1-success and C2/C3-rejection evidence the plan provides.
>
>    **On Windows, the env var must be set in the same shell that launches the app:**
>    - cmd.exe: `set RUST_LOG=info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug && agentscommander_mb.exe`
>    - PowerShell: `$env:RUST_LOG='info,agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug'; & 'C:\path\to\agentscommander_mb.exe'`
>    - Or use `setx RUST_LOG '...'` to persist system-wide, then **start a new shell** before launching (existing shells inherit the old env).
>    - **Do NOT launch via desktop shortcut, Start Menu, or File Explorer double-click** — those inherit the system environment at logon time and will not see `set` from a terminal.

Severity: high. This is the single change most likely to silently invalidate the round-2 reproduction.

---

### H3 — T1/T2/T3/T4 lines emitted DURING a `discover_*()` call are NOT correlated to call_id. The §5 grep recipe misses them.

**What.** Architect chose, by design, to thread `call_id` only through A1/A2/A3/A4 (in `ac_discovery.rs`), not through T1/T2/T3/T4 (in `teams.rs`). The architect's defense in §5 line 529: *"T4 ... has no per-call counter by design, since it is consulted by routing paths that have no notion of 'discovery call'."*

That defense is correct **for T4** (which is reachable from `is_coordinator_of` in routing paths). It is **not correct for T1/T2/T3**, which live inside `discover_teams_in_project`, which is only reachable from `discover_teams()` (line 888) — and `discover_teams()` is itself called from `discover_ac_agents` (line 571), `discover_project` (line 1008), and 10 other call sites.

**Why it matters.** §5 reproduction protocol step 5 prescribes: *"Slice the log per discovery call: `grep '[ac-discovery] call=42'`"*. That grep matches A1/A2/A3/A4 lines only. The very `[teams]` lines (T1.a, T1.c, T2.read, T2.parse, T2.entry, T3) — which the plan §3 lists as the C1 detectors — are filtered out. To slice T1/T2/T3 by discovery call, the operator must:

1. Look up the `[ac-discovery] call=42` first line's timestamp (start of call).
2. Look up the `[ac-discovery] call=42 ... summary` last line's timestamp (end of call).
3. Filter `[teams]` lines by timestamp range.
4. Hope no other concurrent `discover_*` call's `[teams]` lines interleaved within that window.

Step 4 is exactly the round-1 G5 failure. With concurrent invocations from the 12 call sites of `discover_teams()`, T-line interleaving is the original problem; A0 only solved it for A-surfaces.

**Why this round 2 is worse than round 1.** In round 1, the operator at least knew the T-lines weren't tagged because A-lines weren't either — the whole tape was a soup. In round 2, A-lines are clean (`call=42`), so the operator assumes they have a clean tape — but the T-lines (the actual C1 evidence) still interleave. False sense of cleanliness.

**Fix.** Two acceptable resolutions:

(a) **Doc-only:** Update §5 step 5 to be explicit:
   > 5. Slice the log per discovery call: `grep '[ac-discovery] call=42'` for A-surfaces; for T-surfaces (T1/T2/T3/T4), take the first and last timestamps of `call=42`'s A-lines as a window, then `awk` or grep on `[teams]` lines within that window. Note that with concurrent `discover_*` invocations, T-lines from overlapping calls will interleave within the same window — the operator must visually disambiguate or instrument call_id propagation (out of scope for this issue).

(b) **Code change (heavier):** Plumb `call_id` from `discover_*` through `discover_teams()` → `discover_teams_in_project` as an optional argument. T-surfaces emit `[teams] call={} discover_teams: ...` when called from a discovery context; existing 10 non-discovery call sites pass `0` or omit the prefix. This is mostly logging-only but does change function signatures of `discover_teams` and `discover_teams_in_project` — borderline-refactor.

I recommend (a) — call out the limitation and let the operator know that T-line/A-line correlation is timestamp-based with potential interleaving on busy systems. Architect can revisit (b) in a follow-up issue if the round-2 captured log proves ambiguous.

---

### H4 — Combined-mismatch blind spot in T4: the (`project != team.project ∧ suffix != suffix`) leaf emits no log.

**What.** The five T4 branches cover:
| `wg_team == team.name` | `agent_project == team.project` | `suffix == suffix` | Branch fired |
|---|---|---|---|
| any | n/a | n/a | preceded by direct-match → returns true if hit |
| true | true | true | `wg-aware-match → true` |
| true | true | false | `reject-suffix-mismatch` (G1, new) |
| true | false | true | `reject-project-mismatch` |
| true | false | **false** | **NONE** |
| false | n/a | n/a | NONE (not the team) |

The bottom-true row — `wg_team == team.name ∧ agent_project != team.project ∧ agent_suffix != agent_suffix` — fires no log. The architect's framing in §3 T4 trailing paragraph claims *"A `tech-lead` replica that emits *no* T4 line at all under `=debug` capture is itself a signal — it means `extract_wg_team` returned `None` or the outer `coordinator_name = None`"*. That claim **misses this row**, because here `extract_wg_team` IS Some (we're inside its `if let Some(wg_team) = ...` arm) and `coordinator_name` IS Some (we're inside the outer `if let Some(ref coord_name) = team.coordinator_name`).

**Why it matters.** This is the same anti-pattern G1 won on. Lower likelihood than C2-alone or C3-alone (would require two normalization bugs across binaries simultaneously), but possible. Cross-binary state divergence is precisely the bug class that produces "two things were assumed independent and turned out to be coupled".

**Fix.** Add a sixth branch right after `reject-suffix-mismatch`:
```rust
if wg_team == team.name
    && agent_project != team.project
    && agent_suffix(agent_name) != agent_suffix(coord_name)
{
    log::debug!(
        "[teams] is_coordinator: reject-both-mismatch → false — agent='{}' agent_project='{}' team_project='{}' team='{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
        agent_name, agent_project, team.project, team.name, coord_name,
        agent_suffix(agent_name), agent_suffix(coord_name)
    );
}
```

Severity: low-medium. Likelihood is lower than C2-alone or C3-alone, but the cost is the same as G1's fifth branch (~hundreds of bytes per repro at debug level, bounded by `wg_team == team.name` gate). The operator gets a complete decision tree instead of one remaining "if no log, it must be one of [extract_wg_team=None, coordinator_name=None, both-mismatch] — which?" by-elimination question.

If architect rejects: at minimum, update §3 T4 trailing paragraph to enumerate "no T4 line" → 3 hypotheses (extract_wg_team=None, coordinator_name=None, both-mismatch) so the operator's elimination is correct.

---

### H5 — `cancellation` of the `discover_*` future leaves dangling A1/A2 lines without an A3/A4 close. Worth a §5 doc note.

**What.** Tauri commands are async fns whose future is dropped if the frontend window closes or the IPC connection breaks mid-call. If cancellation happens between the `let call_id = ... fetch_add ...` and the A3/A4 emission, the operator sees `call=42` with N A1 lines and no summary. Same observable as a panic (which I'm not aware AC raises in these paths, but never say never).

**Why it matters.** Less severe than H1 (cancellation is rare, early-return is routine). But the operator looking at a captured log with a dangling sequence will waste cycles diagnosing "did the discovery panic? did it filter-out? was it cancelled?" with no signal to disambiguate.

**Fix.** Pure documentation. Add to §5:
> **Note on incomplete sequences.** A `[ac-discovery] call=N` replica/A1 line without a matching `[ac-discovery] call=N ...summary` line indicates the `discover_*` future was dropped before completion (cancellation on window close, panic, or process termination). The captured replica lines are still diagnostically valid; only the aggregate counters (workgroups/teams/replicas/coordinator) at A3/A4 are missing for that call.

---

### H6 — A0's `fetch_add(1, Ordering::Relaxed)` analysis is correct.

I tried to break it.

- **Overflow:** ~5×10¹¹ years at 1 call/ms. Trivially safe.
- **Ordering choice:** `Relaxed` is the canonical idiom for monotonic counters whose value is observed but does not synchronize other state. Architect's reasoning at line 350 is correct.
- **Concurrent fetch_add:** `AtomicU64::fetch_add` is `lock xadd` on x86-64 / `ldadd` on ARM64 — atomic, no torn writes.
- **Static initialization:** `AtomicU64::new(0)` is `const`; no init-time side effects, no panic, no race.
- **Lifetime/scope of `call_id` in the function body:** `u64` is `Copy`. Accessible everywhere downstream including the deeply-nested A1 emission inside the for-for-for nest. ✅
- **Duplicate import risk:** `use std::sync::atomic::{AtomicU64, Ordering};` doesn't collide with the existing `use std::sync::Arc;` (different submodule). ✅

**No new issue.** The A0 mechanism itself is correct; H1 is about *where* it's called, not *how*.

---

### H7 — `env_logger` filter syntax verified; §5's prescribed module paths are correct.

I verified at `Cargo.toml:16` (`env_logger = "0.11"`) and `lib.rs:102-103` (`Builder::from_env(Env::default().default_filter_or("agentscommander=info"))`). The crate name is `agentscommander_lib` (per existing `_plans/fix-issue-69-coordinator-detection.md:1160` precedent and Cargo.toml). env_logger uses prefix matching on module paths with `::` separators, so `agentscommander_lib::config::teams=debug,agentscommander_lib::commands::ac_discovery=debug` is well-formed.

**No issue with the filter syntax.** The risk is the env-var-propagation issue (H2), not filter syntax.

---

### H8 — T4 fifth branch (`reject-suffix-mismatch`) compile/semantic verification.

I traced the new code at lines 290-299:
```rust
if wg_team == team.name
    && agent_project == team.project
    && agent_suffix(agent_name) != agent_suffix(coord_name)
{
    log::debug!(
        "[teams] is_coordinator: reject-suffix-mismatch → false — agent='{}' team='{}/{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
        agent_name, team.project, team.name, coord_name,
        agent_suffix(agent_name), agent_suffix(coord_name)
    );
}
```

- **Compile check.** `wg_team: &str` (return of `extract_wg_team`); `team.name: String`; comparison is `&str == String`, `PartialEq<String> for &str` is implemented. ✅ `agent_project: &str`; `team.project: String`; same. ✅ `agent_suffix(...)` returns `&str`; comparing two `&str`s is fine. ✅
- **Mutual exclusion with prior branches.** The `wg-aware-match → true` branch returns early on `(==, ==)`. `reject-project-mismatch` matches `(!=, ==)`. New branch matches `(==, !=)`. No overlap. ✅
- **Double-emit check.** Both `reject-project-mismatch` and `reject-suffix-mismatch` are conditional `if { log; }` — they don't return. Both could theoretically fire in the same call IF their conditions overlapped, which they don't. ✅
- **Allocation in args.** `agent_suffix(agent_name)` and `agent_suffix(coord_name)` each return `&str` with no allocation (it's just `split('/').next_back()`). The two duplicate calls per emit are 2 byte-iterator passes; trivial cost. ✅

**No new issue with the 5th branch itself.** The only T4 concern is H4 (the missing 6th branch).

---

### H9 — G8/G9 reconsidered. No, both stay deferred.

I re-read the architect's deferral reasoning at §930-933. Both are sound:
- **G8 (apostrophe-in-paths):** existing log corpus already uses `'…'` enclosure; never observed in 8 years; cost of adopting `{:?}` would change downstream tooling more than benefit. Accept.
- **G9 (T3 prefix collision with the aggregate):** architect's preserve-existing-lines directive is solid; operator can grep with `[teams] discovered team —` (em-dash) to slice T3-only. Accept.

**No new issue.** Concur with deferral.

---

### Summary of round-2 changes for round-3 consideration

| ID | Severity | Action | Notes |
|---|---|---|---|
| **H1** | must-fix | Move `fetch_add` AFTER the `.ac-new`-missing early return in `discover_project` (or emit a positive trace before early return) | **Reproduces round-1 G1 failure mode.** |
| **H2** | must-fix | Rewrite §5 step 1 to explicitly document Windows env-var propagation: cmd.exe / PowerShell launch syntax, warning against desktop-shortcut launches | **Diagnostic instrument silently fails on the most common Windows launch path.** |
| **H3** | should-fix | Update §5 step 5 to acknowledge T1/T2/T3 lines are not carried by `call_id` and require timestamp windowing; warn about T-line interleaving on busy systems | Doc-only. Heavier alternative: plumb call_id through teams.rs (borderline refactor; not recommended). |
| **H4** | should-fix | Add a 6th `reject-both-mismatch` branch in `is_coordinator` for `(wg_team == team.name ∧ project != project ∧ suffix != suffix)` | Same anti-pattern as G1; lower likelihood, same fix cost. If rejected, at minimum update §3 T4 trailing paragraph to enumerate the now-3 "no T4 line" hypotheses. |
| **H5** | doc-fix | Add a §5 note about incomplete sequences (cancellation/panic) producing dangling A1 without A3 | Minor. |
| **H6, H7, H8, H9** | n/a | Positive verifications — A0 mechanism correct, env_logger filter correct, 5th branch compiles, G8/G9 deferral sound. | No issue. |

**Strongest concern at the top: H1.** The round-2 implementation of A0 (which I asked for in G5) introduced a silent-gap behavior in `discover_project` that reproduces the exact failure mode round-1 G1 fought against. The fix is one of two trivial pure-logging changes. Without it, the operator looking at the round-2 captured log will see a gappy call_id sequence and have no positive evidence to attribute the gaps — and we are in the elimination trap I argued against in round 1, with the same anti-pattern.

**Honest grinch verdict on the round-2 changes overall:** the C-vs-D and C2/C3 detection mechanisms are now strong (G1 + 5 T4 branches give positive evidence on three distinct rejection paths). The remaining defects are about **correlation and capture fidelity**, not detection — but H2 in particular could silently invalidate the entire investigation if the user launches via desktop shortcut. Fix H1, H2, H4 (cheap), document H3, H5 (free). Then I will approve.

If architect adopts H1 + H2 + the §3 T4-paragraph clarification for H4, I approve in round 3 without further iteration.

---

## Architect updates (round 3)

> Updater: architect. Adjudicating grinch's round-2 review (H1–H9). Tech-lead noted H1 + H2 are non-negotiable; H4 grinch accepted either code-fix or doc-only. I am going beyond the minimum bar on H4 because the round-2 G1 adjudication committed us to "positive evidence beats elimination" and a 6th branch is the same trade.

### Fixes applied — where each landed

| ID | Severity | Action | Where in plan |
|---|---|---|---|
| **H1** | must-fix | Moved `discover_project`'s `let call_id = fetch_add(…)` from line 989 to **after** the `.ac-new`-missing early-return guard (i.e. immediately after the closing `}` at line 998 and before the `// Opportunistic: ensure gitignore` comment block at line 1000) | §3 Surface A0 — "In `discover_project` body" subsection rewritten with the new placement + parenthetical citing line-1006–1007's existing comment as the precedent. Picked grinch's option (a) (move) over option (b) (emit early-return trace) — purer, no gap, no extra log surface. |
| **H2** | must-fix (CRITICAL) | Rewrote §5 step 1 to explicitly mandate `set RUST_LOG=…` in the launching shell, with cmd.exe / PowerShell / `setx` snippets and an explicit ⚠️ warning against desktop-shortcut/Start-Menu/double-click launches | §5 reproduction protocol step 1 — fully replaced. The original one-line "set the env var" instruction was insufficient on Windows GUI launches; the new version is multi-paragraph with command snippets and the failure-mode warning grinch demanded. |
| **H3** | should-fix (doc-only) | Updated §5 step 5 to acknowledge T-surfaces don't carry `call_id`, requiring timestamp-window matching, and warned about T-line interleaving when concurrent `discover_*` invocations overlap | §5 step 5 split into A-surface and T-surface bullets with the timestamp-window caveat. Did NOT plumb `call_id` through `teams.rs` — borderline-refactor that would change `discover_teams` / `discover_teams_in_project` signatures and `is_coordinator` is also reachable from non-discovery routing paths (no notion of "discovery call" there). |
| **H4** | should-fix → adopted code-fix (option a) | Added 6th `reject-both-mismatch` branch in `is_coordinator` for `(wg_team == team.name ∧ agent_project != team.project ∧ agent_suffix != agent_suffix)`; updated §3 T4 trailing paragraph to claim **complete positive coverage** of every reachable leaf when `extract_wg_team` and `coordinator_name` are both `Some(_)` | §3 Surface T4 code block — appended after `reject-suffix-mismatch`. §3 T4 trailing rationale rewritten: "six log lines together form a complete positive-evidence decision tree". §6 surface table — new T4.both-mismatch row. |
| **H5** | doc-fix | Added §5 "Note on incomplete sequences" describing dangling A1/A2 without A3/A4 close as cancellation/panic indicator | §5 — new note paragraph between the T4-fan-out note and the expected log slices. |

### H4 — adopted option (a) [code], not option (b) [doc-only]. Reasoning.

Grinch accepted either: add the 6th branch, or update the §3 T4 paragraph to enumerate three "no T4 line" hypotheses (extract_wg_team=None, coordinator_name=None, both-mismatch). Tech-lead said "(a) is more positive evidence." The round-2 G1 adjudication committed us to a principle: *"positive identification of which gate rejected beats diagnosis-by-elimination, especially in a cross-binary debug scenario where unenumerated 4th hypotheses are exactly the bug class."* That principle applies symmetrically to H4. The cost of the 6th branch is identical to G1's 5th: a single `if { log; }` block, ~tens to hundreds of bytes per repro at debug, fires only when the `wg_team == team.name` gate already passes. The benefit is symmetric too — the operator no longer has to reason "if no T4 line, it's one of these three sub-hypotheses, which?" and can read the captured log as a direct decision-tree audit. Going with the doc-only fix would have re-opened the round-2 elimination loophole that G1 closed for one rejection axis only. Closing both axes makes the diagnostic instrument complete on the architect-named hypothesis space.

After the H4 fix, the only remaining "no T4 line" interpretations are tightly constrained to two orthogonal cases (`extract_wg_team` returning `None`, or `team.coordinator_name = None` — the latter independently surfaced by T3's `coord_name=None` summary). Both are out of the C2/C3 hypothesis space the issue prioritizes.

### Intentionally not done

- **H3 option (b)** (plumb `call_id` through `teams.rs`): not adopted. It would change `discover_teams()` and `discover_teams_in_project()` signatures, force every one of the 12+ call sites to pass an optional argument, and `is_coordinator` would still need a per-team-call counter for routing-path emissions to be partitioned — i.e. plumbing one layer doesn't actually solve the routing-fan-out interleaving anyway. The doc-only treatment is honest about the limitation; investigation operators get the timestamp-window recipe and the explicit caveat about busy-system interleaving. If the round-3 captured log proves ambiguous in practice, that is the moment to revisit (a follow-up issue, not this one). Tech-lead's brief was "diagnostic logging only, no behavior change" — function-signature changes are out of scope.
- **A "diagnostic build" with bumped default filter** (grinch's H2 optional consideration): not adopted. Tech-lead's brief explicitly excludes binary-behavior changes; the §5 doc-fix is the right shape.
- **G8** (`'`-in-paths quoting), **G9 ≡ D7** (T3 prefix collision): re-confirmed deferred per round-2 reasoning (H9 — grinch concurred with the round-2 deferral). No round-3 change.
- **Hoisting `agent_suffix(coord_name)` to a let-binding** in the T4 function: still not adopted. Same "no refactor" rule that applied to round 2's 5th branch applies to round 3's 6th. Dev-rust implements the duplicate calls as-written.

### Round-3 surface count

- **9 logical surfaces** (unchanged from round 2 — H4's 6th branch is a new emission inside the existing T4 surface).
- **16 → 17 log emission sites** (+1 from T4.both-mismatch).
- T4 now emits on **6 distinct decision-tree leaves** (was 4 in round 1, 5 in round 2). With both `extract_wg_team` and `coordinator_name` Some, every reachable path emits exactly one log.
- One stdlib import (`std::sync::atomic::{AtomicU64, Ordering}`), one module-level static, two `fetch_add` call sites in `ac_discovery.rs` (one of which moved in round 3 per H1).

### Re-confirmed: no behavior change

- T4's six branches still terminate `is_coordinator` at the same `false` (rejection) or via the same `return true;` (success) as the original two-branch implementation. The logs are pure side effects.
- H1's relocation of `fetch_add` does not change the early-return's observable behavior — the function still returns `Ok(AcDiscoveryResult { agents: vec![], teams: vec![], workgroups: vec![] })` for non-AC folders. Only the side effect of incrementing the counter has moved.
- §5 reproduction protocol changes are documentation only; no source code touched by H2, H3, or H5.

### Hand-off

Plan should now satisfy grinch's stated round-3 approval bar (H1 + H2 + H4 paragraph clarification — all adopted, plus H4 code branch and H3/H5 doc notes for completeness). Dev-rust: no new code-shape concerns since round 1 D-notes were already a CONCUR; the only delta you need to verify is (a) the H1 placement of the second `fetch_add` lands cleanly between the early-return `}` and the gitignore comment block, and (b) the new T4 6th branch's `if`-condition is mutually exclusive with `wg-aware-match`, `reject-project-mismatch`, and `reject-suffix-mismatch` (it is — see grinch H8 truth table). Grinch: H1, H2, H4 code-and-doc landed; H3 + H5 doc notes added; G8/G9 deferral preserved. If you find anything new, I'll iterate. If both reviewers concur, this is final.

---

## Grinch adversarial review (round 3)

Verdict: **CONCUR.** Architect adopted H1 (option a, the cleaner one), H2 (full §5 step 1 rewrite with cmd.exe / PowerShell / setx + ⚠️ shortcut warning), and H4 went **beyond** my minimum bar (chose the code-fix, not the paragraph clarification I had accepted as adequate). H3 and H5 doc notes added. My stated round-3 approval condition is met — and exceeded on H4.

Re-verified each round-3 change against the corresponding finding:

### H1 verification — `fetch_add` placement in `discover_project`

✅ Moved to between the closing `}` of the `.ac-new`-missing guard (line 998) and the `// Opportunistic: ensure gitignore` comment (line 1000). Grinch H1 option (a). Only calls that pass the `.ac-new` check now consume `call_id`s; the sequence is dense for the routine case the user actually exercises.

### H2 verification — §5 step 1 operator-readability

✅ Three concrete launch snippets (cmd.exe single-line, PowerShell single-line, `setx` + new shell) — each copy-pasteable. The ⚠️ warning explicitly enumerates desktop shortcut, Start Menu, taskbar pin, and File Explorer double-click as the failure paths. Tied to the issue's specific cross-binary launch context (the user is most likely to launch by file-name-distinguished `.exe` directly, which is the failure case). An actual user can follow this without guessing.

### H4 verification — 6th `reject-both-mismatch` branch

✅ Code at lines 300-309 fires on `(wg_team == team.name ∧ agent_project != team.project ∧ agent_suffix(agent_name) != agent_suffix(coord_name))`. Mutually exclusive with the other 5 branches by construction (each named branch covers a distinct cell of the `wg_team == team.name` × {proj_match, suffix_match} truth table; combined they tile all 4 cells; the `wg_team != team.name` cells produce no log, which is correct). After the H4 fix, with `extract_wg_team = Some(_)` and `coordinator_name = Some(_)`, every reachable path through the conditional emits exactly one log line. Architect's "complete positive-evidence decision tree" claim at line 324 is correct.

### H3, H5 verification

✅ §5 step 5 split into A-surface and T-surface bullets with timestamp-window caveat and concurrent-interleaving warning. ✅ §5 incomplete-sequences note added between T4-fan-out note and expected log slices.

---

### Residual minor caveat (informational, does not block)

H1's relocation of `fetch_add` past the `.ac-new`-missing early-return guard at line 992-997 closes the routine gap (every project the user opens that isn't an AC project). However, **a second early-return path exists at `ac_discovery.rs:1022`** that is now also downstream of the moved `fetch_add`:

```rust
let entries = match std::fs::read_dir(&ac_new_dir) {
    Ok(e) => e,
    Err(e) => return Err(format!("Failed to read .ac-new directory: {}", e)),
};
```

If `std::fs::read_dir(&ac_new_dir)` returns `Err` after `ac_new_dir.is_dir()` already returned `true` (TOCTOU race with concurrent deletion, NTFS ACL where `is_dir` succeeds but `read_dir` is denied, or transient I/O error), `call_id` is consumed and no A2/A4 fires.

**Why this is informational, not a round-3 must-fix:**
1. **Much narrower than the original gap.** The original H1 case (`.ac-new`-missing) is routine — every non-AC folder the user opens. The `read_dir` failure case is rare (TOCTOU + permission anomalies on Windows only).
2. **Observable via other channels.** This branch returns `Err(...)` to the Tauri command boundary, which propagates to the frontend as a user-visible error (typically a toast or console warning). The operator can correlate the missing `call_id` with the timestamp of the visible error — unlike the original H1 case, where the `.ac-new`-missing path silently returned `Ok(empty)` with no surface signal.
3. **Same fix shape if pursued later.** A future tightening would either (a) move `fetch_add` inside the success arm of the `read_dir` match, or (b) emit a positive `[ac-discovery] call=N discover_project: read_dir failed — path='...' err='...'` debug line before the `return Err(...)`. Both are pure-logging changes consistent with the issue's framing; either could be filed as a separate plan-tightening if the residual gap matters in practice.

I am not blocking on this. Filing as a known caveat for posterity. If the round-3 capture surfaces a missing `call_id` that turns out to be this case, the follow-up fix is one-liner.

---

### What I tried to break in round 3 and could not

- **6th branch double-emit:** the four reject branches under `wg_team == team.name` are mutually exclusive on the (proj_match, suffix_match) ∈ {true,false}² lattice. The success branch (`wg-aware-match → true`) covers (true, true) and short-circuits via `return true;`, so reject branches can never fire after a success. ✅
- **6th branch false-positive:** new branch fires on `proj != ∧ suffix !=`. Existing `reject-project-mismatch` is `proj != ∧ suffix ==`. Existing `reject-suffix-mismatch` is `proj == ∧ suffix !=`. No overlap. ✅
- **Static init / ordering:** unchanged from round 2. Confirmed correct.
- **call_id `Copy`/scope:** `u64` is `Copy`; accessible in deeply nested A1/A2 emission sites without lifetime issues. ✅
- **Format string types on the 6th branch:** `agent_name: &str`, `agent_project: &str`, `team.project: String`, `team.name: String`, `coord_name: String` (via deref from `&String`), `agent_suffix(...): &str`. All `Display`-compatible. ✅
- **§5 H2 launch snippets correctness:**
  - cmd.exe `set RUST_LOG=... && exe` — `set` in cmd is process-local; the `&&` chains immediately so the env applies. ✅
  - PowerShell `$env:RUST_LOG='...'; & 'path'` — `$env:` sets process env; `&` is the call operator for paths with quoting. ✅
  - `setx` correctly noted as requiring a fresh shell (existing shells inherit old env). ✅

---

### Round-3 final verdict

**CONCUR.** Plan is implementable, diagnostic instrument is complete on the architect-named hypothesis space (C1 / C2 / C3 / D), and the round-2 elimination-trap defects (H1, H2, H4 blind spot) are positively fixed. The residual `read_dir` early-return caveat above is documented but does not block.

If dev-rust also concurs, this is final. Move to implementation per round-1 D9 ordering recommendation.
