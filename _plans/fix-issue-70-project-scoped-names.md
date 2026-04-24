# Fix issue #70 — project-scoped agent and team names

**Issue:** https://github.com/mblua/AgentsCommander/issues/70
**Branch:** `fix/issue-70-project-scoped-names` (based on `main@05dde7e`, v0.7.5)
**Scope:** wire-protocol change across backend routing, CLI, transport JSON, frontend display, and tests.

> **Round 1.** Expect iteration with dev-rust and dev-rust-grinch. Decisions below are the architect's call; rationale is recorded so reviewers can challenge specific points without re-litigating the whole design.

---

## 1. Requirement

Two sub-bugs, per issue #70:

- **Bug A — Team name collisions across projects.** `DiscoveredTeam.name` is the raw team dir name (e.g. `devs`). `is_in_team`'s WG-aware branch at `src-tauri/src/config/teams.rs:154-168` fires on `extract_wg_team(agent_name) == team.name` — any team of the same name in any project matches. Leaks `can_communicate`, `is_any_coordinator`, and `list-peers` membership across unrelated projects.
- **Bug B — WG replica agent names lack project context.** `agent_name_from_path` returns `wg-N-team/agent` (no project). `find_active_session` at `src-tauri/src/phone/mailbox.rs:916-926`, `find_all_sessions` at `src-tauri/src/phone/mailbox.rs:976-984`, and `resolve_repo_path` at `src-tauri/src/phone/mailbox.rs:1236-1272` route via CWD substring/suffix match. With two projects both containing `wg-1-devs/tech-lead`, selection is non-deterministic w.r.t. the sender's intended project.

Origin agents already include `project/` (via `resolve_agent_ref` in `config/teams.rs:38-67`). Bug B is WG-replica-specific. Bug A affects the team record itself.

**Goal:** every agent identity on the wire is project-qualified, and every routing decision compares project-qualified strings with exact equality. No substring/suffix fuzziness that lets a peer in project A stand in for one in project B.

---

## 2. The four design decisions

### Decision 1 — Fully-qualified name format

**Choice: `<project>:<wg>/<agent>` for WG replicas. Origin agents stay `<project>/<agent>` unchanged.**

Candidates considered:

| Option | Verdict |
|---|---|
| (a) `<project>/<wg>/<agent>` — 3 slash components | **Rejected.** Every call site that currently does `agent_name.split('/').next()` to pull the WG prefix (`extract_wg_team` at `config/teams.rs:106`, same-WG check in `can_communicate` at `config/teams.rs:226-231`, WG-aware branches in `mailbox::resolve_repo_path`, `resolve_wg_path_from_sessions`, `deliver_wake` spawn path) would silently start returning the project. A rename of `extract_wg_team` won't catch the `can_communicate` inline split. High churn, high regression risk. |
| (b) `<project>:<wg>/<agent>` — colon separates project boundary | **Chosen.** A single new first-pass helper `split_project_prefix(fqn) -> (Option<&str>, &str)` peels the project. All existing WG-aware code operates unchanged on the `<wg>/<agent>` tail. Colon is unambiguous: agent dir names cannot contain `:` on Windows (ASCII-reserved for drive letters). Pre-existing `project/agent` origin form is untouched. |
| (c) Separate `project` field on `OutboxMessage` + session state | **Rejected.** Every legacy outbox JSON lacks the field, every CLI caller needs a new `--project` arg, every `list-peers` consumer needs to re-key. Sprawls the wire change far beyond the bug. |
| (d) Hash-based opaque ID | **Rejected.** Debuggability matters — log lines and rejected-reason strings must be readable at a glance. |

**Canonical forms after this fix:**

| Entity | Before | After |
|---|---|---|
| Origin matrix agent | `project-folder/agent` | `project-folder/agent` *(unchanged)* |
| WG replica | `wg-N-team/agent` | `project-folder:wg-N-team/agent` |
| `DiscoveredTeam.name` | `devs` | `devs` *(unchanged — new `project` field carries scope, see §3)* |

Why not also project-scope origin agents with a colon? Origin names are already unique by path (`project/` prefix is in-name); the leak only exists for WG replicas. Introducing `:` on origin agents is a gratuitous breaking change to every stored `teams.json`, `conversations.json`, outbox, and saved session — zero upside, wide blast radius. Keep origin untouched.

### Decision 2 — Backward compatibility for `--to` resolution

**Choice: lenient — accept unqualified WG names when the resolution is unambiguous across all currently-known projects. Reject with a listing when ambiguous.**

Resolution order for `--to <target>` in `cli/send.rs`:

1. If `<target>` contains `:` → treat as fully qualified. Exact match only.
2. Else if `<target>` matches shape `wg-N-team/agent` (2 components, first starts with `wg-`):
   a. Enumerate WG replicas across `settings.project_paths` matching this local part.
   b. If exactly 1 → resolve, log at `info` (`"unqualified --to resolved: wg-1-devs/tech-lead → repo-a:wg-1-devs/tech-lead"`).
   c. If >1 → **reject** (`"ambiguous --to 'wg-1-devs/tech-lead': matches repo-a:wg-1-devs/tech-lead, repo-b:wg-1-devs/tech-lead. Qualify with <project>:"`).
   d. If 0 → reject (`"unknown --to 'wg-1-devs/tech-lead'"`).
3. Else (origin-shaped `project/agent` or bare `agent`) → existing resolution path (origin names were never ambiguous in a cross-project sense; leave as-is).

**Why lenient:** thousands of lines of existing docs, `CLAUDE.md`, user habit, and current `_plans/*.md` reply protocols hard-code `wg-N-team/agent`. A strict break forces a day of doc rewrites and retrains muscle memory for no routing correctness gain — the ambiguity case is exactly what rule (2c) catches.

**Why reject-on-ambiguity (not "pick first"):** the old code's bug was that it silently picked one when multiple matched. Rule (2c) surfaces the ambiguity to the user. Zero silent misrouting. Clear error, clear fix ("prefix with `<project>:`"). The cost is that in the future, if a user creates a colliding project, some `--to` commands start failing — which is the exact moment they *need* to know about the collision.

### Decision 3 — Migration of existing on-disk state

**Choice: tolerate on read, auto-upgrade on write. No forced cleanup.**

Inventory of affected on-disk artifacts:

| Artifact | Location | Current contents | Treatment |
|---|---|---|---|
| Outbox messages | `<repo>/.agentscommander/outbox/*.json` | `OutboxMessage` JSON with `from`, `to`, `sender_agent`, `preferred_agent`, `target` | **Tolerate.** Parse unchanged (string fields, no schema break). On route, apply Decision 2's lenient resolution. Log at `info` when legacy form is normalized. |
| Delivered/rejected archives | `outbox/{delivered,rejected}/*.json` | Same shape as above | Read-only historical data. Not re-routed. No migration. |
| Conversation logs | `~/.agentscommander/conversations/*.json` | `PhoneMessage.from/to` | **Tolerate.** Display as-is. `find_existing` participant match uses string equality; legacy conversations stay queryable by their legacy participants. |
| WG messaging files | `<wg-root>/messaging/*.md` | Filename encodes short `from`/`to` | **Tolerate.** `validate_filename_shape` at `phone/messaging.rs:114-186` already accepts `[a-z0-9]+` segments — the new forms pass. No rename pass. |
| `sessions.toml` / session-state | `session/manager.rs` in-memory; not persisted to disk | N/A | No migration. |
| Exported `teams.json` (host's CLAUDE.md references) | `~/.agentscommander/teams.json` | Display listings, not authoritative | Emit new forms on next write. Old entries tolerated on read. |

**In-flight outbox items at upgrade time.** A message written by v0.7.5 with `to = "wg-1-devs/tech-lead"` is picked up by v0.7.6 (this fix). It goes through lenient resolution (Decision 2). If unambiguous → delivered. If ambiguous → rejected with the new clear error. Zero data loss; worst case is one message needing the sender to re-qualify.

**Why not auto-upgrade files.** Walking every outbox / conversation / messaging file to rewrite `from`/`to` is ~200 LOC of migration code that exists to save users one-time friction on ~5 in-flight messages. Tolerate-on-read costs nothing at steady state and keeps the delta small.

**Why not reject.** Forcing users to delete `~/.agentscommander/conversations/` or `outbox/` is destructive. No.

### Decision 4 — Filename budget (measured, not hand-waved)

**Measurements (all grepped / read, not estimated):**

- `PTY_SAFE_MAX = 1024` at `src-tauri/src/phone/messaging.rs:12`
- `PTY_WRAP_FIXED = 19` at `src-tauri/src/phone/messaging.rs:20` (verified by contract test `format_pty_wrap_matches_pty_wrap_fixed` at `messaging.rs:660-664`)
- `MAX_SLUG_LEN = 50` at `src-tauri/src/phone/messaging.rs:14`
- `MAX_COLLISION_SUFFIX = 99` → `.N` suffix max 3 chars
- Filename shape: `YYYYMMDD-HHMMSS-<from_short>-to-<to_short>-<slug>[.N].md` built by `build_filename` at `messaging.rs:99-107`, validated by `validate_filename_shape` at `messaging.rs:114-186`
- `agent_short_name` at `messaging.rs:74-83` maps `wg-N-team/agent` → `wgN-agent` (compacts WG prefix)

**Current worst-case filename:**
- Fixed chars: 8 (date) + 1 + 6 (time) + 1 + 4 (`-to-`) + 1 + 3 (`.N`) + 3 (`.md`) = 27 + 3 = **30**
- `from_short = wgN-{agent}` realistic ≤ 4 + 1 + 30 = 35 (agent dir names uncapped; 30 is the realistic upper bound observed in-repo)
- `to_short` same ≤ 35
- slug ≤ 50
- **Worst filename: 30 + 35 + 35 + 50 = 150 chars**

**Current worst-case PTY notification** (`\n[Message from <from>] Nuevo mensaje: <abs_path>. Lee este archivo.\n\r`):
- overhead = 19 (`PTY_WRAP_FIXED`) + `from.len()` where `from = wg-N-team/agent` ≤ ~55
- body = 34 (literal) + `abs_path.len()`
- `abs_path` = path-to-wg-root (~80 chars) + `/messaging/` (11) + filename (≤150) = ≤ **241**
- Total: (34 + 241) + (19 + 55) = 275 + 74 = **349 chars** — margin: **675 bytes under PTY_SAFE_MAX**

**Worst case after adding `<project>:` to sender FQN only** (chosen path, per below):
- overhead = 19 + `from.len()` where `from = <project>:wg-N-team/agent`, realistic `project` ≤ 30 → sender ≤ ~86
- body unchanged (filename shape unchanged, see below)
- Total: 275 + (19 + 86) = **380 chars** — margin: **644 bytes under PTY_SAFE_MAX**

**Mitigation — chosen: filename shape unchanged; project carried in JSON body only.**

Rationale:
- The messaging directory is `<project-root>/.ac-new/<wg>/messaging/` — the **project is already implicit in the file's path**. There is no disambiguation need inside the filename.
- `OutboxMessage.from` and `.to` in the JSON body already carry the authoritative FQN (including project after this fix). The JSON is what routing consults.
- Keeping `from_short` and `to_short` in filenames as today (WG-local `wgN-agent`) preserves human readability of message files and leaves 100 bytes of NTFS `NAME_MAX` headroom.
- The only PTY-side growth is the `from` FQN in the injection wrap (~30 bytes). PTY_SAFE_MAX remains comfortably above ceiling.

Alternatives weighed:
- **Project prefix in filename** (`<projectN>-<wgN>-<agent>`): adds ~30 chars × 2 to each filename. Pushes pathological case against NTFS `NAME_MAX=255` and Windows legacy `MAX_PATH=260`. Rejected — no routing benefit (JSON body is authoritative) for real filesystem-limit risk.
- **Short-hash project prefix** (first 8 chars of project + hash-on-collision): adds ~10 chars; overkill for a problem that doesn't exist once we accept that the messaging dir's location encodes the project.
- **Raise PTY_SAFE_MAX**: unnecessary — we're not near it.

---

## 3. Data model changes

### 3.1 `DiscoveredTeam` gains `project` field
`src-tauri/src/config/teams.rs:5-17`

```rust
#[derive(Debug, Clone)]
pub struct DiscoveredTeam {
    pub name: String,
    /// NEW — project folder this team was discovered in (dir name, not path).
    /// Forms the left-hand side of the canonical FQN for WG replicas matched to this team.
    pub project: String,
    pub agent_names: Vec<String>,
    pub agent_paths: Vec<Option<PathBuf>>,
    pub coordinator_name: Option<String>,
    pub coordinator_path: Option<PathBuf>,
}
```

Populated in `discover_teams_in_project` at `config/teams.rs:394-401` — `project: project_folder.clone()` is already in scope from `config/teams.rs:324-328`.

### 3.2 New helpers in `config/teams.rs` (add near top, after imports)

```rust
/// Split a possibly-qualified agent name into (project, local) parts.
/// Returns (None, name) when no `:` separator is present (backward-compat path).
pub fn split_project_prefix(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((proj, local)) if !proj.is_empty() && !local.is_empty() => (Some(proj), local),
        _ => (None, name),
    }
}

/// Derive the fully-qualified agent name from a CWD.
/// WG replica CWD `<...>/<project>/.ac-new/wg-N-team/__agent_alice`
///     → `<project>:wg-N-team/alice`
/// Non-WG CWD `<...>/<project>/<agent>`
///     → `<project>/<agent>` (unchanged from `agent_name_from_path`)
pub fn agent_fqn_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

    // WG replica detection: look for `.ac-new/wg-N-team/__agent_X` pattern
    if let Some(ac_idx) = parts.iter().position(|p| *p == ".ac-new") {
        if ac_idx > 0 && ac_idx + 2 < parts.len() {
            let project = parts[ac_idx - 1];
            let wg = parts[ac_idx + 1];
            let agent_dir = parts[ac_idx + 2];
            if wg.starts_with("wg-") && agent_dir.starts_with("__agent_") {
                let agent = agent_dir.strip_prefix("__agent_").unwrap_or(agent_dir);
                return format!("{}:{}/{}", project, wg, agent);
            }
        }
    }

    // Fall back to existing 2-component derivation (origin matrix agents).
    agent_name_from_path(path)
}
```

Public re-exports: both helpers should be `pub` and referenced from `cli/send.rs`, `cli/list_peers.rs`, `phone/mailbox.rs`. Kill the three shadow copies of `agent_name_from_path` at `cli/list_peers.rs:52`, `cli/send.rs:76` (`agent_name_from_root`), and `phone/mailbox.rs:1306` — all should call into `config::teams` canonical helpers. Exception: `cli/send.rs`'s helper already delegates cleanly; just route it through the canonical `agent_fqn_from_path`.

---

## 4. Routing fixes (Bug B)

### 4.1 `MailboxPoller::find_active_session` — `src-tauri/src/phone/mailbox.rs:898-965`

**Current (buggy):**
```rust
let mut matches: Vec<_> = sessions.iter()
    .filter(|s| {
        let normalized = s.working_directory.replace('\\', "/");
        self.agent_name_from_path(&s.working_directory) == agent_name
            || normalized.ends_with(agent_name)
            || normalized.contains(&format!("/{}", agent_name))
    })
    .collect();
```

**Replace with (exact-match against FQN; legacy 2-component target tolerated via normalization):**
```rust
// Resolve target to a FQN. If caller passed an unqualified WG name,
// we accept it here only when the resolution narrows to one candidate
// session (the routing equivalent of Decision 2).
let target = crate::config::teams::split_project_prefix(agent_name);

let mut matches: Vec<_> = sessions.iter()
    .filter(|s| {
        let session_fqn = crate::config::teams::agent_fqn_from_path(&s.working_directory);
        match target {
            // Fully qualified target → exact match only.
            (Some(_), _) => session_fqn == agent_name,
            // Unqualified target → match sessions whose LOCAL part matches.
            (None, local) => {
                let (_, session_local) = crate::config::teams::split_project_prefix(&session_fqn);
                session_local == local
            }
        }
    })
    .collect();
```

**Reject ambiguity at the caller** (see §4.4): `deliver_wake` is not the right place to reject (it already picks a "best" match deterministically by status/temp); but `close-session` and CLI-side resolution MUST reject ambiguity. `find_active_session` remains a "best effort" chooser for wake — what we fix is the set it chooses FROM: previously it mixed cross-project candidates; now the filter is project-aware, so the set contains only correct-project candidates when the target is qualified.

> **architect round 2 (→ §AR2-G2):** "`deliver_wake` is not the right place to reject" was wrong. Grinch §G2 shows the silent-pick bug re-materializes whenever the target is unqualified (legacy in-flight message, buggy/old client, direct outbox write). The fix lifts target resolution to `process_message` so `find_active_session` receives an ALREADY-QUALIFIED target; its filter then collapses to `session_fqn == agent_name`. Keep the "best chooser" sort for the within-project case (multiple sessions for the same FQN — e.g. Active + Idle). See §AR2-G2 for the full design.

### 4.2 `MailboxPoller::find_all_sessions` — `src-tauri/src/phone/mailbox.rs:969-984`

Apply the same filter swap as §4.1. `close-session` authorization at `phone/mailbox.rs:1013` compares `msg.from`, `target`, and team coordinator relationships — all via string equality through `is_coordinator_of`, so after Decision 1 + §5, this call works correctly when both sides are FQNs.

> **architect round 2 (→ §AR2-G1):** grinch §G1 proves the close-session path still leaks cross-project when `msg.target` is unqualified (lenient `is_in_team` × `find_all_sessions` local-match × any-team `is_coordinator_of`). The fix: `handle_close_session` resolves `msg.target` to FQN via the shared resolver at its entry point, **reject-on-ambiguity**, before calling `is_coordinator_of` or `find_all_sessions`. §DR1's CLI-side fix becomes belt-and-braces, not the authoritative gate. See §AR2-G1.

### 4.3 `MailboxPoller::resolve_repo_path` — `src-tauri/src/phone/mailbox.rs:1230-1303`

**Current (three substring match loops at 1236-1244, 1249-1257, 1261-1272):**
```rust
if self.agent_name_from_path(cwd) == agent_name
    || normalized.ends_with(agent_name)
    || normalized.contains(&format!("/{}", agent_name))
```

**Replace each loop with:**
```rust
let path_fqn = crate::config::teams::agent_fqn_from_path(cwd);
let matches = match crate::config::teams::split_project_prefix(agent_name) {
    (Some(_), _) => path_fqn == agent_name,
    (None, local) => {
        let (_, path_local) = crate::config::teams::split_project_prefix(&path_fqn);
        path_local == local
    }
};
if matches { return Some(cwd.clone()); }
```

The WG replica fallback at `mailbox.rs:1274-1299` (the `if agent_name.starts_with("wg-")` branch) must be generalized to handle both `wg-N-team/agent` and `project:wg-N-team/agent`. Peel the project via `split_project_prefix` first; within the loop, when iterating `project_paths`, if the target has a project, short-circuit to only that project's base.

```rust
let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
if local.starts_with("wg-") {
    if let Some((wg_name, agent_short)) = local.split_once('/') {
        let replica_dir = format!("__agent_{}", agent_short);
        for rp in &cfg.project_paths {
            let base = std::path::Path::new(rp);
            if !base.is_dir() { continue; }

            // When a project is specified, only explore that project's subtree.
            let mut dirs_to_check = vec![base.to_path_buf()];
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if !p.is_dir() { continue; }
                    let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if let Some(want) = target_project {
                        if dir_name != want { continue; }
                    }
                    dirs_to_check.push(p);
                }
            } else if let Some(want) = target_project {
                if base.file_name().and_then(|n| n.to_str()) != Some(want) { continue; }
            }

            for dir in dirs_to_check {
                let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
                if candidate.is_dir() {
                    return Some(candidate.to_string_lossy().to_string());
                }
            }
        }
    }
}
```

> **architect round 2 (→ §AR2-G3, §AR2-G4):** two bugs in the round-1 §4.3 code.
> - **§G3 seeding bug.** `let mut dirs_to_check = vec![base.to_path_buf()];` is unconditional. When `target_project = Some("proj-a")` and `rp` points at `C:/repos/proj-b`, the project-b root stays seeded and gets matched. Fix: condition the seed on `base.file_name() == want` when a project is specified.
> - **§G4 multi-match arbitrary pick.** All three loops `return Some(...)` on first match. Across projects with colliding local parts, iteration order determines the "winner" — silent misroute. Fix: when target is unqualified, collect candidates; return `None` on multi-match so the caller surfaces ambiguity via the shared resolver.
>
> Full replacement code lives in §AR2-G3.

### 4.4 `MailboxPoller::resolve_wg_path_from_sessions` — `src-tauri/src/phone/mailbox.rs:1405-1441`

Current uses `agent_name.split_once('/')` directly, so the target must be a local form. Change to peel project first:
```rust
let (_target_project, local) = crate::config::teams::split_project_prefix(agent_name);
let (wg_name, agent_short) = local.split_once('/')?;
if !wg_name.starts_with("wg-") { return None; }
```
If `target_project` is `Some`, add a final filter: only return a candidate whose derived path has that project component. If `None`, current behavior (pick any sibling WG) is fine.

### 4.5 Anti-spoof check — `src-tauri/src/phone/mailbox.rs:256-257`

```rust
let expected_from = self.agent_name_from_path(&repo_path.to_string_lossy());
if expected_from != msg.from { ... reject ... }
```

Swap `agent_name_from_path` → `crate::config::teams::agent_fqn_from_path`. For a WG replica outbox, `repo_path` resolves through `outbox_dir.parent().parent()` → the agent's replica root — `agent_fqn_from_path` will derive `project:wg-N-team/agent`. Legacy `msg.from` (unqualified) must be accepted when its local part matches the local part of `expected_from` (same lenient rule).

> **architect round 2 (→ §DR5, §AR2-norm):** the explicit code contract for local-only fallback lives in §DR5. Round 2 also **canonicalizes `msg.from` in-place after anti-spoof**: if `msg.from` was unqualified and passed via local-match, rewrite `msg.from = expected_from` before any downstream call. This closes grinch §G5 (response-dir lookup using `resolve_repo_path(&msg.from)`). See §AR2-norm.

### 4.6 Token-session anti-spoof — `src-tauri/src/phone/mailbox.rs:323-339`

Same swap: `self.agent_name_from_path(&session.working_directory)` → `agent_fqn_from_path`. Comparison tolerant of legacy `msg.from` via the same local-part fallback.

---

## 5. Team membership fixes (Bug A)

### 5.1 `extract_wg_team` — `src-tauri/src/config/teams.rs:105-113`

**Before:**
```rust
fn extract_wg_team(agent_name: &str) -> Option<&str> {
    let prefix = agent_name.split('/').next()?;
    if !prefix.starts_with("wg-") { return None; }
    prefix.strip_prefix("wg-").and_then(|s| s.split_once('-').map(|(_, team)| team))
}
```

**After:**
```rust
fn extract_wg_team(agent_name: &str) -> Option<&str> {
    // Peel optional `<project>:` prefix; operate on the local part.
    let (_, local) = split_project_prefix(agent_name);
    let prefix = local.split('/').next()?;
    if !prefix.starts_with("wg-") { return None; }
    prefix.strip_prefix("wg-").and_then(|s| s.split_once('-').map(|(_, team)| team))
}
```

### 5.2 `agent_suffix` — `src-tauri/src/config/teams.rs:116-118`

Safe as-is (works on the local part after split_once). No change.

### 5.3 `is_in_team` WG-aware branch — `src-tauri/src/config/teams.rs:153-168`

**Before:**
```rust
if let Some(wg_team) = extract_wg_team(agent_name) {
    if wg_team == team.name { ... suffix match ... }
}
```

**After (project-aware):**
```rust
if let Some(wg_team) = extract_wg_team(agent_name) {
    let (agent_project, _) = split_project_prefix(agent_name);
    let project_matches = match agent_project {
        Some(p) => p == team.project,
        None => true, // legacy unqualified name — tolerate (see Decision 3).
                       // A tighter mode could `false` here; see §9.
    };
    if wg_team == team.name && project_matches {
        let suffix = agent_suffix(agent_name);
        ...
    }
}
```

### 5.4 `is_coordinator` WG-aware branch — `src-tauri/src/config/teams.rs:182-186`

Same project-match guard as §5.3.

> **architect round 2 (→ §DR8, §G13, §AR2-strict):** round 1 left the `None => true` / `None => false` dial open. Round 2 **locks in `None => false` for `is_coordinator` (strict)** per dev-rust §DR8 and grinch §G13. Rationale: `is_coordinator` is the authorization gate for destructive operations; lenient tolerance here is a privilege-escalation vector (grinch §G13 trace). `is_in_team` and `can_communicate` remain lenient per §DR8 — those are reachability/display paths where legacy tolerance is the stated intent of Decision 3. Defense-in-depth alongside the mailbox-side target resolution in §AR2-G1.

### 5.5 `can_communicate` same-WG rule (rule 2) — `src-tauri/src/config/teams.rs:225-232`

**Before:**
```rust
if from.starts_with("wg-") && to.starts_with("wg-") {
    let from_wg = from.split('/').next().unwrap_or("");
    let to_wg = to.split('/').next().unwrap_or("");
    if !from_wg.is_empty() && from_wg == to_wg { return true; }
}
```

**After (peel project, require same project):**
```rust
let (from_proj, from_local) = split_project_prefix(from);
let (to_proj, to_local) = split_project_prefix(to);
if from_local.starts_with("wg-") && to_local.starts_with("wg-") {
    let from_wg = from_local.split('/').next().unwrap_or("");
    let to_wg = to_local.split('/').next().unwrap_or("");
    // Projects must be the same, or at least one side unqualified (legacy tolerance).
    let project_match = match (from_proj, to_proj) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    if !from_wg.is_empty() && from_wg == to_wg && project_match { return true; }
}
```

### 5.6 `list_peers::execute_wg_discovery` — `src-tauri/src/cli/list_peers.rs:311-389`

- Line 314: `my_full_name = format!("{}/{}", wg.my_wg_name, wg.my_agent_name)` → include project prefix. Resolve `project` via walking `wg.ac_new_dir.parent()` (the project folder containing `.ac-new/`).
- Line 345, 368: `peer_full_name` / `peer_name` construction — same prefix.
- `build_wg_peer` at `list_peers.rs:280-308`: `PeerInfo.name` now uses the FQN.

### 5.7 `list_peers::execute` (non-WG caller path) — `src-tauri/src/cli/list_peers.rs:391-590`

- Line 413: `my_name = agent_name_from_path(&root)` — origin agents already return `project/agent`, no change needed.
- Lines 555-559 WG-replica discovery: `peer_name = format!("{}/{}", wg_name, agent_short)` → `format!("{}:{}/{}", project_folder_here, wg_name, agent_short)` where `project_folder_here` is the dir name holding this `.ac-new/` (available via `repo_dir.file_name()` at line 517).

### 5.8 Delete shadow `agent_name_from_path` in `list_peers.rs` — `cli/list_peers.rs:52-62`

Redirect all call sites to `crate::config::teams::agent_fqn_from_path` (for WG-aware use) or `crate::config::teams::agent_name_from_path` (for origin legacy use). Delete the local shadow to enforce single source of truth.

### 5.9 `list_agents` in `phone/manager.rs` — `src-tauri/src/phone/manager.rs:12-36`

No change; `team.agent_names` contains origin-form names already project-scoped by `resolve_agent_ref`.

---

## 6. Transport / CLI fixes

### 6.1 `cli/send.rs` sender derivation — `src-tauri/src/cli/send.rs:76-86, 105`

Replace `agent_name_from_root` with a delegating wrapper:
```rust
pub(crate) fn agent_name_from_root(root: &str) -> String {
    crate::config::teams::agent_fqn_from_path(root)
}
```
Sender FQN is now always project-scoped when the caller runs from a WG replica CWD. Origin agents unchanged.

### 6.2 `cli/send.rs` `--to` lenient resolution — insert before `can_communicate` at `src-tauri/src/cli/send.rs:121-133`

Add after line 105 (after `let sender = agent_name_from_root(&root);`):

```rust
// Lenient --to resolution (Decision 2). Qualifies unqualified WG targets when unambiguous.
let resolved_to = match crate::cli::send::resolve_to_target(&args.to, &settings_project_paths()) {
    Ok(fqn) => fqn,
    Err(e) => { eprintln!("Error: {}", e); return 1; }
};
// Use resolved_to downstream in place of args.to.
```

Implement `resolve_to_target` in `cli/send.rs`:
- If target contains `:` → return as-is.
- If target matches `wg-N-team/agent` shape → enumerate candidates across projects, reject on ambiguity with the listing message from Decision 2 (2c).
- Else → return as-is (origin-form or bare — delegate to legacy).

`settings_project_paths()` — pull `settings::load_settings().project_paths` and also include `settings.root_token`'s CWDs if applicable. Helper lives in `cli/send.rs` as a small free function.

> **architect round 2 (→ §AR2-shared, §AR2-G8):** round 2 **lifts `resolve_to_target` out of `cli/send.rs`** into `config::teams::resolve_agent_target` so both CLI (`send`, `close_session`) and mailbox (`process_message`) call into the same function — single source of truth for the reject-on-ambiguity rule. Error enum `ResolutionError` with four variants (`InvalidShape`, `UnknownQualified`, `NoMatch`, `Ambiguous`) differentiates the failure modes per grinch §G8. The two-level `project_paths`-plus-children scan mirrors `discover_teams_in_project` per §DR4. Full signature in §AR2-shared.

### 6.3 `cli/send.rs` PTY_SAFE_MAX clamp sanity — `src-tauri/src/cli/send.rs:182-194`

Clamp computation unchanged; `sender.len()` is now ~30 bytes longer. Log-warn at `body.len() > 150` (tightened from current 200) to catch pathological project/wg names early. Lift the error threshold's user-facing message to mention `<project>:` as a possible overflow source.

### 6.4 `cli/close_session.rs` — `src-tauri/src/cli/close_session.rs:99-100`

`sender_agent` and `preferred_agent` are unrelated to identity (they are `lastCodingAgent` IDs); no change. But `msg.from` derivation must use `agent_fqn_from_path` (same as §6.1).

### 6.5 `cli/list_peers.rs` — covered in §5.6–5.8.

### 6.6 `cli/create_agent.rs` session-request flow

Audit `cli/create_agent.rs` for any place that writes an agent-name-like string to `~/.agentscommander/session-requests/*.json`. Session-request's `session_name` is `{wg}/{agent}` per the DiscoveryBranchWatcher pattern in `commands/ac_discovery.rs:312`. Extending to `{project}:{wg}/{agent}` means the sidebar's session-name lookups, the SessionManager's name-keyed operations, and the user-visible session item label all shift. List of call sites to check is in §7; owner of the concrete change is dev-rust.

---

## 7. Frontend impact

> **architect round 2 (→ §DR6, §G13):** §7.1 through §7.5 are **REMOVED**. Dev-rust §DR6 and grinch §G13 independently verified that the cited `ProjectPanel.tsx` lines consume `AcAgentReplica.name` (short dir name from `ac_discovery`), **not** `PeerInfo.name` from `list-peers`. There is no frontend consumer of `list-peers` JSON today — a repo-wide grep for `listPeers` / `PeerInfo` returned zero hits. The sub-sections below are preserved for the audit trail only; **do not implement them**. The sole surviving frontend-visible concern — `:` in spawned `Session.name` — is handled in the backend at `mailbox.rs:508` per §AR2-session-name.

Minimal but non-zero. The frontend renders whatever `list-peers` and Tauri commands return — the strings just get longer with `project:` prefix.

### 7.1 `src/sidebar/components/ProjectPanel.tsx:491`

```tsx
title={`${wg.name}/${peer.name}`}
```

After fix, `peer.name` returned by `list-peers` is already FQN (`project:wg-N-team/agent`). The `${wg.name}/${peer.name}` template concatenates `wg.name` with an already-qualified name, producing junk. Change to:

```tsx
title={peer.name}
```

### 7.2 `src/sidebar/components/ProjectPanel.tsx:493`

```tsx
{peer.name} RUNNING
```
`peer.name` is now `project:wg-N-team/agent` — long. Consider rendering the local part only for badge text, full FQN for the tooltip:
```tsx
title={peer.name}
{peer.name.includes(':') ? peer.name.split(':')[1] : peer.name} RUNNING
```
Dev-rust: confirm with UX (very narrow badge space). Safe to defer as a follow-up if needed — no functional break.

### 7.3 `src/sidebar/components/ProjectPanel.tsx:674`

```tsx
if (peer.name === item.replica.name) return false;
```

`peer.name` (from `list-peers`) is a FQN; `item.replica.name` is the short form (`agent` from `AcAgentReplica.name` at `ac_discovery.rs:72`). These will stop matching. Adjust to compare local parts:

```tsx
const peerLocal = peer.name.includes(':') ? peer.name.split('/').pop() : peer.name.split('/').pop();
if (peerLocal === item.replica.name) return false;
```

Or derive a stable comparison on the backend (preferred, if dev-rust wants to add a `short_name` field to `PeerInfo`). Architect leaves the choice to dev-rust.

### 7.4 Other peer-name display sites

Grep remaining for `peer.name` and `replica.name` before merging. Ripgrep pattern: `peer\.name|\bname:.*FQN`.

### 7.5 `src/shared/types.ts`

No schema change required — all identifiers are `string`. Bump a comment on the `name` field noting the new FQN rule so future devs don't re-hash this. **Do not** introduce a `Project` type — kept strings to avoid frontend re-plumbing.

---

## 8. Tests

> **architect round 2 (→ §AR2-tests):** round 1 listed 7 tests; dev-rust §DR7 added 5; grinch §G9 added 10. Round 2 deduplicates to **22 tests total** across unit/integration layers. See §AR2-tests for the canonical deduplicated list with ownership (per-module) and critical regression markers.

### 8.1 `config/teams.rs` unit tests

Add beside the existing `is_coordinator_for_cwd_matches_wg_replica` test at `teams.rs:252-272`:

- `is_in_team_rejects_cross_project_wg_match` — two `DiscoveredTeam` values with identical `name = "dev-team"` but distinct `project`, verify an agent in project A does NOT match team in project B via the WG branch.
- `can_communicate_rejects_cross_project_same_wg` — `proj-a:wg-1-devs/alice` and `proj-b:wg-1-devs/bob` → false.
- `can_communicate_allows_legacy_unqualified` — unqualified both sides, same wg → true (legacy tolerance).
- `agent_fqn_from_path_wg_replica` — CWD under `.ac-new/wg-N-team/__agent_alice` → `project:wg-N-team/alice`.
- `agent_fqn_from_path_origin` — CWD at `{project}/{agent}` → `project/agent` (unchanged shape).
- `split_project_prefix_present` / `split_project_prefix_absent`.
- `extract_wg_team_peels_project_prefix`.

### 8.2 `phone/messaging.rs`

`validate_filename_shape` accepts current shape unchanged — no new test required. Add a regression test that ensures an old short-form filename (`wgN-agent-to-wgN-agent-slug.md`) still round-trips: it does, because the filename budget decision keeps the shape unchanged.

### 8.3 `phone/mailbox.rs`

- Extend existing `wake_action_*` tests with a `find_active_session_exact_fqn` test (will need a small helper to mock sessions). Alternatively, keep this an integration test at the CLI boundary: two projects, same WG/agent name, send with `--to project-a:wg-1-devs/tech-lead`, assert the message lands in project A's session inbox only.
- `resolve_repo_path_project_scoped` — same two-project fixture, verify the returned path contains the correct project folder.

### 8.4 `cli/send.rs` resolution tests

- `resolve_to_target_passes_through_qualified` — `proj:wg-1-devs/x` → unchanged.
- `resolve_to_target_qualifies_unambiguous_unqualified` — one project has `wg-1-devs/x`, target `wg-1-devs/x` → returns `proj:wg-1-devs/x`.
- `resolve_to_target_rejects_ambiguous` — two projects both have `wg-1-devs/x` → error listing both candidates.
- `resolve_to_target_rejects_unknown` — zero candidates → error.

### 8.5 `cli/list_peers.rs`

- New test fixture with two projects and colliding team names — assert `list-peers` from a replica in project A does NOT return project B's replicas as teammates (currently does; this is Bug A's primary symptom).

### 8.6 Frontend

No automated test required (no existing frontend test harness touches this code). Manual smoke: create two projects both with `wg-1-devs/tech-lead`, open sidebar in each, verify running-peer badges list only the correct project's peers.

---

## 9. Edge cases and constraints

- **Legacy tolerance in `is_in_team` (§5.3):** I chose "unqualified agent name matches ANY project's team with matching name". This is deliberate — it keeps legacy outbox/conversations/session messages routable during the transition period. A stricter mode (unqualified = never match) is implementable by flipping `None => true` to `None => false` in §5.3 and §5.5. I do not recommend that switch in this PR; leave it as a follow-up toggle if operational experience shows legacy tolerance masks real bugs.
- **Windows `MAX_PATH`.** Filename shape unchanged (Decision 4); abs_path stays within current envelope (~240 chars). No Windows 260-limit exposure.
- **Project folder canonicalization.** `DiscoveredTeam.project` uses the dir name of the project root (the folder containing `.ac-new/`). Different `project_paths` pointing at the same physical folder via different casings are converged upstream by `DiscoveryBranchWatcher::canonical_key` at `ac_discovery.rs:276-283`. Teams discovery (`config/teams.rs:309-312`) iterates those paths directly — if two entries differ only by trailing slash, we get duplicate `DiscoveredTeam` entries with the same `(project, name)`. This is pre-existing and out of scope.
- **Colon in project folder name.** Unlikely on any filesystem (reserved on Windows). If it happened, `split_project_prefix` would mis-peel. Guard: in `discover_teams_in_project`, log a warn and skip projects whose folder name contains `:`. Low-cost defense.
- **Sidebar drag/drop and session renaming.** Session `name` field is human-readable and displayed in the sidebar; the change from `wg-N-team/agent` → `project:wg-N-team/agent` makes the session list visually longer. If this is a UX regression, dev-rust can render the `project:` prefix dim or truncate it. Not in-scope for this fix; flag for follow-up if the sidebar team complains.
- **Telegram bridge.** `telegram/` module is unaudited here. If it consumes `msg.from` or `msg.to` for display, it will transparently start showing FQN. No schema change.
- **`send --to` in CLAUDE.md docs and `_plans/*.md`.** Do NOT rewrite these in this PR. Decision 2's lenient resolution makes them continue to work. Post-merge, an editorial pass can optionally add project prefixes; that's a separate chore.

---

## 10. Dependencies

- **No new crates.** Entire fix uses existing `std`, `serde`, `thiserror`, `chrono`, `uuid`, `tokio`.
- **No schema version bump** needed on `OutboxMessage` — string fields accept either form.
- **Version bump:** `tauri.conf.json`, `Cargo.toml`, `Titlebar.tsx` APP_VERSION → 0.7.6. Per CLAUDE.md versioning rule.

---

## 11. Implementation order (suggested to dev-rust, non-binding)

> **architect round 2 (→ §AR2-order):** round 1's 11 steps were pruned to 10 by §DR9 (dropped step 6 frontend). Round 2 inserts a **new step 4** that lifts `resolve_agent_target` into `config::teams` before any CLI or mailbox site uses it, because it's now a shared dependency. Updated 12-step order in §AR2-order.

1. Add `split_project_prefix`, `agent_fqn_from_path` in `config/teams.rs` + unit tests (§3.2, §8.1).
2. Add `project` field to `DiscoveredTeam` + populate in `discover_teams_in_project` (§3.1). Compile; fix breakage.
3. Update `extract_wg_team`, `is_in_team`, `is_coordinator`, `can_communicate` (§5.1-5.5) with project-aware guards + tests.
4. Update `cli/send.rs` + `cli/list_peers.rs` + `cli/close_session.rs` (§5.6-5.8, §6.1-6.4). Delete shadow `agent_name_from_path`.
5. Update `phone/mailbox.rs` routing helpers (§4.1-4.6).
6. Frontend nit fixes (§7).
7. Integration smoke: two colliding projects, confirm routing and list-peers are project-scoped.
8. Version bump + commit.

---

## 12. Non-goals / things NOT to do in this PR

- Do NOT project-scope origin agents (keep `project/agent` as-is — Decision 1).
- Do NOT migrate existing on-disk files (tolerate-on-read, Decision 3).
- Do NOT change filename shape (project is implicit in messaging dir location, Decision 4).
- Do NOT add a `Project` type to `src/shared/types.ts` — keep identifiers as strings.
- Do NOT refactor the shadow `agent_name_from_path` copies beyond redirecting them; leave wider module cleanup to a follow-up.
- Do NOT touch #69 surfaces — this PR is forward-compatible with #69's `is_coordinator` boolean.
- Do NOT raise `PTY_SAFE_MAX` — measurements show it's not the binding constraint.

---

## Dev-rust review (round 1)

**Reviewer:** wg-5-dev-team/dev-rust
**Date:** 2026-04-22
**Scope:** plan enrichment, no code edits.

### Summary

The plan is coherent and the decisions are well-reasoned. Line numbers verified against HEAD of `fix/issue-70-project-scoped-names` — all accurate within ±2 lines. The two material gaps I found are:

1. **Close-session privilege-escalation window** — the target-side of `close-session` can still leak cross-project under the lenient `is_in_team` rule. Needs a CLI-side `resolve_to_target` on the target BEFORE the message hits the wire. (See §DR1 below.)
2. **Frontend §7 is wrong about the source of `peer.name`** — the lines cited (491/493/674) render `AcAgentReplica.name` from `ac_discovery`, NOT `PeerInfo.name` from `list-peers`. No frontend consumer of `list-peers` exists today. (See §DR6.)

Two call sites are missing from the plan (§DR2). The `DiscoveredTeam` constructor in the existing test at `teams.rs:252-272` also needs the new `project` field or the crate will not compile. Coverage gaps noted in §DR7.

### §DR1 — Close-session cross-project leak (critical)

Plan §4.2 asserts the close-session path works once both sides are FQNs. Correct — but it does not address the case where `msg.target` is an unqualified legacy name. The path then becomes:

1. Sender FQN `proj-a:wg-1-devs/tech-lead` passes `is_coordinator(from, team_a)` — strict, OK.
2. Target unqualified `wg-1-devs/tech-lead` passes `is_in_team(target, team_a)` via the §5.3 `None => true` tolerance — accepted.
3. The same unqualified target ALSO satisfies `is_in_team(target, team_b)` if `proj-b` has a team of the same name — nothing rules this out, but `is_coordinator_of` only needs ONE team to match, so authorization succeeds.
4. `find_all_sessions(app, target)` at `mailbox.rs:1028` runs with the unqualified target. Per §4.2's filter, the unqualified branch matches sessions whose LOCAL part equals `wg-1-devs/tech-lead` — **across all projects**.
5. Coordinator of project A closes sessions in project B. Unauthorized.

**Why this matters:** `is_in_team` tolerance is defensible for display/routing tolerance. It is NOT defensible when paired with `find_all_sessions`'s unqualified-local matcher on a destructive operation.

**Required fix — CLI-side resolution:**

In `cli/close_session.rs` (insertion point: after line 48, before the authorization check at line 75), apply `resolve_to_target` exactly as send does:

```rust
let resolved_target = match crate::cli::send::resolve_to_target(&args.target, ...) {
    Ok(fqn) => fqn,
    Err(e) => { eprintln!("Error: {}", e); return 1; }
};
// Use resolved_target in place of args.target for both auth check and message.target.
```

Rationale: target on the wire is always FQN post-resolution. Mailbox's `handle_close_session` never sees an unqualified target. The lenient `is_in_team` rule stays intact for the normal send path where it's needed, and close-session becomes strict by construction.

**Defense-in-depth (optional):** additionally make `is_coordinator` §5.4 strict (`None => false`). Narrow cost — legacy in-flight close-session messages from pre-upgrade agents get rejected, and the sender retries. Cheap for a coordinator-auth path.

### §DR2 — Missing call sites

The plan's §11 order does not include these sites that consume `agent_name_from_path` on a WG-replica CWD:

1. **`src-tauri/src/lib.rs:540`** — `start_only_coordinators` session-restore path calls `agent_name_from_path(&ps.working_directory)` then feeds it into `is_in_team` and `is_any_coordinator`. Without the change, restored coordinator flags leak cross-project via the lenient tolerance. **Fix:** switch this single call to `agent_fqn_from_path`.

2. **`src-tauri/src/config/teams.rs:205-208` — `is_coordinator_for_cwd`** — the shared wrapper used by `session/manager.rs:313` (`refresh_coordinator_flags`), `commands/entity_creation.rs:1133` (`emit_coordinator_refresh`), `commands/ac_discovery.rs:853` (discovery refresh), and `commands/session.rs:291` (session creation). Internally calls `agent_name_from_path(working_directory)`. **Fix:** swap the single line inside the helper to `agent_fqn_from_path(working_directory)`. All four downstream call sites then inherit project-precision with zero additional edits.

Doing these two one-line changes closes the UI coordinator-flag leak symmetrically with the mailbox routing fixes.

### §DR3 — Pre-existing test breakage (compile-blocker)

`teams.rs:252-272` constructs a `DiscoveredTeam` literal:

```rust
let teams = vec![DiscoveredTeam {
    name: "dev-team".into(),
    agent_names: vec![...],
    agent_paths: vec![...],
    coordinator_name: Some(...),
    coordinator_path: None,
}];
```

Adding `project: String` to the struct per §3.1 makes this (and the `is_coordinator_for_cwd_empty_teams` test that builds an empty `Vec<DiscoveredTeam>`, which is fine) fail to compile. Add `project: "foo".into()` here and in any equivalent test fixture. Plan §11 step 2 ("Compile; fix breakage") implicitly covers this, but call it out since the existing test is a regression anchor and should be updated, not deleted.

### §DR4 — Lenient `resolve_to_target` must mirror discovery's two-level scan

`discover_teams` at `teams.rs:285-315` scans `settings.project_paths` AND their immediate children (lines 296-306) — a parent-path entry like `C:/repos/` yields both `repo-a` and `repo-b` as projects. The plan's §6.2 `resolve_to_target` says "enumerate WG replicas across `settings.project_paths` matching this local part". Without mirroring the two-level scan, a user with `project_paths = ["C:/repos/"]` (parent-only) would see `resolve_to_target` enumerate zero candidates, fail with "unknown" or erroneously resolve to the WG one level deeper. **Fix:** the resolver enumerates the same way as `discover_teams_in_project` — check the base and its immediate non-dot children.

### §DR5 — Anti-spoof lenient-fallback explicit contract

Plan §4.5 says "Legacy `msg.from` (unqualified) must be accepted when its local part matches the local part of `expected_from` (same lenient rule)." Worth being explicit in code that the fallback compares LOCAL parts via `split_project_prefix` — not a suffix match. Proposed logic:

```rust
let expected_from = agent_fqn_from_path(&repo_path.to_string_lossy());
let accept = if expected_from == msg.from {
    true
} else {
    let (_, exp_local) = split_project_prefix(&expected_from);
    let (msg_proj, msg_local) = split_project_prefix(&msg.from);
    // Legacy: msg.from unqualified, local parts must match.
    msg_proj.is_none() && exp_local == msg_local
};
if !accept { reject }
```

Rationale: prevents a legacy attacker from crafting `msg.from = "different:wg-1-devs/tech-lead"` (qualified, but wrong project) and slipping past a naïve suffix match. A local-only fallback is the narrowest tolerance possible.

Same treatment applies to the token-session anti-spoof at `mailbox.rs:323-339` per §4.6.

### §DR6 — Frontend §7 misidentifies the source of `peer.name`

Verified at `ProjectPanel.tsx:400`:

```tsx
runningPeers?: () => AcAgentReplica[]
```

The `peer` rendered at lines 488-494 is `AcAgentReplica`, NOT `PeerInfo`. `AcAgentReplica.name` is the short agent dir name (e.g. `"tech-lead"`), populated in `ac_discovery.rs:550-558` from the stripped `__agent_<name>` dir name — unchanged by this PR.

Line-by-line audit:

- **Line 491** `title={`${wg.name}/${peer.name}`}` — source `peer` is `AcAgentReplica`. `wg.name` is the WG dir name, `peer.name` is the short agent name → title renders `wg-1-devs/tech-lead`. No concatenation bug. **No change needed.**
- **Line 493** `{peer.name} RUNNING` — renders the short agent name. Badge stays readable. **No change needed.**
- **Line 674** `if (peer.name === item.replica.name) return false;` — both sides are `AcAgentReplica.name` (short), so the equality still matches. **No change needed.**

**I also grepped for any frontend consumer of the `list-peers` JSON output — none exists.** Frontend discovery goes exclusively through the `ac_discovery` commands.

**Conclusion:** §7.1, §7.2, §7.3, §7.4 changes are all **unnecessary and should be removed from the plan**. The backend `PeerInfo.name` shape change (FQN in list-peers) is invisible to the UI today. If a frontend consumer is ever added, it can peel `:` or receive a `short_name` on `PeerInfo` at that point. No need to pre-add fields for hypothetical consumers.

**One genuine frontend-adjacent concern survives:** `mailbox.rs:508` sets `session_name = msg.to.clone()` when `deliver_wake` spawns a persistent session. If `msg.to` is `proj-a:wg-1-devs/tech-lead`, the resulting `Session.name` contains `:` and renders in the sidebar with the long FQN. Minor UX regression in a rarely-hit path (persistent-session spawn via wake). Suggested mitigation: strip the `<project>:` prefix from the display name at spawn time — `let session_name = split_project_prefix(&msg.to).1.to_string()`. The canonical FQN is still the agent name recorded in `teams::agent_fqn_from_path(&cwd)`; the sidebar label just gets the local form.

### §DR7 — Test coverage gaps

Plan §8 lists 7 tests. Add:

1. **`close_session_strict_target_rejects_cross_project`** — regression test for §DR1. Fixture: two projects with colliding team names. Coordinator of A invokes close-session with unqualified target. Without the CLI-side `resolve_to_target` call, asserts rejection or at minimum ambiguity error. Without this test the bug re-appears silently on any future refactor.
2. **`is_coordinator_for_cwd_project_qualified`** — directly test the updated `is_coordinator_for_cwd`. Two projects, same team name, coord in A's replica CWD — true for team A, false for team B.
3. **`anti_spoof_legacy_msg_from_accepted_by_local_match`** — positive test for §DR5 lenient fallback: `msg.from = "wg-1-devs/tech-lead"` (unqualified), `expected_from = "proj-a:wg-1-devs/tech-lead"` (FQN) → accepted.
4. **`anti_spoof_cross_project_qualified_msg_from_rejected`** — negative test for §DR5: `msg.from = "proj-b:wg-1-devs/tech-lead"`, `expected_from = "proj-a:wg-1-devs/tech-lead"` → rejected (both qualified, projects differ).
5. **`resolve_to_target_two_level_scan`** — §DR4 regression: `settings.project_paths = ["C:/repos/"]` (parent), contains `proj-a` and `proj-b` siblings both with `wg-1-devs/tech-lead`. `resolve_to_target("wg-1-devs/tech-lead")` must see BOTH candidates and raise ambiguity, not resolve to one arbitrarily.

Total: 7 (plan) + 5 (additions) = 12 tests. All unit-level, no integration harness needed.

### §DR8 — Decision calls on open points

**§9 tolerance dial (open point 1).**

**My call:** Keep `None => true` lenient in `is_in_team` (§5.3) and `can_communicate` (§5.5). **Tighten `is_coordinator` (§5.4) to `None => false`** as defense-in-depth alongside §DR1's CLI-side `resolve_to_target`.

**Reasoning:**
- `is_in_team` lenient is a display/reachability convenience. Tightening here would invalidate every legacy outbox, conversation, and in-flight message at upgrade time. High cost, low benefit — the real routing correctness comes from the FQN-on-wire path, not from rejecting unqualified legacy reads.
- `can_communicate` lenient is symmetric with `is_in_team` for the send path. Consistent semantics.
- `is_coordinator` is the authorization gate for destructive operations (`close-session`). Lenient tolerance here is a privilege escalation vector. §DR1's CLI-side fix handles the common path; strict `is_coordinator` is the belt-and-braces line of defense. Cost: pre-upgrade in-flight close-session messages get rejected → sender retries. Acceptable.
- The plan's "Or is it a safe transition aid that can be tightened later once all disk state has rotated?" framing is correct for §5.3/§5.5. It is NOT correct for §5.4 — the tightening should happen now because the risk is disproportionately larger there.

**§7.2-7.3 frontend badge choice (open point 2).**

**My call:** **Do nothing to the frontend in this PR.** The plan's §7.1-§7.4 changes are based on a wrong type identification (see §DR6). No change is needed, no `short_name` field on `PeerInfo` is needed.

**Reasoning:**
- `peer.name` at the cited lines is `AcAgentReplica.name` (short), not `PeerInfo.name` (FQN). The flow goes through `ac_discovery`, which is unchanged by this PR.
- No frontend consumer of `list-peers` JSON exists. Adding `short_name` to `PeerInfo` is premature abstraction — it can be added at the moment a consumer appears.
- The only real frontend-visible effect is session names containing `:` from the `deliver_wake` spawn path. Fix inline in the backend at `mailbox.rs:508` by stripping the `<project>:` prefix from the display name (the canonical FQN is preserved in the session's working_directory and derivable via `agent_fqn_from_path`).

### §DR9 — Implementation order — adjusted

Merging §DR2 and §DR6 into plan §11:

1. Add `split_project_prefix`, `agent_fqn_from_path` in `config/teams.rs` + unit tests (§3.2, §8.1).
2. Add `project` field to `DiscoveredTeam` + populate in `discover_teams_in_project` (§3.1). Update `teams.rs:253` test fixture with `project: "foo".into()`. Compile; fix breakage.
3. Update `extract_wg_team`, `is_in_team`, `is_coordinator`, `can_communicate` (§5.1-5.5) with project-aware guards. **`is_coordinator` gets `None => false`; others keep `None => true` per §DR8.** Update `is_coordinator_for_cwd` (§DR2) to use `agent_fqn_from_path`. Add tests.
4. Update `cli/send.rs` (§6.1-6.3) + add `resolve_to_target`. **Apply `resolve_to_target` in `cli/close_session.rs` BEFORE the auth check (§DR1).** Delete shadow `agent_name_from_path` / `agent_name_from_root`.
5. Update `phone/mailbox.rs` routing helpers (§4.1-4.6). Apply the explicit local-only lenient anti-spoof fallback per §DR5. Strip `<project>:` from `session_name` at spawn time in `deliver_wake` (§DR6 tail).
6. Update `cli/list_peers.rs` (§5.6-5.8) — backend only, no frontend changes needed.
7. **NEW:** Update `src-tauri/src/lib.rs:540` session-restore coordinator check to `agent_fqn_from_path` (§DR2).
8. ~~Frontend nit fixes (§7).~~ **REMOVED** — §DR6 shows no frontend changes are needed.
9. Integration smoke: two colliding projects, confirm routing, close-session rejection, and list-peers are project-scoped.
10. Version bump + commit.

(Step count dropped from 11 to 10 because §7 goes away. §DR1 and §DR2 are absorbed into existing steps.)

### §DR10 — Non-goals addendum

Add to §12:
- Do NOT migrate existing session names in the `Session` store. Post-upgrade-spawned sessions will have `:` in their name (§DR6 display-strip mitigates it). Pre-existing session names persist until destroyed naturally. Accept this as transient UX noise.
- Do NOT add a `PeerInfo::short_name` field unless/until a frontend consumer of `list-peers` actually appears (§DR6).

### Feasibility check — summary

| File | Line ref in plan | Actual location | Drift |
|---|---|---|---|
| `config/teams.rs` | 5-17 (DiscoveredTeam) | 5-17 | none |
| `config/teams.rs` | 105-113 (extract_wg_team) | 105-113 | none |
| `config/teams.rs` | 153-168 (is_in_team WG branch) | 154-168 | ±1 line |
| `config/teams.rs` | 182-186 (is_coordinator WG branch) | 182-186 | none |
| `config/teams.rs` | 225-232 (can_communicate rule 2) | 226-232 | ±1 line |
| `config/teams.rs` | 394-401 (teams.push) | 394-401 | none |
| `phone/mailbox.rs` | 256-257 (anti-spoof) | 256-257 | none |
| `phone/mailbox.rs` | 323-339 (token-session anti-spoof) | 323-339 | none (313 is where `let session_name` starts) |
| `phone/mailbox.rs` | 898-965 (find_active_session) | 898-965 | none |
| `phone/mailbox.rs` | 916-926 (find_active_session filter) | 918-926 | ±2 lines (916 is comment) |
| `phone/mailbox.rs` | 969-984 (find_all_sessions) | 969-984 | none |
| `phone/mailbox.rs` | 1230-1303 (resolve_repo_path) | 1230-1303 | none |
| `phone/mailbox.rs` | 1274-1299 (WG fallback in resolve_repo_path) | 1275-1300 | ±1 line |
| `phone/mailbox.rs` | 1405-1441 (resolve_wg_path_from_sessions) | 1405-1441 | none |
| `phone/messaging.rs` | 12 (PTY_SAFE_MAX=1024) | 12 | none |
| `phone/messaging.rs` | 14 (MAX_SLUG_LEN=50) | 14 | none |
| `phone/messaging.rs` | 20 (PTY_WRAP_FIXED=19) | 20 | none |
| `phone/messaging.rs` | 74-83 (agent_short_name) | 74-83 | none |
| `phone/messaging.rs` | 99-107 (build_filename) | 99-107 | none |
| `phone/messaging.rs` | 114-186 (validate_filename_shape) | 114-186 | none |
| `phone/messaging.rs` | 660-664 (contract test) | 660-664 | none |
| `cli/send.rs` | 76-86 (agent_name_from_root) | 76-86 | none |
| `cli/send.rs` | 105 (sender derivation) | 105 | none |
| `cli/send.rs` | 121-133 (routing check) | 121-133 | none |
| `cli/send.rs` | 182-194 (PTY_SAFE_MAX clamp) | 184-194 | ±2 lines |
| `cli/list_peers.rs` | 52-62 (shadow agent_name_from_path) | 52-62 | none |
| `cli/list_peers.rs` | 280-308 (build_wg_peer) | 280-308 | none |
| `cli/list_peers.rs` | 311-389 (execute_wg_discovery) | 311-389 | none |
| `cli/list_peers.rs` | 391-590 (execute) | 391-590 | none |
| `cli/list_peers.rs` | 555-559 (WG replica discovery peer_name) | 555-559 | none |
| `cli/close_session.rs` | 99-100 (sender_agent/preferred_agent) | 99-100 | none |
| `ProjectPanel.tsx` | 491 (title) | 491 | **WRONG TYPE** — see §DR6 |
| `ProjectPanel.tsx` | 493 (peer.name RUNNING) | 493 | **WRONG TYPE** — see §DR6 |
| `ProjectPanel.tsx` | 674 (equality check) | 674 | **WRONG TYPE** — see §DR6 |

No code drift. Line references are stable — this review's additions (§DR1-§DR10) can be implemented as specified without re-verifying against HEAD.

### End of dev-rust review

---

## Dev-rust-grinch review (round 1)

**Reviewer:** wg-5-dev-team/dev-rust-grinch
**Date:** 2026-04-22
**Scope:** adversarial plan annotations; no code edits.
**Premise:** I tried to break this plan. Below is what I could break.

### Executive summary

Two critical gaps. Both flow from the same structural weakness: the plan makes `resolve_to_target` a **CLI-side** enforcement, but the enforcement boundary for wire-protocol safety is the **mailbox poller** (the only code that any outbox message — CLI-written, legacy in-flight, or manually crafted — must pass through). Once I shift the viewpoint from "the CLI qualifies targets" to "an adversary writes outbox JSON by hand," the design leaks:

- **§G1 (critical)** — close-session cross-project kill: a coordinator of project B writes an outbox JSON with qualified `from = "proj-b:..."` and UNQUALIFIED `target = "wg-1-devs/dev-rust"`. §DR1's fix runs at the CLI — bypassed by any direct outbox write. `is_coordinator_of` passes under lenient `is_in_team` (§5.3 `None => true`). `find_all_sessions` then matches sessions across all projects. Cross-project destroy. Dev-rust's strict `is_coordinator` does NOT fix this — strict only rejects unqualified `from`, not unqualified `target`.
- **§G2 (critical)** — silent cross-project delivery for wake: the architect's stated principle ("rule 2c surfaces ambiguity; zero silent misrouting") is violated at the mailbox. `find_active_session` (§4.1) and `resolve_repo_path` (§4.3) with an unqualified target match local-part across projects and pick one via `sort_by_key(status/temp)`. Any legacy in-flight wake message, or any outbox write that skips CLI qualification, routes to an arbitrary project.

Both gaps share the same remedy: move `resolve_to_target` into the mailbox as the first step of `process_message` (or at minimum of `handle_close_session` and `deliver_wake`). CLIs are untrusted writers to the outbox — they can be buggy, old, or malicious. The mailbox is the only enforcement point that sees every message.

Below: critical, concerns, nits, open-point verdicts, test additions, final verdict.

### §G1 — CRITICAL — close-session cross-project kill via direct outbox write

**Attack.** Legit coordinator of project B is `proj-b:wg-1-devs/tech-lead`. They write an outbox JSON directly under `<proj-b-root>/.agentscommander/outbox/` with `{ "from": "proj-b:wg-1-devs/tech-lead", "action": "close-session", "target": "wg-1-devs/dev-rust" }`. No colon in `target` — mirrors the legacy on-disk form Decision 3 tolerates. `mode`/`to` irrelevant for action dispatch.

**Trace through the post-fix pipeline (all dev-rust's decisions applied).**

1. `process_message`: `is_app_outbox = false`. `expected_from = agent_fqn_from_path(proj-b repo) = "proj-b:wg-1-devs/tech-lead"`. Equal to `msg.from`. Anti-spoof passes.
2. Token path — passes via session token.
3. `can_reach(msg.from, msg.to, teams)` — `msg.to` is irrelevant for action dispatch; skip.
4. Action dispatch → `handle_close_session`.
5. `is_coordinator_of("proj-b:wg-1-devs/tech-lead", "wg-1-devs/dev-rust", teams)`:
   - team_A (project `proj-a`): `is_coordinator(sender, team_A)` under dev-rust's strict `None => false` → sender has project `proj-b`, team_A.project = `proj-a` → project mismatch → false. Skip.
   - team_B (project `proj-b`): `is_coordinator(sender, team_B)` under strict → project match → true. `is_in_team("wg-1-devs/dev-rust", team_B)` under lenient `None => true` (target unqualified) → suffix match on `dev-rust` → true. AND = true.
   - Overall: true. **Authorized.**
6. `find_all_sessions("wg-1-devs/dev-rust")` with §4.2's filter: target is unqualified → match sessions whose LOCAL part equals `wg-1-devs/dev-rust`. Returns sessions across BOTH `proj-a` and `proj-b`.
7. All returned sessions force- or graceful-closed. **proj-a's dev-rust killed by a proj-b coordinator.**

**Why dev-rust's strict `is_coordinator` alone doesn't help.** Strict only fires when `msg.from` is unqualified. In this attack, `msg.from` is qualified and legitimate (attacker is really the coordinator of proj-b). The unqualified field is `msg.target`, and `is_in_team` (target side) is lenient by dev-rust's §DR8. The defense gap is specifically: *lenient target × strict sender × cross-project `find_all_sessions`*.

**Why §DR1's CLI-side fix is insufficient.** §DR1 rewrites `args.target` via `resolve_to_target` in `cli/close_session.rs`. That's CLI hygiene. An adversary — or a buggy/old CLI, or a manual outbox write — does not go through `cli/close_session.rs`. The outbox is a trust boundary; the CLI is not the enforcement point.

**Required fix.** Inside `mailbox.rs::handle_close_session`, BEFORE the auth check at line 1011, resolve the target:

```rust
let resolved_target = {
    let cfg = settings.read().await;
    // Symmetric with CLI's resolve_to_target: reject ambiguity, require qualified
    // OR unambiguous-unqualified. DO NOT fall through to `pick first` here.
    match resolve_target_server_side(target, &cfg.project_paths) {
        Ok(fqn) => fqn,
        Err(e) => {
            return self.reject_message(path, msg, &format!(
                "close-session target unresolvable: {}", e
            )).await;
        }
    }
};
// Use resolved_target for both is_coordinator_of and find_all_sessions.
```

`resolve_target_server_side` may reuse `cli::send::resolve_to_target` provided it is lifted out of `cli/` into a shared module (suggest `phone::resolution` or add to `config::teams`). Keep the symmetric ambiguity-rejection rule: the mailbox must NOT silently pick one.

**Why not just tighten `is_in_team`.** Because dev-rust's §DR8 argument for keeping lenient is valid — legacy in-flight wake messages and conversation history need it. Server-side target resolution is the surgical fix for the destructive-action path without collateral damage to display/routing tolerance.

### §G2 — CRITICAL — silent cross-project routing in deliver_wake and resolve_repo_path

Architect's Decision 2 rationale states: *"the old code's bug was that it silently picked one when multiple matched… Zero silent misrouting. Clear error, clear fix."* The proposed §4.1/§4.3 filters preserve that bug for unqualified targets at the mailbox.

**Trace.**

- Sender `proj-a:wg-1-devs/tech-lead` writes outbox with `to = "wg-1-devs/dev-rust"` (unqualified — could be a legacy in-flight message per Decision 3, or a buggy sender, or a post-upgrade CLI that failed to resolve for any reason).
- `can_communicate` passes — same-WG rule with `project_match = true` because one side is unqualified (dev-rust §5.5 lenient rule).
- Action = None → mode = wake → `deliver_wake(msg)`.
- `find_active_session("wg-1-devs/dev-rust")`: §4.1's filter matches sessions with local `wg-1-devs/dev-rust` across ALL projects. If both `proj-a` and `proj-b` have such sessions, both match. The pre-existing `sort_by_key((is_temp, status))` picks "best" — which is STATUS-ordered, not project-ordered. proj-b's session might win on status.
- Message delivered to proj-b's session. Silent misroute. Sender intended proj-a.

Same pattern in `resolve_repo_path`'s three loops (§4.3): first-match win, order is CWD/project-path/team-path iteration order — effectively random from the caller's perspective.

**Same remedy as §G1.** Do target resolution at the mailbox entry point (or at minimum at the start of `deliver_wake` and inside every `resolve_repo_path` / `find_active_session` caller). Reject-on-ambiguity, resolve-on-unambiguous, error-on-unknown — the same rule CLI applies.

**Cost/benefit.** The plan's Decision 3 says "tolerate on read, auto-upgrade on write" — for routing, this means: a legacy-shaped message triggers a resolve pass at the mailbox. If unambiguous, log-info and proceed with the resolved FQN (this matches `--to` resolution at the CLI). If ambiguous, reject with a clear message naming candidates. Dead-letter? The message stays in `rejected/` with the reason text — the sender can see it and re-queue with a qualified target. Small footgun, zero silent data corruption.

### §G3 — CRITICAL — plan §4.3 WG fallback has a concrete bug

In the architect's proposed §4.3 code block (lines 265-299 of the plan), the early initialization:

```rust
let mut dirs_to_check = vec![base.to_path_buf()];
if let Ok(entries) = std::fs::read_dir(base) {
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() { continue; }
        let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(want) = target_project {
            if dir_name != want { continue; }
        }
        dirs_to_check.push(p);
    }
}
```

Problem: `dirs_to_check` is **unconditionally seeded with `base`**. When `target_project = Some("proj-a")` and `rp` in `project_paths` points at `"C:/repos/proj-b"` (a project root, not a parent), the child-filter correctly rejects children — but `base` itself (proj-b root) stays in `dirs_to_check`. The subsequent loop then checks `proj-b/.ac-new/wg-name/__agent_x/` and returns it. Cross-project match despite the explicit target_project filter.

**Fix.** Seed `dirs_to_check` subject to the same filter:

```rust
let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");
let mut dirs_to_check: Vec<PathBuf> = if let Some(want) = target_project {
    if base_name == want { vec![base.to_path_buf()] } else { Vec::new() }
} else {
    vec![base.to_path_buf()]
};
if let Ok(entries) = std::fs::read_dir(base) {
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() { continue; }
        let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(want) = target_project {
            if dir_name != want { continue; }
        }
        dirs_to_check.push(p);
    }
}
```

The `else if let Some(want) = target_project { if base.file_name()... != Some(want) { continue; } }` fallback in the original is dead code once the seeding is correct (it only fires when `read_dir` errored, in which case base is either matching-or-not already decided above).

### §G4 — should-fix — `resolve_repo_path` loops 1 and 2 not discussed for ambiguity

Plan §4.3 proposes replacing the match logic in all three loops (lines 1236-1244, 1249-1257, 1261-1272) with the local-part fallback. Each loop returns on first match (`return Some(cwd.clone())`). For an unqualified target with matches in multiple projects, the return is deterministic only by iteration order — effectively random from the caller's standpoint.

**Why it matters.** This function services `deliver_wake` (spawn path), `handle_close_session` response-dir lookup, `resolve_agent_command` config lookup. Silent arbitrary-project selection undercuts Decision 2. Fix: for unqualified target, collect all matches and if > 1 return None (treat as "cannot resolve unambiguously") — forcing caller to reject or surface the ambiguity upstream. This is strictly safer than `return first` and aligns with Decision 2.

### §G5 — should-fix — `resolve_repo_path(&msg.from)` for response dir uses same arbitrary-pick

`handle_close_session` at `mailbox.rs:1085` calls `self.resolve_repo_path(&msg.from, app)` to locate the sender's responses directory. If `msg.from` is legacy unqualified (accepted via the anti-spoof lenient fallback per §DR5) and multiple projects have matching replicas, the response JSON lands in an arbitrary project's `responses/` — not necessarily the sender's. Impact: low (response is informational), but it is a correctness hole on a code path that exists specifically to close the loop with the sender.

**Fix.** Symmetric with §G4: `resolve_repo_path` for any unqualified input should reject on multi-match. Or, apply the server-side `resolve_to_target` to `msg.from` at the same point we resolve `msg.target`.

### §G6 — should-fix — `agent_fqn_from_path` behavior with subdirectory CWDs

Proposed `agent_fqn_from_path` finds the FIRST `.ac-new` in the path components. Cases:

1. **Subdirectory inside a replica** (e.g., a CWD `C:/repos/proj/.ac-new/wg-1-devs/__agent_alice/some/deep/subdir`) now resolves to `proj:wg-1-devs/alice`, which is *semantically correct* (alice "owns" her subdirs) but *behaviorally divergent* from the current `agent_name_from_path` which would return `some/deep/subdir`-derived nonsense. Document this as intentional; any call site that previously relied on the "last two components" shape may silently change.
2. **Path with a stray `.ac-new` segment** (e.g., `C:/.ac-new/repos/proj/.ac-new/wg-1-devs/__agent_x`) — `iter().position` finds the FIRST occurrence at index 0, `ac_idx > 0` fails, falls through to `agent_name_from_path` → returns unqualified. Pathological but possible if a user puts a project under a directory structure containing `.ac-new`. Consider `rposition` or "last occurrence" semantics, which are more robust against parent-path noise.
3. **Windows `\\?\` UNC prefix** (`\\?\C:\repos\proj\.ac-new\wg-1-devs\__agent_x`) — normalizes to `//?/C:/repos/proj/.ac-new/wg-1-devs/__agent_x`, `split('/')` filter-empty gives `["?", "C:", "repos", "proj", ".ac-new", "wg-1-devs", "__agent_x"]`. `.ac-new` at index 4, condition `ac_idx > 0 && 6 < 7` holds, FQN = `proj:wg-1-devs/x`. Robust — verified this works, noting it here for the test suite.

**Fix.** Use `rposition` (last `.ac-new`) or explicitly walk from the right. Add a test for case (1) confirming the intentional new behavior, and a test for (2) asserting the fallback.

### §G7 — should-fix — Decision 1 rationale overstates origin-agent uniqueness

The plan's Decision 1 rationale reads: *"Origin names are already unique by path (`project/` prefix is in-name)"*. This is not quite true. `agent_name_from_path` produces `<last-parent-dir-name>/<last-dir-name>`. Two origin agents in different project trees can still collide by parent-dir name:

- `C:/repos/tech-lead/coord`
- `D:/other/tech-lead/coord`

Both yield `tech-lead/coord`. Low-likelihood in practice, but the stated rationale promises uniqueness that doesn't hold. This PR doesn't have to fix origin uniqueness (it's out of scope for issue #70), but the Decision 1 prose should be softened ("origin names are conventionally unique by `project/` prefix" or similar) so a future reader doesn't treat it as a hard invariant.

### §G8 — should-fix — `resolve_to_target` must reject on empty candidate set DISTINCT from unknown

Plan §6.2 (2d): `0 candidates → reject ("unknown --to 'wg-1-devs/tech-lead'")`. This is correct for `send`. For `close-session` (post §G1 server-side resolution), the error message should differentiate "target shape is not a WG replica local form" vs "no matching WG replica exists across known projects" vs "unknown but project-qualified" — otherwise a coordinator debugging why close-session failed gets an opaque error. Surface the three paths explicitly.

### §G9 — should-fix — Test coverage gaps

The plan + dev-rust = 12 tests. I want these added, aimed specifically at the surfaces above:

1. **`close_session_rejects_direct_outbox_write_with_unqualified_target`** (§G1 regression) — fixture: two projects with colliding team names, legit coordinator of proj-B. Write outbox JSON directly (bypass CLI). Assert: mailbox rejects (or resolves and fails auth), **no session in proj-A is destroyed**. This is the most important test in the suite. Without it, a future refactor can silently regress §G1.
2. **`deliver_wake_rejects_unqualified_to_with_cross_project_matches`** (§G2 regression) — fixture: two projects, both have `wg-1-devs/dev-rust` sessions, outbox message with unqualified `to`. Assert: rejected (or resolved unambiguously; never delivered to arbitrary one).
3. **`resolve_repo_path_wg_fallback_honors_target_project`** (§G3 regression) — `project_paths = ["C:/repos/proj-b"]`, `target_project = Some("proj-a")`, proj-b's `.ac-new/wg-1-devs/__agent_alice/` exists. Assert: returns None (not proj-b's path).
4. **`resolve_repo_path_returns_none_on_ambiguous_unqualified`** (§G4 regression) — two projects with matching replicas, unqualified input. Assert: returns None, not an arbitrary path.
5. **`agent_fqn_from_path_deeper_cwd_returns_replica_fqn`** (§G6 case 1) — `.ac-new/wg-1-devs/__agent_alice/deep/subdir` → `proj:wg-1-devs/alice`. Lock in the intentional behavior.
6. **`agent_fqn_from_path_handles_unc_prefix`** (§G6 case 3) — `\\?\C:\...` works.
7. **`agent_fqn_from_path_pathological_ac_new_prefix`** (§G6 case 2) — path with stray `.ac-new` above the real one falls through. Optional if implementation switches to `rposition`.
8. **`deliver_wake_spawned_session_name_has_no_colon`** (§DR6 tail regression) — msg.to = `proj-a:wg-1-devs/tech-lead`, assert spawned Session.name = `wg-1-devs/tech-lead`.
9. **`resolve_to_target_round_trip_integration`** (tech-lead's (6) from the task brief) — CLI send from proj-a → mailbox routes with FQN → receiver writes reply → CLI read sees reply. Full loop. Likely lives as an integration test rather than a unit test; flag to dev-rust.
10. **`is_coordinator_rejects_legacy_unqualified_from`** — dev-rust §DR8 endorsement (strict `None => false`). unqualified `"wg-1-devs/tech-lead"` → `is_coordinator` returns false for every team. Locks in the strict choice against accidental softening.

Total after this review: plan §8 (7) + §DR7 (5) + §G9 (10) = **22 tests**. Several overlap conceptually; dev-rust should dedupe when writing them, not when counting them.

### §G10 — nit — Document that `list_peers.rs:423` is idempotent for origin names

`crate::config::teams::agent_name_from_path(cn) == my_name`: `cn` is an origin display name `"project/agent"`. `agent_name_from_path("project/agent")` returns `"project/agent"` — identity on already-split input. So the comparison is equivalent to `cn == my_name` (the second disjunct). Not a bug, but the double-compare is dead. A one-line comment or a rename helps readability; not blocking.

### §G11 — nit — Project folder name equal to `wg-N-something`

A user could (theoretically) name a project folder `wg-1-experiment`. `agent_name_from_path` of a non-WG agent under such a project yields e.g. `wg-1-experiment/alice`. Then `extract_wg_team("wg-1-experiment/alice")` = `Some("experiment")` → `is_in_team` WG branch fires against any team named `experiment`. Cross-project false positive for team membership. Extremely low-likelihood, but Decision 1's rationale rejection of format (a) `project/wg/agent` partly rested on "existing call sites that split first component for WG prefix" — the existence of projects that LOOK like WG prefixes is an argument for making `extract_wg_team` stricter (require the local prefix shape `wg-<N>-<team>` where `<N>` is digits), not just `starts_with("wg-")`. Out of scope for this PR; note for follow-up.

### §G12 — nit — Filename budget recheck

Architect measured 644-byte margin. I re-ran the worst case mentally: `from` is now `<project>:wg-N-team/agent` — assume worst-case `project` = 30 chars (reasonable for dir names), `wg-N-team` ≤ 20 chars, `agent` ≤ 30 chars → `from.len()` ≤ 80. `PTY_WRAP_FIXED = 19`. Body: `"Nuevo mensaje: <abs_path>. Lee este archivo."` = 44 + `abs_path.len()`. Worst `abs_path` ≈ 241 (as architect measured). Total ≤ 19 + 80 + 44 + 241 = **384**. Under PTY_SAFE_MAX=1024 by 640 bytes. Matches architect's 644 within a few bytes of measurement variance. ✓

Unicode risk: `from.len()` is in bytes. A project name with multi-byte UTF-8 (e.g., Cyrillic) could balloon. Not a bug at the proposed threshold, but the `body.len() > 150` log-warn in §6.3 is cheap insurance. Keep as-is.

### §G13 — Decision verdict on dev-rust's open points

**Point 1 — `is_coordinator` strict (`None => false`):**

**ENDORSE.** Concrete failure scenario without it: an attacker coordinator in proj-B writes `msg.from = "wg-1-devs/tech-lead"` (unqualified), `target = "proj-a:wg-1-devs/dev-rust"` (qualified) in proj-B's outbox. Anti-spoof accepts legacy msg.from via dev-rust §DR5 local-match. `is_coordinator(unqualified_from, team_A)` LENIENT would match on suffix alone (suffix = `tech-lead` equals team_A's coordinator suffix) → authorized on team_A (wrong project!). `is_in_team(qualified_target, team_A)` passes (same project). Close-session succeeds against proj-A. Strict `is_coordinator` blocks this at step 1 because unqualified `from` returns false.

**Point 2 — drop §7:**

**ENDORSE.** Independently verified: `ProjectPanel.tsx:400` types `runningPeers` as `() => AcAgentReplica[]`. `ProjectPanel.tsx:491, 493, 674` consume `peer.name` where `peer: AcAgentReplica` — which is the short dir name from `ac_discovery.rs`, NOT `PeerInfo.name` from `list-peers`. I also grepped the full `src/` for `list-peers` / `listPeers` / `PeerInfo` — zero matches. There is no frontend consumer of `list-peers` output. §7.1-§7.4 changes are based on a wrong type identification and should be removed. The only surviving concern is the `:` character in `Session.name` for wake-spawned sessions — already addressed by §DR6 tail (strip prefix before assigning `session_name` at `mailbox.rs:508`).

### §G14 — Final verdict

**Status: needs another round (round 2).**

The plan has two critical flaws (§G1, §G2) that share a root cause (CLI-as-enforcement-boundary is wrong for wire-protocol safety). Round 2 must:

1. Move `resolve_to_target` into a shared module callable from both CLI and mailbox (e.g., `phone::resolution`).
2. Apply server-side target resolution at the start of `handle_close_session` and `deliver_wake`, with **reject-on-ambiguity** semantics identical to the CLI's rule 2c.
3. Additionally tighten `resolve_repo_path` to return None (not arbitrary pick) on multi-match when input is unqualified (§G4, §G5).
4. Fix the `dirs_to_check` seeding bug in the proposed §4.3 code block (§G3).
5. Add the 10 tests in §G9.
6. Absorb §DR8's strict `is_coordinator` (this alone is insufficient per §G1, but still necessary).
7. Leave §G6, §G7, §G8 fixes as low-priority but address at least §G6's `rposition` switch since it's a one-line hardening.

**Approval scope of this review.**

- Decision 1 (format): approved.
- Decision 2 (CLI-lenient `--to`): approved.
- Decision 3 (tolerate-on-read): approved, but calls for server-side resolve that dev-rust did not add.
- Decision 4 (filename budget): approved with measurement confirmation (§G12).
- §4.1/§4.2/§4.3 filter change: approved in direction, rejected in detail until §G1-§G5 are addressed.
- §5 team-membership project-guards: approved with dev-rust's strict/lenient dial (§DR8 + §G13).
- §6 CLI resolution: approved but incomplete without mailbox-side mirror.
- §7 frontend: removed per §DR6 / §G13.

**If round 2 closes §G1-§G5**, I will consider the plan ready for implementation. I cannot approve round 1.

### End of dev-rust-grinch review

---

## Architect round 2

**Date:** 2026-04-22
**Reviewers addressed:** dev-rust §DR1-§DR10, dev-rust-grinch §G1-§G14
**Scope:** §§4 / 5.4 / 6 / 7 / 8 / 11 revised; original round-1 prose retained above with `> architect round 2:` quote-blocks. Sections §AR2-* below are the authoritative updated design.

### Executive summary (round 2)

The key shift: **the mailbox is the enforcement boundary, not the CLI.** Any outbox message — CLI-written, legacy in-flight, old client, or hand-crafted JSON — flows through `MailboxPoller::process_message`, which becomes the single authoritative point for FQN resolution and anti-spoof canonicalization. `resolve_to_target` lifts from `cli/send.rs` into `config::teams::resolve_agent_target` and is called by the CLI (`send`, `close_session`) and by the mailbox (`process_message` canonicalization + `handle_close_session` target resolve). Reject-on-ambiguity semantics are identical across all callers — zero silent misrouting.

Grinch's three critical items (§G1 close-session kill, §G2 silent wake misroute, §G3 `dirs_to_check` seeding) are accepted wholesale. §G4 + §G5 (arbitrary-pick in `resolve_repo_path`) are fixed by the caller-side canonicalization plus a hardened `resolve_repo_path` that returns `None` on unqualified multi-match. §G6 `rposition`, §G7 rationale softening, §G8 differentiated errors — accepted. §DR8's strict `is_coordinator` (`None => false`) locked in as defense-in-depth. §DR6's frontend removal accepted; only the `session_name` colon-strip survives. Nothing rejected.

### §AR2-shared — shared resolver in `config::teams`

**Location:** `src-tauri/src/config/teams.rs`, added as a public free function alongside `agent_fqn_from_path` and `split_project_prefix`.

**Not `phone::resolution`:** considered and rejected. `phone::` owns transport; `config::teams` already owns team/agent discovery (`discover_teams`, `agent_name_from_path`, `is_coordinator_of`, `can_communicate`). Target resolution reuses the same two-level project-paths scan as `discover_teams_in_project`. `phone::mailbox` and `cli::send` both already import `config::teams` — no new module crossings.

**Signature:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ResolutionError {
    /// Target string is neither FQN (contains `:`) nor a WG-local-form (`wg-N-team/agent`)
    /// nor a bare agent name. Examples: empty, contains path separators, has too many colons.
    #[error("target '{0}' is not a valid agent name shape")]
    InvalidShape(String),

    /// Target is fully qualified (`proj:wg-N/agent`) but no matching replica exists on disk
    /// under the `project_paths` scan (including two-level expansion).
    #[error("target '{0}' is qualified but not found in any known project")]
    UnknownQualified(String),

    /// Target is unqualified (WG-local or bare) and scan found zero matching replicas.
    #[error("target '{0}' not found in any known project")]
    NoMatch(String),

    /// Target is unqualified and matches >1 replica across projects. Candidates are FQN so
    /// the user can re-issue the command with a project-qualified form.
    #[error("target '{target}' is ambiguous; candidates: {}", candidates.join(", "))]
    Ambiguous { target: String, candidates: Vec<String> },
}

/// Resolve an agent target to a canonical FQN.
///
/// Accepts:
/// - Fully qualified WG: `<project>:<wg>/<agent>` → validated, returned as-is.
/// - Origin form: `<project>/<agent>` → validated, returned as-is. (Origin names are
///   conventionally — though not absolutely — unique; see §AR2-G7.)
/// - Unqualified WG: `wg-N-team/<agent>` → resolved against `project_paths` (two-level scan);
///   unambiguous → returned qualified; ambiguous → `Ambiguous` error.
/// - Bare `<agent>`: delegated to legacy (returned as-is) per Decision 2 step 3.
///
/// `project_paths` is the same slice consumed by `discover_teams`. The function applies the
/// base-plus-immediate-non-dot-children scan used by `discover_teams_in_project` so that a
/// parent-dir entry like `C:/repos/` resolves correctly (§DR4).
pub fn resolve_agent_target(
    target: &str,
    project_paths: &[String],
) -> Result<String, ResolutionError>;
```

**Callers after round 2:**
- `cli::send::execute` — replaces the inlined round-1 `resolve_to_target` of §6.2. Maps `ResolutionError::Display` to stderr + exit 1.
- `cli::close_session::execute` — called before building `OutboxMessage` (§DR1's original site). Same error handling.
- `phone::mailbox::MailboxPoller::process_message` — canonicalizes `msg.to` (§AR2-norm).
- `phone::mailbox::MailboxPoller::handle_close_session` — canonicalizes `msg.target` (§AR2-G1).

### §AR2-norm — `process_message` canonicalization step

**Insertion point:** `phone/mailbox.rs:process_message`, immediately after the anti-spoof block at lines 252-269 (§4.5), before the token validation at line 271.

**Design:**

```rust
// (1) Canonicalize msg.from. If it passed anti-spoof as unqualified via local-match
// (§DR5), upgrade it to the expected_from FQN so downstream code (resolve_repo_path
// for response dir, logs, archives) sees the canonical form. Closes grinch §G5.
if let (None, _) = crate::config::teams::split_project_prefix(&msg.from) {
    let (exp_proj, _) = crate::config::teams::split_project_prefix(&expected_from);
    if exp_proj.is_some() {
        log::info!(
            "[mailbox] canonicalized legacy msg.from '{}' → '{}'",
            msg.from, expected_from
        );
        msg.from = expected_from.clone();
    }
}

// (2) Canonicalize msg.to via the shared resolver. Empty `to` is allowed for action
// dispatch paths (close-session); skip resolution in that case.
if !msg.to.is_empty() {
    let paths = {
        let cfg = app.state::<SettingsState>();
        let c = cfg.read().await;
        c.project_paths.clone()
    };
    match crate::config::teams::resolve_agent_target(&msg.to, &paths) {
        Ok(fqn) if fqn != msg.to => {
            log::info!("[mailbox] canonicalized msg.to '{}' → '{}'", msg.to, fqn);
            msg.to = fqn;
        }
        Ok(_) => {}
        Err(e) => {
            return self.reject_message(path, &msg, &format!(
                "Unresolvable target: {}", e
            )).await;
        }
    }
}
```

(`msg.target` for close-session is canonicalized inside `handle_close_session` — see §AR2-G1 — because it's only meaningful for action dispatch and has its own error wording.)

**Why mutate `msg` vs. thread canonical aliases:** downstream consumers (`resolve_repo_path`, `find_active_session`, `can_reach`, `is_coordinator_of`, logging, `move_to_delivered`, `reject_message`, response-dir derivation) are numerous. A single in-place mutation makes every downstream site project-scoped for free, and the archived `delivered/` / `rejected/` JSON records the canonical form — better for audit. (Dev-rust is free to refactor into a `canonical_from: &str` + `canonical_to: &str` threading if they prefer; see §AR2-open point 2.)

### §AR2-G1 — close-session server-side resolve (grinch §G1)

**Accepted in full.** Grinch's attack trace is concrete.

**Insertion point:** `phone/mailbox.rs:handle_close_session`, after `let target = msg.target.as_deref().ok_or_else(...)` at line 993-996, before the `is_master` check at line 999.

```rust
let resolved_target = {
    let paths = {
        let cfg = app.state::<SettingsState>();
        let c = cfg.read().await;
        c.project_paths.clone()
    };
    match crate::config::teams::resolve_agent_target(target, &paths) {
        Ok(fqn) => fqn,
        Err(e) => {
            return self.reject_message(path, msg, &format!(
                "close-session target unresolvable: {}", e
            )).await;
        }
    }
};
let target = resolved_target.as_str();
// Downstream (line 1013 is_coordinator_of, line 1028 find_all_sessions, line 1019 error
// message, line 1078 response JSON "target") all use the qualified FQN.
```

After this change, even if §DR1's CLI-side `resolve_to_target` is absent, skipped, or bypassed by a direct outbox write, the destructive path rejects or canonicalizes at the trust boundary. Grinch §G1's attack is blocked:
- Unqualified `target = "wg-1-devs/dev-rust"` with two project matches → `Ambiguous` → reject with candidates listed.
- Unqualified with one match → canonicalized to that project's FQN → `is_coordinator_of` checks against the correct project; `find_all_sessions` matches exact FQN only.

### §AR2-G2 — deliver_wake server-side resolve (grinch §G2)

**Accepted in full.** Round 1's "find_active_session is a best-effort chooser" framing was wrong. With §AR2-norm canonicalizing `msg.to` before mode dispatch, `deliver_wake` and `find_active_session` receive a guaranteed-qualified target. The §4.1 filter simplifies:

```rust
let mut matches: Vec<_> = sessions.iter()
    .filter(|s| {
        crate::config::teams::agent_fqn_from_path(&s.working_directory) == agent_name
    })
    .collect();
```

The existing sort (`(is_temp, status)`) stays — it disambiguates multiple sessions sharing the same canonical FQN (Active vs. Idle vs. Exited replicas), which is orthogonal to the cross-project bug.

**Legacy tolerance:** the canonicalizer upgrades unambiguous-unqualified `msg.to` per Decision 3. Only ambiguous and truly-unknown cases reject — same semantics as the CLI.

### §AR2-G3 — `resolve_repo_path` WG fallback seeding fix

**Accepted.** Grinch's corrected snippet is functionally equivalent; I prefer this shape for readability (identical behavior, no dead fallback branch):

```rust
let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
if !local.starts_with("wg-") { return None; }
let (wg_name, agent_short) = local.split_once('/')?;
let replica_dir = format!("__agent_{}", agent_short);

for rp in &cfg.project_paths {
    let base = std::path::Path::new(rp);
    if !base.is_dir() { continue; }
    let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let project_matches = |dir_name: &str| -> bool {
        match target_project { Some(want) => dir_name == want, None => true }
    };

    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
    if project_matches(base_name) {
        dirs_to_check.push(base.to_path_buf());
    }
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() { continue; }
            let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !project_matches(dir_name) { continue; }
            dirs_to_check.push(p);
        }
    }

    for dir in dirs_to_check {
        let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
        if candidate.is_dir() {
            // Coupling with §AR2-G4: first hit within a project is the unique hit because
            // an FQN cannot match two replica dirs under the same project.
            return Some(candidate.to_string_lossy().to_string());
        }
    }
}
None
```

Base seeding now honors the project filter. No cross-project leak via an unfiltered seed.

### §AR2-G4 — `resolve_repo_path` returns `None` on unqualified multi-match

**Accepted (covers §G4 and §G5).** All three outer loops change from "first match wins" to "collect all matches; return `None` on >1 when the target is unqualified".

**Collector pattern:**

```rust
async fn resolve_repo_path(&self, agent_name: &str, app: &tauri::AppHandle) -> Option<String> {
    let (target_project, _) = crate::config::teams::split_project_prefix(agent_name);
    let is_qualified = target_project.is_some();
    let mut matches: Vec<String> = Vec::new();

    let match_cwd = |cwd: &str, out: &mut Vec<String>| {
        let path_fqn = crate::config::teams::agent_fqn_from_path(cwd);
        let hit = if is_qualified {
            path_fqn == agent_name
        } else {
            let (_, path_local) = crate::config::teams::split_project_prefix(&path_fqn);
            let (_, want_local) = crate::config::teams::split_project_prefix(agent_name);
            path_local == want_local
        };
        if hit && !out.iter().any(|m| *m == cwd) {
            out.push(cwd.to_string());
        }
    };

    // Loop 1: session CWDs
    // Loop 2: settings.project_paths
    // Loop 3: discovered team agent_paths (use team.project to short-circuit when
    //         target_project is Some)
    // Loop 4: WG fallback (§AR2-G3 shape)

    // For qualified input, short-circuit on first hit inside each loop is safe.
    // For unqualified, accumulate then decide:
    match matches.len() {
        0 => None,
        1 => Some(matches.pop().unwrap()),
        _ => {
            log::warn!(
                "[mailbox] resolve_repo_path('{}'): {} candidates, refusing arbitrary pick: {:?}",
                agent_name, matches.len(), matches
            );
            None
        }
    }
}
```

In practice, after §AR2-norm canonicalized `msg.to` and `msg.from`, `resolve_repo_path` is almost always called with a qualified input — so the collector branch is a belt. Worn anyway because any future caller that bypasses canonicalization still gets correctness.

### §AR2-G6 — `agent_fqn_from_path` uses `rposition`

**Accepted.** One-line edit in §3.2:

```rust
// Before (round 1): .iter().position(|p| *p == ".ac-new")
// After  (round 2): .iter().rposition(|p| *p == ".ac-new")
```

Handles the pathological parent-path `.ac-new` case (grinch §G6 case 2). Subdirectory-inside-replica behavior (case 1) unchanged — the right-most `.ac-new` is the identity anchor. UNC-prefix case (3) already robust.

Tests 3, 4, 5 from §AR2-tests lock in each case.

### §AR2-G7 — Decision 1 rationale softened

**Accepted.** The round-1 text in §2 (Decision 1) reads "Origin names are already unique by path". Grinch §G7 notes two origin projects can collide by parent-dir name across drives. Softened wording for the dev-rust implementation pass (not an in-place edit to §2 — keep round-1 prose intact per ground rules):

> Origin names are **conventionally** scoped by the `project/` prefix already carried in-name. Edge cases exist (e.g., two project folders on different drives sharing the same dir name — `C:/repos/app/coord` and `D:/other/app/coord` both yield `app/coord`). Issue #70 does not include origin agents in scope; a strict origin-uniqueness guarantee is a follow-up if needed.

No code change — rationale text only. If tech-lead wants an in-place edit to §2, say so and I'll update §2 directly; ground rules suggested otherwise.

### §AR2-G8 — differentiated error strings

**Accepted.** Covered by the `ResolutionError` enum in §AR2-shared. `thiserror::Display` drives all user-facing strings — no duplication between CLI and mailbox. Four distinct messages (see enum docs above) cover: invalid shape / qualified-unknown / unqualified-no-match / ambiguous-with-candidates.

### §AR2-strict — strict `is_coordinator` (`None => false`) locked in

**Accepted from §DR8 and §G13.** `is_coordinator` in `config/teams.rs:173-189` — explicit strict guard when `agent_name` is unqualified:

```rust
if let Some(wg_team) = extract_wg_team(agent_name) {
    let (agent_project, _) = split_project_prefix(agent_name);
    let Some(agent_project) = agent_project else {
        // Strict: unqualified `from` cannot hold coordinator authority.
        // (is_in_team and can_communicate remain lenient — §DR8.)
        return false;
    };
    if wg_team == team.name
        && agent_project == team.project
        && agent_suffix(agent_name) == agent_suffix(coord_name)
    {
        return true;
    }
}
```

Defense-in-depth alongside §AR2-G1's mailbox target-resolve. Both must hold for cross-project `close-session` to be impossible. Cost: legacy in-flight `close-session` messages with unqualified `msg.from` get rejected; acceptable per Decision 3's implicit "destructive ops are strict" principle.

### §AR2-session-name — strip `:` from spawned `Session.name`

**From §DR6 tail.** At `phone/mailbox.rs:508`:

```rust
// Before:
let session_name = msg.to.clone();

// After:
let session_name = {
    let (_, local) = crate::config::teams::split_project_prefix(&msg.to);
    local.to_string()
};
```

Canonical FQN stays recoverable via `agent_fqn_from_path(&cwd)` at any list-sessions time; only the sidebar label uses the local form.

### §AR2-tests — deduplicated test inventory

Round 1 §8 (7) + §DR7 (5) + §G9 (10) = raw 22. Canonical deduplicated list below, grouped by module. **[CRIT]** = critical regression; **[INT]** = integration (needs two-project fixture harness).

**`config/teams.rs` — 11 tests**

1. [CRIT] `agent_fqn_from_path_wg_replica`
2. `agent_fqn_from_path_origin`
3. [CRIT] `agent_fqn_from_path_deeper_cwd_returns_replica_fqn` (§G6 case 1)
4. [CRIT] `agent_fqn_from_path_handles_unc_prefix` (§G6 case 3)
5. `agent_fqn_from_path_pathological_ac_new_prefix` (§G6 case 2 — rposition)
6. `split_project_prefix_present` + `split_project_prefix_absent`
7. `extract_wg_team_peels_project_prefix`
8. [CRIT] `is_in_team_rejects_cross_project_wg_match`
9. [CRIT] `can_communicate_rejects_cross_project_same_wg` + `can_communicate_allows_legacy_unqualified`
10. [CRIT] `is_coordinator_for_cwd_project_qualified` (§DR7#2)
11. [CRIT] `is_coordinator_rejects_legacy_unqualified_from` (strict — §G9#10)

**`config/teams.rs::resolve_agent_target` — 5 tests**

12. [CRIT] `resolve_agent_target_passes_through_qualified`
13. [CRIT] `resolve_agent_target_qualifies_unambiguous_unqualified`
14. [CRIT] `resolve_agent_target_rejects_ambiguous`
15. `resolve_agent_target_rejects_unknown`
16. [CRIT] `resolve_agent_target_two_level_scan` (§DR7#5)

**`phone/mailbox.rs` — 5 tests**

17. [CRIT] [INT] `close_session_rejects_direct_outbox_write_with_unqualified_target` (§G9#1 — highest priority)
18. [CRIT] [INT] `deliver_wake_rejects_unqualified_to_with_cross_project_matches` (§G9#2)
19. [CRIT] `resolve_repo_path_wg_fallback_honors_target_project` (§G9#3 — §AR2-G3 regression)
20. [CRIT] `resolve_repo_path_returns_none_on_ambiguous_unqualified` (§G9#4 — §AR2-G4 regression)
21. [CRIT] `deliver_wake_spawned_session_name_has_no_colon` (§G9#8 — §AR2-session-name regression)

**`phone/mailbox.rs` anti-spoof — 2 tests**

22. [CRIT] `anti_spoof_legacy_msg_from_accepted_by_local_match` (§DR7#3)
23. [CRIT] `anti_spoof_cross_project_qualified_msg_from_rejected` (§DR7#4)

**Integration — 1 test**

24. [INT] `resolve_to_target_round_trip_integration` (§G9#9)

**Total: 24 tests** (raw 22 grew by +2 because test 6 and test 9 are each logically pairs and I kept them as single `#[test]` files with two `#[test]` functions — counted honestly as 2 each). Dev-rust free to consolidate if the pairing makes them lopsided; what matters is coverage, not count.

### §AR2-order — updated implementation order

Replaces §DR9's 10-step order:

1. Add `split_project_prefix`, `agent_fqn_from_path` (with `rposition`) in `config/teams.rs` + tests 1-7.
2. Add `ResolutionError` enum and `resolve_agent_target` in `config/teams.rs` + tests 12-16.
3. Add `project: String` field to `DiscoveredTeam`; populate in `discover_teams_in_project`. Update `teams.rs:253` test fixture with `project: "foo".into()`. Compile.
4. Update `extract_wg_team`, `is_in_team`, `is_coordinator` (strict `None => false`), `can_communicate` with project guards. Update `is_coordinator_for_cwd` per §DR2. Add tests 8-11.
5. Update `cli/send.rs`: sender via `agent_fqn_from_path`, `--to` via `resolve_agent_target`. Delete shadow `agent_name_from_root`. Map `ResolutionError` → exit 1.
6. Update `cli/close_session.rs`: `--target` via `resolve_agent_target` (§DR1).
7. Update `cli/list_peers.rs`: delete shadow `agent_name_from_path`; use `agent_fqn_from_path`. Emit FQN in WG replica discovery (§5.6-§5.8).
8. Update `phone/mailbox.rs`:
   - `process_message`: add §AR2-norm canonicalization (both `msg.from` and `msg.to`).
   - `handle_close_session`: canonicalize `msg.target` via `resolve_agent_target` (§AR2-G1).
   - `find_active_session` / `find_all_sessions`: simplified exact-FQN filter (§AR2-G2).
   - `resolve_repo_path`: collector pattern with multi-match `None` (§AR2-G4) + §AR2-G3 seed fix.
   - `resolve_wg_path_from_sessions`: peel project prefix (§4.4).
   - `deliver_wake`: `<project>:` strip from `session_name` at spawn (§AR2-session-name).
   - Anti-spoof: explicit local-only fallback contract (§DR5).
9. Update `src-tauri/src/lib.rs:540` session-restore → `agent_fqn_from_path` (§DR2).
10. Run the test suite. Fix compile / runtime issues.
11. Integration smoke: two-project fixture; run tests 17-24 manually or via harness.
12. Version bump to 0.7.6 (`tauri.conf.json`, `Cargo.toml`, `Titlebar.tsx`). Commit.

### §AR2-non-goals — additions to §12

- Do NOT project-scope origin agents in this PR (§AR2-G7 follow-up).
- Do NOT change `OutboxMessage` JSON schema (`from`, `to`, `target` stay `String`).
- Do NOT backfill historical `delivered/` / `rejected/` archives.
- Do NOT create a `phone::resolution` module (rejected in §AR2-shared — resolver lives in `config::teams`).
- Do NOT pre-emptively add `PeerInfo::short_name` — §DR10 reconfirmed.

### §AR2-open — open sub-points (candidates for tech-lead escalation — none expected to need round 3)

Two minor judgment calls where dev-rust could reasonably take a different tack without correctness impact:

1. **`ResolutionError::Ambiguous` display format.** I use inline comma-separated candidates. If long FQN lists make the error unreadable, dev-rust can switch to newline-separated. Non-architectural.
2. **`process_message` msg mutation.** §AR2-norm mutates `msg` in place. Dev-rust may prefer threading `canonical_from: &str` + `canonical_to: &str` through the downstream call graph. Equivalent semantics. Architect leaves the shape to dev-rust.

If grinch identifies a genuinely new attack vector in round 2 that §AR2 does not address, flag — round 3 beats shipping an unaddressed critical.

### Nothing rejected

Every grinch and dev-rust finding is incorporated. §G10, §G11, §G12 are nits that need no plan change (§G10 readability, §G11 follow-up out of scope, §G12 confirms measurement). §DR10's non-goals are merged into §AR2-non-goals.

### End of architect round 2

---

## Dev-rust review (round 2)

**Reviewer:** wg-5-dev-team/dev-rust
**Date:** 2026-04-22
**Scope:** absorption check + §AR2-norm stress test + implementation-hazard audit. No code edits.

### §DR2-Summary — absorption check

All §DR1-§DR10 and §G1-§G14 items absorbed into §AR2-*. Verdict below. No structural slippage. Two small implementation hazards (§DR2-3 and §DR2-4) and one tiny test-coverage gap (§DR2-5) are worth noting, none blocking.

| Item | Architect §AR2 site | Absorption |
|---|---|---|
| §DR1 close-session CLI fix | §AR2-order step 6 | ✅ preserved as belt-and-braces |
| §DR2a `lib.rs:540` | §AR2-order step 9 | ✅ |
| §DR2b `is_coordinator_for_cwd` | §AR2-order step 4 | ✅ |
| §DR3 test fixture compile-blocker | §AR2-order step 3 | ✅ explicit |
| §DR4 two-level scan | §AR2-shared doc | ✅ |
| §DR5 anti-spoof local-only fallback | §AR2-order step 8 (anti-spoof bullet) | ✅ |
| §DR6 drop §7 frontend | §AR2-order (no frontend steps) + §7 annotation | ✅ |
| §DR7 5 tests | §AR2-tests #10, #16, #17, #22, #23 | ✅ |
| §DR8 strict `is_coordinator` | §AR2-strict | ✅ locked in |
| §DR9 order 10 steps → AR2 12 steps | §AR2-order | ✅ expanded for shared resolver |
| §DR10 non-goals | §AR2-non-goals | ✅ reconfirmed |
| §G1 close-session cross-project kill | §AR2-G1 | ✅ server-side resolve |
| §G2 silent cross-project wake | §AR2-G2 + §AR2-norm | ✅ canonicalize before dispatch |
| §G3 `dirs_to_check` seeding | §AR2-G3 | ✅ fixed |
| §G4 multi-match first-wins | §AR2-G4 collector | ✅ returns None |
| §G5 `resolve_repo_path(&msg.from)` | §AR2-norm step (1) | ✅ canonicalizes msg.from |
| §G6 `rposition` | §AR2-G6 | ✅ |
| §G7 Decision 1 softening | §AR2-G7 | ✅ rationale only |
| §G8 differentiated errors | `ResolutionError` enum in §AR2-shared | ✅ four variants |
| §G9 10 tests | §AR2-tests #3, #4, #5, #8, #11, #17, #18, #19, #20, #21 | ✅ 10 entries mapped |
| §G10-§G12 nits | no plan change | ✅ consistent |
| §G13 strict `is_coordinator` endorse | §AR2-strict | ✅ |
| §G14 round-2 gate | §AR2 closes §G1-§G5 | ✅ grinch's exit criteria met |

No item lost, narrowed, or paraphrased incorrectly.

### §DR2-1 — §AR2-G1 is the authoritative gate; §DR1 CLI fix correctly reduced to belt-and-braces

§AR2-G1 says: "§DR1's CLI-side fix becomes belt-and-braces, not the authoritative gate." Verified §AR2-order step 6 still includes `cli/close_session.rs` resolution. The CLI stays because it gives users immediate feedback (no round-trip through mailbox) on common input errors — UX win. Mailbox-side resolver is the enforcement gate. Clean separation.

### §DR2-2 — §AR2-norm stress test

Tech-lead flagged four hazards. Taking them in order.

**Hazard A — mutation visibility.** `process_message` reads the outbox file with `std::fs::read_to_string(path)` at entry and deserializes into `msg: OutboxMessage`. Adding `mut` makes it `let mut msg: OutboxMessage`. Mutation at §AR2-norm is local to this task's `msg`; `reject_message` / `move_to_delivered` clone `msg` before serialization so the ARCHIVE records the post-canonicalization form (good — audit trail). Retry path re-reads from disk and re-canonicalizes each cycle — no persistence across failures. Conversation history writer uses `PhoneMessage.from/to` which is a separate type; it will record whatever is passed to it. **Verdict: safe.**

**Hazard B — `ResolutionError` failure UX.** `reject_message` writes `rejected/{id}.reason.txt` with the error text. CLI's `send` poll loop (`cli/send.rs:282-311`) watches for this file and prints its contents. `thiserror::Display` on `Ambiguous` renders `"target 'X' is ambiguous; candidates: a, b"`. The wrap in §AR2-norm adds `"Unresolvable target: "` prefix → final line `"Error: message rejected — Unresolvable target: target 'wg-1-devs/dev-rust' is ambiguous; candidates: proj-a:wg-1-devs/dev-rust, proj-b:wg-1-devs/dev-rust"`. Readable. User knows exactly how to fix. **Verdict: ergonomic.**

**Hazard C — ordering.** Tech-lead asked to verify (a) anti-spoof is still first and (b) downstream sees canonicalized form consistently. Confirmed: §AR2-norm explicitly inserts between lines 268 and 271, which is between anti-spoof and token-validation. Downstream of that point — token validation at 271, `can_reach` at 366, action dispatch at 384, `deliver_wake` at 414 — all use `msg.from` / `msg.to` directly (no cached copies). In-place mutation means they automatically see canonical forms. The sort / filter in `find_active_session` (§AR2-G2) expects FQN; post-canonicalization they always get FQN. **Verdict: ordering is sound.**

**Hazard D — legacy fallback when `expected_from` unavailable.** Two cases:
- **`is_app_outbox = true`** (message in app-private outbox; master/root token writer): anti-spoof skipped → `expected_from` is undefined. §AR2-norm step (1) cannot canonicalize. `msg.from` stays as-is. Downstream: `can_communicate` lenient (OK), `is_coordinator` strict (unqualified `from` returns false — OK, blocks coordinator paths). Non-coordinator paths continue to route. **Acceptable degradation.**
- **Outbox path structure too shallow** (edge case — `outbox_dir.parent().parent()` returns None): anti-spoof skipped → `expected_from` undefined. Same behavior as `is_app_outbox`.

**Verdict: degradation is graceful.** §AR2-norm step (1) needs to tolerate the "no `expected_from` available" case. See §DR2-3 for the implementation hazard this creates.

### §DR2-3 — Implementation hazard: `expected_from` scope in `process_message`

**Non-blocking, but worth calling out so dev-rust doesn't miss it.** In current code (`mailbox.rs:252-269`), `expected_from` is declared at line 256 inside an `if let Some(repo_path) = outbox_dir.parent().and_then(...)` block that closes at line 268. §AR2-norm step (1)'s code block references `expected_from` as if it's in scope at line 271+ — but that scope has already ended.

**Required restructure** (for round-2 implementor, not plan text):

```rust
let mut expected_from: Option<String> = None;
if !is_app_outbox {
    let outbox_dir = path.parent().unwrap_or(Path::new(""));
    if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
        let derived = self.agent_fqn_from_path(&repo_path.to_string_lossy());
        // anti-spoof with §DR5 lenient local-match fallback
        let accept = if derived == msg.from {
            true
        } else {
            let (_, exp_local) = split_project_prefix(&derived);
            let (msg_proj, msg_local) = split_project_prefix(&msg.from);
            msg_proj.is_none() && exp_local == msg_local
        };
        if !accept {
            return self.reject_message(path, &msg, &format!(
                "Outbox-sender mismatch: outbox belongs to '{}' but message claims '{}'",
                derived, msg.from
            )).await;
        }
        expected_from = Some(derived);
    }
}

// §AR2-norm step (1): canonicalize msg.from if unqualified and expected_from is FQN.
if let Some(ref exp) = expected_from {
    if let (None, _) = split_project_prefix(&msg.from) {
        if let (Some(_), _) = split_project_prefix(exp) {
            log::info!(
                "[mailbox] canonicalized legacy msg.from '{}' → '{}'",
                msg.from, exp
            );
            msg.from = exp.clone();
        }
    }
}
```

Naive copy-paste of the plan's §AR2-norm code block will fail to compile because `expected_from` is not in scope. Dev-rust should hoist (as above) or re-derive (less clean). Hoist preferred.

### §DR2-4 — Implementation hazard: composing §AR2-G3 and §AR2-G4

**Non-blocking, but subtle.** §AR2-G3's proposed code block ends with:

```rust
for dir in dirs_to_check {
    let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
    if candidate.is_dir() {
        return Some(candidate.to_string_lossy().to_string());
    }
}
```

That `return Some(...)` makes the **WG-fallback loop** first-match-wins within a single project. §AR2-G4 requires the OUTER `resolve_repo_path` to return `None` on unqualified multi-match across projects. The two are NOT contradictory — §AR2-G3 returns on first hit WITHIN a given `rp`, while §AR2-G4's collector accumulates across all `rp` iterations — but a naive implementation may literal-paste §AR2-G3 inside §AR2-G4's collector and lose the accumulation.

**Correct composition:**

```rust
// Inside §AR2-G4's collector, for the WG-fallback "loop 4":
for rp in &cfg.project_paths {
    // ... §AR2-G3's corrected dirs_to_check construction ...
    for dir in dirs_to_check {
        let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
        if candidate.is_dir() {
            let c_str = candidate.to_string_lossy().to_string();
            if !matches.iter().any(|m| *m == c_str) {
                matches.push(c_str);
            }
            // Do NOT return; continue to ensure cross-project ambiguity is detected.
            break; // break INNER loop (dirs_to_check) — within a single rp, first hit is the unique hit
        }
    }
}
// Fall through to the match matches.len() { 0 | 1 | _ } decision in §AR2-G4.
```

Two key swaps: `return Some(...)` → `matches.push(...); break;`. The `break` stops the inner per-`rp` scan after finding one hit (correct — an FQN can only match one replica dir per project), while the outer loop continues across all `rp` so cross-project ambiguity is detected. Dev-rust should cross-check this when implementing.

### §DR2-5 — Test coverage: add explicit `msg.from` canonicalization regression

§AR2-tests covers §AR2-norm step (2) via test #18 (`deliver_wake_rejects_unqualified_to_with_cross_project_matches`) and step (1) anti-spoof acceptance via test #22 (`anti_spoof_legacy_msg_from_accepted_by_local_match`). But NO test explicitly verifies that msg.from is UPGRADED to FQN after §AR2-norm step (1) runs.

**Recommended addition (test #25):** `process_message_canonicalizes_legacy_msg_from`. Fixture: repo outbox with `msg.from = "wg-1-devs/tech-lead"` (unqualified), repo path implies `expected_from = "proj-a:wg-1-devs/tech-lead"`. After processing, assert the delivered (or rejected) JSON records `msg.from = "proj-a:wg-1-devs/tech-lead"`. Without this, a future refactor can drop the upgrade step silently.

This closes grinch §G5's implicit coverage (resolve_repo_path(&msg.from) gets canonical input) — the test makes the canonicalization contract explicit, not incidental.

Total: 25 tests (was 24).

### §DR2-6 — Non-critical observation: master/root token now forced to qualified `msg.to`

**Worth surfacing, not a concern.** §AR2-norm runs unconditionally — it does NOT check `is_master` / `is_app_outbox` before canonicalizing `msg.to`. For messages written via master/root token to the app outbox, if `msg.to` is ambiguous across projects, the message is rejected with the Ambiguous error.

This is a **BEHAVIOR CHANGE from current code**: today master/root can write unqualified targets and the mailbox will route to one arbitrary project. Post-fix, master/root must qualify ambiguous targets.

I endorse the stricter behavior — it matches Decision 2's "zero silent misrouting" principle and catches ops-user errors loudly. But it deserves a ONE-LINE CALLOUT in §AR2-non-goals or §AR2-norm itself so ops users migrating existing scripts are forewarned. Not round-3-worthy, just documentation.

### §DR2-7 — Line/code feasibility re-check

Spot-checked all new §AR2 code blocks against HEAD of `fix/issue-70-project-scoped-names`:

| §AR2 block | Plan claim | Actual | Drift |
|---|---|---|---|
| §AR2-norm insertion point | "immediately after anti-spoof (lines 252-269), before token validation at line 271" | blank line at 270, token block starts at 271 | ✅ (note: `expected_from` scope caveat in §DR2-3) |
| §AR2-G1 close-session insertion | "after `let target = ...` at lines 993-996, before `is_master` at line 999" | target at 993-996, blank 997, is_master at 999 | ✅ exact |
| §AR2-G2 find_active_session filter | "exact-match FQN" | filter at 918-926 | ✅ simplification replaces 3-condition `||` chain |
| §AR2-G3 WG fallback | "seed dirs_to_check behind project filter" | current fallback at 1274-1299 | ✅ drop-in replacement |
| §AR2-G4 collector | "all three loops + WG fallback collect into `matches`" | loops at 1236-1244, 1249-1257, 1261-1272, 1274-1299 | ✅ structural; see §DR2-4 for composition caveat |
| §AR2-G6 rposition | "§3.2 one-line edit" | §3.2 plan text at plan line 175 | ✅ |
| §AR2-session-name strip | "mailbox.rs:508" | `let session_name = msg.to.clone();` at 508 | ✅ exact |
| §AR2-strict is_coordinator | "teams.rs:173-189" | function body at 173-189 | ✅ exact |

**No drift.** Round-2 can be implemented as specified, modulo the two implementation-hazard callouts above.

### §DR2-8 — Calls on §AR2-open #1 and #2

**§AR2-open #1 — `ResolutionError::Ambiguous` display format.**

**My call: inline, comma-separated.** Reasoning:
- Primary expected case is 2-3 candidates. Inline stays readable (`"candidates: proj-a:wg-1-devs/dev-rust, proj-b:wg-1-devs/dev-rust"` ≈ 70 chars).
- CLI error output already goes to a single `eprintln!` line — the CLI's `send` shows the reason via `"Error: message rejected — {reason}"`. Multi-line errors inside a one-line print render with literal `\n` which looks worse than inline commas.
- Log lines and telemetry consume `Display` format — inline is grep-friendly.
- If a pathological case appears with >10 candidates, we have much bigger routing problems than error aesthetics.

Match architect's proposal.

**§AR2-open #2 — mutate `msg` in place vs thread `canonical_from` / `canonical_to` aliases.**

**My call: mutate in place.** Reasoning:
- Audit-trail: archived `delivered/` / `rejected/` JSON records the canonical form that the routing decision was made on. This is the correct historical record.
- Threading aliases cascades into signatures for `can_reach`, `is_coordinator_of`, `find_active_session`, `find_all_sessions`, `resolve_repo_path`, `handle_close_session`, `deliver_wake`, `inject_into_pty`, logging — ~10 signatures. Plus every test fixture. Heavy refactor for a style preference.
- Rust convention against input mutation is real but not absolute — `&mut self` methods that mutate state are idiomatic. `let mut msg` in a function scope is fine.
- Convention: mark §AR2-norm with a block comment `// SINGLE POINT OF TRUTH: msg.from / msg.to canonicalization. Downstream code does NOT re-mutate msg.` so a future reader doesn't add a second mutation site.

Match architect's proposal.

### §DR2-Verdict — Consensus

**Ready for grinch round 2.** All §DR and §G items absorbed. §AR2-norm stress test passes. Implementation hazards noted in §DR2-3 and §DR2-4 are dev-rust-facing implementation clarity — they will be handled at the keyboard, not at the design table. §DR2-5 adds one test (25 total). §DR2-6 is a behavior-change callout for docs.

I cannot see a genuinely NEW attack vector or design flaw in round 2. If grinch finds one, loop back to architect for round 3. Otherwise this plan is implementable and I am prepared to execute §AR2-order as-is (with §DR2-3/§DR2-4 clarifications applied during step 8).

### End of dev-rust review (round 2)

---

## Dev-rust-grinch review (round 2)

**Reviewer:** wg-5-dev-team/dev-rust-grinch
**Date:** 2026-04-22
**Scope:** re-attack §G1-§G5 through §AR2; probe §AR2-norm surface for new holes; endorse/pushback on open points.
**Mandate:** no re-review of round-1 decisions. Narrow focus.

### Executive summary (round 2)

**Verdict: APPROVED.** I re-ran every round-1 attack through §AR2's code paths and each one fails at a specific, named enforcement point. §AR2-norm's ordering, mutation semantics, idempotency, and degradation paths hold up under stress. Dev-rust's §DR2-3 (`expected_from` scope) and §DR2-4 (§AR2-G3/§AR2-G4 composition) are correct implementation catches. No new attack vector. No architectural gap. One tiny shape-validation nit (§G2-7) that is not round-3-worthy.

I endorse both §AR2-open decisions as dev-rust proposed.

### §G2-1 — §G1 re-attack: CLOSED by §AR2-G1

**Attack.** Coordinator of proj-B writes outbox JSON directly in `<proj-B>/.agentscommander/outbox/<id>.json`:

```json
{ "from": "proj-b:wg-1-devs/tech-lead", "action": "close-session", "target": "wg-1-devs/dev-rust" }
```

**Trace.**

1. `process_message` reads JSON, parses to `msg: OutboxMessage`.
2. Anti-spoof (§4.5, lines 252-269): `is_app_outbox=false`. `expected_from = agent_fqn_from_path(<proj-B>) = "proj-b:wg-1-devs/tech-lead"`. Matches `msg.from`. Pass. (§DR2-3's hoist makes `expected_from` live in outer scope, `Some("proj-b:...")`.)
3. §AR2-norm (1): `msg.from` is qualified. `split_project_prefix(&msg.from).0 == Some("proj-b")`. Inner `if` fails → skip upgrade. Correct.
4. §AR2-norm (2): `msg.to` is empty (close-session messages have no `to`). `if !msg.to.is_empty()` short-circuits. Skip.
5. Token validation passes via session token.
6. Action dispatch → `handle_close_session`.
7. §AR2-G1 insertion: `resolve_agent_target("wg-1-devs/dev-rust", project_paths)`:
   - target has no `:` → unqualified → two-level scan → finds replicas in BOTH proj-A and proj-B.
   - `matches.len() == 2` → `Err(ResolutionError::Ambiguous { target: "wg-1-devs/dev-rust", candidates: ["proj-a:wg-1-devs/dev-rust", "proj-b:wg-1-devs/dev-rust"] })`.
8. `reject_message(path, msg, "close-session target unresolvable: target 'wg-1-devs/dev-rust' is ambiguous; candidates: proj-a:wg-1-devs/dev-rust, proj-b:wg-1-devs/dev-rust")`.
9. Outbox file moved to `rejected/`; reason written; no sessions touched.

**proj-A's dev-rust is safe.** §G1 closed. Note: the fix is at the enforcement boundary — if §DR1's CLI-side fix is bypassed (direct outbox write, as in this attack), §AR2-G1 still catches it. Belt-and-braces achieved.

**Unambiguous variant** (only proj-B has `dev-rust`): `resolve_agent_target` returns `Ok("proj-b:wg-1-devs/dev-rust")`. `is_coordinator_of("proj-b:wg-1-devs/tech-lead", "proj-b:wg-1-devs/dev-rust", teams)` — strict is_coordinator, same project, suffix matches coord_name → true. Lenient is_in_team on qualified target, same project → true. Authorized. `find_all_sessions("proj-b:wg-1-devs/dev-rust")` with §AR2-G2's exact-FQN filter matches only proj-B sessions. Correct close.

### §G2-2 — §G2 re-attack: CLOSED by §AR2-norm (2) + §AR2-G2

**Attack.** Sender writes outbox with `from = "proj-a:wg-1-devs/tech-lead"`, `to = "wg-1-devs/dev-rust"` (unqualified, legacy or buggy), `mode = "wake"`. Both projects have `wg-1-devs/dev-rust` sessions.

**Trace.**

1. Anti-spoof passes (msg.from exact-matches expected_from).
2. §AR2-norm (1): msg.from qualified → skip.
3. §AR2-norm (2): `msg.to = "wg-1-devs/dev-rust"` non-empty. `resolve_agent_target`:
   - Unqualified → two-level scan → 2 candidates.
   - `Err(Ambiguous)`.
4. `reject_message("Unresolvable target: target 'wg-1-devs/dev-rust' is ambiguous; candidates: ...")`.
5. No `deliver_wake`. No silent cross-project delivery. No `find_active_session` called with unqualified input.

**§G2 closed.** Note the critical change: §AR2-G2's simplified filter (`agent_fqn_from_path(cwd) == agent_name`) is only safe **because** §AR2-norm ran first. The two are tightly coupled — if a future refactor splits them or skips §AR2-norm for some path, §G2 re-opens. Test #18 (`deliver_wake_rejects_unqualified_to_with_cross_project_matches`) is the regression anchor. Dev-rust should resist any temptation to "optimize" by skipping §AR2-norm when `msg.to` looks qualified at CLI time — the canonicalization step is the enforcement boundary.

### §G2-3 — §G3 re-attack: CLOSED by §AR2-G3

**Attack.** `settings.project_paths = ["C:/repos/proj-a"]` (project root, not parent). Target = `proj-b:wg-1-devs/dev-rust`. Old bug: `dirs_to_check` seeded with base → proj-a scanned → if any WG-1-devs/dev-rust replica somehow lives at `C:/repos/proj-a/.ac-new/wg-1-devs/__agent_dev-rust/`, it'd be returned as if it were proj-b's. Impossible in practice (proj-a's replica resolves to `proj-a:...`), but the filter should refuse anyway on name mismatch.

**Trace through §AR2-G3.**

1. `split_project_prefix("proj-b:wg-1-devs/dev-rust")` → `target_project = Some("proj-b")`, `local = "wg-1-devs/dev-rust"`.
2. `local.starts_with("wg-")` → true. `local.split_once('/')?` → `("wg-1-devs", "dev-rust")`.
3. Outer loop: `rp = "C:/repos/proj-a"`. `base = Path::new(...)`. `base_name = "proj-a"`.
4. `project_matches("proj-a")` with `target_project = Some("proj-b")` → `"proj-a" == "proj-b"` → **false**.
5. `dirs_to_check` starts empty (seed skipped).
6. `read_dir(base)` lists proj-a's children (code dirs, `.ac-new`, etc.). For each child, `project_matches(dir_name)`: only `dir_name == "proj-b"` passes. None match (proj-a's children don't include a `proj-b` subdir by the attack premise). `dirs_to_check` stays empty.
7. Inner `for dir in dirs_to_check` never executes. Falls through.
8. Function returns `None`.

**§G3 closed.** Base seeding now honors the project filter. The `else` branch on `read_dir` failure in grinch's original snippet was dead code; architect's cleaner shape (conditional seed via `project_matches(base_name)`) has no dead paths.

### §G2-4 — §G4 re-attack: CLOSED by §AR2-G4 (pending §DR2-4's correct implementation)

**Attack.** Unqualified target `"wg-1-devs/dev-rust"` reaches `resolve_repo_path` directly (e.g., a caller bypassing §AR2-norm — though none exist today). Both proj-A and proj-B have matching replicas.

**Trace through §AR2-G4's collector.**

1. `is_qualified = false`.
2. Loop 1 (session CWDs): each CWD's FQN's local part compared to `"wg-1-devs/dev-rust"`. Sessions in both projects match → both paths accumulated in `matches`.
3. Loops 2-3 (project_paths / team agent_paths): similar accumulation if matches exist there.
4. Loop 4 (WG fallback with §AR2-G3 + §DR2-4 correct composition): scan proj-A and proj-B both (target_project=None means no project filter); both have `.ac-new/wg-1-devs/__agent_dev-rust/` → both paths pushed.
5. `matches.len() > 1` → log-warn → return `None`.

**§G4 closed.** But: §DR2-4's implementation caveat is load-bearing. If dev-rust naively copy-pastes §AR2-G3's `return Some(...)` inside §AR2-G4's collector loop, §G4 re-opens (first-match wins; no accumulation). Dev-rust's `matches.push(...); break;` swap is correct. I endorse §DR2-4 as a **mandatory implementation note**, not a nit.

### §G2-5 — §G5 re-attack: CLOSED by §AR2-norm (1)

**Attack.** Legit sender writes outbox with legacy `from = "wg-1-devs/tech-lead"` (unqualified), outbox under `<proj-A>/.agentscommander/outbox/`. Any action. Response-dir derivation calls `resolve_repo_path(&msg.from)`.

**Trace.**

1. Anti-spoof derives `expected_from = "proj-a:wg-1-devs/tech-lead"`. Lenient fallback (§DR5) accepts unqualified `msg.from` via local-part match. `expected_from = Some(...)`.
2. §AR2-norm (1): `msg.from` unqualified (`split_project_prefix(&msg.from).0 == None`). `expected_from` qualified. **Upgrade `msg.from = "proj-a:wg-1-devs/tech-lead"`.** Log-info emitted.
3. Any downstream caller that derives the sender's repo_path (`handle_close_session` line 1085, logging, archive writes) now passes the qualified form to `resolve_repo_path`. Exact match → proj-A's path. Correct.

**§G5 closed.** Responses go to the right project. Logs record canonical form. Archive (delivered/rejected JSON) records canonical form — audit trail is clean.

### §G2-6 — §AR2-norm ordering: no spoofing vector

**Probe.** Tech-lead's concern: anti-spoof uses `msg.from` AS-IS, then §AR2-norm upgrades `msg.from` to `expected_from`. Can an attacker exploit the temporal gap to stamp a legitimate identity over a spoofed one?

**Analysis.**

- Attacker writes `msg.from = "wg-1-devs/tech-lead"` (unqualified, "borrow any project's tech-lead role") to their own outbox at `<proj-B>/.agentscommander/outbox/`.
- Anti-spoof computes `expected_from = "proj-b:wg-1-devs/tech-lead"` from the outbox PATH. Lenient fallback: `msg.from` is unqualified AND `exp_local == msg_local` ("wg-1-devs/tech-lead" == "wg-1-devs/tech-lead") → accept.
- §AR2-norm (1) upgrades `msg.from` to `"proj-b:wg-1-devs/tech-lead"`. This is the **correct identity** of the outbox owner (derived from the physical outbox path). The attacker IS proj-B's tech-lead (they wrote to proj-B's outbox).
- Authorization uses the upgraded form. Attacker gets authority OF proj-B's tech-lead — which is who they actually are.

**No privilege escalation.** §AR2-norm upgrades to the authoritative identity the anti-spoof already validated. Attacker gains no authority they shouldn't have.

**Cross-case probe.** What if attacker writes `msg.from = "proj-a:wg-1-devs/tech-lead"` (QUALIFIED but claiming proj-A, while actually writing to proj-B's outbox)?

- Anti-spoof: `expected_from = "proj-b:..."`. `msg.from != expected_from`. Lenient fallback requires `msg_proj.is_none()` — but `msg_proj = Some("proj-a")` — fallback fails. **Reject.**
- Attacker cannot spoof a different project's identity even if they qualify it.

Ordering is sound. No attack vector.

### §G2-7 — Nit: `resolve_agent_target` FQN shape validation

**Observation.** The plan's §AR2-shared says qualified inputs (`contains ':'`) are "validated, returned as-is." The `ResolutionError::InvalidShape` variant exists for "not a valid agent name shape" but the doc-comment is ambiguous about what "validation" entails for qualified inputs.

**Concrete hole** (low-severity). If dev-rust implements "contains ':' → return as-is without further validation," multi-colon inputs like `"foo:bar:wg-1/x"` pass through. Downstream consumers then receive malformed FQNs:

- `find_active_session` filter `agent_fqn_from_path(cwd) == "foo:bar:wg-1/x"` — no session's FQN has two colons (output format is single-colon). No match. Passes through to spawn.
- `resolve_repo_path` WG fallback: `split_project_prefix("foo:bar:wg-1/x") = (Some("foo"), "bar:wg-1/x")`. `local.starts_with("wg-")` — "bar:..." doesn't start with "wg-". Returns None.
- `resolve_wg_path_from_sessions`: same — `local.split_once('/')?` yields `("bar:wg-1", "x")`, `wg_name` doesn't start with `wg-`. None.
- Deliver_wake fails with "cannot resolve repo path" error, not an informative "InvalidShape" error.

**Severity: nit.** Not security, not correctness (message rejects, doesn't misroute). UX only — user sees a confusing "cannot resolve" message when the real issue is "your target has too many colons."

**Suggested strengthening** (optional, for dev-rust to choose): in `resolve_agent_target`, when `target.contains(':')`, validate:
- Exactly one `:` (reject multi-colon as `InvalidShape`).
- Left side non-empty.
- Right side matches `wg-N-<team>/<agent>` shape (at least one `/`, left of `/` starts with `wg-` followed by digit-hyphen-team).

If shape fails → `InvalidShape`. If shape passes but no replica dir exists on disk → `UnknownQualified`. This differentiation was the original point of §G8 — make it explicit for all four error arms.

Not round-3-worthy. Flag for dev-rust's implementation pass.

### §G2-8 — Future handler invariant

**Observation.** §AR2-G1 fixes `handle_close_session` specifically. The action dispatch at `mailbox.rs:384-395` currently supports only `"close-session"`. Any future action handler added here must follow the same pattern: resolve `msg.target` (and any other user-supplied name fields) via `resolve_agent_target` BEFORE any privileged operation.

**Suggested mitigation.** Add a block comment near the action dispatch documenting this invariant, and consider enforcing it structurally — e.g., a helper `resolve_action_target(msg) -> Result<String, ...>` that wraps the resolve + reject chain, and every new action handler takes the resolved target as an argument rather than reading `msg.target` directly.

Not round-3-worthy. Design hygiene for the next feature, not a bug today.

### §G2-9 — Idempotency confirmed

**Probe.** A message is retried (delivery mid-way crash). Is §AR2-norm idempotent?

**Trace.**

- Retry path: `process_message` re-reads the outbox JSON from disk. If the original JSON was unqualified, msg.from/msg.to start unqualified again. §AR2-norm canonicalizes. Same result each time.
- If §AR2-norm rejected with Ambiguous on first try, `reject_message` removed the outbox file (mailbox.rs:1492 `std::fs::remove_file(path)`). No retry. Correct.
- If §AR2-norm succeeded and `deliver_wake` later failed, the file stays in outbox/. Next poll re-reads, re-canonicalizes, re-tries. Idempotent.
- `retry_tracker` (mailbox.rs:52) keyed by path; path is stable until move_to_delivered or reject_message. OK.

**Idempotent.** No rot, no double-process.

### §G2-10 — Mutation visibility and archive correctness

**Probe.** Dev-rust §DR2-2 Hazard A says "reject_message/move_to_delivered clone msg before serialization, archive records post-canonicalization form." Verified at mailbox.rs:1450 and 1483 (`let mut stripped = msg.clone(); stripped.token = None;`). Both archive paths preserve canonical `from`/`to`. Audit trail is clean.

**No separate conversation-writer concern detected.** `PhoneMessage.from/to` in the conversation-history layer is a distinct type populated by different code paths; the wire-protocol canonicalization is scoped to the mailbox/outbox flow.

### §G2-11 — Endorsements

**§AR2-open #1 (Ambiguous display format, inline comma-separated):** **ENDORSE.** Dev-rust's reasoning (single-line eprintln!, grep-friendly logs, ≤3 candidates in practice) is correct. Multi-line renders as literal `\n` in the CLI's `eprintln!` — ugly.

**§AR2-open #2 (mutate `msg` in place):** **ENDORSE.** Threading aliases cascades into ~10 signatures + tests for zero correctness benefit. `let mut msg` in a function scope is idiomatic Rust. The archive recording canonical form is the correct audit behavior. Dev-rust's suggested block-comment `// SINGLE POINT OF TRUTH: msg.from / msg.to canonicalization. Downstream code does NOT re-mutate msg.` is a good guardrail against future drift — include it.

**§DR2-3 (`expected_from` scope hoist):** correct, mandatory implementation detail. Without it, the plan doesn't compile. Dev-rust will handle at the keyboard.

**§DR2-4 (§AR2-G3 + §AR2-G4 composition):** correct, **mandatory** implementation detail. Without the `matches.push; break;` swap, §G4 regresses silently. Flag this as a must-apply, not a nit.

**§DR2-5 (test #25 `process_message_canonicalizes_legacy_msg_from`):** endorse. Locks the §AR2-norm (1) contract. Without it, a future refactor can drop the upgrade step without breaking existing tests. **Recommend marking this test [CRIT].** Total: 25 tests.

**§DR2-6 (master/root behavior change):** endorse. One-line addition to §AR2-non-goals or a callout in `CLAUDE.md` / release notes. Ops users with existing unqualified `--outbox` scripts need awareness.

### §G2-12 — Round-2 verdict

**APPROVED.**

- §G1-§G5 all closed at named §AR2-* enforcement points. I traced each attack through the post-fix code and it fails where architect says it fails.
- §AR2-norm is well-ordered, idempotent, and mutation-safe. No new attack vector.
- Dev-rust's §DR2-3/§DR2-4 are correct implementation catches. §DR2-5 test is essential. §DR2-6 deserves a callout but isn't blocking.
- One implementation nit (§G2-7 multi-colon validation) and one architectural-invariant note (§G2-8 future handlers) — neither warrants round 3.

**Test count final: 25 (24 from §AR2-tests + §DR2-5).** Critical regression tests (marked [CRIT]) in `config/teams.rs`, `resolve_agent_target`, and `phone/mailbox.rs` are the non-negotiable floor — without them, any of the closed §G1-§G5 attacks can re-open on future refactor.

**Implementation-mandatory notes for dev-rust** (not plan changes; captured for the keyboard):

1. Apply §DR2-3 `expected_from` hoist — non-negotiable, compile-blocker.
2. Apply §DR2-4 composition (`matches.push; break;` inside §AR2-G4 collector for WG fallback) — non-negotiable, §G4 regression blocker.
3. Add `// SINGLE POINT OF TRUTH` comment at §AR2-norm per §AR2-open #2 endorsement.
4. Optional but recommended: tighten `resolve_agent_target` shape validation per §G2-7 (one `:` max, local form validated).
5. Optional: add action-dispatch comment per §G2-8.

Ready for implementation. I do not see grounds for round 3.

### End of dev-rust-grinch review (round 2)
