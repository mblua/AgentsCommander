# 1757 - Codex "Hooks need review" blocking menu

Status: READY_FOR_IMPLEMENTATION
Issue: https://github.com/mblua/AgentsCommander/issues/1757
Branch: `feat/1757-codex-hooks-blocking-menu`
Base: `f5bb4f6fc55e1bc4f5eb67f8b35a571685a2ebc2` (`main` at branch point)
Owner: Rust. Single phase. `PARTITION: 1 phase`.

Every line number in this plan is pinned to the base SHA above and describes the tree BEFORE this
plan's edits. Anchors are symbol names; line numbers are navigation aids only.

## Task class and accepted threat model

Routine application-code change with an append-only persistence migration. No release, signing,
packaging, untrusted build host, or security-boundary change. Baseline gates apply. Enhanced
provenance controls (independently anchored executable hashes, DLL closure inventories,
poisoned-PATH tests, SDK binary manifests) are NOT applicable and are not required. The
"destructive or irreversible migration" enhanced-control trigger does not fire either: the migration
appends one array element, removes no key and rewrites no existing value.

One enhanced control IS applied, and its hazard is named. Hazard: the shipped regex is derived from
a screenshot and from upstream source, not from raw mirror rows, so a unit test written against
invented row text would pass whether or not the pattern matches the real dialog. Requirement that
makes it applicable: the issue's Verification-difficulty veto, which puts the proof itself under
review. Why the baseline is insufficient: the repository's three existing `MenuGuard` tests all type
their own row text, so following the local precedent would reproduce exactly the defect the veto
targets. The control is section 7's replay plus deliberately-broken pattern, and it is
proportionate: about forty lines of test code, no new harness, no new dependency.

## 1. Requirement

AgentsCommander's menu guard does not detect the Codex "Hooks need review" startup dialog. A Codex
session sits waiting for a keypress and nothing marks it blocked. Observed 2026-09-04 on
AgentsCommander v0.30.3, Windows 11. The dialog, as transcribed in the issue:

```
Hooks need review
1 hook is new or changed.
Hooks can run outside the sandbox after you trust them.

> 1. Review hooks
  2. Trust all and continue
  3. Continue without trusting (hooks won't run)

Press enter to confirm or esc to go back
```

The guard must detect it on new installs AND on installs that already exist.

## 2. Evidence

Verified at the base SHA. E1 to E12 restate the dispatch's evidence. E13 to E17 are added here.

E1. `default_blocking_menus_for_command` (`src-tauri/src/config/settings.rs:978`) returns, for the
`codex` stem, exactly one entry: pattern `^\s*Do you trust the contents of this directory\?`,
notification `codex is waiting for you to answer the folder-trust menu in this terminal`,
`enabled: true`, `captured_against: Some("codex 0.x / Linux")`. `pi` gets one folder-trust entry;
every other stem gets an empty vec. The stem comes from
`crate::config::coding_agents_catalog::command_executable_basename`
(`src-tauri/src/config/coding_agents_catalog.rs:626`, returns `Option<String>`).

E2. `MenuGuard::evaluate_logical_rows` (`src-tauri/src/pty/menu_guard/mod.rs:88`) iterates only the
configured entries, compiles each `pattern` with the `regex` crate, and matches with
`re.is_match(&row.text)` per logical row; first match wins and breaks. An unconfigured dialog cannot
be detected at all.

E3. Rows are logical rows from `logical_rows` (`src-tauri/src/pty/watchers/frame.rs:56`): wrapped
physical rows are concatenated with no separator, there is no trailing padding, and a wrapped line
starting at physical row 0 is skipped. Text is ANSI-stripped mirror text.

E4. `MenuGuard::scan_tick` (`menu_guard/mod.rs:202`) runs on a 250 ms interval (`:350`), reads only
frames whose stamp changed, and resolves entries as
`settings.agents.iter().find(|a| a.id == session.agent_id).and_then(|a| a.blocking_menus.clone()).unwrap_or_default()`.

E5. `menu_guard_enabled` (declared `settings.rs:637`) defaults to `true` in `AppSettings::default()`
(`settings.rs:972`) and is not implicated.

E6. `materialize_blocking_menus` (`settings.rs:1002`) fills `blocking_menus` only when it is `None`.
It is called from `load_settings_from_path` (`:2059`, inside the `[settings-migration]` block,
setting `needs_save = true`), and from the two read-only CLI loaders `load_settings_for_cli`
(`:2211`) and `load_settings_for_cli_strict` (`:2282`), which apply it in memory only and must not
write, per the documented section 463 contract.

E7. Core compatibility problem. Every existing install already has a materialized `blockingMenus`
array on the Codex agent, so it is `Some([folder-trust])`, not `None`. A change confined to
`default_blocking_menus_for_command` reaches new installs only, and no existing user, including the
reporter, gets the new pattern.

E8. `test_blocking_menus_defaults_materialization` (`settings.rs:9011`) asserts
`codex_menus.len() == 1` and pins that entry's four fields. Adding a default breaks it.

E9. `test_blocking_menus_tolerant_parsing` (`settings.rs:9099`) pins that an explicit empty array is
never overwritten by materialization. It and `test_blocking_menus_explicit_empty_array`
(`settings.rs:9139`) both use the `pi` stem, so neither is affected by this plan.

E10. `MenuGuard` unit tests live at `menu_guard/mod.rs:365-489`. All three build `LogicalRow.text`
from string literals typed by their author. None of them observes a real terminal.

E11. The `settings.local.json` overlay (#1737, `src-tauri/src/config/local_overlay.rs`) is merged
before the migration block runs. `merge_value` (`local_overlay.rs:476-489`) recurses only into
objects, so arrays replace the base array whole. `collect` (`local_overlay.rs:495-511`) therefore
pushes the single owned path `["agents"]` for an overlay-supplied `agents` array, which makes
`owns_top_level("agents")` (`local_overlay.rs:236-240`) true, and `restore_base`
(`local_overlay.rs:403-420`) writes the base file's `agents` array back into `settings.json` on every
save. An operator's escape hatch through `settings.local.json` therefore requires supplying the WHOLE
`agents` array; it cannot override one agent's `blockingMenus` alone. That is coarser than a reader
may expect. It is stated here as a fact; this plan does not design around it.

E12. There is no settings file watcher and no UI or CLI surface for `blockingMenus`. A hand edit
needs the app closed and restarted. A search for `blockingMenus` under `src/` returns zero hits at
the base SHA. Documentation is #1758 and is out of scope.

E13 (added). `agents` is NOT in `OVERLAY_INELIGIBLE_DISK_KEYS` (`settings.rs:2409-2418`) and NOT in
`OVERLAY_INELIGIBLE_LEGACY_KEYS` (`settings.rs:2424-2429`), so an overlay MAY own it. It is the
`source_key` of the single `DerivedIdClosure` (`settings.rs:2438-2442`).

E14 (added). The D7c precedent for suppressing a post-merge migration whose destination the overlay
owns is `settings.local_overlay_state.owns_top_level(OVERLAY_KEY_*)`, applied at `settings.rs:2016`,
`:2029`, `:2039` (GUI loader), `:2180`, `:2189`, `:2197` (`load_settings_for_cli`), `:2257`, `:2266`,
`:2274` (`load_settings_for_cli_strict`), and inside `apply_issue_248_migration` at `:2312`. The
destination keys are listed in `OVERLAY_MIGRATION_DESTINATION_KEYS` (`settings.rs:2455-2460`), whose
own doc comment says the array "is what S29 pins and what a future author greps". S29 is
`the_migration_destination_table_is_pinned_to_serialized_field_names` (`settings.rs:10264`).

E15 (added, and NOT in the dispatch's list). There is a byte-pinned golden that carries the codex
agent's materialized `blockingMenus` array: `EXPECTED_NON_PROJECT_SETTINGS_JSON`
(`settings.rs:9404-9556`), produced by `s6_normalized_non_project_settings` (`settings.rs:9558`) from
`S6_FIXTURE_JSON` (`settings.rs:9361`) by driving the real `load_settings_from_path` plus
`save_settings_to_path_preserving_project_paths`, and asserted by
`a_no_overlay_save_writes_the_control_captured_on_the_pinned_base` (`settings.rs:9585`). The
fixture's codex agent carries no `blockingMenus` key, so it is materialized, and the golden contains
the folder-trust entry in sorted-key pretty JSON. **Adding a Codex default changes this golden.** It
is a third existing test that must be updated deliberately, and it is the only test in the repository
that observes the real load-and-save chain end to end.

E16 (added). Both `agent_json` test helpers (`settings.rs:9755-9763` and
`src-tauri/src/commands/config.rs:5374-5382`) build agents with `"blockingMenus": []`. A migration
rule that never activates an explicitly empty array leaves every overlay fixture in both files
untouched. A rule that appends unconditionally would perturb them.

E17 (added, upstream ground truth). The dialog comes from openai/codex PR #21755, "Improve hooks
trust flow in TUI", which adds `codex-rs/tui/src/startup_hooks_review.rs`. In
`selection_view_params` the title line is built as:

```rust
header.push(Line::from("Hooks need review".bold()));
```

a bare `Line` with no prefix span, followed by the count line and the dim sandbox line, with items
`"Review hooks"`, `"Trust all and continue"` and `"Continue without trusting (hooks won't run)"`.
`SelectionViewParams` is rendered by `ListSelectionView`, which applies no additional x-offset to the
header; the surrounding menu surface is inset by up to 2 columns of spaces, and the selection marker
is U+203A (single right-pointing angle quotation mark), not the ASCII `>` the issue transcribes. The
issue's block is therefore a transcription, not raw mirror text. `codex --version` on the observation
host on 2026-09-04, measured after the report was filed, prints `codex-cli 0.153.2`.

## 3. Decisions

Ratified by the user and not open here:

D1. The delivery includes a one-shot migration that brings the new pattern to already-materialized
Codex `blockingMenus` arrays. A code-default-only change was explicitly rejected.

D2. A user who deliberately deleted the entry is not a case to protect. No tombstones, no deletion
memory, no suppression list. The stated escape hatch is the local settings file, whose coarseness is
recorded at E11.

D3. Documentation is out of scope (#1758).

Decided here:

D4. **One pattern literal, two consumers.** A `const CODEX_HOOKS_REVIEW_PATTERN` and a private
`codex_hooks_review_menu()` constructor are the single source of the new entry. Both
`default_blocking_menus_for_command` (new installs) and the migration (existing installs) call the
constructor. Neither writes the pattern inline, so the two paths cannot drift.

D5. **The pattern is `^[^A-Za-z0-9]*Hooks need review\b`.** Adopted from the tech lead's proposal.
`^` anchors to the start of the logical row. `[^A-Za-z0-9]*` absorbs the menu-surface inset and any
box-drawing gutter that `^\s*` would not; E17 shows Codex renders spaces rather than a border today,
so the class is insurance, not an observed requirement. `\b` prevents a match inside
`Hooks need reviewers`. There is no end anchor, so a right-hand border cannot defeat it.

D6. **The notification follows the existing English convention**:
`codex is waiting for you to answer the hooks-review menu in this terminal`. This is the folder-trust
string with `folder-trust` replaced by `hooks-review`.

D7. **`capturedAgainst` is `codex 0.153.2 / Windows`.** The format matches the two existing rows
(`pi 0.52 / Windows`, `codex 0.x / Linux`). 0.153.2 is what `codex --version` printed on the
observation host on 2026-09-04 (E17). The implementer MUST re-run `codex --version` on that host
before writing the value and reconcile. If it still prints 0.153.2, write the string as given. If it
prints a different version, that means the install moved after the observation, so keep 0.153.2, because
the field records the build the dialog was observed against and not the build present at
implementation time, and note the discrepancy in the PR body. A placeholder such as `0.x` is NOT
acceptable.

D8. **The migration extends an active blocking-menu set; it never activates an inactive one.** The
precondition is `blocking_menus == Some(list)` with `!list.is_empty()`. `None` is left to
`materialize_blocking_menus`, which now supplies both defaults. `Some(vec![])` is the field's
documented "explicitly disabled" state (`settings.rs:88-90`) and is left alone. This is not a
tombstone, a deletion memory or a suppression list: it is an existing, documented, user-visible state
of the field, and honoring it is what keeps the change's blast radius to one golden and one unit test
(E16).

D9. **Idempotence is content-keyed, with no new persisted key.** The migration appends only when no
`BlockingMenuEntry::Valid` entry in the array already carries `CODEX_HOOKS_REVIEW_PATTERN`. Once it
has run, the pattern is present, so it never runs again, across restarts, without a marker field, a
schema-version bump, or any change to the settings schema. The presence test ignores `enabled`, which
gives the user a durable, non-destructive off switch: setting `enabled: false` on the entry keeps the
pattern present, so the migration will not re-add it. Deleting the entry outright does bring it back,
which is exactly the case D2 declines to protect.

D10. **`BlockingMenuEntry::Invalid` entries do not count as present.** An `Invalid` entry is arbitrary
JSON that `evaluate_logical_rows` already skips, so treating it as "the pattern is configured" would
disable the feature for a user whose file is malformed. Scanning `Invalid` JSON for a `pattern` field
would be speculative work with no caller.

D11. **The migration is one function called from all three loaders**, mirroring
`apply_issue_248_migration`, and it performs its own overlay-ownership check internally. The
alternative, an inline `if` block duplicated three times, is what the loaders do for the 0.8.0
migrations and is the shape whose lockstep the repository has to mandate by comment
(`settings.rs:2139`, `:2226`). A single call line per loader keeps that lockstep mechanical.

D12. **`agents` joins `OVERLAY_MIGRATION_DESTINATION_KEYS`.** After D11 the migration writes `agents`
after the merge, so `agents` becomes a migration destination in the D7c sense and the array's own doc
comment stops being true if it is omitted. S29 is updated in the same change.

D13. **Order.** The new entry is appended AFTER the folder-trust entry in
`default_blocking_menus_for_command`, and the migration appends at the end of the array. Both paths
therefore produce `[folder-trust, hooks-review]`, so a new install and a migrated install are
byte-identical in `settings.json`.

D14. **Call order in the loaders.** The migration runs immediately AFTER `materialize_blocking_menus`
in all three loaders, and that is the whole rule. Running after materialization means a `None` array
is first filled with both defaults and the migration is then a no-op on it, which is why the migration
does not need to handle `None`.

The position relative to `repair_coding_agent_profiles_config` is NOT uniform across the three
loaders, and this plan does not make it uniform. Read the third column as an insertion point
OUTSIDE the enclosing statement: in the GUI loader `:2059-2062` is an `if` block
(`:2061` is `needs_save = true;` INSIDE it and `:2062` is its closing brace), so the new call goes
after `:2062`, at the same nesting level as the `if`. Inserting after `:2061` would put the migration
inside the block, where it runs only when materialization changed something, that is only on installs
whose array was `None`, on which it is a no-op: the entire E7 population would never receive the
pattern. The two CLI rows carry single-statement calls, so their anchors are the call lines
themselves. At the base SHA:

| loader | `materialize_blocking_menus` | `repair_coding_agent_profiles_config` | the new call lands |
|---|---|---|---|
| `load_settings_from_path` | `settings.rs:2059` | `settings.rs:2063` | after the block's closing brace at `:2062`, so BEFORE repair |
| `load_settings_for_cli` | `settings.rs:2211` | `settings.rs:2210` | after `:2211`, so AFTER repair |
| `load_settings_for_cli_strict` | `settings.rs:2282` | `settings.rs:2281` | after `:2282`, so AFTER repair |

Both CLI loaders already run repair BEFORE materialize, so no single line satisfies "after materialize
and before repair" in all three, and any plan text that asks for both halves is unsatisfiable at two of
the three sites. The difference is behaviourally inert: `repair_coding_agent_profiles_config`
(`settings.rs:1418-1480`) reads the `agents` slice only for `agent.id` (`:1468-1477`) and never reads
or writes `blocking_menus`, so the two functions do not interact in either order.

## 4. Design

Two source files change, plus this plan.

### 4.1 `src-tauri/src/config/settings.rs`

**C1. New constant and constructor**, immediately above `default_blocking_menus_for_command`
(`:978`):

```rust
/// #1757 - the Codex "Hooks need review" startup dialog. One literal, two consumers:
/// `default_blocking_menus_for_command` (new installs) and `apply_issue_1757_migration`
/// (installs whose `blockingMenus` array was already materialized against the older default).
/// Neither writes the pattern inline, so the two paths cannot drift.
pub(crate) const CODEX_HOOKS_REVIEW_PATTERN: &str = r"^[^A-Za-z0-9]*Hooks need review\b";

fn codex_hooks_review_menu() -> BlockingMenuConfig {
    BlockingMenuConfig {
        pattern: CODEX_HOOKS_REVIEW_PATTERN.to_string(),
        notification: "codex is waiting for you to answer the hooks-review menu in this terminal"
            .to_string(),
        enabled: true,
        captured_against: Some("codex 0.153.2 / Windows".to_string()),
    }
}
```

**C2. The `codex` arm of `default_blocking_menus_for_command`** becomes a two-element vec: the
existing folder-trust entry unchanged and first, then
`BlockingMenuEntry::Valid(codex_hooks_review_menu())`. The `pi` arm and the `_` arm are unchanged.

**C3. New migration**, immediately after `materialize_blocking_menus` (`:1002-1011`):

```rust
/// #1757 - one-shot migration for installs whose Codex `blockingMenus` array was already
/// materialized against the pre-#1757 default, which is every install created before this
/// change. `materialize_blocking_menus` only fills `None`, so those users would otherwise
/// never receive the hooks-review pattern.
///
/// Keyed by content, not by a marker field: the entry is appended only when no valid entry
/// already carries `CODEX_HOOKS_REVIEW_PATTERN`, so the function is idempotent across
/// restarts and adds no key to the settings schema. The check ignores `enabled`, so setting
/// `enabled: false` on the entry is a durable off switch; deleting it is not (#1757 D2).
///
/// Extends an ACTIVE set, never activates an inactive one: `None` belongs to
/// `materialize_blocking_menus`, and `Some(vec![])` is the field's documented
/// "explicitly disabled" state.
///
/// #1737 (D7c): an overlay that owns `agents` supplies the whole array and `restore_base`
/// writes the base array back on save, so appending here would be discarded on write and
/// would overwrite the operator's in-memory array meanwhile. Owning `agents` therefore
/// suppresses the migration, which then runs correctly the first time the overlay is removed.
///
/// Returns true when any agent's array changed.
pub fn apply_issue_1757_migration(settings: &mut AppSettings) -> bool {
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_AGENTS)
    {
        return false;
    }
    let mut changed = false;
    for agent in &mut settings.agents {
        if crate::config::coding_agents_catalog::command_executable_basename(&agent.command)
            .as_deref()
            != Some("codex")
        {
            continue;
        }
        let Some(entries) = agent.blocking_menus.as_mut() else {
            continue;
        };
        if entries.is_empty() {
            continue;
        }
        if entries.iter().any(|entry| {
            entry
                .valid()
                .is_some_and(|c| c.pattern == CODEX_HOOKS_REVIEW_PATTERN)
        }) {
            continue;
        }
        entries.push(BlockingMenuEntry::Valid(codex_hooks_review_menu()));
        changed = true;
    }
    changed
}
```

**C4. Three call sites**, each immediately after the existing `materialize_blocking_menus` call, per
the D14 table. In the GUI loader that is also before `repair_coding_agent_profiles_config`; in the two
CLI loaders repair has already run by that point, which is correct and intended:

- `load_settings_from_path`, after the `if materialize_blocking_menus(...) { ... }` block at `:2059`:

```rust
    if apply_issue_1757_migration(&mut settings) {
        log::info!("[settings-migration] #1757 - added the Codex hooks-review blocking menu");
        needs_save = true;
    }
```

- `load_settings_for_cli`, after `:2211`: `apply_issue_1757_migration(&mut settings);`
- `load_settings_for_cli_strict`, after `:2282`: `apply_issue_1757_migration(&mut settings);`

Both CLI loaders discard the return value and perform no write, exactly as they already do for
`materialize_blocking_menus` and `repair_coding_agent_profiles_config`. The section 463 contract is
preserved: neither loader calls any save function, before or after this change.

**C5. Overlay key constant, and the two comments that stop being true when it is added.**

Insert the new constant as the FIRST of the `OVERLAY_KEY_*` group (`:2450-2453`), immediately below
the doc block that introduces the group and above `OVERLAY_KEY_MAIN_ALWAYS_ON_TOP`, so the group stays
alphabetical:

```rust
pub(crate) const OVERLAY_KEY_AGENTS: &str = "agents";
```

and insert `OVERLAY_KEY_AGENTS` as the FIRST element of `OVERLAY_MIGRATION_DESTINATION_KEYS`
(`:2455-2460`), keeping the array's existing alphabetical order.

Two neighbouring comments assert facts this insertion falsifies. The doc block at `:2444-2449` says
every destination key "is written from a legacy source key on the typed struct AFTER the merge", which
is not true of `agents`, and says "The four suppression sites name these constants", which stops being
four. The `#[allow(dead_code)]` note at `:2454` repeats "the four constants individually", and there
will be five. D12's whole argument is that this array's own doc comment stops being true if `agents`
is omitted, so leaving these false would contradict the decision that motivates the change. Both are
rewritten in the same commit.

Replace the doc block at `:2444-2449` with exactly:

```rust
/// #1737 (D7c) - migration destination keys. Each names a top-level key that a
/// migration WRITES after the merge, so a migration whose destination the overlay
/// owns would silently overwrite the override in memory. Owning the destination
/// suppresses the migration (plan D7c, evidence 2.11b). Four of the five are
/// written from a legacy source key on the typed struct; `agents` is the
/// exception, rewritten in place by `apply_issue_1757_migration` (#1757 D12).
/// Every suppression names one of these constants; the array is what S29 pins
/// and what a future author greps.
```

and replace the single line at `:2454` with exactly:

```rust
#[allow(dead_code)] // read only from test code: the suppressions name the five constants individually
```

Both replacements are comments. `cargo fmt` does not rewrap comments in this repository's
configuration, which the base SHA proves: the existing `:2454` line is 107 columns and passes
`cargo fmt --all -- --check`; the replacement is 102. Neither block contains a Markdown list item, so
`clippy::doc_lazy_continuation` cannot fire on the continuation lines.

**C6. Doc-comment update** on `AgentConfig::blocking_menus`: the two existing doc lines are `:87-88`
and the field itself is `:90`. Keep both sentences and append a third: `Some(vec![])` is also
what stops a future default from being back-filled by a one-shot migration; per-entry
`enabled: false` is the way to disable one pattern while keeping the array active.

### 4.2 `src-tauri/src/pty/menu_guard/mod.rs`

Tests only. No production line in this file changes. See sections 7 and 8.

One consequence, because T11 needs a type this file does not import. The production `use` at `:17` is
`use crate::pty::watchers::{FrameStamp, ScreenRowsSince};` and carries no `ScreenFrame`, which 7.2
step 3 constructs. Add `use crate::pty::watchers::ScreenFrame;` INSIDE `#[cfg(test)] mod tests`
(`:365-366`, which already opens `use super::*;` at `:367`), never to the `:17` line. Widening `:17`
rewrites a production line and AC2's `grep -c "^-[^-]"` would print 1 instead of 0.

### 4.3 What does NOT change

No new settings key, no schema-version bump, no serde rename, no `AppSettings` field, no TypeScript,
no Tauri command, no IPC event, no CLI verb, no workflow, no dependency, no `Cargo.toml`, no
`Cargo.lock`, no `package.json`, no `package-lock.json`, no `src-tauri/module-arcs.txt`, and no
production line in `pty/menu_guard/mod.rs`, `pty/watchers/frame.rs`, `pty/output.rs` or
`config/local_overlay.rs`.

## 5. Compatibility matrix

The state of one agent's `blockingMenus` before the first launch that carries this change, and what
that launch produces. "Overlay owns `agents`" means `settings.local.json` supplies an `agents` array.

| # | stem | before | overlay owns `agents` | after, in memory | written to `settings.json` |
|---|---|---|---|---|---|
| 1 | codex | key absent (`None`) | no | `[folder-trust, hooks-review]` | yes, by `materialize_blocking_menus` |
| 2 | codex | `[folder-trust]` | no | `[folder-trust, hooks-review]` | yes, by this migration |
| 3 | codex | `[folder-trust, hooks-review]` | no | unchanged | no (migration returns false) |
| 4 | codex | `[hooks-review with enabled:false]` | no | unchanged | no (pattern present) |
| 5 | codex | `[]` | no | unchanged | no (D8) |
| 6 | codex | `[user pattern]` | no | `[user pattern, hooks-review]` | yes |
| 7 | codex | `[12345]`, Invalid only | no | `[12345, hooks-review]` | yes (D10) |
| 8 | codex | anything | yes | unchanged | no; base `agents` restored by `restore_base` |
| 9 | pi, claude, other | anything | either | unchanged | no |
| 10 | codex | `[Invalid entry whose raw JSON happens to contain the pattern text]` | no | `[invalid, hooks-review]` | yes (D10) |
| 11 | codex | `[a user pattern that matches the same rows, spelled differently]` | no | `[user pattern, hooks-review]` | yes |

Row 3 is the second and every later launch, which is what makes the migration one-shot. Row 8 defers
the migration to the first launch without the overlay, exactly as `apply_issue_248_migration` defers.
Row 9 holds because the stem gate uses the same `command_executable_basename` that
`default_blocking_menus_for_command` uses, so the migration can never target an agent the default
would not.

Rows 10 and 11 are the two shapes D9's presence test deliberately does not collapse, and both are
correct. Row 10 follows directly from D10: an `Invalid` entry is arbitrary JSON that
`evaluate_logical_rows` already skips, so it is not "the pattern is configured" no matter what text it
happens to contain, and the user gets one working entry appended. Row 11 leaves the user with two
entries that match the same rows, which is harmless because `evaluate_logical_rows` breaks on the
first match; D9 keys on textual equality of a `Valid` entry's `pattern` and makes no attempt at
semantic equivalence, which would be speculative work with no caller.

Both CLI loaders reach the same in-memory result as the GUI loader and write nothing; the next GUI
launch finalizes rows 1, 2, 6 and 7 to disk.

Cost disclosure: rows 2, 6 and 7 fire `needs_save` on exactly one launch and produce exactly one
`[settings-migration]` log line on that launch. Rows 3, 4, 5, 8 and 9 add zero saves and zero log
lines. This does not change the pre-existing per-launch save behavior driven by
`repair_coding_agent_profiles_config`.

## 6. Dependency cycles and layering

New module-to-module arcs introduced by this plan: **zero**.

- `config::settings -> config::coding_agents_catalog` already exists (`src-tauri/module-arcs.txt:672`)
  and is what `default_blocking_menus_for_command` already uses for the stem.
- `config::settings -> config::local_overlay` already exists (`module-arcs.txt:674`) and is what the
  ten existing `owns_top_level` call sites in `settings.rs` already use (`:2016`, `:2029`, `:2039`,
  `:2180`, `:2189`, `:2197`, `:2257`, `:2266`, `:2274`, `:2312`).
- `pty::menu_guard -> config::settings` (`:884`), `-> pty::watchers` (`:886`) and
  `-> pty::watchers::frame` (`:887`) already exist; the new test uses only those three.
- `vt100` appears zero times in `module-arcs.txt`, because external crates are not module arcs, so
  building a `vt100::Parser` in a test adds nothing to the graph. A parser is already built in library
  code at `src-tauri/src/telegram/bridge.rs:710`.

Per-arc verdict: there are no new arcs to classify, so no arc can be internal to or cross a
pre-existing SCC boundary.

Layering: no lower layer gains a UI transport. `apply_issue_1757_migration` takes `&mut AppSettings`
and no `AppHandle`, `tauri::` type or transport, exactly like `apply_issue_248_migration`. The
persistence layer stays free of the UI surface. No function moves between layers.

The measurement that must confirm this is AC10.

## 7. Verification

The category veto on verification difficulty applies: the proof itself will be reviewed, not only the
plan. This section states what each piece of evidence can and cannot establish.

### 7.1 What a unit test over invented rows cannot prove

Every existing `MenuGuard` test (E10) types its own `LogicalRow.text`. Such a test passes whether or
not the pattern matches the real dialog, because the author writes both sides of the comparison. The
claim under test therefore has to be split:

- **Claim A, pattern semantics.** Given a row whose text is `T`, the pattern matches iff `T` is the
  dialog's title row. Testable with no Codex.
- **Claim B, row fidelity.** The dialog's title row, as it reaches `evaluate_logical_rows`, really is
  a string of that shape. Not testable by writing the string down.

### 7.2 The three grounds for Claim B, and who owns each

**G1, upstream literal. Documentary, no gate, MANDATORY.** E17 quotes
`header.push(Line::from("Hooks need review".bold()));` from
`codex-rs/tui/src/startup_hooks_review.rs`, a bare `Line` with no prefix span and no `Block` or
border, rendered by `ListSelectionView` with no header x-offset inside a surface inset by up to 2
columns of spaces. The implementer re-reads that file at implementation time, confirms the literal is
byte-identical to the one inside `CODEX_HOOKS_REVIEW_PATTERN`, and quotes the line plus its URL and
the commit or tag it was read at in the PR body. Expected result: exact string match on
`Hooks need review`. Failure behavior: if upstream has renamed the literal, STOP and report; do not
adjust the pattern without a new decision, because the version in `capturedAgainst` and the literal
must describe the same build.

**G2, AC mirror fidelity. Executable, no gate, MANDATORY.** A test in `menu_guard/mod.rs` proves that
AC's own transform, SGR stripping, trailing-blank trimming and the wrap join, does not defeat the
pattern. It drives a real `vt100::Parser` rather than typing row text:

1. `let mut parser = vt100::Parser::new(30, 120, 0);`
2. Feed this exact fixture. It is a literal, declared as a `const` beside the test in the same
   `#[cfg(test)]` module, so no construction rule is left for the implementer to resolve. `\u{203a}`
   is the U+203A selection marker of E17; `\x1b[1m`, `\x1b[33m` and `\x1b[2m` are bold, yellow and
   dim; every content row is inset by two columns and every row ends `\r\n`:

```rust
const CODEX_HOOKS_REVIEW_FRAME: &str = concat!(
    "  \x1b[1mHooks need review\x1b[0m\r\n",
    "  \x1b[33m1 hook is new or changed.\x1b[0m\r\n",
    "  \x1b[2mHooks can run outside the sandbox after you trust them.\x1b[0m\r\n",
    "\r\n",
    "  \u{203a} 1. Review hooks\r\n",
    "    2. Trust all and continue\r\n",
    "    3. Continue without trusting (hooks won't run)\r\n",
    "\r\n",
    "  \x1b[2mPress enter to confirm or esc to go back\x1b[0m\r\n",
);
```

   then `parser.process(CODEX_HOOKS_REVIEW_FRAME.as_bytes());`. The title is the first row fed, so it
   lands at physical row 0; `logical_rows` skips a WRAPPED line that starts at row 0 and this row is
   not wrapped, so it survives.
3. Build the frame exactly as `PtyManager::get_screen_rows_since` does
   (`src-tauri/src/pty/output.rs:1734-1745`):
   `let screen = parser.screen();`
   `let rows: Vec<String> = screen.rows(0, 120).collect();`
   `let wrapped: Vec<bool> = (0..rows.len() as u16).map(|r| screen.row_wrapped(r)).collect();`
   then `ScreenFrame { rows, wrapped, cursor_row: screen.cursor_position().0, stamp: None }`.
4. `let rows = logical_rows(&frame);`
5. `guard.evaluate_logical_rows(id, &rows, &default_blocking_menus_for_command("codex"))` must report
   `is_blocked` and `matched_notification.as_deref() == Some("codex is waiting for you to answer the hooks-review menu in this terminal")`.
   The `.as_deref()` is required to compile: the field is `Option<String>` (`menu_guard/mod.rs:41`),
   so it cannot be compared to an `Option<&str>` directly. This is the assertion the 7.3 control guards.

What G2 proves: the two-column inset, the trailing-blank trim and the wrap join leave a row the
pattern matches, and it proves it through the product's own frame-building and row-joining code rather
than through a hand-written string.

Honest accounting of the SGR half of that claim, so nobody credits it with more than it carries.
`vt100::Screen::rows` yields row text with no formatting, so every SGR choice in the fixture collapses
to the same string and "the SGR attributes do not defeat the pattern" is close to tautological. The
escape sequences stay in the fixture because a real capture carries them and a fixture without them
would not resemble one, not because they are load-bearing. Likewise the U+203A markers sit only on
item rows, which the pattern is never asserted against. What is load-bearing is the two-column inset,
the trailing-blank trim and the wrap join, and that is what the 7.3 control discriminates.

What G2 does not prove: that Codex emits this exact byte sequence. That is G1's job.

**G3, live capture. Gated, tech-lead owned, NOT required for this plan to be implementable.** The
strongest evidence is the real `LogicalRow.text`. Obtaining it needs all of:

- a host with `codex` installed at the version written into `capturedAgainst`;
- a Codex configuration in which at least one hook is new or changed, so the startup review prompt
  actually fires. That configuration key belongs to Codex, not to AgentsCommander; the operator takes
  it from the Codex documentation or `codex --help` for that version. This plan does not guess it;
- the Codex session running inside an AgentsCommander session, so the mirror is populated;
- capture through the shipped path:
  `agentscommander terminal-snapshot --token <live-token> --root <verified-root> --to <fqn> --format json`
  (`src-tauri/src/cli/terminal_snapshot.rs`), whose payload carries `lines[].wrapped` and
  `lines[].cells[].text`.

Reconstruction caveat, so nobody reports a false match: the snapshot serializes one cell per column,
and a blank cell's `text` is the empty string, whereas `Screen::rows(0, cols)` renders interior blanks
as spaces and trims trailing ones. To compare a snapshot against `LogicalRow.text` the operator must
map each narrow cell's empty `text` to one space, drop every `WideContinuation` cell, right-trim the
row, then join across `wrapped`. That reconstruction is a derivation and is therefore weaker than G2's
replay, which is why G2 and not G3 is the mandatory control.

If G3 is granted, the captured title row is added verbatim to the G2 test's positive corpus and
`capturedAgainst` is set from the captured session's `codex --version`. If G3 is not granted, this
plan ships on G1 plus G2, and section 13 records exactly what stays unproven.

### 7.3 The deliberately-broken control, executed and not merely described

The G2 test also evaluates the SAME frame against a deliberately-broken variant of the shipped
pattern and asserts it does NOT match:

```rust
let broken = vec![BlockingMenuEntry::Valid(BlockingMenuConfig {
    pattern: r"^Hooks need review$".to_string(),
    notification: "control".to_string(),
    enabled: true,
    captured_against: None,
})];
let control = guard.evaluate_logical_rows(Uuid::new_v4(), &rows, &broken);
assert!(
    !control.is_blocked,
    "control pattern matched: the replayed rows are neither inset nor decorated, so the shipped \
     pattern's tolerance for a gutter is untested and the main assertion proves nothing"
);
```

`^Hooks need review$` is the pattern a careless author writes. It matches a bare, unindented row and
fails the moment there is an inset or a right-hand decoration, so it discriminates exactly the
property the shipped pattern claims. If the control ever starts passing, the fixture has drifted to a
bare row and the main assertion has stopped being evidence. The control lives in the same test
function as the assertion it guards, so it cannot be deleted separately.

AC7 exercises the second half of the same idea from the other direction: it mutates the shipped
constant and requires the test to go red.

### 7.4 Negative corpus, Claim A

The same test asserts the shipped codex entries do NOT mark blocked for any of these rows, evaluated
on a fresh session id so no earlier episode masks the result:

- `1 hook is new or changed.`
- `Hooks can run outside the sandbox after you trust them.`
- `Press enter to confirm or esc to go back`
- `The docs explain why Hooks need review is shown.` (mid-line prose)
- `Hooks need reviewers` (the `\b` case)
- `hooks need review` (case)

## 8. Tests

Existing tests to update. Three, all deliberate.

T1. `test_blocking_menus_defaults_materialization` (`settings.rs:9011`). Codex now has two entries.
Keep the existing four assertions on entry `[0]` verbatim and add four on entry `[1]` pinning
`CODEX_HOOKS_REVIEW_PATTERN`, the hooks-review notification, `enabled == true`, and
`captured_against == Some("codex 0.153.2 / Windows")`. Change `codex_menus.len()` from 1 to 2. The pi
and Claude assertions are untouched, as is the final "subsequent call returns false" assertion.

T2. `EXPECTED_NON_PROJECT_SETTINGS_JSON` (`settings.rs:9404`). The codex agent's `blockingMenus`
array is `:9413-9420`. Two edits inside it, given as bytes because this is the one byte-pinned
artifact the change touches.

First, the existing entry's closing brace at `:9419` gains the separating comma: `        }` becomes
`        },`.

Second, insert exactly these six lines immediately after it, before the `      ],` at `:9420`:

```
        {
          "capturedAgainst": "codex 0.153.2 / Windows",
          "enabled": true,
          "notification": "codex is waiting for you to answer the hooks-review menu in this terminal",
          "pattern": "^[^A-Za-z0-9]*Hooks need review\\b"
        }
```

That block is the literal file content, not an escaped rendering of it. The golden is a raw Rust
string (`r##"..."##` at `:9404`), so the bytes above go into the source verbatim.

Every backslash of the shipped pattern is DOUBLED in the golden. The golden is JSON TEXT emitted by
`serde_json::to_string_pretty` (`s6_normalized_non_project_settings`, `settings.rs:9558-9581`) and
compared by string equality, never parsed: `a_no_overlay_save_writes_the_control_captured_on_the_pinned_base`
(`:9585-9590`) is a plain `assert_eq!` against the raw string. `serde_json` writes one backslash out as
two, so the Rust constant `r"^[^A-Za-z0-9]*Hooks need review\b"`, which holds ONE backslash byte,
serializes to `"^[^A-Za-z0-9]*Hooks need review\\b"`, which carries TWO. Writing a single `\b` in
the golden would pin the U+0008 backspace escape instead of the pattern and fail AC6.

The sibling entry doubles both of its own escapes for exactly this reason, and it is the measurement
to copy: its Rust source at `:989` is `pattern: r"^\s*Do you trust the contents of this directory\?"`
with 2 backslash bytes, and the golden line at `:9418` is
`          "pattern": "^\\s*Do you trust the contents of this directory\\?"` with 4. Count the
backslash bytes on the line you write before moving on; the shipped line above must carry exactly 2.

Keys are in the sorted order the pretty printer emits (`capturedAgainst`, `enabled`, `notification`,
`pattern`); the object brace sits at column 8 and the fields at column 10, matching the sibling. No
other byte of the golden changes, and a wrong guess here fails AC6.

T3. S29 `the_migration_destination_table_is_pinned_to_serialized_field_names` (`settings.rs:10264`).
Add `"agents"` as the first element of the pinned literal array and add the matching
`assert_eq!(OVERLAY_MIGRATION_DESTINATION_KEYS[0], OVERLAY_KEY_AGENTS);`, shifting the four existing
index assertions by one. The test's closing loop, which asserts every key is a serialized
`AppSettings` field, already passes for `agents`: `pub agents: Vec<AgentConfig>` (`settings.rs:317`)
has no `skip_serializing_if`, so it is always present in `serde_json::to_value(&AppSettings::default())`.

New tests in `settings.rs`. T4 to T9 go in the file's top-level test module, `#[cfg(test)] mod tests`
(`settings.rs:4967-4968`), which already hosts T1 (`:9011`) and provides `tempfile` (used at `:9559`
among others). T10 goes in the nested `local_overlay_1737` module named in its own entry. No new
module is created.

T4. `issue_1757_appends_to_an_already_materialized_codex_array`: an `AppSettings` with a codex agent
whose `blocking_menus` is `Some([folder-trust])`; assert the call returns true, the array has two
entries, entry `[0]` still carries the four folder-trust field values, and entry `[1]` matches the
four hooks-review field values.

T5. `issue_1757_is_idempotent`: call twice; the second call returns false and the array still has two
entries.

T6. `issue_1757_never_activates_an_explicitly_empty_array`: `Some(vec![])` stays `Some(vec![])` and
the call returns false.

T7. `issue_1757_respects_a_disabled_hooks_entry`: an array holding only the hooks-review entry with
`enabled: false` is left alone and the call returns false.

T8. `issue_1757_ignores_non_codex_agents`: a `pi` agent and a `claude` agent, both with non-empty
arrays, are unchanged and the call returns false.

T9. `issue_1757_reaches_settings_json_through_the_real_load_chain`: the only new test that observes
the real chain. Write this exact fixture as `settings.json` into a `tempfile::tempdir()`, call
`load_settings_from_path(&path)`, then read the file back off disk:

```json
{
  "defaultShell": "test-shell",
  "defaultShellArgs": [],
  "rootToken": "issue-1757-fixture-token",
  "agents": [
    {
      "id": "codex",
      "label": "Codex",
      "command": "codex",
      "color": "#000000",
      "blockingMenus": [
        {
          "pattern": "^\\s*Do you trust the contents of this directory\\?",
          "notification": "codex is waiting for you to answer the folder-trust menu in this terminal",
          "enabled": true,
          "capturedAgainst": "codex 0.x / Linux"
        }
      ]
    }
  ],
  "codingAgentProfiles": {
    "schemaVersion": 2,
    "profileSlots": { "A": { "label": "" } },
    "defaultProfileByAgent": {},
    "profilesByAgent": {
      "codex": { "A": { "enabled": true, "command": "", "env": {}, "notes": "" } }
    }
  }
}
```

Those are the FILE's bytes, and the file is parsed by `serde_json`, so `\s` and `\?` are spelled
`\\s` and `\\?`: neither `\s` nor `\?` is a legal JSON escape (RFC 8259), and a fixture spelled with
single backslashes does not parse. It does not fail loudly either: `parse_settings_json` returns
`Err` (`settings.rs:1102-1103`), `load_settings_from_path` logs and falls back to
`default_settings_with_overlay` (`:1992-1995`), and `AppSettings::default()` carries `agents: vec![]`
(`:896`), so the load would silently produce zero agents and the test would exercise neither the
migration nor the `needs_save` half of C4's GUI call site. Declare the fixture as a RAW Rust string
(`r##"..."##`) so the doubling survives into the file, and `serde_json::from_str::<Value>` it once in
the test before writing, or write it and assert the load found one agent, so a future edit that
breaks the escapes fails on the fixture rather than passing through the fallback.

Assert on the bytes read back from disk: the codex agent's `blockingMenus` array now has two entries,
the second carries `CODEX_HOOKS_REVIEW_PATTERN`, and the first still carries the four folder-trust
field values unchanged.

Why the fixture is pinned instead of minimal. The GUI loader has exactly five sources of `needs_save`
(`settings.rs:2054-2071`), and a test that leaves any of the other four live cannot observe the
`needs_save = true` half of C4's GUI call site: the write would happen anyway, the read-back would
show two entries, and an implementer who omitted the flag would still get a green T9. This fixture
makes the other four inert, so the migration's own flag is the only thing that can drive the write:

| source | why it is inert on this fixture |
|---|---|
| `issue_248_migrated` (`:2054`) | no `startOnlyCoordinators` key, so `legacy_start_only_coordinators` is `None` |
| `profile_migrated_to_v2` (`:1987`, from `:1104`) | `migrate_profiles_object_to_v2` (`:1239-1264`) emits exactly `schemaVersion`, `profileSlots`, `defaultProfileByAgent` and `profilesByAgent`, with exactly these values, so `changed` (`:1215`) is false. `serde_json` is declared without features (`src-tauri/Cargo.toml:11`), so `preserve_order` is off, `Map` is a `BTreeMap`, and the `Value` comparison is key-order independent |
| `materialize_blocking_menus` (`:2059`) | the one agent already carries `blockingMenus`, so nothing is `None` |
| `repair_coding_agent_profiles_config` (`:2063`) | `schema_version` is 2 (`:1424`), `profileSlots` holds the valid letter `A` and nothing else (`:1429-1443`), `defaultProfileByAgent` is empty (`:1445`), every cell letter is valid (`:1451`), `profileLabelsByAgent` is empty (`:1462`), and `profilesByAgent["codex"]` already holds `A` for the only agent (`:1468-1477`), so every `changed` assignment is skipped |
| `root_token` (`:2067`) | `rootToken` is present |

Two fixture rules that follow from that table and must not be relaxed. It MUST carry
`codingAgentProfiles` in the shape above; drop the key and the repair fires, sets the flag on its own,
and the test is green whether or not C4 sets it. It MUST NOT carry `profileLabelsByAgent`;
`migrate_profiles_object_to_v2` does not emit that key, so its presence makes the v2 comparison
unequal, `profile_migrated_to_v2` true, and the same hole reopens from the other side.

The discriminator, stated plainly: delete `needs_save = true;` from C4's GUI call site and the load
performs no write at all, so the file on disk still holds one entry and this test goes RED.

This test is also required because the loaders' migration chain is a sequence of inline statements,
not a callable function: a test that restates the chain over an in-memory `AppSettings` cannot fire
when a future author changes the loader, and only driving `load_settings_from_path` over a fixture
file observes it.

T10. `an_overlay_owned_agents_array_suppresses_the_1757_migration`, in the `local_overlay_1737` test
module (`settings.rs:9193`), which already provides `seed` (`:9211`), `base_fixture` (`:9202`) and
`disk_object` (`:9226`) and imports the settings module with `use super::super::*` (`:9194`).

It does NOT use that module's `agent_json` helper (`:9755-9763`). That helper hardcodes
`"blockingMenus": []` and has no parameter for it, so an overlay codex agent built from it carries an
EMPTY array; `merge_value` replaces the array whole, the in-memory codex agent ends up with
`Some(vec![])`, and D8's `entries.is_empty()` early return then stops the migration whether or not the
overlay guard is present. A T10 built on `agent_json` is green in both worlds and proves nothing. The
fixtures are therefore written out here in full.

Two local helpers, declared in the test module:

```rust
fn folder_trust_entry() -> Value {
    json!({
        "pattern": r"^\s*Do you trust the contents of this directory\?",
        "notification": "codex is waiting for you to answer the folder-trust menu in this terminal",
        "enabled": true,
        "capturedAgainst": "codex 0.x / Linux",
    })
}

fn codex_agent_with(blocking_menus: Value) -> Value {
    json!({
        "id": "codex",
        "label": "Codex",
        "command": "codex",
        "color": "#000000",
        "blockingMenus": blocking_menus,
    })
}
```

Base `settings.json`: `base_fixture()` with

```rust
base["agents"] = json!([codex_agent_with(json!([folder_trust_entry()]))]);
```

Overlay `settings.local.json`:

```rust
json!({"agents": [codex_agent_with(json!([folder_trust_entry()]))]})
```

The overlay's codex agent MUST carry that NON-EMPTY one-entry array. That is the entire discriminating
power of the test: `[folder-trust]` is precisely the shape the migration would act on if the guard
were absent, and `[]` is not.

Assert `settings.local_overlay_state.owns_top_level(OVERLAY_KEY_AGENTS)` first, so a fixture that
stopped producing an overlay-owned `agents` array fails loudly instead of passing vacuously. Then
assert all four counts, reading the in-memory side from
`settings.agents.iter().find(|a| a.id == "codex").unwrap().blocking_menus` and the on-disk side from
`disk_object(&path)["agents"][0]["blockingMenus"]`:

| moment | in-memory entries | on-disk entries |
|---|---|---|
| after `load_settings_from_path(&path)` | 1 | 1 |
| after `save_settings_to_path_preserving_project_paths(&settings, &path).unwrap()` | 1 | 1 |

**Deleting the `owns_top_level(OVERLAY_KEY_AGENTS)` early return from C3 makes the in-memory number 2
and this test RED.** Without the guard the migration reaches an array that is `Some([folder-trust])`:
non-empty, so D8 does not stop it, and carrying no `CODEX_HOOKS_REVIEW_PATTERN`, so D9 does not stop
it. It appends, and the first row's in-memory cell becomes 2.

That in-memory number survives the loader's own save, which is what makes the guard observable through
`load_settings_from_path` at all. `base_fixture()` carries no `codingAgentProfiles`, so the repair
fires, `needs_save` is true and the loader saves before returning. Inside
`save_settings_value_locked`, `effective` captures the serialized IN-MEMORY object while an overlay is
in force (`settings.rs:4068-4072`); `restore_base` (`:4077`) then puts the BASE `agents` array into
what is written to disk; and `reapply_from` (`:4088-4090`) puts the effective array back into the
value the caller adopts (`:4096`, adopted by the loader at `:2097`). So a guardless build returns 2 in
memory while disk holds 1, and the two rows above separate them.

The on-disk number cannot do that job and is asserted for the other direction. `restore_base` writes
the captured base array back on every save, so the on-disk count is 1 with and without the guard. A
test asserting only the on-disk half would pass against a migration that corrupts the operator's
in-memory array; a test asserting only the in-memory half would not observe that the base file stays
clean. Both halves are needed, and only the in-memory half discriminates the guard.

New test in `pty/menu_guard/mod.rs`:

T11. `codex_hooks_review_matches_the_mirror_of_a_styled_inset_dialog`: the G2 replay of section 7.2,
carrying the 7.3 broken-pattern control and the 7.4 negative corpus in the same test function.

Test count delta: `src-tauri/src/config/settings.rs` +7 (T4 to T10),
`src-tauri/src/pty/menu_guard/mod.rs` +1 (T11), total +8. Three existing tests are updated in place;
none is deleted or renamed.

## 9. Delivery nonfunctional gates

**G-1, CI-to-plan parity.** Source of truth: `.github/workflows/`, tree OID
`1e8f12e81254df5b214beedade125dcb8b8a7bc8` at the base SHA. Both changed source files are under
`src-tauri/src/`, so `pr-regression-gates.yml` applies. Four jobs in it are relevant:

- `rust-regression` (windows-latest, job header `:46`) runs `cargo check --all-targets` (`:84`),
  `cargo clippy --workspace --all-targets -- -D warnings` (`:88`) and
  `cargo test --lib --bins --tests` (`:92`). It is the only leg that can SELECT a `src-tauri` lib
  test, so it is the only leg that will execute any test in section 8.
- `rust-fmt` (ubuntu-latest, job header `:250`) runs `cargo fmt --all -- --check` in `src-tauri`
  (`:263-265`). Every added or edited Rust line must already be rustfmt-clean.
- `rust-regression-linux` (`:94`) runs `cargo test --lib "$TEST" -- --exact` on one unrelated test
  name (`:161`), and `rust-regression-macos` (`:208`) type-checks only. Both COMPILE the new tests
  and execute none of them.
- the four `terminal-snapshot-portable` legs (`:267`) select other packages with `-p` and are
  unaffected.

Every test in section 8 must therefore be platform-neutral, and all of them are: no `cfg(unix)`, no
filesystem-permission manipulation, no process spawn, no network. Accepted debt: none. Acceptance is
every triggered and configured-required check green for the exact PR-head SHA. Owner: CI, at PR time.
Failure behavior: a red required check blocks delivery, and evidence from any other SHA does not
satisfy this gate.

**G-2, deterministic toolchain and build.** The repository pins `vt100 = "=0.15.2"`
(`src-tauri/Cargo.toml:29`) and resolves through the committed `Cargo.lock`; this plan adds no
dependency and touches neither file. Local commands run with `working-directory: src-tauri`, matching
the workflow. Record `cargo --version` and `rustc --version` in the PR body for reproducibility. No
enhanced provenance control applies (see "Task class"). Owner: implementer, before pushing.

**G-3, authorized traceable Git.** Issue #1757 is open; the branch
`feat/1757-codex-hooks-blocking-menu` already exists from `main` at the base SHA. All state-changing
Git runs inside `room-19-ac-dev-team-v4/repo-AgentsCommander`. Delivery is by PR; no direct push to
`main`. Preconditions before the first edit: `git rev-parse --abbrev-ref HEAD` is the branch above,
`git rev-parse HEAD` is the base SHA or a descendant of it on that branch, and `git status --porcelain`
is empty. Failure behavior: a dirty, unknown or unauthorized base blocks the edit. Bounded
target-branch drift: fetch `main` before the first edit and again before opening the PR, and classify
the drift by changed paths. Drift touching `src-tauri/src/config/settings.rs`,
`src-tauri/src/pty/menu_guard/mod.rs`, `src-tauri/src/pty/watchers/frame.rs`,
`src-tauri/src/pty/output.rs`, `src-tauri/src/config/local_overlay.rs`, `src-tauri/Cargo.toml`,
`Cargo.lock` or `.github/workflows/**` requires re-running the smallest affected evidence. Drift
elsewhere is recorded and does not reopen this design. Owner: implementer.

**G-4, process state, configuration and working directory.** Every `cargo` command uses an explicit
`src-tauri` working directory. Record `CARGO_TARGET_DIR` and `RUSTFLAGS` if either is set in the
shell, and otherwise leave them alone. Build output stays in the gitignored `src-tauri/target`. No
task-created cache, log or download lands in a tracked path. Owner: implementer.

**G-5, validation and scope before acceptance.** The intended path set is exactly:

```
plans/1757-codex-hooks-blocking-menu.md
src-tauri/src/config/settings.rs
src-tauri/src/pty/menu_guard/mod.rs
```

Postcondition: AC1, including its `git add -f` trap, because `/plans/` is ignored (`.gitignore:11`)
and the plan file is otherwise dropped from the commit without a warning. `Cargo.lock`,
`package-lock.json`, `src-tauri/module-arcs.txt` and `.github/**` must not appear in the diff. A
disposable candidate tree is not used: two source files edited directly, with the recovery in G-6, is
clearer and equally safe. Owner: implementer, before opening the PR.

**G-6, mutation ownership and no-clobber recovery.** Immediately before the first edit, re-confirm the
G-3 preconditions. Recovery on failure is `git restore --source=HEAD -- <the exact path>` for only the
paths this work changed, and only while their current content is still this work's output; if a path
has been changed externally, preserve it and report the conflict. A broad `git reset`, an
unconditional `git restore`, or `git clean` is forbidden. AC7 temporarily mutates
`src-tauri/src/config/settings.rs` and must restore it by reverting that single edit by hand if the
file carries any other uncommitted work at that moment. After success or recovery, prove the path set
with AC1. Owner: implementer.

**G-7, bounded execution and durable diagnostics.** `cargo test` runs with the runner's timeout and
without interactive stdin. The full stdout and stderr of the final green run are kept outside
disposable scratch and the lib target's `test result:` line is quoted in the PR body. A timed-out or
failed run is reported as failed; a cleanup defect must not erase the primary failure. Owner:
implementer.

**G-8, evidence discipline.** Zero and absence are asserted as typed states: T5, T6, T7 and T8 each
assert the migration returns FALSE and mutates nothing, and T10 asserts an unchanged count on both the
in-memory and the on-disk side, so "the migration did nothing" is a checked outcome rather than an
unobserved one. Every acceptance command in section 10 states its expected result, and the two
zero-expecting commands in AC2 are paired with a nonzero-expecting command over the same diff so that
a lost stdout cannot be read as a pass. G3 in section 7.2 is the only evidence this plan cannot
produce locally; it is named, its owner is the tech lead, and section 13 records what stays unproven
without it. Owner: implementer and tech lead.

## 10. Acceptance criteria

Run from the repository root of `repo-AgentsCommander` unless a command says `src-tauri`.
`<BASE>` is `f5bb4f6fc55e1bc4f5eb67f8b35a571685a2ebc2`.

**AC1, scope.** `git diff --name-only <BASE>..HEAD` prints exactly these three lines and no others:
`plans/1757-codex-hooks-blocking-menu.md`, `src-tauri/src/config/settings.rs`,
`src-tauri/src/pty/menu_guard/mod.rs`. `git status --porcelain` prints nothing afterwards.

Trap, because it fails silently: `/plans/` is listed in `.gitignore:11`, so this plan file is
INVISIBLE to `git status`, to `git add .` and to `git add -A`. The tracked plan files in `plans/` are
tracked because they predate the rule or were force-added; `plans/1741-probe-ansi-windows-spawn-target.md`
and `plans/1745-running-chips-after-repos.md` were both added on 2026-09-04 this way. Stage this file
explicitly with `git add -f plans/1757-codex-hooks-blocking-menu.md`. Without the `-f` the commit is
made without it, nothing warns, and AC1 prints two lines instead of three.

**AC2, no production change in the guard.** Both commands run over the same diff:

- `git diff -U0 <BASE>..HEAD -- src-tauri/src/pty/menu_guard/mod.rs | grep -c "^-[^-]"` prints `0`.
- `git diff -U0 <BASE>..HEAD -- src-tauri/src/pty/menu_guard/mod.rs | grep -c "^+[^+]"` prints a
  number greater than 40.

The second command is the paired nonzero-expecting check: if it prints `0` or nothing, the diff was
not read and the first command's `0` is meaningless. If the first command prints anything other than
`0`, a line was removed from this file and the change is out of scope.

**AC3, one production literal.** Two commands, both scoped to `src-tauri/src` so that this plan file,
which is tracked, cannot satisfy either:

- `rg -l "Hooks need review" src-tauri/src` prints exactly two paths:
  `src-tauri/src/config/settings.rs` and `src-tauri/src/pty/menu_guard/mod.rs`. Any third path is a
  duplicated literal and fails D4.
- `rg -n "Hooks need review" src-tauri/src` prints a non-empty list. Exactly one printed line also
  contains `pub(crate) const CODEX_HOOKS_REVIEW_PATTERN`: that is the declaration. Exactly one other
  printed line is production, and it is allowed: C1's own doc comment, the `///` line immediately
  above the declaration that opens `/// #1757 - the Codex "Hooks need review" startup dialog.`. A
  clause that required every non-declaring line to sit in a test module could not pass on a correct
  implementation of C1, because C1 prescribes that comment. Open every REMAINING printed line and
  confirm it sits inside a `#[cfg(test)]` module. There are only a handful, and the check is a read,
  not line-number arithmetic, so it survives this plan's own edits shifting the file.

The scope to `src-tauri/src` is belt and braces: `rg` honors `.gitignore`, and `/plans/` is ignored
(`.gitignore:11`), so this plan file would not be searched anyway. Do not rely on that alone, because
a reviewer running `rg --no-ignore` would see it.

**AC4, build, format and lint.** From `src-tauri`, all three exit 0:
`cargo fmt --all -- --check`, `cargo check --all-targets`, and
`cargo clippy --workspace --all-targets -- -D warnings`. If `cargo fmt --all -- --check` fails, run
`cargo fmt --all` and then re-check AC1: the tree was rustfmt-clean at `<BASE>`, so a formatting run
must touch only the two files this change edits. If it touches a third file, revert that file and
format the new code by hand, because a reformat of unrelated code is out of scope.

**AC5, tests.** From `src-tauri`: `cargo test --lib --bins --tests` exits 0. Read the `test result:`
line that follows the `Running unittests src/lib.rs` header, not any other target's line and not a
sum across targets. Its passed count must be exactly 8 higher than the passed count on that same line
from the same command run at `<BASE>`. Record BOTH absolute numbers in the PR body; a delta reported
without its two operands is not evidence.

**AC6, the named tests.** Each command below reports `0 failed` AND a passed count greater than 0. A
passed count of 0 means the filter matched nothing and is a FAILURE, not a pass. From `src-tauri`:

- `cargo test --lib issue_1757`
- `cargo test --lib test_blocking_menus_defaults_materialization`
- `cargo test --lib a_no_overlay_save_writes_the_control_captured_on_the_pinned_base`
- `cargo test --lib the_migration_destination_table_is_pinned_to_serialized_field_names`
- `cargo test --lib codex_hooks_review_matches_the_mirror_of_a_styled_inset_dialog`
- `cargo test --lib an_overlay_owned_agents_array_suppresses_the_1757_migration`

**AC7, the control discriminates.** Temporarily replace `CODEX_HOOKS_REVIEW_PATTERN`'s value with
`r"^Hooks need review$"`, run `cargo test --lib codex_hooks_review_matches_the_mirror_of_a_styled_inset_dialog`
from `src-tauri`, and confirm it FAILS. Restore the constant and confirm the same command passes.
Record both outcomes in the PR body. A test that stays green under this mutation does not pin the
pattern and must be strengthened before the PR is opened. Restore per G-6.

**AC8, upstream literal.** The PR body quotes the current
`codex-rs/tui/src/startup_hooks_review.rs` line that builds the title, with its URL and the commit or
tag it was read at, and states that the quoted literal is byte-identical to the literal inside
`CODEX_HOOKS_REVIEW_PATTERN`.

**AC9, capturedAgainst provenance.** The PR body records the output of `codex --version` on the
observation host and states which branch of D7 was taken.

**AC10, cycle neutrality.** Two runs of the detector, each on a CLEAN tree, the first with the
worktree at `<BASE>` and the second with it at the final branch head. `<OUT>` is a directory OUTSIDE
the repository, so that neither graph becomes an untracked file and breaks AC1.

```
# run 1, worktree at <BASE>
node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph <OUT>/pre.json --quiet

# run 2, worktree at the final branch head
node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph <OUT>/post.json --quiet
node scripts/02-module-arc-record.mjs --graph <OUT>/post.json --out src-tauri/module-arcs.txt
```

where `<VAULT>` is
`D:/0_repos/AgentsCommander_iac/.ac/room-19-ac-dev-team-v4/repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust`.
The third command REWRITES the tracked `src-tauri/module-arcs.txt` in place; byte-identity is proven
by the `git status` clause below, and any other outcome means the arc set moved and this plan's
section 6 is wrong.

Green iff all five hold: `coverage.graphShape.cyclicSccs` is equal pre and post; every cyclic SCC
member set is identical set to set, module by module, not merely equal in count; zero new
`from -> to` pairs cross a previously-clean SCC boundary; the regenerated
`src-tauri/module-arcs.txt` is byte-identical, so `git status --porcelain -- src-tauri/module-arcs.txt`
prints nothing; and the structural guards `loops_layering`, `instance_gitignore_layering` and
`project_settings_layering` stay green under AC5. Exit code 1 from the detector means gating cycles
exist and the graph WAS written, which is the normal outcome in this repository; only exit 3 means no
graph. Never conflate them. Expected result given section 6: zero new arcs, so every clause holds
trivially, and `src-tauri/module-arcs.txt` must not appear in AC1's file list.

**AC11, exact-head CI.** Every triggered and configured-required check on the PR is green for the
exact PR-head SHA. A rerun that erases an earlier failure does not satisfy this; if any check was
rerun, walk its per-attempt history and report what the first attempt did.

## 11. Preserve list

Do not change any of:

- the folder-trust entry's four field values, for `codex` or for `pi`;
- `materialize_blocking_menus`'s "fills only `None`" semantics, or its signature;
- the section 463 contract: neither CLI loader may call any save function;
- `evaluate_logical_rows`, `scan_tick`, the 250 ms interval, the episode suppression and re-arm
  behavior, or any other production line in `pty/menu_guard/mod.rs`;
- `logical_rows` and its row-0 wrapped-line skip;
- `merge_value`, `collect`, `restore_base`, `reapply_from`, or any line of `config/local_overlay.rs`;
- `OVERLAY_INELIGIBLE_DISK_KEYS` and `OVERLAY_INELIGIBLE_LEGACY_KEYS`;
- the `pi` arm and the `_` arm of `default_blocking_menus_for_command`;
- `test_blocking_menus_tolerant_parsing` and `test_blocking_menus_explicit_empty_array`;
- every byte of `EXPECTED_NON_PROJECT_SETTINGS_JSON` other than the inserted entry and the comma the
  insertion adds to the preceding closing brace;
- `Cargo.toml`, `Cargo.lock`, `package.json`, `package-lock.json`, `.github/**`, and
  `src-tauri/module-arcs.txt`.

## 12. Out of scope

- Documentation of `blockingMenus` (#1758).
- Any UI or CLI surface for editing `blockingMenus`.
- A settings file watcher, so that a hand edit takes effect without a restart.
- Multi-row conjunction in the guard. `evaluate_logical_rows` matches one pattern against one row;
  requiring the title AND an option row would change the engine, and this issue does not justify it.
- Patterns for any other Codex dialog, for `pi`, or for any other agent.
- Tombstones, deletion memory, suppression lists, or any new persisted settings key (D2, D9).
- Removing `apply_issue_1757_migration` in a later release (see R6).

## 13. Residuals, stated rather than hidden

R1. The shipped pattern's prefix class is insurance derived from upstream source (E17) and a
screenshot, not from raw mirror rows. Without G3 (section 7.2) nothing proves that the byte sequence
Codex actually emits produces a row the pattern matches; G1 proves the literal and G2 proves that AC's
transform preserves a row of that shape. This is the plan's largest residual and the gate is the tech
lead's to open or leave closed.

R2. A gutter containing a letter or a digit defeats `^[^A-Za-z0-9]*`. Upstream renders no such gutter
today (E17). If a future Codex adds one, the guard silently stops firing, and nothing in AC detects
that.

R3. The pattern fires on any logical row that begins with the phrase, including prose. The MECHANISM
is pre-existing and shared by the two folder-trust patterns, which have the same start-anchored,
unbounded-tail shape: `^\s*Do you trust the contents of this directory\?` behaves identically. What
this change adds is LIKELIHOOD, and that is the honest statement of the residual. `Do you trust the
contents of this directory?` is close to a sentence nobody writes by accident; `Hooks need review` is
ordinary prose that reaches column 0 readily, in a heading, a changelog line, a commit message or a
code comment. This plan file itself carries it at column 0 inside its own section 1 fence, so a Codex
session that pages through this very file will mark itself blocked. The mechanism is old; the exposure
is new. The episode model clears the state when the text scrolls away.

R4. The `capturedAgainst` version is one probe late relative to the observation: the reporter's exact
build at the moment of observation was not recorded, and the `codex --version` probe ran after the
report, so a same-day auto-update would make 0.153.2 one patch off. This is a documentation-accuracy
residual with no behavioral effect (D7).

R5. The local-overlay escape hatch requires supplying the whole `agents` array (E11). A user who wants
to suppress just this one entry is better served by `enabled: false` on the entry itself (D9), which
no current documentation tells them; that documentation is #1758.

R6. `apply_issue_1757_migration` is a permanent function that becomes a no-op for every install once
it has run. Nothing in this plan removes it later, and removing it would resurrect E7 for anyone who
skips the intervening releases.
