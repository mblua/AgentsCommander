# Implementation Plan: #1273 Extract `config::instance_gitignore` from the 88 module cyclic SCC

Status: READY_FOR_IMPLEMENTATION

**Certified in one pass by the architect (Lite path).** This is the complete cold-start specification:
the exact file contents to write, the exact commands to run, the exact numbers to expect, and the exact
arcs the change adds and removes. Nothing is left to decide. The implementer will have no memory of the
exchange that produced this document, so nothing outside it is assumed.

**Everything in this plan was measured, not predicted.** The architect built an independent laboratory
before certifying: a copy of the real `src-tauri/src/` (189 files), the change of Sections 5.1 to 5.3
applied to it, the guard of Section 5.4 compiled against it, and 22 probes run through it. Section 9.3.4
is the probe table. The graph numbers were re-derived with the architect's own Tarjan implementation over
the committed arc record, not carried forward from anybody's report. Where a number came from
`__agent_dev-rust/EVIDENCE-1273.md` it was re-measured here and the figure is restated in place rather
than referenced.

**Say this in the commit and in the report, because otherwise a success reads as a failure.** The knot
goes from 88 members to 87. `cyclicSccs` stays at **1**; it does not drop to 0. The remaining 87 module
knot is untouched by design. Per `break-dependency-cycles` Section 2, **88 to 87 is a rehearsal of the
procedure, not a perceptible improvement**, and the durable return is the guard of Section 5.4 and the
placement precedent it sets. The realistic effort split is **1:5**: one commit fixes the cycle, five
harden the guard. That is expected, not a sign of trouble.

---

## 1. Issue and objective

**Issue:** #1273, `refactor(config): extract config::instance_gitignore from the 88-module cyclic SCC`.
It stays **OPEN**. No commit message, PR body or comment may contain `Closes #1273` or any other closing
keyword.

**Branch:** `refactor/1273-extract-instance-gitignore-wg11`, already created, already checked out.
**Base:** `origin/main` @ `55e49b0f7b46d31225f09bac5bf8a847ea9c4b0e`, working tree clean.

**Objective.** Take `agentscommander_lib::config::instance_gitignore`
(`src-tauri/src/config/instance_gitignore.rs`) out of the crate's single cyclic SCC by removing the arc

```
agentscommander_lib::config::instance_gitignore -> agentscommander_lib::config::root_agent
```

and keeping the other two arcs that touch the module exactly as they are.

**This is a structural change with no behavioural change.** The `.gitignore` this module writes must be
byte for byte what it writes today, for every input, and the seeding must still happen at exactly the
same moment in startup.

---

## 2. Evidence and current state

### 2.1 The knot, re-derived here

Computed for this plan by running Tarjan over the 976 arcs of the committed record
`src-tauri/module-arcs.txt` at `55e49b0f`, with an implementation written for this purpose rather than
by trusting any instrument's verdict about its own output. It agrees with `EVIDENCE-1273.md`, which
reached the same figures independently:

```
modules with at least one arc 174 | unique arcs 976 | sccs 87 | cyclicSccs 1
cyclic SCC sizes: [88]
sccSize(agentscommander_lib::config::instance_gitignore) = 88   level 3 (the knot's pseudo-level)
sccSize(agentscommander_lib::config::root_agent)         = 88   level 3
sccSize(agentscommander_lib::logging)                    = 88   level 3
sccSize(agentscommander_lib::config)                     = 1    level 0
```

**Two counting conventions exist and they are both right; do not read a disagreement into them.** The
record carries only modules that appear as an endpoint of some arc, so it yields **174 modules and 87
SCCs**. The levelizer also counts isolated modules and reports **184 modules and 97 SCCs** on the same
tree. The difference is the ten modules with no arc at all, `agentscommander_new[bin:main]` among them.
No gate in this plan uses either total. The gates are `cyclicSccs`, knot **membership**, and `sccSize`,
and those are identical under both conventions.

### 2.2 The three arcs that touch the target, and only three exist

Lines 547, 548 and 618 of the committed record:

```
547  agentscommander_lib::config::instance_gitignore -> agentscommander_lib::config
548  agentscommander_lib::config::instance_gitignore -> agentscommander_lib::config::root_agent
618  agentscommander_lib::logging                    -> agentscommander_lib::config::instance_gitignore
```

The target has **exactly two outgoing arcs** and **exactly one incoming arc**. There is no fourth.

- Arc 547, `-> config`, is **not removable and is not removed**. `ensure_instance_gitignore` calls
  `super::config_dir()` at `instance_gitignore.rs:29` and `super::agent_local_dir_name()` at line 40.
  This arc is why Section 4.3's non-absorption argument is about `config` and why Section 5.4 guards
  `config` as well as the target.
- Arc 548, `-> config::root_agent`, is the one this change removes.
- Arc 618, `logging ->`, is **not touched**. `logging.rs:481` calls
  `crate::config::instance_gitignore::ensure_instance_gitignore()` from `init_logger_inner`, and that
  call stays exactly where it is.

### 2.3 The 88 module knot, verbatim

This is the set acceptance criterion 2 compares against, **set to set and not by count**. Produced by
the architect's own Tarjan run over the committed record and identical to the list in
`EVIDENCE-1273.md`:

```
 1. agentscommander_lib
 2. agentscommander_lib::api
 3. agentscommander_lib::api::audit
 4. agentscommander_lib::api::auth
 5. agentscommander_lib::api::error
 6. agentscommander_lib::api::message_store
 7. agentscommander_lib::api::schema
 8. agentscommander_lib::cli
 9. agentscommander_lib::cli::agency_templates
10. agentscommander_lib::cli::create_agent
11. agentscommander_lib::cli::create_agent_matrix
12. agentscommander_lib::cli::list_peers
13. agentscommander_lib::commands::ac_discovery
14. agentscommander_lib::commands::config
15. agentscommander_lib::commands::entity_creation
16. agentscommander_lib::commands::pty
17. agentscommander_lib::commands::repos
18. agentscommander_lib::commands::resource_monitor
19. agentscommander_lib::commands::role_templates
20. agentscommander_lib::commands::session
21. agentscommander_lib::commands::telegram
22. agentscommander_lib::commands::voice
23. agentscommander_lib::commands::wg_delete_diagnostic
24. agentscommander_lib::commands::window
25. agentscommander_lib::config::activity_log
26. agentscommander_lib::config::agent_command
27. agentscommander_lib::config::agent_memory
28. agentscommander_lib::config::archive_gate
29. agentscommander_lib::config::coding_agent_mutations
30. agentscommander_lib::config::coding_agent_profiles
31. agentscommander_lib::config::coding_agents_catalog
32. agentscommander_lib::config::config_seed
33. agentscommander_lib::config::coordinator_clocks
34. agentscommander_lib::config::injected_messages
35. agentscommander_lib::config::instance_gitignore      <-- the target, and the only module that leaves
36. agentscommander_lib::config::loops
37. agentscommander_lib::config::placeholders
38. agentscommander_lib::config::projects
39. agentscommander_lib::config::root_agent              <-- the cut counterpart
40. agentscommander_lib::config::seed_manifest
41. agentscommander_lib::config::seeded_context_templates
42. agentscommander_lib::config::session_context
43. agentscommander_lib::config::sessions_persistence
44. agentscommander_lib::config::settings
45. agentscommander_lib::config::teams
46. agentscommander_lib::logging
47. agentscommander_lib::loops::non_stop_watchdog
48. agentscommander_lib::phone::mailbox
49. agentscommander_lib::phone::messaging
50. agentscommander_lib::phone::terminal_snapshot
51. agentscommander_lib::phone::types
52. agentscommander_lib::pty::backend
53. agentscommander_lib::pty::container_backend
54. agentscommander_lib::pty::container_paths
55. agentscommander_lib::pty::container_repos
56. agentscommander_lib::pty::container_runtime
57. agentscommander_lib::pty::container_tokens
58. agentscommander_lib::pty::docker_runtime
59. agentscommander_lib::pty::git_watcher
60. agentscommander_lib::pty::idle_detector
61. agentscommander_lib::pty::inject
62. agentscommander_lib::pty::local_backend
63. agentscommander_lib::pty::manager
64. agentscommander_lib::pty::output
65. agentscommander_lib::pty::terminal_snapshot
66. agentscommander_lib::pty::watchers
67. agentscommander_lib::pty::watchers::frame
68. agentscommander_lib::pty::watchers::history
69. agentscommander_lib::resource_monitor::registry
70. agentscommander_lib::resource_monitor::types
71. agentscommander_lib::resource_monitor::watchdog
72. agentscommander_lib::resource_monitor::windows
73. agentscommander_lib::session::auto_close
74. agentscommander_lib::session::context_alerts
75. agentscommander_lib::session::manager
76. agentscommander_lib::session::selection
77. agentscommander_lib::session::session
78. agentscommander_lib::telegram::bridge
79. agentscommander_lib::telegram::claude_watcher
80. agentscommander_lib::telegram::codex_watcher
81. agentscommander_lib::telegram::gemini_watcher
82. agentscommander_lib::telegram::manager
83. agentscommander_lib::testability::reset
84. agentscommander_lib::testability::ui_automation
85. agentscommander_lib::testability::window_info
86. agentscommander_lib::update_check
87. agentscommander_lib::web
88. agentscommander_lib::web::commands
```

**Entries 78 and 79 are WG10's targets and are out of scope in both directions.** They appear here only
because the membership set has to be verbatim to be comparable. Section 3.2 forbids touching those two
files; nothing in this plan reasons about them.

### 2.4 What the dependency actually is: one string literal

The whole of arc 548 is the constant

```rust
pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";
```

defined at `src-tauri/src/config/root_agent.rs:13`. It is a plain `&'static str`, not a type, not a
trait, not a function, not macro generated. Both production uses interpolate it into a `format!` that
builds a `.gitignore` rule path. **There is no behavioural coupling to `config::root_agent` at all**, and
`instance_gitignore.rs` contains no `use` statement naming that module: every reference is an inline
path.

### 2.5 The edit surface is 8 sites, not 2, and the graph is right to say 2

The record reports 2 sites for arc 548 and the source has 2 **in production**. It also holds **6 more**
that the record is configured not to see: the levelizer declares `source.target.includeTests: false`, and
all six are inside `#[cfg(test)] mod tests`, which opens at `instance_gitignore.rs:414` and closes at
1061. Verified in the source for this plan:

| line | region | current text |
|---|---|---|
| 105 | production, in `required_rules` | `super::root_agent::ROOT_AGENT_DIR_NAME,` |
| 108 | production, in `required_rules` | `format!("/{}/config.json", super::root_agent::ROOT_AGENT_DIR_NAME),` |
| 836 | `#[cfg(test)]` | `super::super::root_agent::ROOT_AGENT_DIR_NAME` |
| 866 | `#[cfg(test)]` | `super::super::root_agent::ROOT_AGENT_DIR_NAME` |
| 936 | `#[cfg(test)]` | `super::super::root_agent::ROOT_AGENT_DIR_NAME` |
| 942 | `#[cfg(test)]` | `super::super::root_agent::ROOT_AGENT_DIR_NAME` |
| 946 | `#[cfg(test)]` | `super::super::root_agent::ROOT_AGENT_DIR_NAME` |
| 968 | `#[cfg(test)]` | `super::super::root_agent::ROOT_AGENT_DIR_NAME` |

**All eight must change.** The guard of Section 5.4 reads whole files including `#[cfg(test)]` regions,
exactly as `loops_layering.rs` and `project_settings_layering.rs` both do and both document, so a guard
written the same way goes red unless the six test references are repointed too. They are six mechanical
one line substitutions. Section 5.3 gives every one of them.

### 2.6 Where else the constant is named

57 references in 7 files, counted in the source:

| file | references | shape | in the knot? |
|---|---|---|---|
| `src/config/root_agent.rs` | 32 | 1 definition at line 13 plus **31 bare `ROOT_AGENT_DIR_NAME`**, same module | yes |
| `src/config/session_context.rs` | 10 | `crate::config::root_agent::ROOT_AGENT_DIR_NAME` | yes |
| `src/config/instance_gitignore.rs` | 8 | `super::` and `super::super::`, the target | yes |
| `src/phone/mailbox.rs` | 3 | `crate::config::root_agent::ROOT_AGENT_DIR_NAME` | yes |
| `src/commands/ac_discovery.rs` | 2 | `crate::config::root_agent::ROOT_AGENT_DIR_NAME` | yes |
| `src/config/placeholders.rs` | 1 | `crate::config::root_agent::ROOT_AGENT_DIR_NAME` | yes |
| `src/config/coding_agent_profiles.rs` | 1 | `crate::config::root_agent::ROOT_AGENT_DIR_NAME` | yes |

**Only the 8 in the target change.** The 31 bare in-file references and the 17 fully qualified external
ones keep resolving through the re-export of Section 5.2, which is why Section 4.4 chooses it. Every one
of the other six files is inside the knot, and editing a knot member to repoint a reference is how a
change acquires arcs nobody asked for.

### 2.7 What the instrument does and does not record

Load bearing for Sections 5.4, 7 and 9, and all of it established by reading the committed record
against the source:

- **References under `#[cfg(test)]` are not recorded**, and neither are integration test targets.
  `src-tauri/tests/` holds 22 files and contributes zero arcs to the 976. That is why the guard lives
  there and may name the forbidden path as freely as it likes.
- **`mod` declarations create no arc**, and neither does a `const`. Nothing this change adds to
  `src/config/mod.rs` can create an outgoing arc from `config`.
- **`use super::` and `super::` in an expression path ARE resolved.** Measured for #1265 on production
  code: `src/web/commands.rs:12` writes `use super::broadcast::WsBroadcaster;` and the corresponding arc
  exists in the record. So rewriting a reference as `super::` does not delete an arc, and this plan does
  not try to.
- **A fully unanchored path IS lost.** Measured: `src/lib.rs:1178` constructs
  `loops::scheduler::LoopScheduler::new()` and **no `lib -> loops` arc exists** among the 976. The arc
  count is a floor, not a total. This is the one blind spot that could satisfy criterion 5 cosmetically,
  and it is why the guard's first assertion is deliberately anchorless (Section 9.3.3) and why criterion
  8 exists at all.
- **Do not evade the detector.** Every reference this plan introduces is a real path to where the
  constant really is. An arc disappears because the dependency is gone, never because the spelling
  changed.

### 2.8 Co-change: the two modules are not one unit

Run before any code decision, because a pair that always changes together is one unit and cutting the
arc would then be the wrong fix. Measured by `dev-rust` on the full depth clone at
`C:\Users\maria\0_repos\AgentsCommander` (same origin, not shallow, `origin/main` exactly the workgroup
clone's shallow boundary `f15f59a4`), over 1930 non-merge commits, plus a normalized window of 139
commits since the target module was born. `ratio = both / either`.

| pair | full window | normalized window | both |
|---|---|---|---|
| calibration ceiling, `loops/scheduler` and `loops/delivery` | 0.231 | n/a | 3 |
| **`instance_gitignore` and `root_agent`** | **0.000** | **0.000** | **0** |
| `logging` and `instance_gitignore` | 0.063 | 0.200 | 1 |
| control, `logging` and `root_agent` | 0.000 | 0.000 | 0 |

`instance_gitignore.rs` was born on 2026-07-26 in `70c72893` and has three commits in its entire life.
`root_agent.rs` was last touched on 2026-07-19, seven days **before** the target existed, so the two
files have never appeared in the same commit and could not have. The one commit touching both `logging`
and `instance_gitignore` is the birth commit itself; excluding it with the rev-range `70c72893..f15f59a4`
gives `both = 0`. The 0.200 is birth asymmetry, not co-change.

**Neither pair is a hidden single unit. Cutting is right; merging would be absurd.** Source:
`EVIDENCE-1273.md` Section 3, restated here.

**One correction to carry forward.** The workgroup clone holds 16 commits and is shallow at `f15f59a4`,
not at the `1b0e9348` that older briefs name. `1b0e9348` is not in this clone at all, and in the deep
clone it is an ordinary two-parent merge touching zero files. Any instruction to exclude it is stale.
Co-change is not measurable on the workgroup clone and does not need to be re-measured for this change.

---

## 3. Scope

### 3.1 In scope

- `src-tauri/src/config/mod.rs` (Section 5.1)
- `src-tauri/src/config/root_agent.rs` (Section 5.2)
- `src-tauri/src/config/instance_gitignore.rs` (Section 5.3)
- `src-tauri/tests/instance_gitignore_layering.rs` (new; the guard, Section 5.4)
- `src-tauri/module-arcs.txt` (regenerated, Section 9.2)
- This plan file.

**Six files, and no others.** If anything else appears in `git status` at the end, something outside
scope was touched.

### 3.2 Out of scope, and the hard prohibitions

- **Do not touch `src-tauri/src/telegram/claude_watcher.rs` or `src-tauri/src/telegram/bridge.rs`.**
  WG10 is working on them in parallel and the two changes are verified disjoint. Do not read WG10's
  branch or session. Those two modules appear in the Section 2.3 list only because that list must be
  verbatim.
- **The rest of the knot is untouchable.** Do not modify anything inside it and do not tidy anything
  adjacent to it opportunistically. If its membership moves by anything other than the target leaving,
  something outside scope was touched: stop and report.
- **Cut A is refused and this is a closed decision. Do not reopen it.** Cutting
  `logging -> config::instance_gitignore` instead was simulated and also yields a knot of 87 with
  identical membership, so it is not refused on topology. It is refused because the seeding call has to
  be re-invoked somewhere and both realistic homes fail. Re-invoking from `src/lib.rs` was measured
  leaving the knot at **88** and `sccSize(target)` at **88**, because `agentscommander_lib` is member 1
  of the knot and `src/lib.rs:994` already calls `crate::logging::init_logger()`. The other home,
  `src/main.rs`, is the module `agentscommander_new[bin:main]`, which has **zero arcs in the entire
  976**, so routing through it deletes the arc from the record while the dependency survives in the
  code: the exact cosmetic satisfaction that `break-dependency-cycles` Section 8 warns about, and a
  change that would pass and prove nothing. Cut A also inverts the layering, see Section 4.1.
- **Do not repoint the other 49 references.** Section 4.4 closes that decision. Five of the six files
  holding them are knot members and editing them risks deleting arcs this change did not ask to delete.
- **No new arc from `config::instance_gitignore` into anything.** Trading this arc for another one is
  not a fix. After the change the module's entire outgoing dependency is `crate::config`.
- **No behavioural change**, no new feature, no signature change, no frontend change, no change to
  `src/shared/types.ts`, no change to any config, schema, file format or persisted state.
- **`I` (instability) justifies nothing here and must not appear in the implementation report.** With
  the cycle present, both sides' `Ce`/`Ca` include the very arc being deleted, so the ordering hint is
  computed over a graph containing the thing being removed, and the instrument's own note says it must
  not derive a code movement. Cost and layering are the reasons; `I` is not one. For the record, and it
  changes nothing: `02-levelize.mjs split --module agentscommander_lib::config::instance_gitignore`
  returns `suggestion.modules` **empty** for this module, so any claim that `I` endorsed some particular
  cut is not reproducible on this tree. Do not carry such a claim forward as measured.

---

## 4. The decided solution

**Move `pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";` out of
`src/config/root_agent.rs` and into `src/config/mod.rs`, leave a `pub use` re-export behind, and repoint
the target's 8 references at the new home.**

The arc `config::instance_gitignore -> config::root_agent` then disappears because the only reason for
it is gone. Nothing else moves.

### 4.1 Why this direction

Cut B (`instance_gitignore -> root_agent`) against cut A (`logging -> instance_gitignore`). Both extract
the target; the topology does not break the tie and is stated here so nobody re-derives it hoping it
will.

| | Cut A | **Cut B, chosen** |
|---|---|---|
| knot after | 87 | 87 |
| `cyclicSccs` after | 1 | 1 |
| `sccSize(target)` after | 1 | 1 |
| **level of the target after** | **4, above the whole knot** | **1, below the whole knot** |
| what moves | a startup side effect | one `pub const &str` |
| production sites | 1 | 2 |
| test sites | 0 | 6 |
| behaviour change | **yes**, the seeding has to be re-invoked and its ordering relative to logger init changes | **none** |
| a new call site is required | **yes**, and both obvious candidates fail (Section 3.2) | no |
| risk of cosmetic satisfaction | **high** | low |

**Layering decides it, and it decides it in opposite directions.** Cut A leaves `instance_gitignore`
still depending on `root_agent`, so it lands at level 4, **above** the entire knot: a 413 line filesystem
utility declared to be a higher level concept than the 88 modules beneath it, and nothing could ever
depend on it without dragging the knot along. That is one inversion traded for another, which
`break-dependency-cycles` Section 5 refuses. Cut B drops the target to level 1, **below** the knot,
depending on `config` at level 0 alone, with its single consumer `logging` above it at level 3. That is
the direction a dependency is supposed to run.

**Cost agrees.** Cut B touches more sites and is still the cheaper change, because all 8 are the same
mechanical substitution of one path prefix and none alters behaviour, while cut A moves an operation and
changes when it happens. `break-dependency-cycles` Section 6 applies: the minimal diff is a proxy, never
the goal.

### 4.2 Why the constant moves rather than the call

`ROOT_AGENT_DIR_NAME` is a **directory name for the instance layout**. `instance_gitignore` needs it to
write a `.gitignore` rule; `root_agent` needs it to build the Root Agent directory. Every consumer of it
is a consumer of a name, not of `config::root_agent`'s behaviour. It is a shared low level naming fact
that happens to live inside a 3711 line module sitting in the knot. Moving the name below both sides is
not a workaround for the cycle; it is where the name belonged already.

### 4.3 Why this placement, and the proof the knot cannot absorb it

**Destination: `agentscommander_lib::config`, the file `src-tauri/src/config/mod.rs`.**

**Proof that the knot cannot absorb the target through it.** A module joins a cyclic SCC only if it can
reach a member of that SCC and be reached from it. Measured over the 976 arcs of the committed record:
`agentscommander_lib::config` appears on the left of the separator **zero times** and on the right **49
times**. It is a pure sink. The set of modules reachable from it is therefore empty, so it can share an
SCC with nothing, so `config::instance_gitignore`, whose entire outgoing dependency after this change is
`config`, cannot either. This is the reachability computation on this exact graph, not an appeal to the
candidate not currently sharing an SCC with the knot, and it holds for any future arc **into** `config`
as well, because absorption needs a path out. Simulation confirms it: `sccSize(config) = 1`, level 0,
before and after.

**A `pub const &str` cannot create an outgoing arc**, so this placement is safe by construction and
cannot regress on its own. Only a later, unrelated edit to `src/config/mod.rs` could, which is what
Section 5.4's second test exists to catch.

**It does not change the host's role.** `config/mod.rs` is 349 lines and already owns exactly this kind
of fact: `agent_local_dir_name()`, `config_dir()`, `instance_base()`, `resolve_instance_location()`.
`ROOT_AGENT_DIR_NAME = "ac-root-agent"` is another instance layout name in the same family. The target
already calls `super::config_dir()` and `super::agent_local_dir_name()` from this very module, so after
the change it reads all three of its facts from one place.

**It costs zero new arcs.** `config::instance_gitignore -> config` exists today at record line 547 and
`config::root_agent -> config` exists at line 568. The record goes **976 to 975**: one arc removed, none
added. Section 7.1 enumerates it.

**The simulated alternative is refused, and here is why it is the weaker of the two.** A new leaf module
`agentscommander_lib::config::dir_names` (`src/config/dir_names.rs`) is structurally equivalent for the
knot: `cyclicSccs` 1, knot 87, `sccSize(target)` 1 at level 1, the new module at level 0. It costs one
extra file and takes the record to **977** (one removed, two added). It buys a dedicated, trivially
guardable home, and that is a real argument. It is refused because **it does not reduce the exposure it
appears to reduce**. The arc `config::instance_gitignore -> config` is not removable: the module calls
`super::config_dir()` and `super::agent_local_dir_name()`. So `config` has to stay a pure sink whatever
happens, and the new leaf would be a **second** premise to keep clean rather than a replacement for the
first, for one more file and one more arc. Choosing `config/mod.rs` leaves exactly one module to guard,
which is the one that had to be guarded anyway.

**Rejected candidates, with the reason.** `config::root_agent`, leaving it where it is: member 39 of the
knot. `config::session_context`, `config::placeholders`, `config::coding_agent_profiles`,
`phone::mailbox`, `commands::ac_discovery`: all knot members. `src/lib.rs`: knot member 1, and measured
putting the target straight back in. `src/main.rs`: zero arcs in the whole record, so anything placed
there is invisible to the instrument, which makes the acceptance criterion unverifiable rather than
satisfied. `break-dependency-cycles` Section 5, never into the knot.

### 4.4 The re-export, and why the other 49 references are not repointed

**`src/config/root_agent.rs` keeps the name reachable with `pub use crate::config::ROOT_AGENT_DIR_NAME;`
and this is a closed decision.**

- **It is still a move and not a duplication under `break-dependency-cycles` Section 8.** There is
  exactly one definition and it is no longer in `config::root_agent`. Criterion 8 and the third test of
  Section 5.4 assert precisely that, by equality, and the re-export is deliberately not counted as a
  definition.
- **It creates no arc.** `config::root_agent -> config` already exists at record line 568.
- **It is required, not merely convenient.** 31 of the 32 references inside `root_agent.rs` are bare
  `ROOT_AGENT_DIR_NAME` in the same module and stop compiling the moment the `pub const` leaves, so
  *some* import has to be left behind. Making it `pub use` rather than `use` costs nothing and keeps the
  17 fully qualified external references in five other files resolving unchanged.
- **Repointing all 57 is refused.** It would edit five knot members to change a spelling, and each such
  edit can delete an arc nobody asked to delete: if a file's only reference to `config::root_agent` is
  the constant, repointing it removes `<that module> -> config::root_agent` from the record and the arc
  diff of Section 7.1 stops being the complete enumeration criterion 5 requires. The end state is
  slightly cleaner and the blast radius is not this issue's to spend.
- **It also makes the liveness probe possible.** Because `crate::config::root_agent::ROOT_AGENT_DIR_NAME`
  still resolves after the change, the probe of Section 8 step 11 **compiles**, so running it produces a
  guard verdict instead of a build error. Section 9.3.5 explains why that distinction is the whole point
  of the liveness step. Verified in a minimal crate before certification: all five spellings
  (`super::ROOT_AGENT_DIR_NAME`, `super::super::ROOT_AGENT_DIR_NAME`,
  `super::root_agent::ROOT_AGENT_DIR_NAME`, `crate::config::root_agent::ROOT_AGENT_DIR_NAME`, and the
  bare in-module `ROOT_AGENT_DIR_NAME`) compile and resolve to `"ac-root-agent"` against this exact
  shape, with `clippy -D warnings` clean.

**If a later issue wants the re-export gone, that is a separate change with its own arc diff.** It is
not this one, and it must not be smuggled in.

### 4.5 Accepted cost

One arc removed, none added. Three source files edited, one test file added, 10 lines added to
`config/mod.rs`, 7 net lines added to `root_agent.rs`, and 8 in-place substitutions in the target, whose
line count does not change. Against that, a **1523 line** guard. That ratio is the 1:5 of
`break-dependency-cycles` Section 2 and it is the point of the exercise, not an overrun.

---

## 5. Affected surfaces: exact files and symbols

Every file in this section was written, formatted and, for Section 5.4, compiled and run before this
plan was certified. `rustfmt --check --edition 2021` exits **0** on each of them **as written**, measured
on `rustc 1.93.1`, so do not reformat them and do not reflow them by hand: a diff from `cargo fmt
--check` means a file was retyped rather than copied.

The three source edits are in-place substitutions and insertions that leave every one of the target's
eight reference line numbers unchanged: **105, 108, 836, 866, 936, 942, 946, 968** before and after.

### 5.1 `src-tauri/src/config/mod.rs`

One insertion. **After line 32, `use std::sync::OnceLock;`**, insert a blank line and then these ten
lines. The file goes from 349 to **359** lines.

```rust
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// #1273: the Root Agent instance directory name.
///
/// It lives here because `config::instance_gitignore` needs it to write one
/// `.gitignore` rule and `config::root_agent` needs it to build the directory,
/// and `config` is below both. This module already owns the instance-layout
/// facts of the same family: `agent_local_dir_name()`, `config_dir()` and
/// `instance_base()`. `config::root_agent` re-exports it, so every existing
/// reader of `crate::config::root_agent::ROOT_AGENT_DIR_NAME` keeps resolving.
pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";

/// #1077: authoritative, once-resolved location of the running AgentsCommander
```

The first two lines and the last are context and are not to be retyped; they show where the block goes.
The `pub mod` list at the top of the file is **not** touched: no module is added, so `reorder_modules`
has nothing to do. A `const` is not reordered by rustfmt, and the block is separated from the `use`
group by a blank line, so its position is stable.

**This adds no arc.** A `const` with a literal initializer names nothing.

### 5.2 `src-tauri/src/config/root_agent.rs`

One replacement. **Line 13 is deleted and eight lines take its place.** The file goes from 3711 to
**3718** lines.

Delete:

```rust
pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";
```

Write in its place:

```rust
/// #1273: `ROOT_AGENT_DIR_NAME` moved to `crate::config`, below both this module
/// and `config::instance_gitignore`, so the instance .gitignore seeder no longer
/// reaches into this module for one string. Re-exported rather than imported
/// because 17 references outside this file spell
/// `crate::config::root_agent::ROOT_AGENT_DIR_NAME`. Do not turn it back into a
/// `pub const`: `tests/instance_gitignore_layering.rs` asserts the constant is
/// defined exactly once, in `src/config/mod.rs`.
pub use crate::config::ROOT_AGENT_DIR_NAME;
```

so that lines 12 to 22 of the file read:

```rust

/// #1273: `ROOT_AGENT_DIR_NAME` moved to `crate::config`, below both this module
/// and `config::instance_gitignore`, so the instance .gitignore seeder no longer
/// reaches into this module for one string. Re-exported rather than imported
/// because 17 references outside this file spell
/// `crate::config::root_agent::ROOT_AGENT_DIR_NAME`. Do not turn it back into a
/// `pub const`: `tests/instance_gitignore_layering.rs` asserts the constant is
/// defined exactly once, in `src/config/mod.rs`.
pub use crate::config::ROOT_AGENT_DIR_NAME;
pub const ROOT_AGENT_SESSION_NAME: &str = "Root Agent";
```

**The blank line above the comment is load bearing and must stay.** `reorder_imports` sorts a
*contiguous* run of `use` items. Line 12 is blank today and separates this `pub use` from the `use
std::...` group above it, so it forms a group of one and rustfmt leaves it exactly where it is. Attached to
the group above, rustfmt would sort it to the top of the file and `cargo fmt --check` would report a diff
that Section 8 reads as a copy error. Measured: with the blank line in place, `rustfmt --check --edition
2021` on this file exits 0.

**The comment is required, not decorative.** The `pub` on this line is what keeps 17 references in five
other files compiling, and a future edit to `use` instead of `pub use` would break them in a way whose
cause is not visible at the failure site.

**The other 31 references in this file are not touched.** They are bare `ROOT_AGENT_DIR_NAME` in the same
module and resolve through this `use` exactly as they resolved through the `const`.

### 5.3 `src-tauri/src/config/instance_gitignore.rs`

Eight in-place substitutions and nothing else. The file stays at **1061** lines and every other byte of it
is unchanged. **Do not reformat the file**; measured, `rustfmt --check --edition 2021` exits 0 on the
result as written, and rustfmt keeps the first `format!` multi-line even after it shortens.

**Production, lines 103 to 108.** After the edit, `required_rules` reads:

```rust
    Ok([
        format!(
            "/{}/{}/config.json",
            super::ROOT_AGENT_DIR_NAME,
            escaped_agent_local_dir
        ),
        format!("/{}/config.json", super::ROOT_AGENT_DIR_NAME),
```

- **line 105**: `super::root_agent::ROOT_AGENT_DIR_NAME,` becomes `super::ROOT_AGENT_DIR_NAME,`
- **line 108**: `format!("/{}/config.json", super::root_agent::ROOT_AGENT_DIR_NAME),` becomes
  `format!("/{}/config.json", super::ROOT_AGENT_DIR_NAME),`

**Tests, six lines, all the same substitution:** `super::super::root_agent::ROOT_AGENT_DIR_NAME` becomes
`super::super::ROOT_AGENT_DIR_NAME`, at lines **836, 866, 936, 942, 946 and 968**. For orientation, they
sit inside these three tests:

| line | enclosing test |
|---|---|
| 836, 866 | `literal_gitignore_segment_encoding_is_canonical` |
| 936, 942, 946 | `git_fixture_treats_unix_metacharacter_agent_names_as_literal` (`#[cfg(unix)]`) |
| 968 | `escaped_canonical_line_controls_detection_and_repair` |

**Lines 936, 942 and 946 are inside a `#[cfg(unix)]` test and will not compile on Windows.** Repoint them
anyway. A reference this platform does not build is still a reference the guard reads, still a reference
the next Linux CI run compiles, and leaving it behind is exactly the half-finished move the guard is
there to catch.

**Verification after this edit, and it is exact:** `rg -n "root_agent" src/config/instance_gitignore.rs`
from `src-tauri` must produce **no output and exit 1**. There is no remaining occurrence of that
identifier in the file, in production or in tests, in code or in a literal.

**What is deliberately not touched in this file:** `use super::*;` at line 416 stays exactly where it is
and there must not be a second one anywhere (Section 5.4 asserts the count); `super::config_dir()` at
line 29 and `super::agent_local_dir_name()` at line 40 stay, and they are why arc 547 survives; the two
`use super::super::injected_messages::{...}` imports in the test module at lines 1004 and 1025 stay
untouched.

### 5.4 New file: `src-tauri/tests/instance_gitignore_layering.rs`

The structural guard. Section 9.3 is the reasoning behind it and the evidence that it is alive; this
section is the content to write. **Create it with exactly these 1523 lines.**

Measured before this plan was certified, on `rustc 1.93.1`, in a laboratory crate whose `src/` is a copy
of the real `src-tauri/src/` (189 files) carrying the change of Sections 5.1 to 5.3:

| | |
|---|---|
| `rustfmt --check --edition 2021` | exit 0, **as written** |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test --test instance_gitignore_layering` | **3 passed, 0 failed, 1.50 s** |
| encoding | pure ASCII, zero bytes above 127, LF endings, 70 423 bytes |

So do not reformat it and do not reflow it by hand; no encoding or line ending question arises on
Windows.

**It guards three things.** `instance_gitignore_names_nothing_that_reaches_the_knot` is the main event:
four assertions over the target's own files. `the_constant_home_names_nothing_at_all` is the one Section
4.3 depends on: `src/config/mod.rs` must stay the pure sink the non-absorption argument measures it to
be, because an **outgoing** arc from that one file puts `config` in the knot and this module back in with
it, and nothing else in the repository would go red.
`the_root_agent_dir_name_constant_is_defined_exactly_once` is criterion 8: the constant moved, it was not
copied.

**Once this file exists, it is the canonical copy and this section is a snapshot.** Section 9.3.6 invites
reviewers to append entries to the guard's `KNOWN UNCOVERED SPELLINGS` list, and the first appended entry
makes the file and this section diverge. That is expected and correct: the file runs, this section does
not. **Append to `src-tauri/tests/instance_gitignore_layering.rs` and leave this section alone.** The
guard's own module header says the same thing, where a reader who never opens this plan will find it.

```rust
//! #1273 layering guard: `crate::config::instance_gitignore` may not name
//! `crate::config::root_agent`, nor anything else that can reach the crate's
//! cyclic SCC, and the module that now holds `ROOT_AGENT_DIR_NAME`,
//! `crate::config`, may not name anything at all.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1 with
//! `sccSize(agentscommander_lib::config::instance_gitignore) = 1`. A green
//! result here means "no known spelling is present", never "the cycle is
//! impossible".
//!
//! WHY BOTH ANCHORS ARE ASSERTED, FROM DAY ONE. Every reference this module
//! makes is written `super::` or `super::super::`; it contains no `crate::` at
//! all. A guard that asserted only the `crate::`-anchored set would therefore
//! observe **nothing** in this module and pass everything, which is exactly the
//! failure `project_settings_layering.rs` records against itself as entry 13 of
//! its own uncovered list and issue #1268 tracks: adding
//! `use crate::session::manager::SessionManager;` to that guarded module moved
//! the knot 88 to 89 and `sccSize` 1 to 89 with that file green throughout,
//! because its `crate::`-anchored set was collected and never asserted. Here
//! the two sets are asserted by equality, both of them, and the `crate::` table
//! is deliberately **empty**: this module names nothing under `crate::` today
//! and any first entry is a decision about the crate's shape.
//!
//! WHY THE CONSTANT'S NEW HOME IS GUARDED TOO. #1273 took
//! `config::instance_gitignore` out of the knot by moving one `pub const` down
//! into `crate::config`, and the argument that the knot cannot absorb it rests
//! on `crate::config` having **zero outgoing arcs**, measured over the 976 of
//! `src-tauri/module-arcs.txt`. **That premise fails on an outgoing arc, not an
//! incoming one.** One `use crate::<any knot member>::...;` in `src/config/mod.rs`
//! puts `config` into the knot and drags this module straight back in with it,
//! and the assertions about `instance_gitignore` stay green throughout, because
//! that file never changed. The premise is load bearing either way: the arc
//! `config::instance_gitignore -> config` is not removed by #1273 and is not
//! removable, so `config` has to stay clean whatever else happens.
//!
//! WHAT IT READS. Not a directory. The files it scans are resolved by walking
//! `mod` declarations down from `src/lib.rs`, honouring `path = "..."` in both
//! `#[path]` and `#[cfg_attr(..., path = ...)]`, and collecting **every**
//! declaration of a segment rather than the first, so a module declared twice
//! under opposite `cfg`s contributes both files. A directory walk decides from
//! names; the compiler decides from the module tree, and code lives in the gap.
//!
//! **This resolver is not rustc and does not claim to be.** It over-reads on
//! purpose: `cfg` is not evaluated, so both arms of a platform module are
//! scanned even though only one is compiled. Reading a file rustc does not
//! compile costs a false red, which is argued about; missing one costs a false
//! green, which is believed. Where it cannot over-read safely it refuses: two
//! candidate files for one declaration, or a `mod x;` nested inside an inline
//! `mod y { ... }` block, are hard failures naming the file rather than a guess.
//!
//! The guarded module is read together with every module below it. The constant's
//! home is read **shallow**, its own `src/config/mod.rs` and no descendant,
//! because every child of `config` is a separate module in the graph with its own
//! arcs and most of them are inside the knot; descending would assert that the
//! whole `config` subtree names nothing, which is neither true nor this issue's
//! business.
//!
//! Comments and the bodies of string and character literals are removed before
//! anything is matched: neither can be a dependency, neither may hide a path
//! from the scan, and neither may feed one to it. That is why the string
//! `"ac-root-agent"` and the many `.gitignore` fixtures containing it, in this
//! module's own tests, do not trip the forbidden-name check below.
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: the four `ALLOWED_*` tables are the whole contract,
//! and the spellings the scan is known to miss are listed below instead of being
//! left unsaid.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that puts this module back within reach of the knot and still passes this
//! file, it is appended here. Appending an entry is part of reviewing #1273 and
//! is expected; it changes nothing else.
//!
//! **This file is the canonical copy.** Section 5.4 of
//! `plans/1273-extract-instance-gitignore-from-scc.md` quotes it verbatim, but
//! that quote is a snapshot taken when the plan was certified. The first appended
//! entry makes the two diverge, and that is expected: this file runs, the plan
//! does not. Append here and leave the plan alone.
//!
//! **"The detector still catches it" is not a closure.** Several entries below
//! say so and it is true and measured, but the whole reason this file exists is
//! that the detector is run by hand and is deliberately not wired to CI. An
//! entry the detector catches is still uncovered *here*, and still reaches
//! nobody until somebody remembers to run the instrument.
//!
//!   1. Re-export laundering. A third module writes
//!      `pub use crate::config::root_agent::SOMETHING;` and this module imports
//!      from there. No `root_agent` token appears in the scanned file. The
//!      detector still catches it: the laundering module carries the arc, this
//!      module reaches the knot through it, and the knot grows instead of
//!      thinning.
//!   2. Macro-generated paths. A `macro_rules!` defined elsewhere, or any
//!      procedural macro, whose expansion contains the path. The text is not in
//!      the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `include!`. A file textually included from outside the module tree is
//!      pulled in without a `mod` declaration, so walking the tree does not
//!      reach it.
//!   4. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in a knot member and which is wired together
//!      outside this module. No path text appears in the scanned files.
//!   5. `concat!` and friends. `concat!("root", "_agent")` builds the name out
//!      of fragments none of which contains it, and the bodies of those literals
//!      are removed before the scan in any case.
//!   6. A `mod x;` declaration nested inside an inline `mod y { ... }` block.
//!      rustc resolves it against the inline module's own directory and this
//!      resolver does not, so it would scan a file rustc does not compile. It is
//!      refused rather than read: `module_body` rejects the whole file with a
//!      hard failure naming it. The spelling is still uncovered in the sense
//!      that the reference is not read, but it cannot be read as green.
//!   7. NTFS alternate data streams. `#[path = "carrier.rs:evil"]` compiles from
//!      a stream that carries code, and a `mod` declaration hidden inside a
//!      stream of another file is not reachable. Git stores only the main
//!      stream, so a clone has no `:evil` and the build fails rather than hiding
//!      anything.
//!   8. **The fully unanchored path, and it is the important one.** The detector
//!      shares this blind spot and it is measured on production code:
//!      `src/lib.rs:1178` constructs `loops::scheduler::LoopScheduler::new()` and
//!      **no `lib -> loops` arc exists** among the 976. A path that begins with
//!      neither `crate::` nor `super::` is invisible to the record AND to both
//!      equalities here. Inside this module such a path needs a name in scope,
//!      which needs an import this guard does see, with one exception: the single
//!      `use super::*;` in the test module, entry 9. `names_the_replaced_module`
//!      closes the one spelling that matters today, a bare `root_agent`, and
//!      closes it under every anchor and none. It does not close the class.
//!   9. A second `use super::*;`, or the existing one moving to the top of the
//!      file. Written at module level rather than inside `mod tests`, that glob
//!      pulls `crate::config`'s children into scope under no name this scan can
//!      follow, `root_agent` among them, and a bare `root_agent::...` would then
//!      compile with no `super::` token anywhere. Because the observed set is
//!      deduplicated, a second identical glob would not move it. The count is
//!      therefore asserted separately, at exactly one. **Moving** the one glob is
//!      not caught by the count, and it is not exploitable as it stands, because
//!      `mod tests` reaches its parent's items through that glob and would stop
//!      compiling without it. It is written down because that is an argument
//!      about today's code, not a property of the matcher.
//!  10. Aliasing beyond the spellings `aliases_a_module_group` knows.
//!      `use crate as c;`, `use crate::config as c;` and `use super as s;` are
//!      refused by name; a rename reached some other way is not.
//!  11. A path assembled across a `cfg` boundary in a way the resolver
//!      over-reads into but the equality tables do not distinguish. This
//!      resolver scans both arms of a platform module, so a forbidden reference
//!      in either arm is caught, but which arm rustc compiled is not known here
//!      and the failure message cannot say.
//!  12. A `#[cfg(test)]` reference holding an equality up on its own, and here
//!      it is a live weakness rather than a theoretical one. Whole files are
//!      read, test regions included, while the detector is run with
//!      `includeTests: false`. Everywhere else that makes this guard stricter,
//!      which is the safe direction; here it makes it laxer. Six of the eight
//!      references to `ROOT_AGENT_DIR_NAME` in this module are inside
//!      `#[cfg(test)] mod tests`, so deleting the two production references at
//!      `required_rules` would leave the pair
//!      `("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME")` standing
//!      and the equality green. Unlike the equivalent entry in
//!      `project_settings_layering.rs`, **that deletion can be made to compile**:
//!      hard-coding the directory name in `required_rules` does it. The
//!      shrinking-set argument is correspondingly weaker for this one pair, and
//!      the fourteen-rule behavioural tests in the module are what actually
//!      hold the production references up.
//!  13. An unanchored path in the constant's home. `src/config/mod.rs` already
//!      writes `profile::config_dir_name()` three times, a path beginning with
//!      neither `crate::` nor `super::`, which is why it creates no arc and why
//!      that module measures zero outgoing arcs. A new unanchored path from
//!      there into a knot member would be invisible to the detector and to this
//!      guard alike, while putting `config` and this module back into the knot
//!      the moment anybody anchored it. The two equalities on that file catch
//!      the anchored forms only.
//!  14. (append here: one entry per spelling a reviewer proves still passes)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Every `(file, child)` reference the guarded module is allowed to make under
/// `crate::`, sorted.
///
/// **Empty, and the emptiness is the contract.** Before #1273 this module
/// contained no `crate::` path at all, and it still does not: everything it
/// needs is one level up and is written `super::`. An empty equality therefore
/// refuses **every** `crate::`-anchored reference, not merely a reference to
/// `config::root_agent`, and that breadth is the point. The module's exposure is
/// that it may name any of the 87 remaining knot members and fall straight back
/// in; `root_agent` is only today's spelling.
///
/// Adding the first row here is a decision about the crate's shape and must be
/// argued in the commit, not slipped in to get green.
const ALLOWED_GUARDED_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the guarded module is allowed to make under
/// `super::`, sorted.
///
/// This is where this module actually lives, so this is the table that has to be
/// right. Six pairs, one file:
///
/// - `*` is the single `use super::*;` inside `#[cfg(test)] mod tests`, where
///   `super` is this module itself. See entry 9 of the uncovered list for why
///   its count is asserted separately.
/// - `ROOT_AGENT_DIR_NAME` is the constant #1273 moved into `crate::config`,
///   reached as `super::ROOT_AGENT_DIR_NAME` from `required_rules` and as
///   `super::super::ROOT_AGENT_DIR_NAME` from the test module. **It is listed
///   because it must be there**: this is an equality, so if it silently
///   disappears the assertion fails rather than passing quieter. Its absence
///   would mean the name was reached some other way.
/// - `agent_local_dir_name` and `config_dir` are `crate::config`'s own
///   functions, called from `ensure_instance_gitignore`. They predate #1273 and
///   are the reason the arc `config::instance_gitignore -> config` exists and is
///   not removable.
/// - `injected_messages` is `super::super::injected_messages::...` in two tests.
///   It is a sibling module **inside the knot**, and it is allowed here only
///   because it is `#[cfg(test)]` and therefore contributes no arc to the record
///   (`includeTests: false`). If it ever appears in production code in this
///   module, the record gains `config::instance_gitignore ->
///   config::injected_messages` and this module is back in the knot. This guard
///   cannot tell the two positions apart; the detector can, and is the check
///   that decides.
/// - `super` is the leading segment of every `super::super::...` path, reported by
///   the matcher as itself rather than dropped.
///
/// The pair is the contract, not the child on its own. Keying on the child alone
/// would make the observed set a union over every scanned file, so a reference
/// added to a future submodule of this module would leave the set unmoved and
/// pass.
const ALLOWED_GUARDED_SUPER_REFERENCES: [(&str, &str); 6] = [
    ("src/config/instance_gitignore.rs", "*"),
    ("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME"),
    ("src/config/instance_gitignore.rs", "agent_local_dir_name"),
    ("src/config/instance_gitignore.rs", "config_dir"),
    ("src/config/instance_gitignore.rs", "injected_messages"),
    ("src/config/instance_gitignore.rs", "super"),
];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `crate::`, sorted.
///
/// **Empty.** `src/config/mod.rs` measures zero outgoing arcs over the 976, and
/// that measurement is the whole non-absorption argument of #1273: a module with
/// no way out cannot reach a knot member, so it cannot share an SCC with one, so
/// nothing that depends only on it can either.
const ALLOWED_HOST_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `super::`, sorted.
///
/// One row, the `use super::*;` in that file's own `#[cfg(test)] mod tests`,
/// where `super` is `crate::config` itself. From `src/config/mod.rs`, `super::`
/// at module level would mean the crate root, so a row appearing here for any
/// other child is a reference from `config` up into the crate root's children,
/// which is the inversion this guard exists to refuse.
const ALLOWED_HOST_SUPER_REFERENCES: [(&str, &str); 1] = [("src/config/mod.rs", "*")];

/// The module #1273 cut this one away from, matched as a bare identifier under
/// every anchor and under none.
///
/// This is the one check in the file that does not depend on an anchor, and it
/// exists because of entry 8: a bare `root_agent::ROOT_AGENT_DIR_NAME` would
/// compile if the name were in scope, would rebuild the dependency, and would be
/// invisible to the arc record and to both equalities. Comments and literal
/// bodies are removed first, so the string `"ac-root-agent"` and the
/// `.gitignore` fixtures in this module's tests do not match it.
const FORBIDDEN_NAME: &str = "root_agent";

const CRATE_ANCHOR: &str = "crate::";
const SUPER_ANCHOR: &str = "super::";

/// The module this guard is written about, as path segments below `crate`.
const GUARDED_MODULE: [&str; 2] = ["config", "instance_gitignore"];

/// The module #1273 moved the constant into, as path segments below `crate`.
const HOST_MODULE: [&str; 1] = ["config"];

/// The constant #1273 moved, and the one file that may define it.
///
/// `defines_the_constant` decides what counts as a definition, because the
/// re-export left behind in `src/config/root_agent.rs` names the same identifier
/// and must not be counted as one, while a `static` copy must be.
const CONSTANT_NAME: &str = "ROOT_AGENT_DIR_NAME";
const CONSTANT_HOME: &str = "src/config/mod.rs";

/// The one glob import the guarded module is allowed to contain, counted rather
/// than merely observed. See entry 9 of the uncovered list.
const GLOB_IMPORT: &str = "use super::*;";

/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Literals {
    Keep,
    Drop,
}

/// Whether a module's submodules are read with it.
///
/// `WithSubmodules` for the guarded module, so a reference cannot be parked in a
/// future child of it. `OwnFilesOnly` for the constant's home, because every
/// child of `config` is a separate module in the graph with its own arcs and
/// most of them are knot members.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    OwnFilesOnly,
    WithSubmodules,
}

/// Replace every comment, and optionally every string or character literal, with
/// a single space, leaving code behind.
///
/// A comment is whitespace to the Rust lexer, so `super /* x */ ::root_agent` is
/// the same path as `super::root_agent`; collapsing whitespace alone would leave
/// that spelling intact and break the anchor. Tracking literals is what makes
/// comment removal correct at all: `"https://host"` carries a `//` that would
/// otherwise blank the rest of its line. Dropping literal bodies additionally
/// stops prose or a string from holding an observed set at its expected value
/// after the real references are gone, and it is what keeps the many
/// `"ac-root-agent"` fixtures in the guarded module's tests from matching the
/// forbidden name.
///
/// A comment or literal that never closes is an error rather than a truncated
/// result: a scanner that cannot delimit what it is reading must say so, because
/// the alternative is a green result that proves nothing.
fn scrub(body: &str, literals: Literals) -> Result<String, &'static str> {
    let source: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut index = 0usize;

    let emit = |out: &mut String, text: &[char]| {
        if literals == Literals::Keep {
            out.extend(text.iter());
        } else {
            out.push(' ');
        }
    };

    while index < source.len() {
        let character = source[index];
        let preceded_by_identifier = index
            .checked_sub(1)
            .map(|previous| source[previous])
            .is_some_and(|previous| previous.is_alphanumeric() || previous == '_');

        if character == '/' && source.get(index + 1) == Some(&'/') {
            while index < source.len() && source[index] != '\n' {
                index += 1;
            }
            out.push(' ');
            continue;
        }

        if character == '/' && source.get(index + 1) == Some(&'*') {
            let mut depth = 0usize;
            while index < source.len() {
                if source[index] == '/' && source.get(index + 1) == Some(&'*') {
                    depth += 1;
                    index += 2;
                } else if source[index] == '*' && source.get(index + 1) == Some(&'/') {
                    depth -= 1;
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            if depth != 0 {
                return Err("a block comment is never closed, so the scan cannot be trusted");
            }
            out.push(' ');
            continue;
        }

        // `r"..."`, `r#"..."#`, `br"..."` and `br#"..."#`, only at a token
        // boundary so the `r` ending an identifier is not read as a prefix.
        if (character == 'r' || character == 'b') && !preceded_by_identifier {
            let mut cursor = index;
            if source[cursor] == 'b' {
                cursor += 1;
            }
            if source.get(cursor) == Some(&'r') {
                cursor += 1;
                let mut hashes = 0usize;
                while source.get(cursor) == Some(&'#') {
                    hashes += 1;
                    cursor += 1;
                }
                if source.get(cursor) == Some(&'"') {
                    cursor += 1;
                    let closing: Vec<char> = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let mut closed = false;
                    while cursor < source.len() {
                        if source[cursor..].starts_with(closing.as_slice()) {
                            cursor += closing.len();
                            closed = true;
                            break;
                        }
                        cursor += 1;
                    }
                    if !closed {
                        return Err("a raw string is never closed, so the scan cannot be trusted");
                    }
                    emit(&mut out, &source[index..cursor]);
                    index = cursor;
                    continue;
                }
            }
        }

        if character == '"' {
            let start = index;
            index += 1;
            let mut closed = false;
            while index < source.len() {
                match source[index] {
                    '\\' => index += 2,
                    '"' => {
                        index += 1;
                        closed = true;
                        break;
                    }
                    _ => index += 1,
                }
            }
            if !closed {
                return Err("a string literal is never closed, so the scan cannot be trusted");
            }
            emit(&mut out, &source[start..index]);
            continue;
        }

        // `'x'` and `'\n'` are literals; `'a` is a lifetime. Only a literal is
        // consumed, so a lifetime cannot swallow the code that follows it.
        if character == '\'' {
            if source.get(index + 1) == Some(&'\\') {
                let mut cursor = index + 3;
                while cursor < source.len() && source[cursor] != '\'' {
                    cursor += 1;
                }
                if cursor >= source.len() {
                    return Err(
                        "a character literal is never closed, so the scan cannot be trusted",
                    );
                }
                emit(&mut out, &source[index..cursor + 1]);
                index = cursor + 1;
                continue;
            }
            if source.get(index + 2) == Some(&'\'') {
                emit(&mut out, &source[index..index + 3]);
                index += 3;
                continue;
            }
        }

        out.push(character);
        index += 1;
    }

    Ok(out)
}

/// Collapse every run of ASCII whitespace (newlines included, so this is also
/// CRLF-safe) to one space, then delete the space on both sides of the
/// punctuation a Rust path or use-tree is built from.
///
/// This is what widens the net past a raw substring match. `use
/// super::{root_agent::ROOT_AGENT_DIR_NAME, config_dir};` does not contain the
/// text `super::root_agent` at all: the braces are in the way. Reflowed across
/// lines by rustfmt it does not contain it either. After normalization every one
/// of those forms is the same text and the use-tree can be read.
///
/// `U+200E` and `U+200F` are replaced first because Rust's lexer treats them as
/// whitespace and `char::is_whitespace` does not, so `split_whitespace` would
/// leave `super<U+200E>::root_agent` intact and the anchor would never match a
/// path rustc compiles without a warning. They are the only two characters where
/// the two definitions disagree; `U+0085`, `U+2028` and `U+2029` are covered.
fn normalized(body: &str) -> String {
    let body = body.replace(['\u{200E}', '\u{200F}'], " ");
    let mut out = body.split_whitespace().collect::<Vec<_>>().join(" ");
    for token in ["::", "{", "}", ","] {
        out = out.replace(&format!(" {token}"), token);
        out = out.replace(&format!("{token} "), token);
    }
    out
}

/// Whether the source renames a module group this scan depends on being spelled
/// out, as in `use crate as c;`, `use crate::config as c;` or `use super as s;`.
///
/// After such a rename `c::root_agent::...` reaches the cut module under a name no
/// text scan can follow, so the rename itself is refused instead of followed.
/// Anchored on the path punctuation in front of `config` so that English prose
/// about configuration does not trip it, and on `use crate`/`use super` rather
/// than the bare keywords for the same reason.
fn aliases_a_module_group(body: &str) -> bool {
    [
        "use crate as ",
        "::config as ",
        "{config as ",
        ",config as ",
        "config::{self as ",
        "use super as ",
        "use super::{self as ",
    ]
    .iter()
    .any(|spelling| body.contains(spelling))
}

/// The leading identifier of a use-tree item: `root_agent` from
/// `root_agent::{a, b}`, from `root_agent as r` and from `root_agent`. A
/// non-identifier item such as `*` is returned as itself, so a glob is reported
/// rather than silently dropped.
///
/// A leading `r#` is dropped first: `r#root_agent` is the raw-identifier
/// spelling of `root_agent` and names the same module, but reading it literally
/// stops at the `#` and reports the child as `r`, so the reference would be
/// caught by the equality assertion instead of by the #1273 message that
/// explains it.
fn leading_segment(item: &str) -> String {
    let item = item.strip_prefix("r#").unwrap_or(item);
    let mut segment: String = item
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    if segment.is_empty() {
        if let Some(character) = item.chars().next() {
            segment.push(character);
        }
    }
    segment
}

/// Split a brace group on the commas that belong to it, so a nested group such
/// as `root_agent::{a, b}, config_dir` yields two items and not three.
fn split_top_level(group: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in group.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                items.push(&group[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    items.push(&group[start..]);
    items
}

/// Every child named directly under `anchor` anywhere in `body`, which must
/// already be scrubbed and normalized, in source order.
///
/// `super::super::X` reports two children, `super` and `X`, because the scan
/// finds the anchor twice: once at the start and once immediately after it. That
/// is deliberate. `super` in the observed set is the marker that a path climbed
/// two levels, and dropping it would make `super::super::session::manager` and
/// `super::session::manager` indistinguishable.
///
/// An unclosed group is an error rather than an empty result, for the same reason
/// an unclosed comment is.
fn children_under(body: &str, anchor: &str) -> Result<Vec<String>, &'static str> {
    let mut children = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(anchor) {
        let anchor_at = from + offset;
        let after = anchor_at + anchor.len();
        let inside_longer_identifier = body[..anchor_at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if inside_longer_identifier {
            from = after;
            continue;
        }
        if body[after..].starts_with('{') {
            let mut depth = 0usize;
            let mut end = None;
            for (index, character) in body[after..].char_indices() {
                match character {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(after + index);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                return Err("an anchored `{` group is never closed, so the scan cannot be trusted");
            };
            for item in split_top_level(&body[after + 1..end]) {
                if item.trim().is_empty() {
                    continue;
                }
                children.push(leading_segment(item));
            }
            from = end;
        } else {
            children.push(leading_segment(&body[after..]));
            from = after;
        }
    }
    Ok(children)
}

/// Whether `body`, which must be scrubbed and normalized, names the module #1273
/// cut away from, as a bare identifier and under no anchor at all.
///
/// Both boundaries are checked, so `root_agent_defaults` and
/// `x_root_agent` are not hits and `r#root_agent` is.
fn names_the_replaced_module(body: &str) -> bool {
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(FORBIDDEN_NAME) {
        let at = from + offset;
        let after = at + FORBIDDEN_NAME.len();
        let opens = !body[..at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        let closes = !body[after..]
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if opens && closes {
            return true;
        }
        from = after;
    }
    false
}

/// Whether `body`, which must be scrubbed and normalized, defines the constant.
///
/// The needle is the keyword and the name together, and what follows decides. A
/// following `:` is what makes it a definition, so `pub const ROOT_AGENT_DIR_NAME:
/// &str = ...` and `static ROOT_AGENT_DIR_NAME: &str = ...` both count while
/// `pub use crate::config::ROOT_AGENT_DIR_NAME;` does not: the re-export #1273
/// leaves in `src/config/root_agent.rs` names the identifier and defines nothing,
/// and counting it would make the "moved, not duplicated" assertion fail on the
/// very shape the plan requires.
fn defines_the_constant(body: &str) -> bool {
    for keyword in ["const ", "static "] {
        let needle = format!("{keyword}{CONSTANT_NAME}");
        let mut from = 0usize;
        while let Some(offset) = body[from..].find(&needle) {
            let at = from + offset;
            let after = at + needle.len();
            let preceded_by_identifier = body[..at]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');
            if !preceded_by_identifier && body[after..].trim_start().starts_with(':') {
                return true;
            }
            from = after;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Resolving what the compiler compiles
// ---------------------------------------------------------------------------

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn relative_of(path: &Path) -> String {
    path.strip_prefix(manifest_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Read a file as text and remove its comments, keeping or dropping literals.
///
/// Bytes that are not valid UTF-8 are replaced rather than refused, so no file is
/// ever skipped for its encoding. A file that cannot be delimited afterwards is
/// still a hard failure.
fn scrubbed(path: &Path, literals: Literals) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", relative_of(path)))?;
    let text = String::from_utf8_lossy(&bytes);
    scrub(&text, literals).map_err(|reason| format!("{}: {reason}", relative_of(path)))
}

/// The directory rustc searches for the children of the module whose own file is
/// `file`: the file's own directory for a crate root or a `mod.rs`, and a
/// directory named after the file otherwise.
fn child_directory(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new("."));
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    if stem == "lib" || stem == "main" || stem == "mod" {
        parent.to_path_buf()
    } else {
        parent.join(stem)
    }
}

/// A file's two readings: `structure` has literal bodies removed and is what
/// braces and declarations are counted in; `with_literals` keeps them and is
/// what a `path = "..."` value is read out of.
struct ModuleBody {
    structure: String,
    with_literals: String,
}

/// Both readings of `owner`, after refusing the file outright if it declares a
/// module inside an inline module block.
///
/// The refusal is the honest answer to a case this resolver gets wrong: rustc
/// resolves `mod x;` inside `mod y { ... }` against `y`'s own directory, and
/// this resolver reads it as a child of the file. A scanner that cannot tell
/// which file it should be reading has to say so.
fn module_body(owner: &Path) -> Result<ModuleBody, String> {
    let structure = normalized(&scrubbed(owner, Literals::Drop)?);
    if let Some(identifier) = nested_module_declaration(&structure) {
        return Err(format!(
            "{} declares `mod {identifier};` inside an inline `mod ... {{ ... }}` block. \
             rustc resolves that against the inline module's own directory and this resolver \
             does not, so the file it would scan is not the file rustc compiles. Refusing the \
             file rather than reading the wrong one: move the declaration to the top level of \
             its file.",
            relative_of(owner)
        ));
    }
    Ok(ModuleBody {
        structure,
        with_literals: normalized(&scrubbed(owner, Literals::Keep)?),
    })
}

/// The identifier of the first `mod <ident>;` that sits inside an inline
/// `mod ... { ... }` block, if there is one.
///
/// `body` must be scrubbed with `Literals::Drop` and normalized, because a brace
/// inside a string literal is not a block and would throw the depth off.
fn nested_module_declaration(body: &str) -> Option<String> {
    let mut depth = 0usize;
    for (index, character) in body.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 || !body[index..].starts_with("mod ") {
            continue;
        }
        let disqualified = body[..index]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if disqualified {
            continue;
        }
        let after = index + "mod ".len();
        let identifier: String = body[after..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '#')
            .collect();
        if !identifier.is_empty() && body[after + identifier.len()..].starts_with(';') {
            return Some(
                identifier
                    .strip_prefix("r#")
                    .unwrap_or(&identifier)
                    .to_string(),
            );
        }
    }
    None
}

/// Every byte offset at which `mod <segment>;` is declared in `body`, which must
/// be normalized. A preceding identifier character or quote disqualifies the hit,
/// so neither `submod x;` nor a `mod x;` sitting inside a string is read as one.
///
/// **All of them, not the first.** The standard per-platform module is two
/// declarations of one name under opposite `cfg`s, and reading only the first
/// means scanning the Unix file in a Windows build while reporting that the set
/// of files is the set rustc compiles.
fn find_declarations(body: &str, segment: &str) -> Vec<usize> {
    let needle = format!("mod {segment};");
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(&needle) {
        let at = from + offset;
        let disqualified = body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"');
        if !disqualified {
            found.push(at);
        }
        from = at + needle.len();
    }
    found
}

/// The text of the item that ends at `declaration_at`, so its attributes can be
/// read without scanning back into the previous item.
fn attributes_before(body: &str, declaration_at: usize) -> &str {
    let start = body[..declaration_at]
        .rfind([';', '}', '{'])
        .map(|index| index + 1)
        .unwrap_or(0);
    &body[start..declaration_at]
}

/// Every file named by a `path = "..."` in the item's attributes, in order.
///
/// Both `#[path = "x.rs"]` and `#[cfg_attr(<cond>, path = "x.rs")]` are read.
/// Matching the bare key rather than the text `#[path` is what covers the second
/// form, which is otherwise invisible: the resolver would fall back to the
/// default candidates while rustc compiles the file the `cfg_attr` names.
fn path_attributes(attributes: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = attributes[from..].find("path") {
        let at = from + offset;
        let after = at + "path".len();
        let preceded_by_identifier = attributes[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = attributes[after..].trim_start();
        if !preceded_by_identifier {
            if let Some(rest) = rest.strip_prefix('=') {
                if let Some(rest) = rest.trim_start().strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        values.push(rest[..end].to_string());
                    }
                }
            }
        }
        from = after;
    }
    values
}

/// Every file rustc might compile for the child `segment` of the module whose
/// file is `owner`.
///
/// `owner_body` must be `ModuleBody::with_literals`, because the `path` value is
/// a literal.
///
/// Two rules earn their keep here and both are refusals rather than guesses:
///
/// - **A `path` value is resolved beside the owner file first.** For a `mod`
///   declaration at the top level of a file, rustc reads `path` relative to the
///   directory the file is in, not relative to the module's own subdirectory.
/// - **Two existing candidates for one declaration is a hard failure.** rustc
///   itself rejects `x.rs` and `x/mod.rs` both existing; for a `path` value, two
///   candidates is exactly the situation where this scan cannot know which file
///   it is meant to read, and the house rule is that it must then say so.
fn resolve_children(owner: &Path, owner_body: &str, segment: &str) -> Result<Vec<PathBuf>, String> {
    let declarations = find_declarations(owner_body, segment);
    if declarations.is_empty() {
        return Err(format!(
            "{} declares no `mod {segment};`",
            relative_of(owner)
        ));
    }

    let module_directory = child_directory(owner);
    let file_directory = owner.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut resolved = Vec::new();

    for at in declarations {
        let values = path_attributes(attributes_before(owner_body, at));
        if values.is_empty() {
            let named = module_directory.join(format!("{segment}.rs"));
            let directory = module_directory.join(segment).join("mod.rs");
            match (named.is_file(), directory.is_file()) {
                (true, true) => {
                    return Err(format!(
                        "{} declares `mod {segment};` and both {} and {} exist. rustc rejects \
                         that outright and this scan will not pick one.",
                        relative_of(owner),
                        relative_of(&named),
                        relative_of(&directory)
                    ))
                }
                (true, false) => resolved.push(named),
                (false, true) => resolved.push(directory),
                (false, false) => {
                    return Err(format!(
                        "{} declares `mod {segment};` but none of these files exists: {}, {}",
                        relative_of(owner),
                        relative_of(&named),
                        relative_of(&directory)
                    ))
                }
            }
            continue;
        }

        for value in values {
            let beside_the_file = file_directory.join(&value);
            let inside_the_module = module_directory.join(&value);
            let mut hits: Vec<PathBuf> = Vec::new();
            for candidate in [beside_the_file, inside_the_module] {
                if candidate.is_file() && !hits.contains(&candidate) {
                    hits.push(candidate);
                }
            }
            match hits.len() {
                1 => resolved.push(hits.remove(0)),
                0 => {
                    return Err(format!(
                        "{} declares `mod {segment};` with `path = \"{value}\"` and no file \
                         exists at {} or at {}",
                        relative_of(owner),
                        relative_of(&file_directory.join(&value)),
                        relative_of(&module_directory.join(&value))
                    ))
                }
                _ => {
                    return Err(format!(
                        "{} declares `mod {segment};` with `path = \"{value}\"` and both {} and \
                         {} exist. rustc compiles the one beside the file; this scan will not \
                         guess, because guessing wrong is how a forbidden reference stays \
                         unread. Remove one of them.",
                        relative_of(owner),
                        relative_of(&file_directory.join(&value)),
                        relative_of(&module_directory.join(&value))
                    ))
                }
            }
        }
    }

    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}

/// Every `mod <ident>;` declared in `body`, which must be `ModuleBody::structure`,
/// deduplicated. `resolve_children` finds every declaration of each name, so one
/// entry per name is enough here.
fn declared_children(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find("mod ") {
        let at = from + offset;
        let after = at + "mod ".len();
        let disqualified = body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"');
        if !disqualified {
            let identifier: String = body[after..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '#')
                .collect();
            if !identifier.is_empty() && body[after + identifier.len()..].starts_with(';') {
                found.push(
                    identifier
                        .strip_prefix("r#")
                        .unwrap_or(&identifier)
                        .to_string(),
                );
            }
        }
        from = after;
    }
    found.sort();
    found.dedup();
    found
}

/// The files rustc compiles for `module`, and for every module below it when
/// `reach` says so, resolved by walking `mod` declarations down from the crate
/// root.
///
/// The walk carries a frontier rather than a single file, because a segment can
/// be declared more than once under opposite `cfg`s and this resolver keeps both
/// arms. An error at any step is propagated rather than skipped: a module that
/// cannot be located is the one case where reading nothing must not look like
/// reading nothing forbidden.
fn files_of(module: &[&str], reach: Reach) -> Result<Vec<PathBuf>, String> {
    let root = manifest_dir().join("src").join("lib.rs");
    if !root.is_file() {
        return Err(format!("{} does not exist", relative_of(&root)));
    }

    let mut frontier = vec![root];
    for segment in module {
        let mut next = Vec::new();
        for owner in &frontier {
            next.extend(resolve_children(
                owner,
                &module_body(owner)?.with_literals,
                segment,
            )?);
        }
        next.sort();
        next.dedup();
        frontier = next;
    }

    if reach == Reach::OwnFilesOnly {
        return Ok(frontier);
    }

    let mut files = Vec::new();
    let mut queue = frontier;
    while let Some(current) = queue.pop() {
        if files.contains(&current) {
            continue;
        }
        let body = module_body(&current)?;
        for child in declared_children(&body.structure) {
            queue.extend(resolve_children(&current, &body.with_literals, &child)?);
        }
        files.push(current);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Every file rustc compiles for the whole crate, as repository-relative paths.
///
/// Used for one question only, and computed lazily because it is the expensive
/// and failure-prone walk: when a file under `src/` cannot be delimited, is it a
/// file the compiler reads? A `.md` that is not in the module tree cannot hold a
/// definition of anything and must not turn a layering guard red. A `.rs` file
/// the compiler does read and this scan could not is the opposite, and must.
///
/// A file whose own body cannot be read is **recorded as reached and not
/// descended into**, which is the only sensible answer: the question being asked
/// is exactly "is this unreadable file in the tree", so failing the walk because
/// of it would refuse to answer the question it was called to answer.
fn crate_sources() -> Result<BTreeSet<String>, String> {
    let root = manifest_dir().join("src").join("lib.rs");
    if !root.is_file() {
        return Err(format!("{} does not exist", relative_of(&root)));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut queue = vec![root];
    while let Some(current) = queue.pop() {
        if files.contains(&current) {
            continue;
        }
        let Ok(body) = module_body(&current) else {
            files.push(current);
            continue;
        };
        for child in declared_children(&body.structure) {
            queue.extend(resolve_children(&current, &body.with_literals, &child)?);
        }
        files.push(current);
    }
    Ok(files.iter().map(|path| relative_of(path)).collect())
}

/// Every file under `root`, sorted, filtered by nothing.
///
/// **Do not add an extension filter here.** `rustc` decides what to compile from
/// the module tree; a filter decides from the name, and production code lives in
/// the gap between the two. On a case-insensitive filesystem `mod x;` resolves
/// `x.RS` while `"RS" == "rs"` is false, and `#[path = "carrier.inc"]` compiles a
/// file no extension filter matches. Reading every file closes both and is still
/// a pure text scan.
fn every_file_under(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

/// What one module's files were observed to name.
///
/// `anchored` and `relative_up` are `(file, child)` pairs under `crate::` and
/// under `super::` respectively; `aliases` is the files that rename a module
/// group; `forbidden` is the files that name `root_agent` under no anchor at
/// all; `globs` is the total number of `use super::*;` across the module.
struct Observation {
    anchored: Vec<(String, String)>,
    relative_up: Vec<(String, String)>,
    aliases: Vec<String>,
    forbidden: Vec<String>,
    globs: usize,
}

/// Read every file of `module` and report what it names.
///
/// A file reached through the module tree is a file rustc compiles, so a `scrub`
/// failure on one of them is fatal here and says so: it is source the compiler
/// reads and this scan could not.
fn observe(module: &[&str], reach: Reach) -> Observation {
    let files = files_of(module, reach).unwrap_or_else(|reason| {
        panic!(
            "the module {module:?} could not be resolved from the module tree, so this scan \
             proves nothing: {reason}\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: this guard exists to prove that a specific \
             dependency is absent. If the module cannot be located, the guard has read nothing \
             and must say so rather than pass. Rename or move the module and this message names \
             the file whose `mod` declaration no longer resolves; update GUARDED_MODULE or \
             HOST_MODULE, or the declaration, to match."
        )
    });
    assert!(
        !files.is_empty(),
        "the module {module:?} resolved to no files at all; the scan proves nothing"
    );

    let mut anchored = Vec::new();
    let mut relative_up = Vec::new();
    let mut aliases = Vec::new();
    let mut forbidden = Vec::new();
    let mut globs = 0usize;
    for path in &files {
        let relative = relative_of(path);
        let code = scrubbed(path, Literals::Drop).unwrap_or_else(|reason| {
            panic!(
                "{reason}\n\
                 \n\
                 This file is in the module tree, so rustc compiles it and this scan could not \
                 read it. That is a hard failure, not a skip."
            )
        });
        let body = normalized(&code);
        let name = |children: Result<Vec<String>, &'static str>| {
            children
                .unwrap_or_else(|reason| panic!("{relative}: {reason}"))
                .into_iter()
                .map(|child| (relative.clone(), child))
                .collect::<Vec<_>>()
        };
        anchored.extend(name(children_under(&body, CRATE_ANCHOR)));
        relative_up.extend(name(children_under(&body, SUPER_ANCHOR)));
        if aliases_a_module_group(&body) {
            aliases.push(relative.clone());
        }
        if names_the_replaced_module(&body) {
            forbidden.push(relative.clone());
        }
        globs += body.matches(GLOB_IMPORT).count();
    }
    anchored.sort();
    anchored.dedup();
    relative_up.sort();
    relative_up.dedup();
    Observation {
        anchored,
        relative_up,
        aliases,
        forbidden,
        globs,
    }
}

fn expected(table: &[(&str, &str)]) -> Vec<(String, String)> {
    table
        .iter()
        .map(|(file, child)| ((*file).to_string(), (*child).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// The guards
// ---------------------------------------------------------------------------

/// #1273: `config::instance_gitignore` used to name
/// `super::root_agent::ROOT_AGENT_DIR_NAME`, which made a 413-line filesystem
/// utility depend on a 3711-line module inside the crate's 88-member cyclic SCC
/// and held it inside that SCC. The constant moved down into `crate::config`, so
/// both sides depend downward on a module that depends on nothing.
///
/// This test lives in `src-tauri/tests/`, which is a separate leaf crate the
/// detector marks `enabled: opts.includeTests` and the record is emitted with
/// `includeTests: false`. It therefore adds no arc and no module, is outside the
/// tree it reads, and never has to excise itself from its own scan. Whole files
/// are read, `#[cfg(test)]` regions included, which is stricter than the
/// detector: a false red is argued about, a false green is believed.
#[test]
fn instance_gitignore_names_nothing_that_reaches_the_knot() {
    let seen = observe(&GUARDED_MODULE, Reach::WithSubmodules);

    assert!(
        seen.forbidden.is_empty(),
        "config::instance_gitignore must not name config::root_agent.\n\
         \n\
         WHY: this module writes the running instance's `.gitignore`. It is a \
         413 line filesystem utility that needs exactly one fact from \
         `config::root_agent`, the directory name `ac-root-agent`, and \
         `config::root_agent` is a 3711 line module inside the crate's 88 member \
         cyclic SCC. Naming it from here put this module inside that SCC too. \
         Issue #1273 moved the constant down into `crate::config`, which both \
         modules already depend on and which depends on nothing, so this module \
         now sits at level 1 below the knot instead of inside it.\n\
         \n\
         INSTEAD: read the name as `super::ROOT_AGENT_DIR_NAME`, which is where \
         it lives. If you need something from `config::root_agent` that is not a \
         name, it belongs in a module below both of them, never in either one.\n\
         \n\
         THIS CHECK IS DELIBERATELY ANCHORLESS. It matches the bare identifier \
         `root_agent` anywhere in the module's code, because the arc record \
         cannot see a path that begins with neither `crate::` nor `super::` \
         (measured: `src/lib.rs:1178` constructs `loops::scheduler::LoopScheduler` \
         and no `lib -> loops` arc exists among the 976). Comments and the bodies \
         of literals are removed first, so the string `\"ac-root-agent\"` and the \
         `.gitignore` fixtures in this module's own tests do not match.\n\
         \n\
         SCOPE: this is a net over the spellings of that reference, not a proof \
         that it cannot return. It matches text and does not resolve names, so a \
         spelling it does not know about passes it; the ones it is known to miss \
         are listed at the top of this file. The authoritative check is the cycle \
         detector, whose `coverage.graphShape.cyclicSccs` must stay at 1 with \
         `sccSize(agentscommander_lib::config::instance_gitignore) = 1`.\n\
         \n\
         OFFENDING FILES: {}",
        seen.forbidden.join(", ")
    );

    assert!(
        seen.aliases.is_empty(),
        "config::instance_gitignore must not rename the crate root or the config \
         module group.\n\
         \n\
         WHY: `use crate as <name>;`, `use crate::config as <name>;` and \
         `use super as <name>;` each put every module under `config`, \
         `config::root_agent` included, within reach under a name this scan \
         cannot follow. Following it would mean resolving names, which a text \
         scan does not do, so the rename is refused instead.\n\
         \n\
         INSTEAD: name the item you need by its real path, so this guard and the \
         cycle detector can both see it.\n\
         \n\
         OFFENDING FILES: {}",
        seen.aliases.join(", ")
    );

    assert_eq!(
        seen.globs, 1,
        "config::instance_gitignore must contain exactly one `use super::*;`.\n\
         \n\
         WHY: the one that exists is inside `#[cfg(test)] mod tests`, where \
         `super` is this module itself and the glob pulls in the functions under \
         test. Written at the top level of the file instead, the same three words \
         pull `crate::config`'s children into scope, `root_agent` among them, and \
         a bare `root_agent::ROOT_AGENT_DIR_NAME` would then compile with no \
         `super::` token anywhere: invisible to the arc record, invisible to both \
         equalities below, and the whole of #1273 undone. A text scan cannot tell \
         the two positions apart, and the observed set is deduplicated, so a \
         second identical glob would not move it. The count is asserted instead.\n\
         \n\
         INSTEAD: import what you need by name. If the test module genuinely \
         needs more of its parent, add the names to its existing glob's file, not \
         a second glob.\n\
         \n\
         OBSERVED: {} occurrences of `{GLOB_IMPORT}`",
        seen.globs
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_GUARDED_CRATE_REFERENCES),
        "the set of crate modules named from config::instance_gitignore moved.\n\
         \n\
         WHY THIS TABLE IS EMPTY: this module contains no `crate::` path at all, \
         before #1273 or after it. Everything it needs is one level up and is \
         written `super::`. An empty equality therefore refuses EVERY \
         `crate::`-anchored reference rather than only a reference to \
         `config::root_agent`, and that breadth is the point: this module's \
         exposure is that it may name any of the 87 remaining members of the knot \
         and fall straight back into it. `root_agent` is only today's spelling.\n\
         \n\
         THIS IS THE ASSERTION #1268 IS ABOUT. `project_settings_layering.rs` \
         collects the same set for its guarded module and never asserts it, and \
         the consequence was measured on this tree: adding \
         `use crate::session::manager::SessionManager;` to \
         `src/commands/project_settings.rs` moved the knot 88 to 89, \
         `sccSize` 1 to 89 and the arc count 976 to 977, with that file green \
         throughout, 3 passed 0 failed. This assertion is why the same three \
         words are red here.\n\
         \n\
         INSTEAD: if this module needs something from elsewhere in the crate, \
         that something belongs below it. Adding the first row to \
         ALLOWED_GUARDED_CRATE_REFERENCES is a decision about the crate's shape \
         and has to be argued in the commit."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_GUARDED_SUPER_REFERENCES),
        "the set of names reached by `super::` from config::instance_gitignore \
         moved.\n\
         \n\
         WHY THIS ANCHOR IS THE LOAD BEARING ONE: every reference this module \
         makes is written `super::` or `super::super::`, so a `crate::`-only \
         guard would observe nothing here and pass everything. This table is the \
         real contract.\n\
         \n\
         Each entry is a (file, child) pair, because the file is half of the \
         rule. `super::super::X` reports two children, `super` and `X`, so a path \
         that climbs two levels is distinguishable from one that climbs one.\n\
         \n\
         A LARGER SET means this module reached further up. That is a decision, \
         not a detail: remove it, or add its pair and say in the commit why. \
         `injected_messages` is a knot member and is allowed only because its two \
         references are `#[cfg(test)]` and contribute no arc; a production \
         reference to it would put this module back in the knot and this guard \
         cannot tell the two apart, so the detector decides.\n\
         \n\
         A SMALLER SET is the more dangerous failure, and it is why this is an \
         equality and not a denylist. `ROOT_AGENT_DIR_NAME` is listed because \
         #1273 put it there: if it silently disappears, the name is being reached \
         some other way and the reason this module is out of the cycle has \
         changed without anybody saying so. A shrinking set also means the scan \
         may have stopped seeing references it used to see, and a guard that \
         observes nothing passes everything. Comments and literal bodies are \
         removed before the scan so no amount of prose can hold this set up while \
         the real references disappear."
    );
}

/// #1273 Section 4.3: the knot cannot absorb `config::instance_gitignore`
/// because, after the cut, everything it depends on is `crate::config`, and
/// `crate::config` has **zero outgoing arcs** over the 976 of
/// `src-tauri/module-arcs.txt`.
///
/// **That is a claim about outgoing arcs from `src/config/mod.rs`, and this test
/// is the only thing that holds it.** The arc
/// `config::instance_gitignore -> config` is not removed by #1273 and is not
/// removable: `ensure_instance_gitignore` calls `super::config_dir()` and
/// `super::agent_local_dir_name()`. So one `use crate::<knot member>::...;` in
/// `src/config/mod.rs` puts `config` into the knot and this module back in with
/// it, 49 other modules follow, and every assertion in the test above stays green
/// because the guarded module's own file did not change.
///
/// It reads `src/config/mod.rs` and nothing below it. Every child of `config` is
/// a separate module in the graph with its own arcs, and 21 of them are knot
/// members; descending would assert that the whole `config` subtree names
/// nothing, which is neither true nor #1273's business.
#[test]
fn the_constant_home_names_nothing_at_all() {
    let seen = observe(&HOST_MODULE, Reach::OwnFilesOnly);

    assert!(
        seen.aliases.is_empty(),
        "the constant's home must not rename the crate root or a module group; \
         see the same assertion for config::instance_gitignore.\n\
         \n\
         OFFENDING FILES: {}",
        seen.aliases.join(", ")
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_HOST_CRATE_REFERENCES),
        "the set of crate modules named from {CONSTANT_HOME} moved.\n\
         \n\
         WHY THIS MATTERS MORE THAN IT LOOKS: #1273 is only correct while this \
         module cannot reach the cyclic SCC. Measured over the 976 arcs of \
         `src-tauri/module-arcs.txt`, `agentscommander_lib::config` appears on \
         the left of the separator zero times and on the right 49 times: it is a \
         pure sink, and that is the entire non-absorption argument. A module with \
         no way out cannot reach a knot member, so it cannot share an SCC with \
         one, so `config::instance_gitignore`, which depends on it, cannot \
         either.\n\
         \n\
         One `use crate::<any knot member>::...;` in this file ends that. `config` \
         joins the knot, `config::instance_gitignore` follows it back in through \
         an arc #1273 never removed and cannot remove, and the 49 other modules \
         that depend on `config` follow too. No other test in this repository \
         would go red.\n\
         \n\
         INSTEAD: if this module needs something, that something belongs below \
         it. Adding the first row to ALLOWED_HOST_CRATE_REFERENCES is a decision \
         about the crate's shape and has to be argued in the commit, and Section \
         4.3 of `plans/1273-extract-instance-gitignore-from-scc.md` has to be \
         rewritten before the dependency is added."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_HOST_SUPER_REFERENCES),
        "the set of names reached by `super::` from {CONSTANT_HOME} moved.\n\
         \n\
         The one allowed row is the `use super::*;` in that file's own \
         `#[cfg(test)] mod tests`, where `super` is `crate::config` itself. At \
         the top level of this file `super::` means the CRATE ROOT, so any other \
         row here is `config` reaching up into `crate`'s children, which is the \
         inversion this guard refuses. It would not even show up under the \
         `crate::` anchor above.\n\
         \n\
         See that assertion for why an outgoing arc from this file is the one \
         thing that undoes #1273."
    );
}

/// #1273 criterion 8: the constant moved, it was not copied. Two definitions
/// would drift, and `\"the arc is gone\"` would be satisfied while the fact the
/// arc was about had been duplicated instead of relocated.
///
/// This reads every file under `src/`, filtered by nothing, because a duplicate
/// can be parked anywhere. Reading everything means reading files that are not
/// Rust, and `scrub` cannot delimit arbitrary text: a Markdown file with an odd
/// number of `"` is not a defect in this tree and must not turn a layering guard
/// red. So a file this scan cannot delimit is fatal only when the module tree
/// reaches it, which is to say only when rustc compiles it and this scan could
/// not read source the compiler reads. **Do not turn this into an extension
/// filter**: the reason for reading everything is in `every_file_under` and it
/// has not changed.
#[test]
fn the_root_agent_dir_name_constant_is_defined_exactly_once() {
    let source_root = manifest_dir().join("src");
    let files = every_file_under(&source_root);
    assert!(
        !files.is_empty(),
        "no files found under src; the scan proves nothing"
    );

    let mut homes: Vec<String> = Vec::new();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    for path in &files {
        let relative = relative_of(path);
        match scrubbed(path, Literals::Drop) {
            Ok(code) => {
                if defines_the_constant(&normalized(&code)) {
                    homes.push(relative);
                }
            }
            Err(reason) => unreadable.push((relative, reason)),
        }
    }

    if !unreadable.is_empty() {
        let compiled = crate_sources().unwrap_or_else(|reason| {
            panic!(
                "a file under src could not be delimited, and the module tree could not be \
                 resolved to decide whether rustc compiles it, so this scan proves nothing: \
                 {reason}"
            )
        });
        let fatal: Vec<String> = unreadable
            .iter()
            .filter(|(relative, _)| compiled.contains(relative))
            .map(|(_, reason)| reason.clone())
            .collect();
        let none: Vec<String> = Vec::new();
        assert_eq!(
            fatal, none,
            "a file the compiler reads could not be delimited, so this scan did not read it \
             and cannot claim the constant is defined exactly once.\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: an unread file that rustc compiles is \
             exactly where a second definition would survive. A scan that quietly skips what \
             it cannot parse passes for the wrong reason.\n\
             \n\
             WHAT IT USUALLY IS: an unterminated string, character literal or block comment. \
             A Rust file in that state does not compile either, so fix the file. Files under \
             `src/` that are NOT in the module tree, such as Markdown, are reported nowhere \
             and are not failures: they cannot define anything.\n\
             \n\
             FILES THE COMPILER READS THAT COULD NOT BE DELIMITED: {fatal:?}"
        );
    }

    assert_eq!(
        homes,
        vec![CONSTANT_HOME.to_string()],
        "{CONSTANT_NAME} must be defined exactly once, in {CONSTANT_HOME}.\n\
         \n\
         WHY: #1273 moved this constant out of `config::root_agent` so that \
         `config::instance_gitignore` would stop depending on a knot member for \
         one string. A move that left a copy behind satisfies every arc \
         assertion and is still wrong: the two copies drift, and the claim that \
         the name has one home stops being true. Arc absence alone is \
         satisfiable without fixing anything, which is why this test exists \
         beside the two above.\n\
         \n\
         WHAT COUNTS AS A DEFINITION: `const` or `static`, then the name, then \
         `:`. The re-export `pub use crate::config::ROOT_AGENT_DIR_NAME;` that \
         #1273 leaves in `src/config/root_agent.rs` names the identifier and \
         defines nothing, so it is deliberately not counted: 49 references \
         outside the guarded module still spell \
         `crate::config::root_agent::ROOT_AGENT_DIR_NAME` and resolve through \
         it.\n\
         \n\
         MORE THAN ONE ENTRY means it was copied rather than moved. NO ENTRY \
         means it was renamed, deleted, or spelled in a way this scan does not \
         recognise, and a guard that finds nothing must fail rather than pass. \
         The list is asserted by equality and not counted: an equal count is not \
         an equal set.\n\
         \n\
         OBSERVED: {homes:?}"
    );
}
```

---

## 6. Required behaviour, edge cases, failure behaviour

**Required behaviour: byte for byte identical observable behaviour.** This is a relocation of one string
constant, not a rewrite.

| Property | Requirement |
|---|---|
| The constant's value | `"ac-root-agent"`, unchanged, and it is the same single definition |
| The 14 rules `required_rules` builds | identical strings, identical order, for every `agent_local_dir` |
| Rule 0 | `/ac-root-agent/<escaped agent local dir>/config.json`, unchanged |
| Rule 1 | `/ac-root-agent/config.json`, unchanged |
| When the seeding runs | `init_logger_inner`, `logging.rs:481`, unchanged; the call is not moved |
| What a failure does | swallowed into the existing `eprintln!` warning, unchanged |
| `crate::config::root_agent::ROOT_AGENT_DIR_NAME` as a path | still resolves, through the re-export |
| The 31 bare references inside `root_agent.rs` | still resolve, through the re-export |

**Edge cases, all preserved as they are today, none to be "fixed" in this change:**

- **An agent local directory name carrying `.gitignore` metacharacters** is escaped by
  `escape_gitignore_path_segment` exactly as before. The escaping code is not touched, and the tests at
  lines 799 and 872 that pin it are only repointed, never rewritten.
- **A name containing a line break, a path separator or NUL** is still rejected before any file is
  created.
- **An existing partial `.gitignore`** is still repaired by appending only the missing rules, and a
  complete one is still byte stable across repeated calls.
- **The config directory being unavailable** still returns the same `Err` string from
  `ensure_instance_gitignore`.
- **A locked or read-only target** still fails fast with the same messages.

**Failure behaviour, preserved.** Do not add logging, error propagation or a return value while moving
this constant. Changing failure behaviour inside a structural fix makes the change unreviewable against
its own acceptance criteria.

**No behavioural test is added.** The module already has fourteen behavioural tests covering exactly the
rules this constant appears in, and six of them are edited by Section 5.3 only in the path they use to
name it. Their assertions, including the literal expected `.gitignore` bytes at lines 463 to 478, are
untouched, so they are the behavioural proof that the relocation changed nothing.

---

## 7. Compatibility, security, and the complete arc enumeration

### 7.1 Arcs added and removed

**One line changes in `src-tauri/module-arcs.txt`, and this is all of them.** Produced by simulating this
exact change over the committed record and re-running Tarjan.

**Removed (1):**

```
agentscommander_lib::config::instance_gitignore -> agentscommander_lib::config::root_agent
```

Currently line 548. Cause: the only references to that path are gone, all eight of them.

**Added (0).**

| Change | Why it adds no arc |
|---|---|
| `pub const ROOT_AGENT_DIR_NAME` in `src/config/mod.rs` | a `const` with a literal initializer names nothing, and `config` keeps zero outgoing arcs |
| `pub use crate::config::ROOT_AGENT_DIR_NAME;` in `src/config/root_agent.rs` | `config::root_agent -> config` already exists, record line 568 |
| `super::ROOT_AGENT_DIR_NAME` at the target's 8 sites | `config::instance_gitignore -> config` already exists, record line 547, carried by `super::config_dir()` and `super::agent_local_dir_name()` |

Net: **976 arcs to 975**. Line 548 disappears and no line takes its place; every other line keeps its
content, and the lines after 548 shift up by one because the record is sorted and rendered fresh.

**Adding `src-tauri/tests/instance_gitignore_layering.rs` must not change that diff at all.** Integration
test targets are separate leaf crates the instrument marks `enabled: opts.includeTests`, and the record
is emitted with `includeTests: false`. Measured on the current tree: `module-arcs.txt` holds zero arcs
from `tests/` while `src-tauri/tests/` holds 22 files. If the arc diff shows anything attributable to the
new test file, the instrument was run with the wrong flags; re-read Section 9.2 before touching anything
else.

### 7.2 Compatibility

- **Frontend: no change, and none is permitted.** No IPC command, event, payload or type is touched.
- **Rust API path change: none that breaks.** `agentscommander_lib::config::root_agent::ROOT_AGENT_DIR_NAME`
  still resolves, through the re-export, and `agentscommander_lib::config::ROOT_AGENT_DIR_NAME` is a new
  additional path to the same item. The library is internal to this app.
- **No config, schema, file format or persisted state is touched.** The `.gitignore` this module writes
  is byte identical.

### 7.3 Security

No new surface. The constant was `pub` in a `pub` module and is `pub` in a `pub` module after. It is a
directory name, not a credential, not a path built from user input, and it is not reachable from either
IPC transport. Nothing about what is written to disk, or where, changes. The escaping of the agent local
directory segment, which is the only security relevant code in the module, is not touched.

---

## 8. Implementation order

Each step leaves the tree in a state the next one can check.

1. Apply the insertion of Section 5.1 to `src-tauri/src/config/mod.rs`.
2. Apply the replacement of Section 5.2 to `src-tauri/src/config/root_agent.rs`.
3. Apply the eight substitutions of Section 5.3 to `src-tauri/src/config/instance_gitignore.rs`, then
   confirm on the spot: from `src-tauri`, `rg -n "root_agent" src/config/instance_gitignore.rs` must
   produce **no output and exit 1**.
4. Create `src-tauri/tests/instance_gitignore_layering.rs` with the content of Section 5.4, verbatim. It
   is already rustfmt clean; reflowing it by hand changes the bytes this plan certified.
5. From `src-tauri`: `cargo fmt --check`. **This runs first, before the compiler.** It exited 0 on the
   tree before this change, measured on this branch at `55e49b0f`, and every edited and new file is
   rustfmt clean as written, so a diff here means a file was retyped rather than copied. It is the
   cheapest step and it is the one that catches the copy error, which is the error that would otherwise
   waste a full compile.
6. From `src-tauri`: `cargo check --all-targets`.
7. From `src-tauri`: `cargo clippy --all-targets -- -D warnings`.
8. From `src-tauri`: `cargo test --lib --bins --tests`. The new guard is an integration test target, so
   it runs under `--tests`.
9. From the repo root: `npm run typecheck`, `npm test` and `npm run test:debt`. All three must pass
   unchanged; the frontend is not edited, and these run because CI runs them.
10. **Prove the guard is alive**, in the three parts of Section 9.3.5. One probe per test, and **all
    three compile**, which is the whole point: a probe that does not compile proves nothing about the
    guard, because the guard's binary never links and never emits a verdict.

    **10a, the guarded module.** Put the pre-#1273 spelling back at
    `src-tauri/src/config/instance_gitignore.rs:105`:

    ```rust
                super::root_agent::ROOT_AGENT_DIR_NAME,
    ```

    This **compiles**, because `config::root_agent` re-exports the constant (Section 4.4, verified in a
    minimal crate). Run `cargo test --test instance_gitignore_layering` and confirm
    `instance_gitignore_names_nothing_that_reaches_the_knot` **fails with the #1273 message and names
    `src/config/instance_gitignore.rs`**. Then remove the probe and confirm green.

    **10b, the constant's home.** Add to `src-tauri/src/config/mod.rs`, immediately above
    `use std::sync::OnceLock;`:

    ```rust
    use crate::session::manager::SessionManager;
    ```

    This compiles; it raises an `unused_imports` warning, which `cargo test` does not deny. It is the
    exact three-word shape that undid #1265 while its guard stayed green. Run the same command and
    confirm `the_constant_home_names_nothing_at_all` **fails on the crate-anchored equality and names
    `src/config/mod.rs`**. Then remove the probe and confirm green.

    **10c, the duplication check.** Create `src-tauri/src/decoy_1273.rs` containing one line:

    ```rust
    pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";
    ```

    No `mod` declaration is added, so rustc never compiles it and the crate builds unchanged. Run the
    same command and confirm `the_root_agent_dir_name_constant_is_defined_exactly_once` **fails and
    lists both `src/config/mod.rs` and `src/decoy_1273.rs`**. Then **delete the file** and confirm
    green.

    Use `cargo test --test instance_gitignore_layering` and not `cargo test --tests` for this loop: it
    targets the one binary and takes seconds, where `--tests` builds and runs all 23 integration
    targets. Step 8 already covers the full suite.

    **If any of 10a, 10b or 10c does not go red, stop and report.** A guard that cannot fail is worth
    nothing and the rest of the verification is void.

    **And if one of them fails to build instead of going red, that is also a stop.** An
    `error[E0603]`, an `error[E0433]` or any other compilation failure is **not** the guard going red:
    the binary never linked and emitted no verdict at all. Record what actually happened and report it,
    rather than reading a failed run as a successful probe.

11. **Confirm all three probes are out of the tree before measuring anything.** Run
    `git status --porcelain` and confirm exactly four paths are modified or added: the three source
    files and the new test file. `src/decoy_1273.rs` must be gone. Then run
    `git diff -- src-tauri/src/config/instance_gitignore.rs` and confirm it shows **only** the eight
    substitutions of Section 5.3 and nothing else. Regenerating the arc record with a probe still in
    place produces a record containing the very arc this change removes. Criterion 5 would catch it,
    but this step exists so nothing depends on that.
12. Regenerate the arc record (Section 9.2).
13. Verify the graph shape and the levels (Section 9.5), then review
    `git diff -- src-tauri/module-arcs.txt` against Section 7.1: exactly **one line removed and none
    added**.
14. Commit the three source files, the new test file, `src-tauri/module-arcs.txt` and this plan. Delete
    the emitted graph. **Never commit a graph:** it is about 4.9 MB, it carries the absolute path of the
    machine that produced it, and it is CRLF sensitive.

    **This plan needs `git add -f`, and it runs BEFORE the commit.** `.gitignore` line 11 ignores
    `plans/`, so a plain `git add plans/1273-extract-instance-gitignore-from-scc.md` does nothing while
    `git status` stays clean and the file is silently left out. That is measured, and it is exactly what
    happened to `plans/1252-break-loops-scheduler-cycle.md`. The order for this step is therefore:

    ```
    git add -f plans/1273-extract-instance-gitignore-from-scc.md
    git add src-tauri/src/config/mod.rs src-tauri/src/config/root_agent.rs \
            src-tauri/src/config/instance_gitignore.rs \
            src-tauri/tests/instance_gitignore_layering.rs src-tauri/module-arcs.txt
    git commit -m "..."
    git show --stat
    ```

    The `git show --stat` is part of this step, not a follow-up: **confirm the plan is in the commit
    before reporting the step done.** The commit message must say that the knot goes 88 to 87, that
    `cyclicSccs` stays at 1, and that this is a rehearsal of the procedure rather than a perceptible
    improvement.

15. `git push -u origin refactor/1273-extract-instance-gitignore-wg11`. **That is the end state.** No
    PR, no merge, no `--admin`, no `gh pr create`. Never touch `main`. **Issue #1273 stays OPEN**; no
    closing keyword anywhere.

If step 5, 6, 7, 8 or 9 fails, fix it before continuing.

**If step 13 disagrees with Section 7.1, revert `src-tauri/module-arcs.txt` before reporting, then
stop.** `git checkout -- src-tauri/module-arcs.txt`. Do not adjust this plan's numbers to match the
output, and do not leave the regenerated record sitting in the tree: a modified arc record is exactly
what criterion 6 fails on, so leaving it there means the next person reads a criterion 6 failure that has
nothing to do with the disagreement being reported. Report the disagreement on a clean tree.

---

## 9. Tests and acceptance criteria

### 9.1 What must be green

| Command | Working directory | Expectation |
|---|---|---|
| `cargo fmt --check` | `src-tauri` | clean; measured exit 0 on this branch at `55e49b0f` before the change |
| `cargo check --all-targets` | `src-tauri` | clean |
| `cargo clippy --all-targets -- -D warnings` | `src-tauri` | clean |
| `cargo test --lib --bins --tests` | `src-tauri` | full suite green, including all three new tests of Section 5.4 |
| `npm run typecheck` | repo root | clean |
| `npm test` | repo root | full vitest suite green |
| `npm run test:debt` | repo root | clean; this change adds no ignored or placeholder test |

**"Green after" only means something if somebody measured green before.** `cargo fmt --check` was
measured at exit 0 on this branch at `55e49b0f` for this plan. The full suite was not re-measured here;
with a cold `target/` it is a full Tauri build and takes tens of minutes. **Measure the baseline before
starting**, or be ready to attribute any failure honestly.

**Pre-existing failures are not this change's business.** There are open issues for tests that fail under
load (#1261, #1258, #1256, #1255, #1254, #1253, #1248, #1241 and others). If a failure appears at the end
of implementation, **identify it against that list before calling it a regression from this change**; a
flake from that set is not evidence about #1273 either way, and reporting it as one wastes the review.

`test:debt` scans `src-tauri/tests/*.rs` as well as `src-tauri/src/**.rs`, so it does read the new guard.
It reports `#[ignore]` attributes and placeholder bodies (`todo!()`, `unimplemented!()`, a
`panic!("TODO...")`). The guard has none: all three of its tests have `assert!`/`assert_eq!` bodies and
none carries `#[ignore]`, and the `panic!` calls inside its resolver carry real failure messages and are
not placeholders. The same scanner already reads `project_settings_layering.rs`, which is built from the
same matcher including the four `'"'` character literals in `scrub`, and passes. Run it anyway; this
paragraph says why it is expected to pass, not that it may be skipped.

Four existing tree-scanning tests were checked against this change and none is expected to move:

- `src-tauri/tests/loops_layering.rs` scans `src/loops/` only, which this change does not touch.
- `src-tauri/tests/project_settings_layering.rs` reads `src/commands/project_settings.rs`,
  `src/web/event_broadcast.rs` and, for its third test, every file under `src/`. This change edits none
  of the first two, and its third test matches only `fn broadcast_all`, which appears in neither
  `config/mod.rs` nor `config/root_agent.rs` nor `config/instance_gitignore.rs`.
- `src-tauri/tests/pty_writer_inventory.rs` walks all of `src/` but matches only four spellings:
  `write_with_permit(`, `backend.write(`, `route_guard.write(` and `for_route_guard`. None appears in
  any file this change edits.
- `session::selection`'s `production_selection_and_lifecycle_sources_have_one_owner` fires on four
  `session_*` event literals and six manager mutator signatures; none appears in the edited files.

The three tests of Section 5.4 are the only tree-scanning tests this change adds, and together they run
in **1.50 s** as measured in the laboratory against a copy of the real `src-tauri/src/`.

### 9.2 Regenerating the arc record

From the repository root, with

```
VAULT = <workgroup>/repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust
GRAPH = an absolute path OUTSIDE the working tree, e.g. %TEMP%\ac-1273\graph.json
```

```
node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "<GRAPH>" --quiet
npm run record:arcs -- --graph "<GRAPH>"
```

Then delete `<GRAPH>`.

- **The detector exits 1 while any cycle remains, and writes the graph anyway. After this fix it will
  still exit 1**, because the 87 module knot survives. Only exit **3** means no graph was written. Do
  not read that 1 as a failed change.
- The run also emits `duplicate-function-node` warnings. They are a `warn`, never an `error`, and do not
  abort the write. They make `functions[]` a lower bound and touch nothing this plan gates on.
- Every flag above is part of the measurement. `scripts/02-module-arc-record.mjs` refuses a graph whose
  `target.rootPath` last segment is not `src-tauri`, or whose `crateDiscovery`, `includeTests` or
  `excludes` differ, with exit 3. Do not add flags.
- **Emit outside the working tree.** `src-tauri/module-arcs.txt` is pinned to LF in `.gitattributes` line
  8; do not defeat that.
- **Never diff the instrument's suggested cut between runs.** The minimum feedback arc set is one of
  several valid cuts and its membership is not unique, so an unchanged knot re-reports with different
  arcs. Compare arc sets and SCC membership, which is what every criterion below does.

### 9.3 The structural guard, and what it can and cannot prove

The instrument that would catch a reintroduced arc is run by hand and is deliberately not wired to CI, so
a guard inside the suite is the only thing that fires without somebody remembering to look. That guard is
`src-tauri/tests/instance_gitignore_layering.rs`. **Its content is Section 5.4 and is not repeated
here.** This section is the reasoning behind it, and it is the part to read before touching the matcher.

#### 9.3.1 What the guard is

A net over the **spellings** a dependency can be written in. Not a proof that the dependency cannot
return. It matches text and does not resolve names, so a spelling it does not know about passes it. A
green result means "no known spelling is present". It never means "the cycle is impossible".

The authoritative check is the cycle detector of Section 9.2, whose `coverage.graphShape.cyclicSccs` must
stay at 1 with `sccSize(agentscommander_lib::config::instance_gitignore) = 1`. The guard says so about
itself, in its module header and again in the `SCOPE` paragraph of its own failure message, where a doc
comment would not be printed. **Report the guard's green and the detector's numbers as two separate
things.**

#### 9.3.2 Why arc absence alone is not enough

`src-tauri/module-arcs.txt` is produced by an instrument with a measured blind spot: `src/lib.rs:1178`
constructs `loops::scheduler::LoopScheduler::new()` with no `crate::` prefix and no corresponding arc
exists among the 976. An arc absent from the record is therefore not by itself proof that the dependency
is gone: rewriting the same reference with an unanchored path deletes the arc and leaves the cycle intact
in the code, which is exactly the cosmetic satisfaction `break-dependency-cycles` Section 8 warns about.

That is why criterion 5 is backed by criterion 8, and why the **first** assertion in the guard is
deliberately anchorless.

#### 9.3.3 What the matcher does, and the classes it closes

1. **The use-tree.** `use super::{config_dir, root_agent::ROOT_AGENT_DIR_NAME};` does not contain the
   text `super::root_agent` at all: the braces are in the way, and rustfmt may reflow it across four
   lines. The guard collapses whitespace, deletes the space either side of `::`, `{`, `}` and `,`, then
   walks the brace group balanced, splits on its own top level commas and takes the leading identifier of
   each item. A leading `r#` is stripped first. Probes P6 and P7.
2. **Conditional compilation.** The guard is an integration test target outside the tree it reads, so it
   never has to excise itself and never cuts a file at its first `#[cfg(test)]`. It reads whole files,
   test regions included. That is stricter than the detector, which ignores them, and strictness is the
   safe direction: a false red is argued about, a false green is believed. It is also why all six test
   references of Section 5.3 must be repointed.
3. **The module tree.** The guard does not walk a directory. It resolves files by following `mod`
   declarations down from `src/lib.rs`, honouring `path = "..."` in both `#[path]` and
   `#[cfg_attr(..., path = ...)]`, collecting **every** declaration of a segment rather than the first, and
   handling both the `x.rs` and `x/mod.rs` forms. A `mod` declaration that resolves to no existing file,
   to two existing files, or that sits inside an inline `mod` block, is a hard failure naming the file,
   never a skip, so an unresolvable tree cannot produce a quiet green. Probes P18, P19 and P20.
4. **The anchorless class, and this is the one this issue needed.** The two equalities below cover
   `crate::` and `super::`. Neither covers a fully unanchored `root_agent::ROOT_AGENT_DIR_NAME`, which is
   the spelling the arc record itself cannot see (Section 9.3.2) and which becomes writable the moment a
   glob puts the name in scope. So the guard also matches the bare identifier `root_agent` anywhere in
   the module's code, under every anchor and under none, and that assertion carries the #1273
   explanation. Probe P11. Comments and literal bodies are removed first, so the string `"ac-root-agent"`
   and the many `.gitignore` fixtures in the module's own tests do not match it. Probe P16.

**Both anchors are asserted by equality, from day one, and that is the load-bearing requirement.** Every
reference this module makes is written `super::` or `super::super::`; it contains no `crate::` at all. A
`crate::`-only guard would therefore observe **nothing** here and pass everything vacuously. Conversely a
`super::`-only guard would miss the whole `crate::` surface. So:

- `ALLOWED_GUARDED_CRATE_REFERENCES` is **empty**, which refuses every `crate::`-anchored reference
  rather than only a reference to `config::root_agent`. The module's exposure is that it may name **any**
  of the 87 remaining knot members and fall straight back in; `root_agent` is only today's spelling.
- `ALLOWED_GUARDED_SUPER_REFERENCES` has six rows and is where this module actually lives.

**This is the assertion #1268 is about, and the precedent is why it is here.**
`project_settings_layering.rs` asserts `seen.anchored` for its emitter module but **collects and never
asserts it for its guarded module**. Its own blind-spot entry 13 records the measurement: adding
`use crate::session::manager::SessionManager;` to `src/commands/project_settings.rs` moved the knot 88 to
89, `sccSize` 1 to 89 and the arc count 976 to 977, with that file **green throughout, 3 passed 0
failed**. `loops_layering.rs` has the same weakness in a different form: it has no `crate::`-anchored
observation at all, its only anchor is `commands::`, and it does not list that among its blind spots.
Probe P3 is the same three words against this guard, and it is red.

**Membership, not counting.** Every set is asserted with `assert_eq!`, expected against observed, so **a
set that shrinks fails too**. `ROOT_AGENT_DIR_NAME` is listed because #1273 put it there: if it silently
disappears, the name is being reached some other way and the reason this module is out of the cycle has
changed without anybody saying so. Probe P12. `assert!(!files.is_empty(), ...)` and the resolver's hard
failure do the same job one level up: **an empty scan cannot pass**.

The pair, and not the child alone, is the contract. Keying on the child would make the observed set a
union over every scanned file, so a reference added to a future submodule would leave the set unmoved and
pass. That defect was found in the #1252 guard after review; this one is written with the fix from the
start.

**The second test is the one Section 4.3 depends on.** The non-absorption argument is that
`agentscommander_lib::config` has zero outgoing arcs, and **that premise fails on an outgoing arc, not an
incoming one**. One `use crate::<knot member>::...;` in `src/config/mod.rs` puts `config` into the knot, and
`config::instance_gitignore` follows it back in through arc 547, which this change never removes and
cannot remove. Every assertion of the first test stays green throughout, because the guarded module's own
file did not change. Probe P4. It reads `src/config/mod.rs` and **nothing below it**: every child of
`config` is a separate module in the graph with its own arcs and 21 of them are knot members, so
descending would assert that the whole `config` subtree names nothing, which is neither true nor this
issue's business.

**The third test** closes the duplication hole criterion 8 names. It reads every file under `src/`,
filtered by nothing, and asserts the list of files defining the constant **equals**
`["src/config/mod.rs"]`. `const` and `static` both count, a following `:` is what makes it a definition,
and the re-export `pub use crate::config::ROOT_AGENT_DIR_NAME;` in `src/config/root_agent.rs` deliberately
does **not** count: it names the identifier and defines nothing. Probes P13, P14 and P15. No entry fails
as loudly as two.

#### 9.3.4 Proving the guard is alive: 22 probes, all measured

Every row below was measured before this plan was certified, by compiling Section 5.4 verbatim in a
laboratory crate and running it against a **copy** of the real `src-tauri/src/` (189 files) carrying the
change of Sections 5.1 to 5.3 plus the injected spelling. **This is not a prediction.**

**Read the last column before running any of these.** The probes were measured as a **text scan over a
copied tree, where nothing is compiled**. On the real tree, `cargo test` compiles the library first, and
several of these spellings do not compile. That does not invalidate them as scan probes: it means the
observable result of running them through `cargo test` is a build error, not a guard verdict. **If you
run a "scan only" row and see a compilation failure, that is the row behaving as documented, not the
guard broken.** The three probes of Section 9.3.5, which are the ones Section 8 step 10 requires, are
chosen precisely because they compile.

| # | Injected spelling | Measured result | Runs as a build? |
|---|---|---|---|
| P0 | the tree as this plan leaves it | **green**, all three tests, 1.50 s | yes |
| P1 | line 105 restored to `super::root_agent::ROOT_AGENT_DIR_NAME` | red, **#1273 message**, names `src/config/instance_gitignore.rs` | **yes**, verified in a minimal crate: the re-export keeps it resolving. This is step 10a |
| P2 | `use crate::config::root_agent::ROOT_AGENT_DIR_NAME;` added to the target | red, #1273 message | scan only |
| P3 | `use crate::session::manager::SessionManager;` in the target, the #1268 shape with no `root_agent` token | red, **crate-anchored equality** | scan only |
| P4 | the same three words in `src/config/mod.rs` | red, **the constant home's crate-anchored equality**, names `src/config/mod.rs` | **yes**. This is step 10b |
| P5 | `use super::logging::init_logger;` in `src/config/mod.rs` | red, the constant home's `super::` equality | scan only |
| P6 | `use super::{config_dir, root_agent::ROOT_AGENT_DIR_NAME};` in the target | red, #1273 message | scan only |
| P7 | `super::r#root_agent::ROOT_AGENT_DIR_NAME` | red, **#1273 message**, not the generic one | scan only |
| P8 | `super::root_agent /* detour */ ::ROOT_AGENT_DIR_NAME` | red, #1273 message | scan only |
| P9 | `use crate::config as c;` in the target | red, **rename message** | scan only |
| P10 | a **second** `use super::*;` at the top of the target file | red, **glob count**, observed 2 | scan only |
| P11 | the unanchored form: bare `root_agent::ROOT_AGENT_DIR_NAME` | red, **#1273 message**. This is the row the anchorless check exists for | scan only |
| P12 | all eight references replaced by the literal `"ac-root-agent"` | red, **`super::` equality, the shrinking set** | scan only |
| P13 | a duplicate `pub const ROOT_AGENT_DIR_NAME` parked in `src/decoy.rs` | red, **defined-exactly-once**, lists both files | **yes**, the file is not in the module tree so rustc never reads it. This is step 10c |
| P14 | the same duplicate written as a `pub static` | red, defined-exactly-once, lists both files | yes, same reason |
| P15 | the definition deleted from `src/config/mod.rs` | red, defined-exactly-once, observed `[]` | scan only |
| P16 | `root_agent::ROOT_AGENT_DIR_NAME` written only inside a comment and inside a string literal | **green**, correctly: neither is a dependency | scan only |
| P17 | a `.md` under `src/` with a stray `"`, **not** in the module tree | **green**, correctly, and reaching green required `crate_sources()` to walk the real 189 file tree successfully | yes |
| P18 | a `.rs` the module tree reaches whose string is never closed | red, **hard failure**, "this scan could not read it" | scan only |
| P19 | `mod instance_gitignore;` renamed in `config/mod.rs` | red, **resolver abort**: "declares no `mod instance_gitignore;`" | scan only |
| P20 | `#[path = "extra_probe.rs"] mod extra_probe;` in the target, forbidden reference at the destination | red, #1273 message, **names `src/config/extra_probe.rs`** | scan only |
| P21 | `super<U+200E>::root_agent::ROOT_AGENT_DIR_NAME` | red, #1273 message | scan only |
| P22 | `use super::{self as s};` in the target | red, **rename message** | scan only |

P0 and P17 are the two that matter most for the resolver: it is strict, and a strict resolver that
refused the real tree would be worse than the defect it closes. It does not refuse it. Every declaration
in the crate resolves, no file is nested in an inline block, and no `path` value has two candidates.

#### 9.3.5 The liveness protocol, and why it is three probes and not one

`break-dependency-cycles` Section 10 is one line: a green guard can be green because it is not looking,
so insert a probe carrying the forbidden reference and confirm the guard fails and names the file. Two
things this repository learned the hard way turn that into the procedure of Section 8 step 10.

**First, the probe must COMPILE.** #1265's original liveness step named a probe that could not compile
after the change: the path it restored had become private, so `cargo test` aborted in the library with
`error[E0603]` and the guard's binary never linked. The run failed, which **looks** like the probe
working, while the guard had in fact never been exercised. A probe that cannot distinguish "the guard
fired" from "the crate did not build" removes no assumption at all. Section 8 step 10 therefore names
three probes that compile, and says explicitly that a build error is a stop and not a pass.

That this is achievable here is a direct consequence of Section 4.4: because `config::root_agent`
re-exports the constant, `super::root_agent::ROOT_AGENT_DIR_NAME` still resolves after the change, so
the most faithful possible probe, the exact pre-#1273 spelling, both compiles and is caught.

**Second, one probe proves one test.** This guard has three tests watching three different premises, and
a probe for the first says nothing about the other two. 10a exercises the guarded module's assertions,
10b exercises the constant home's, 10c exercises the duplication check. All three must be seen red, then
removed, then seen green.

#### 9.3.6 What the guard does not cover, and how that list is maintained

**This is the part that is expected to grow, and growing it is not a plan change.**

The module header of Section 5.4 carries a numbered `KNOWN UNCOVERED SPELLINGS` list, thirteen entries
ending with `14. (append here: ...)`: re-export laundering through a third module, macro-generated paths,
`include!`, runtime indirection, `concat!`-built names, a `mod x;` nested inside an inline `mod y { ... }`,
NTFS alternate data streams, the fully unanchored path as a class, a glob moving rather than multiplying,
module-group aliasing beyond the spellings the matcher knows, `cfg`-arm attribution, a `#[cfg(test)]`
reference holding an equality up on its own, and an unanchored path in the constant's home.

**Two of those entries state a live weakness rather than a theoretical one, and both are deliberate.**

- **Entry 12.** Six of the eight references to the constant in the guarded module are inside
  `#[cfg(test)]`, so deleting the two production references would leave the pair
  `("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME")` standing and the equality green. Unlike
  the equivalent entry in `project_settings_layering.rs`, **that deletion can be made to compile**, by
  hard-coding the directory name in `required_rules`. The shrinking-set argument is correspondingly
  weaker for that one pair, and the module's fourteen behavioural tests are what actually hold the
  production references up.
- **Entry 13.** `src/config/mod.rs` already writes `profile::config_dir_name()` three times, a path
  beginning with neither `crate::` nor `super::`, which is precisely why it creates no arc and why that
  module measures zero outgoing arcs. A new unanchored path from there into a knot member would be
  invisible to the detector and to this guard alike. The two equalities on that file catch the anchored
  forms only.

**Review is expected to find more, and the ones it finds are declared, not hidden.** When a reviewer
demonstrates a spelling that puts the module back within reach of the knot and still passes, append one
entry to that list and nothing else. **Appending an entry is part of the review loop for #1273. It does
not require this plan to be reopened and does not invalidate its digest.** Widening the matcher to cover
a newly found spelling is the same: if it is a spelling, it belongs in the matcher or in the list.

**That last sentence only holds because the file, not this plan, is the canonical copy.**
`src-tauri/tests/instance_gitignore_layering.rs` is what runs; Section 5.4 is a verbatim snapshot taken
when the plan was certified. **Append to the file. Do not edit Section 5.4 to match, and do not read a
difference between them as a defect.**

The one thing that does require reopening the plan is a finding that the guard cannot be a text scan at
all. If that turns up, say so rather than building something more elaborate that still cannot carry it.

#### 9.3.7 The duplication with the two existing guards is accepted

`loops_layering.rs` is 569 lines and `project_settings_layering.rs` is 1434, and both already carry
`normalized`, `leading_segment`, `split_top_level`, `scrub`, `relative_of` and the same
`ANCHOR`/`ALLOWED_*` shape; `project_settings_layering.rs` also carries the module-tree resolver this
guard reuses. Section 5.4 is a third copy. That is roughly 3400 lines of near-duplicate scanner across
three integration test crates, and it is real.

It is accepted rather than fixed because integration tests are separate crates: sharing would need a
`tests/common/` module or an auxiliary crate, which is more scope than #1273 asked for and would put a
refactor of two existing guards inside a structural change that moves one constant. **Do not reopen this
during implementation or review.** #1265 said the conversation becomes unavoidable when a third guard of
this shape appears. This is that third guard, so **open an issue for it and reference this paragraph**,
and do not act on it here.

### 9.4 Objective acceptance criteria

Every number below was produced by re-running Tarjan over the committed record with this exact change
applied to the arc set, using an implementation written for this plan. Verify with:

```
node "<VAULT>/Levelization/02-levelize.mjs" rank "<GRAPH>"
```

reading `coverage.graphShape` and the `modules[]` entries.

**Criterion 1 is adapted and the standard one does not apply here.** `break-dependency-cycles` Section 7
is written for an SCC that vanishes entirely. This one does not vanish, it thins. Reading the surviving
cycle as a failure is the mistake this criterion exists to prevent.

| # | Criterion | Before | After, required |
|---|---|---|---|
| 1 | `coverage.graphShape.cyclicSccs` | 1 | **1**. It does **not** drop to 0 |
| 2 | Knot size **and its membership** | 88 | **87**, membership identical to the Section 2.3 list **minus exactly** `agentscommander_lib::config::instance_gitignore`, compared **set to set**. An equal count is not an equal set, and nothing may join |
| 3 | `sccSize(agentscommander_lib::config::instance_gitignore)` | 88 | **1** |
| 4a | level of `agentscommander_lib::config::instance_gitignore` | 3 (the knot's pseudo-level) | **1** |
| 4b | level of `agentscommander_lib::config::root_agent`, the cut counterpart | 3 | **3**, `sccSize` **87**, still in the knot |
| 4c | level of `agentscommander_lib::config`, the constant's new home | 0, `sccSize` 1 | **0, `sccSize` 1**, and still **zero outgoing arcs** |
| 5 | Arc record diff | | **exactly one line removed, `config::instance_gitignore -> config::root_agent`, and nothing added**. Total 976 to 975 |
| 6 | Arc record regenerated and committed | | `git status` empty on the final tree **and** `git show --stat` lists `plans/1273-extract-instance-gitignore-from-scc.md`. Both, see below |
| 7 | Suites of Section 9.1 | green | **green**, including all three tests of Section 5.4 |
| 8a | `rg -n "root_agent" src/config/instance_gitignore.rs` from `src-tauri` | 8 lines | **no output, `rg` exits 1** |
| 8b | `rg -n "const ROOT_AGENT_DIR_NAME" src` from `src-tauri` | one line, `src/config/root_agent.rs:13` | **one line, in `src/config/mod.rs`, and no other** |
| 8c | `rg -n "pub use crate::config::ROOT_AGENT_DIR_NAME" src` from `src-tauri` | no output | **one line, in `src/config/root_agent.rs`** |
| 8d | the guard's `the_root_agent_dir_name_constant_is_defined_exactly_once` | does not exist | **green** |
| 8e | the guard's `the_constant_home_names_nothing_at_all` | does not exist | **green** |
| 9 | Branch pushed to `origin`, issue #1273 still OPEN, no PR, no merge, `main` untouched | | **all five** |

**Criteria 1 and 4a are satisfied by doing nothing at all.** `cyclicSccs` is 1 today and would still be 1
if no code changed, and a module's own level moving is the arithmetic consequence of the arc going away.
State them, and put the rigour into **2, 3, 4b, 4c and 5**, which cannot be faked. In particular:

- **Criterion 2 is the one that catches a change with side effects.** Compare the 87 surviving members
  against the Section 2.3 list element by element. If any module other than the target left, or any
  module joined, something outside scope was touched: stop and report rather than explaining it.
- **Criterion 4b is the one that proves the cut went the right way.** The target lands at level 1,
  **below** the knot, and the counterpart stays at level 3, inside it. Both levels must be stated. Cut A
  would have satisfied "distinct levels" too, with the target at level 4 **above** the whole knot, which
  is the inversion Section 4.1 refuses. "Distinct" is not the criterion; the direction is.
- **Criterion 4c is the non-absorption premise, restated as a check.** If `config` ever shows an outgoing
  arc, Section 4.3 is void whatever the other numbers say.
- **Criterion 5 is enumeration, not a size limit.** One removal, zero additions. The minimal diff is a
  proxy, never the goal.
- **Criterion 8 is what makes criterion 5 mean something.** Arc absence alone is satisfiable by an
  unanchored rewrite with the cycle intact. 8a proves the reference is gone from the source rather than
  merely from the record; 8b and 8c together prove the constant **moved** rather than being duplicated:
  present at the destination, absent at the origin, with exactly one definition and one re-export.

**Criterion 6 needs both halves, and the `git status` half alone is the accident it exists to prevent.**
`plans/` is ignored by `.gitignore` line 11, so `git status` comes back empty whether the plan was
committed or not. So `git status` empty carries the arc record and the source files, and `git show
--stat` naming the plan file carries the plan. Report them as two observations.

### 9.5 Report to the tech lead

When implementation is done or blocked, reply to `AgentsCommander_ac:wg-11-dev-v5-team/tech-lead` with:

1. The commit SHA, and confirmation that the branch is pushed and that no PR exists.
2. **Criterion 2 in full**: the 87 surviving members compared set to set against the Section 2.3 list,
   stating explicitly that exactly one module left and none joined.
3. Criteria 1, 3, 4a, 4b, 4c and 5, each with its measured number, and the arc diff quoted.
4. Criterion 7: the suites, with the pass/fail counts, and any failure identified against the
   pre-existing list of Section 9.1 before it is called a regression.
5. Criterion 8a to 8e, with the `rg` output quoted.
6. **The three liveness probes of Section 8 step 10, each reported as what it was**: which test went red,
   what the message said, which file it named, and confirmation that the probe was removed and green
   restored. If any probe produced a build error instead of a verdict, say so plainly and treat it as a
   stop.
7. `git status` empty, and `git show --stat` listing the plan file.
8. The sentence that keeps the result from being misread: **the knot went 88 to 87, `cyclicSccs` stayed
   at 1, the remaining 87 module knot is untouched by design, and this is a rehearsal of the procedure
   rather than a perceptible improvement.**
9. Anything the run refused, and why, including any entry appended to the guard's
   `KNOWN UNCOVERED SPELLINGS` list.

**A status flag is not a report.** If blocked, say so and state whether it is infrastructure or the work.
