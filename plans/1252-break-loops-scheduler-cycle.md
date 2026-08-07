# Implementation Plan: #1252 Break the `loops::scheduler` to `commands::loops` return arc

Status: READY_FOR_IMPLEMENTATION

Lite path. Written and certified by the architect in a single pass at Step 4, then **recertified after
wg-1 review**, which approved the shape, the arc enumeration, both justifications and the structural
test, and added two requirements. Round 2 changed exactly three things and nothing else: the test's
failure message now explains why the rule exists (Section 9.3), criterion 2 became the three joint
conditions 2a, 2b and 2c with the anti-duplication check made objectively verifiable (Section 9.5), and
the `events.rs` header was reworded after the test, compiled and run for this plan, reported that
header as an offender against its own explanation (Section 9.3.7). The round-1 digest
`55DEE98C…49F0` is superseded. `dev-rust` implements from this document without enrichment;
`dev-rust-grinch` reviews after implementation.

**Round 3 recertifies after the implementation shipped and was reviewed.** The architecture, the arc
enumeration and criteria 1, 2a, 2c, 3, 4a/4b/4c, 5 and 6 were confirmed as measured and are unchanged.
What was defective was the body of the test that Section 9.3 specified: its matcher filtered on the
literal substring `commands::loops`, which a grouped import does not contain, and
`use crate::commands::{loops::LoopCronPreview, session::CreateSelectionIntent};` inserted into the
production region of `scheduler.rs` was measured passing the test, `cargo fmt --check` and
`cargo clippy -- -D warnings` while the detector reported `cyclicSccs = 2` with both modules back in
one SCC. Round 3 changes exactly five things and nothing else: the guard moves out of `events.rs` to
`src-tauri/tests/loops_layering.rs` and reads use-trees instead of substrings (Sections 5.5 and 9.3);
`events.rs` loses its test module and becomes production only (Section 5.1); the guard's failure
message gains a `SCOPE` paragraph declaring it a net over known spellings rather than a proof, with
the detector named as authoritative (Section 9.3), leaving the approved `WHY` and `INSTEAD` text
untouched; a one-line comment marks the private import in `commands/loops.rs` as structural
(Section 5.3, edit 5); and criterion 2b is restated over the new file (Section 9.5). The round-2
digest `579B9706…7289` is superseded.

This plan is the complete cold-start specification. Everything needed to implement, verify and
report is below: the exact file contents to write, the exact commands to run, the exact numbers to
expect, and the exact arcs the change adds and removes. There is nothing left to decide.

---

## 1. Issue and objective

**Issue:** #1252, https://github.com/mblua/AgentsCommander/issues/1252
**Branch:** `fix/1252-break-loops-scheduler-cycle`, already created from `f15f59a4`.

**Objective.** Remove the dependency arc

```
agentscommander_lib::loops::scheduler -> agentscommander_lib::commands::loops
```

and keep the forward arc `agentscommander_lib::commands::loops -> agentscommander_lib::loops::scheduler`
exactly as it is. The two modules form the only 2 member cyclic SCC in the crate; removing the return
arc dissolves it and takes `coverage.graphShape.cyclicSccs` from 2 to 1.

**This is a structural change with no behavioural change.** Every event the app emits today, with the
same name, the same payload and the same ordering, it must still emit after the change.

---

## 2. Evidence and identified cause

Measured on `f15f59a4`. The first two lines were verified by the tech lead and re-verified here
against the source and against the frozen dependency graph.

```
forward: src-tauri/src/commands/loops.rs:16   use crate::loops::scheduler::LoopScheduler;
return:  src-tauri/src/loops/scheduler.rs:520  crate::commands::loops::emit_loop_change(...)
```

`src-tauri/module-arcs.txt` records both directions, at lines 381 and 638 of 972 arcs.

### 2.1 The return arc is one call site with no `use`

`src/loops/scheduler.rs:510-529` defines a private free function `emit_transition`, which builds a
`LoopConfigDetails` and then calls, fully qualified:

```rust
crate::commands::loops::emit_loop_change(
    app, project_dir, dir, &config.loop_def.id, kind, Some(details.summary), message,
)
```

`emit_transition` has three callers, all inside `LoopScheduler`: `maybe_coalesce_pending` (line 297),
`record_missed_while_closed` (line 351) and `apply_delivery_report` (line 423). It is the only
reference from anywhere under `src/loops/` to `commands::loops`; verified by search, one hit.

The frozen graph confirms the shape of the arc precisely:

```json
{"from":"agentscommander_lib::loops::scheduler","to":"agentscommander_lib::commands::loops",
 "item":"emit_loop_change","file":"src/loops/scheduler.rs","line":520,"column":5,
 "kind":"path","cfgGated":false,"text":"crate::commands::loops::emit_loop_change"}
```

One site, not `cfg` gated.

### 2.2 The cause is where the emitter lives, not how it is called

`emit_loop_change` (`src/commands/loops.rs:319-352`) is not a command. It carries no `#[tauri::command]`
attribute, takes `&AppHandle` rather than being injected with one, and emits two events:

- `loop_event`, with the `LoopEventPayload` struct declared at `src/commands/loops.rs:55-63`;
- `ac_project_refresh_requested`, an inline `serde_json::json!` object whose `reason` field is built
  by the private helper `capitalize_reason` (`src/commands/loops.rs:354-360`).

It has six callers: five Tauri commands in `commands::loops` (`create_loop`, `update_loop`,
`delete_loop`, `toggle_loop`, `run_loop_now`) and `emit_transition` in `loops::scheduler`.

**So the emitter is shared infrastructure that happens to be parked inside the IPC command surface.**
The cycle is not caused by the scheduler doing something wrong; it is caused by the only shared
emitter living in the one module the domain must not depend on. `commands` is the Tauri IPC surface,
`loops` is domain logic, and a domain scheduler reaching up into the command layer to announce a
transition is an inverted dependency independently of what the graph says.

### 2.3 The two modules are outside the 89 module knot, and that is arithmetic, not assumption

`02-levelize.mjs rank` over the frozen graph reports:

```
modules 182, arcs 972, sites 3504, sccs 93, cyclicSccs 2,
arcsInsideScc 453, arcsBetweenSccs 519, quotientArcs 170
```

SCCs with more than one member: exactly two, `sccId 0` with **89** members and `sccId 37` with **2**.
89 + 2 = 91 modules inside cyclic SCCs, 182 - 91 = 91 singleton SCCs, 91 + 2 = 93 SCCs. The count
closes exactly, so the 2 member SCC is the whole of the second cyclic component, and neither of its
members is inside the knot. Its members are:

| module | rank | level | sccId | sccSize |
|---|---|---|---|---|
| `agentscommander_lib::commands::loops` | `[40]` | 4 | 37 | 2 |
| `agentscommander_lib::loops::scheduler` | `[40]` | 4 | 37 | 2 |

**They share a rank today because they share an SCC.** Acceptance criterion 4 is unsatisfiable while
the cycle exists and is satisfied by removing it, which is why it is the right criterion.

### 2.4 The frozen graph is the current graph

`Levelization/fixtures/ac-074bbed0.graph.json` was emitted at commit `074bbed0`. Its arc set was
compared line by line against `src-tauri/module-arcs.txt` at `f15f59a4`: **972 arcs on both sides, zero
arcs present in only one of them.** The module-to-module graph has not moved between those commits, so
every number this plan predicts by simulating over that fixture is a prediction about the current
tree, not an analogy. Section 9 states which of those numbers are gates and which are informative.

### 2.5 What the instrument does and does not count

Established by reading the emitted edges, and load bearing for Section 9:

- **`use` declarations are what get recorded, one site per imported symbol.** The arc
  `commands::loops -> config::loops` carries 26 sites, one for each symbol in the `use` at
  `src/commands/loops.rs:8-14`. The arc `commands::loops -> loops::scheduler` carries **1** site, the
  `use` at line 16, even though `LoopScheduler` is then named 5 times: the instrument does not resolve
  types, so uses of an imported symbol are not additional references.
- **References under `#[cfg(test)]` are not recorded.** `scheduler.rs` has a test module from line 553
  that imports from `crate::config::loops`, and the highest line on any edge out of that module is
  520. The graph is emitted with `includeTests: false`, which the arc record gates on.
- **Integration test targets are not recorded either, and that is stronger.** The instrument
  enumerates `tests/`, `benches/` and `examples/` roots as separate targets and marks each
  `enabled: opts.includeTests`, with the comment that they "are separate leaf crates. They cannot
  participate in a cycle with the lib." Measured: `src-tauri/module-arcs.txt` contains zero arcs whose
  endpoints come from `tests/`, while `src-tauri/tests/` holds 20 files. **This is why the guard of
  Section 9.3 lives at `src-tauri/tests/loops_layering.rs` and can name the forbidden path freely:
  adding a file there adds no arc and no module to the record.** It is also why that guard no longer
  has to excise itself from its own scan, which is what closed the second of the two defects round 3
  was reopened for (Section 9.3.4).
- **`mod` declarations do not create arcs.** `src/loops/mod.rs` declares three child modules and
  `agentscommander_lib::loops` is reported `isolated: true`, rank `[0]`. Adding a fourth `pub mod`
  line adds no arc.
- **Unanchored paths are not resolved.** `src/lib.rs:1178` writes `loops::scheduler::LoopScheduler::new()`
  without a `crate::` prefix and no `agentscommander_lib -> agentscommander_lib::loops::scheduler` arc
  exists. **This is a trap, not a technique:** see the prohibition in Section 3.2.

---

## 3. Scope

### 3.1 In scope

- `src-tauri/src/commands/loops.rs`
- `src-tauri/src/loops/scheduler.rs`
- `src-tauri/src/loops/mod.rs`
- `src-tauri/src/loops/events.rs` (new)
- `src-tauri/tests/loops_layering.rs` (new; the structural guard, Sections 5.5 and 9.3)
- `src-tauri/module-arcs.txt` (regenerated)
- This plan file.

### 3.2 Out of scope, and one hard prohibition

- **The 89 module knot (`sccId 0`) is untouchable.** Do not modify anything in it, and do not clean up
  anything adjacent to it opportunistically. If the knot's membership moves, something outside scope
  was touched: stop and report.
- **No new arc into `commands` from anywhere in `loops` or below.** Trading this cycle for another one
  is not a fix.
- The arcs `loops::delivery -> commands::pty` and `loops::delivery -> commands::session` already exist,
  are not part of any cycle, and are **not** touched by this change. They are out of scope.
- **Do not evade the detector.** Rewriting the call as `super::super::commands::loops::emit_loop_change`,
  or as an unanchored `commands::loops::emit_loop_change`, would make the arc vanish from the record
  while the dependency survives in the code. The arc must disappear because the call is gone. Every
  new reference this plan introduces is written as `use crate::...` for exactly this reason.
- No behavioural change, no new feature, no refactor of `emit_transition`, no change to the frontend,
  no change to `src/shared/types.ts`.
- **`I` (instability) justifies nothing here.** The `I` values of both groups were computed over a
  graph containing this cycle, so they are contaminated by the thing being removed. Cost and layering
  are the reasons; `I` is not, and must not appear in the implementation report either.

---

## 4. The decided solution

**Move the shared emitter down into a new module that both sides depend on:
`agentscommander_lib::loops::events`, at `src-tauri/src/loops/events.rs`.**

Three items move there verbatim from `commands::loops`: the `LoopEventPayload` struct, the
`emit_loop_change` function, and the private `capitalize_reason` helper. `commands::loops` and
`loops::scheduler` then both import `emit_loop_change` from it, and both depend downward.

`emit_transition` **stays** in `scheduler.rs`. It is the scheduler's private adapter, it builds the
summary from the scheduler's own `config` and `state`, and moving it would widen the change for
nothing.

### 4.1 Why this shape, by cost and by layering

**Cost.** The return arc is one fully qualified call site with no `use`. The cheapest correct
intervention is to relocate the callee so that call becomes a downward one. Both other shapes wg-1
listed cost far more for the same single call site, and both are therefore rejected:

- *Injecting an emission handle into the scheduler* means a new trait or handle threaded through
  `LoopScheduler::new`, `start`, `run_loop_now`, `scan_once`, `scan_project`, `scan_loop`,
  `maybe_coalesce_pending`, `record_missed_while_closed` and `apply_delivery_report`. That is nine
  signatures and a construction site in `lib.rs` to remove one call, and the scheduler already carries
  an `&AppHandle` through every one of them, so the injected handle would be a second path to the same
  runtime object.
- *Publishing on a channel that the command layer subscribes to* adds a channel, a subscriber task,
  and a shutdown story, and turns a synchronous emit into an asynchronous one. That is a new failure
  mode (a dropped or lagging receiver silently swallowing Loop events) introduced to fix a compile
  time dependency.

**Layering.** The one alternative with a cheaper arc diff is putting the emitter in the existing
`config::loops`, which both modules already depend on. That would produce a one line diff and is
rejected on two independent grounds:

1. **`config::loops` is inside the 89 module knot** (`sccId 0`, rank `[20]`, level 2). Moving code into
   the knot the project is trying to dismantle is the wrong direction, and it would put the emitter
   inside a cycle instead of merely out of one.
2. **`config::loops` is TOML persistence**: reading, writing and validating Loop config, state and
   audit files. Giving it `tauri::Emitter` and an `AppHandle` makes the persistence layer depend on the
   UI transport. That is a second layering inversion traded for the first one.

`loops::events` lands at rank `[30]`, level 3, `sccSize 1`, outside the knot, with one job. `loops`
already depends on Tauri pervasively (`scheduler` takes `&AppHandle` everywhere and uses
`tauri::Manager`; `delivery` takes `&AppHandle`), so hosting a Tauri emitter inside `loops` introduces
no dependency the group does not already have. What it removes is the dependency on the **command
surface**, which is the actual inversion.

**These alternatives are recorded as closed decisions, not as open options.** Do not reopen them
during implementation.

### 4.2 Accepted cost

The chosen shape adds three arcs and removes one, against the "ideal one line diff" of the rejected
`config::loops` placement. Criterion 6 of the dispatch allows this explicitly, on condition that every
arc is enumerated and justified in the plan rather than discovered in review. Section 7 is that
enumeration. None of the three added arcs points at `commands`, none creates a cycle, and all three
were verified by simulation before this plan was written.

---

## 5. Affected surfaces: exact files and symbols

### 5.1 New file: `src-tauri/src/loops/events.rs`

Create with exactly this content. The bodies of `emit_loop_change` and `capitalize_reason` are moved
verbatim from `src/commands/loops.rs`; do not reformat, reorder or "improve" them.

```rust
//! Loop change events, and the payload the frontend listens for.
//!
//! #1252: this module exists so `loops::scheduler` never has to reach up into the
//! Tauri command layer to announce a Loop transition. The command surface and the
//! scheduler both depend downward on this module, which owns the emitter, so neither
//! depends on the other to emit. It lives under `loops` rather than in
//! `config::loops` because `config::loops` is TOML persistence and is itself inside
//! the crate's 89 module knot: putting an IPC emitter there would trade one layering
//! inversion for another and move code into the cycle instead of out of one.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::loops::AcLoopSummary;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopEventPayload {
    pub kind: String,
    pub project_path: String,
    pub loop_id: String,
    pub summary: Option<AcLoopSummary>,
    pub message: Option<String>,
}

pub fn emit_loop_change(
    app: &AppHandle,
    project_path: &Path,
    changed_path: &Path,
    loop_id: &str,
    kind: &str,
    summary: Option<AcLoopSummary>,
    message: Option<String>,
) {
    let project_path = std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.into());
    let changed_path = std::fs::canonicalize(changed_path).unwrap_or_else(|_| changed_path.into());
    let project_path_string = project_path.to_string_lossy().to_string();
    let changed_path_string = changed_path.to_string_lossy().to_string();
    let _ = app.emit(
        "loop_event",
        LoopEventPayload {
            kind: kind.to_string(),
            project_path: project_path_string.clone(),
            loop_id: loop_id.to_string(),
            summary,
            message,
        },
    );
    let _ = app.emit(
        "ac_project_refresh_requested",
        serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "projectPath": project_path_string,
            "changedPath": changed_path_string,
            "changedName": loop_id,
            "reason": format!("loop{}", capitalize_reason(kind)),
        }),
    );
}

fn capitalize_reason(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Changed".to_string(),
    }
}
```

**That is the whole file: 69 lines, ending with the closing brace of `capitalize_reason`. Nothing is
appended to it.** The structural guard lives at `src-tauri/tests/loops_layering.rs` (Section 5.5),
outside the tree it scans, so it never has to remove itself from its own reading. If the tree already
carries a `#[cfg(test)] mod tests` at the end of this file from an earlier round, delete it: the whole
of it moves to the new file, in a wider form.

The header above names the Tauri command layer in words on purpose, and must keep doing so. The guard
scans this file, and the token it refuses is the path spelling, not the English phrase.

### 5.2 `src-tauri/src/loops/mod.rs`

Add one line, keeping the list alphabetical:

```rust
pub mod delivery;
pub mod events;
pub mod non_stop_watchdog;
pub mod scheduler;
```

### 5.3 `src-tauri/src/commands/loops.rs`

Five edits.

1. **Line 6**, drop `Emitter`, which is only needed by the `app.emit` calls that leave:
   ```rust
   use tauri::{AppHandle, State};
   ```
2. **Lines 8-14**, drop `AcLoopSummary` from the `use crate::config::loops::{...}` list. After the
   struct and the function move, its only two references in this file are gone, and
   `cargo clippy -- -D warnings` fails on an unused import. Every other symbol in that list stays.
3. **Line 16 area**, add the import above the existing scheduler import, alphabetical:
   ```rust
   use crate::loops::events::emit_loop_change;
   use crate::loops::scheduler::LoopScheduler;
   ```
4. **Delete three items**: `pub struct LoopEventPayload` together with its two attributes, which is
   **lines 55-63** counted from its `#[derive(Debug, Clone, Serialize)]`; `pub fn emit_loop_change`
   (lines 319-352); and `fn capitalize_reason` (lines 354-360). **Do not touch lines 48-53**: that is
   `LoopCronPreview`, which carries an identical pair of attributes and stays.
5. **Mark the new import as structural**, with exactly this comment on the line above it:
   ```rust
   // #1252: keep private. A `pub use` here would re-expose the emitter and kill the E0603 backstop.
   use crate::loops::events::emit_loop_change;
   ```

   Why the comment is required. Because this import is not `pub use`, `commands::loops` no longer
   defines the symbol and only holds it privately, so reintroducing the original call is reported as
   *measured* to fail with `error[E0603]: function import 'emit_loop_change' is private`. That is a
   real second line of defence, and it is a fragile one: a future `pub use` on this line removes it in
   complete silence, with no test and no reviewer prompt. A reader has no way to know that a
   visibility keyword is load bearing here unless the line says so.

   **Three constraints on the wording, each of which the plan checks elsewhere.** The comment must not
   contain the token `emit_loop_change`, because criterion 2c counts that token in this file and
   expects exactly 6. It must not contain the path spelling `commands::loops`, because this file is
   `src/commands/loops.rs` and a path in a comment is noise the arc record does not need. And it stays
   on one line, at 97 characters, so it fits rustfmt's 100 column default and `cargo fmt --check` and
   clippy are unaffected. Copy it verbatim rather than rewording it.

The five call sites of `emit_loop_change` (lines 110, 162, 191, 231, 255) are **not** edited: they are
unqualified calls and now resolve through the new import. `use serde::{Deserialize, Serialize};` stays
(`Serialize` is still used by `LoopCronPreview`). `use std::path::{Path, PathBuf};` stays (`Path` is
still used by `workspace_for_project` and `run_loop_now`). `serde_json` and `uuid` were referenced only
by fully qualified paths inside the moved function, so there is no import of either to clean up.

### 5.4 `src-tauri/src/loops/scheduler.rs`

Two edits.

1. **After line 19**, add the import, alphabetical among the `crate::loops::` imports:
   ```rust
   use crate::loops::delivery::{deliver_loop_prompt, LoopDeliveryReport};
   use crate::loops::events::emit_loop_change;
   use crate::shutdown::ShutdownSignal;
   ```
2. **Line 520**, inside `emit_transition`, replace the fully qualified call with the imported one. The
   argument list is unchanged:
   ```rust
   fn emit_transition(
       app: &AppHandle,
       project_dir: &Path,
       dir: &Path,
       config: &LoopConfigToml,
       state: &LoopState,
       kind: &str,
       message: Option<String>,
   ) {
       let details = details_from_parts(dir, config, state);
       emit_loop_change(
           app,
           project_dir,
           dir,
           &config.loop_def.id,
           kind,
           Some(details.summary),
           message,
       );
   }
   ```

Nothing else in `scheduler.rs` changes. In particular, the existing test
`scan_once_calls_archived_candidate_filter` (line 639) reads this file's own production source and
asserts on two strings inside `scan_once`; neither string is touched.

### 5.5 New file: `src-tauri/tests/loops_layering.rs`

The structural guard. Section 9.3 explains what it can and cannot do and why it is shaped this way;
this section is the content to write. **Create it with exactly these 330 lines.** They were compiled
and run before this plan was recertified: `cargo fmt --check` exits 0 on them as written, so do not
reformat them, and `cargo clippy --all-targets -- -D warnings` is clean.

```rust
//! #1252 layering guard: nothing under `src/loops/` may reach up into the Tauri
//! command surface, except through the references `loops::delivery` already
//! carried before that issue.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1. A green result here means
//! "no known spelling is present", never "the cycle is impossible".
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: `ALLOWED_COMMAND_CHILDREN` is the whole contract, and
//! the spellings the scan is known to miss are listed below instead of being
//! left unsaid.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that reaches the command surface from `src/loops/` and still passes this
//! file, it is appended here. Appending an entry is part of reviewing #1252 and
//! is expected; it changes nothing else.
//!
//!   1. Re-export laundering. A module outside `src/loops/` writes
//!      `pub use crate::commands::loops::...` and `src/loops/` imports from there.
//!      No `commands` token appears in the scanned files. The detector still
//!      catches it: the laundering module joins the cycle, so `cyclicSccs` rises.
//!   2. Macro-generated paths. A `macro_rules!` defined outside `src/loops/`, or
//!      any procedural macro, whose expansion contains the path. The text is not
//!      in the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `#[path = "..."]` modules. The scan walks the `src/loops/` directory, not
//!      the module tree. A `loops` submodule pointed by `#[path]` at a file
//!      outside that directory is never read.
//!   4. `include!`. A file textually included from outside `src/loops/` is never
//!      read, for the same reason.
//!   5. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in `commands::loops` and which is wired together
//!      outside `src/loops/`. No path text appears in the scanned files.
//!   6. (append here: one entry per spelling a reviewer proves still passes)

use std::path::{Path, PathBuf};

/// The children of `crate::commands` that `src/loops/` is allowed to name, sorted.
///
/// `loops::delivery` has referenced `commands::pty` and `commands::session` since
/// before #1252; neither is in a cycle. Every other child of `commands`, and
/// `loops` above all, is refused. Adding a name here is a deliberate decision to
/// accept a new upward arc from the domain into the IPC surface.
const ALLOWED_COMMAND_CHILDREN: [&str; 2] = ["pty", "session"];

/// The child #1252 removed, called out separately so its failure carries the
/// explanation of the cycle rather than the generic allowlist message.
const FORBIDDEN_COMMAND_CHILD: &str = "loops";

const ANCHOR: &str = "commands::";

/// Collapse every run of ASCII whitespace (newlines included, so this is also
/// CRLF-safe) to one space, then delete the space on both sides of the
/// punctuation a Rust path or use-tree is built from.
///
/// This is what widens the net past a raw substring match. `use
/// crate::commands::{loops::A, session::B};` does not contain the text
/// `commands::loops` at all: the braces are in the way. Reflowed across lines by
/// rustfmt it does not contain it either. After normalization every one of those
/// forms is the same text, and the use-tree can be read.
fn normalized(body: &str) -> String {
    let mut out = body.split_whitespace().collect::<Vec<_>>().join(" ");
    for token in ["::", "{", "}", ","] {
        out = out.replace(&format!(" {token}"), token);
        out = out.replace(&format!("{token} "), token);
    }
    out
}

/// Whether the source renames the whole command group, as in
/// `use crate::commands as c;`.
///
/// After such a rename `c::loops::...` reaches the forbidden module under a name
/// no text scan can follow, so the rename itself is refused instead of followed.
/// Anchored on the path punctuation in front of `commands` so that English prose
/// about commands does not trip it.
fn aliases_the_command_group(body: &str) -> bool {
    ["::commands as ", "{commands as ", ",commands as "]
        .iter()
        .any(|spelling| body.contains(spelling))
}

/// The leading identifier of a use-tree item: `loops` from `loops::{a, b}`, from
/// `loops as l` and from `loops`. A non-identifier item such as `*` is returned
/// as itself, so a glob is reported rather than silently dropped.
fn leading_segment(item: &str) -> String {
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
/// as `loops::{a, b}, session::c` yields two items and not three.
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

/// Every child of `commands` named anywhere in `body`, which must already be
/// normalized, in source order.
///
/// An unclosed group is an error rather than an empty result: a scanner that
/// cannot delimit what it is reading must say so, because the alternative is a
/// green result that proves nothing.
fn command_children(body: &str) -> Result<Vec<String>, &'static str> {
    let mut children = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(ANCHOR) {
        let anchor_at = from + offset;
        let after = anchor_at + ANCHOR.len();
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
                return Err("a `commands::{` group is never closed, so the scan cannot be trusted");
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

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

fn relative_of(path: &Path) -> String {
    path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("source is below manifest directory")
        .to_string_lossy()
        .replace('\\', "/")
}

/// #1252: `loops::scheduler` used to call `crate::commands::loops::emit_loop_change`,
/// which made the domain depend on the Tauri command surface and put the two
/// modules in a 2 member cycle. The emitter moved to `loops::events` so both
/// sides depend downward.
///
/// This test lives outside `src/loops/` on purpose. The first version lived
/// inside the scanned tree, so it had to excise itself before scanning by
/// cutting each file at its first `#[cfg(test)]`; any production code below a
/// mid-file test helper was invisible to it. Scanning from outside removes the
/// need for that cut, so whole files are read, test regions included. That is
/// stricter than the detector, which ignores `#[cfg(test)]` items, and strictness
/// is the safe direction for a guard: a false red is argued about, a false green
/// is believed.
#[test]
fn no_loops_source_reaches_into_the_command_surface() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loops");
    let files = rust_sources(&root);
    assert!(
        !files.is_empty(),
        "no Rust sources found under src/loops; the scan proves nothing"
    );

    let mut observed: Vec<String> = Vec::new();
    let mut loops_offenders: Vec<String> = Vec::new();
    let mut unlisted_offenders: Vec<String> = Vec::new();
    let mut alias_offenders: Vec<String> = Vec::new();

    for path in &files {
        let relative = relative_of(path);
        let body = normalized(&std::fs::read_to_string(path).expect("read Rust source"));
        let children =
            command_children(&body).unwrap_or_else(|reason| panic!("{relative}: {reason}"));
        if children
            .iter()
            .any(|child| child == FORBIDDEN_COMMAND_CHILD)
        {
            loops_offenders.push(relative.clone());
        }
        if children
            .iter()
            .any(|child| !ALLOWED_COMMAND_CHILDREN.contains(&child.as_str()))
        {
            unlisted_offenders.push(relative.clone());
        }
        if aliases_the_command_group(&body) {
            alias_offenders.push(relative.clone());
        }
        observed.extend(children);
    }
    observed.sort();
    observed.dedup();

    assert!(
        loops_offenders.is_empty(),
        "src/loops must not reference commands::loops.\n\
         \n\
         WHY: `loops` is domain logic and `commands` is the Tauri IPC surface. \
         The domain must not depend on the surface it is announced through. \
         Issue #1252 removed the one call that did, \
         `crate::commands::loops::emit_loop_change` in loops/scheduler.rs, \
         because it put those two modules in a dependency cycle: \
         commands::loops needs LoopScheduler, so the scheduler must not need \
         commands::loops back. Any reference from here rebuilds that cycle.\n\
         \n\
         INSTEAD: emit Loop events through \
         `crate::loops::events::emit_loop_change`, which the command layer and \
         the scheduler both depend on downward. If you need something from \
         commands::loops that is not an event, it belongs in a module below \
         both of them, never above.\n\
         \n\
         SCOPE: this is a net over the spellings of that reference, not a proof \
         that it cannot return. It matches text and does not resolve names, so a \
         spelling it does not know about passes it; the ones it is known to miss \
         are listed at the top of this file. The authoritative check is the cycle \
         detector, whose `coverage.graphShape.cyclicSccs` must stay at 1.\n\
         \n\
         OFFENDING FILES: {}",
        loops_offenders.join(", ")
    );

    assert!(
        alias_offenders.is_empty(),
        "src/loops must not rename the command module group.\n\
         \n\
         WHY: `use crate::commands as <name>;` puts every module under `commands`, \
         `commands::loops` included, within reach under a name this scan cannot \
         follow. Following it would mean resolving names, which a text scan does \
         not do, so the rename is refused instead.\n\
         \n\
         INSTEAD: import the item you need by its real path, so this guard and \
         the cycle detector can both see it.\n\
         \n\
         OFFENDING FILES: {}",
        alias_offenders.join(", ")
    );

    let expected: Vec<String> = ALLOWED_COMMAND_CHILDREN
        .iter()
        .map(|child| (*child).to_string())
        .collect();
    assert_eq!(
        observed,
        expected,
        "the set of command modules named from src/loops moved.\n\
         \n\
         FILES NAMING SOMETHING UNLISTED: {}\n\
         \n\
         A LARGER SET means src/loops gained a dependency on the Tauri command \
         surface. The two that are allowed, `commands::pty` and \
         `commands::session` in loops/delivery.rs, predate #1252 and are in no \
         cycle. A third is a decision, not a detail: remove it, or add its name \
         to ALLOWED_COMMAND_CHILDREN and say in the commit why a new upward arc \
         from the domain into the IPC surface is acceptable.\n\
         \n\
         A SMALLER SET is the more dangerous failure. It usually means the scan \
         stopped seeing references it used to see, because `commands` was \
         renamed or moved or because this matcher was narrowed, and a guard that \
         observes nothing passes everything. The known references are therefore \
         asserted by membership and not counted: an equal count is not an equal \
         set.",
        unlisted_offenders.join(", ")
    );
}
```

The file is pure ASCII on purpose, so no encoding or line ending question arises on Windows.

---

## 6. Required behaviour, edge cases, failure behaviour

**Required behaviour: byte for byte identical observable behaviour.** This is a move, not a rewrite.

| Property | Requirement |
|---|---|
| Event names | `loop_event` and `ac_project_refresh_requested`, unchanged |
| Emission order | `loop_event` first, then `ac_project_refresh_requested`, unchanged |
| `LoopEventPayload` fields and wire names | unchanged; `#[serde(rename_all = "camelCase")]` moves with the struct |
| `ac_project_refresh_requested` object | same five keys, same values, same fresh `uuid::Uuid::new_v4()` per call |
| `reason` value | `format!("loop{}", capitalize_reason(kind))`, unchanged |
| Broadcast scope | `app.emit`, reaching all windows, unchanged |
| Call sites | same six callers, same arguments, same order of evaluation |

**Edge cases, all preserved as they are today, none to be "fixed" in this change:**

- **A path that cannot be canonicalized.** `std::fs::canonicalize` failing falls back to the path as
  given, for both `project_path` and `changed_path`. `delete_loop` emits after `remove_dir_all`, so its
  `changed_path` is a directory that no longer exists and takes that fallback on every call. Keep the
  `unwrap_or_else(|_| path.into())` exactly as written.
- **A non UTF-8 path.** `to_string_lossy` substitutes replacement characters rather than failing.
- **An empty `kind`.** `capitalize_reason("")` returns `"Changed"`, so `reason` becomes `"loopChanged"`.
  No caller passes an empty kind today; the branch stays anyway.

**Failure behaviour, preserved:** both emissions are discarded with `let _ = app.emit(...)`. A failed
emit is not logged, not retried and not propagated, and the caller cannot observe it. **Do not add
logging, error propagation or a return value while moving this code.** Changing failure behaviour
inside a structural fix would make the change unreviewable against its own acceptance criteria.

---

## 7. Compatibility, security, and the complete arc enumeration

### 7.1 Arcs added and removed

This is criterion 6 of the dispatch. Four lines change in `src-tauri/module-arcs.txt`, and these are
all of them.

**Removed (1):**

```
agentscommander_lib::loops::scheduler -> agentscommander_lib::commands::loops
```
Currently line 638. Cause: the only call is gone.

**Added (3):**

```
agentscommander_lib::commands::loops -> agentscommander_lib::loops::events
agentscommander_lib::loops::events -> agentscommander_lib::config::loops
agentscommander_lib::loops::scheduler -> agentscommander_lib::loops::events
```

| Added arc | Cause | Why it is safe |
|---|---|---|
| `commands::loops -> loops::events` | `use crate::loops::events::emit_loop_change;` in `commands/loops.rs` | Points from the IPC surface down into the domain, the same direction as the forward arc that is being kept. `loops::events` does not depend on `commands`, so no cycle. |
| `loops::scheduler -> loops::events` | `use crate::loops::events::emit_loop_change;` in `scheduler.rs` | Sibling arc inside `loops`, pointing at a level 3 module from a level 4 one. `loops::events` does not depend on `scheduler`, so no cycle. |
| `loops::events -> config::loops` | `use crate::config::loops::AcLoopSummary;` in `events.rs`, required by the payload field | The same dependency `commands::loops` already had for the same symbol; it moves with the struct. `config::loops` does not depend on `loops::events`, so `loops::events` cannot be pulled into the 89 module knot that `config::loops` belongs to. Verified by simulation: `loops::events` comes out `sccSize 1`. |

**No arc points at `commands` from `loops` or below**, which is the dispatch's hard constraint.

Sorted position in the regenerated record, for reviewing the diff: the `commands::loops` line lands
between current lines 380 and 381; the `loops::events` line between 632 and 633; the
`loops::scheduler` line between 644 and 645; and current line 638 disappears. Net: 972 arcs to 974.

### 7.2 Compatibility

- **Frontend: no change, and none is permitted.** `src/shared/types.ts:1138` declares the TypeScript
  `LoopEventPayload`, `src/shared/ipc.ts:726-728` listens on `"loop_event"`, and
  `src/sidebar/loop-event-toast.ts` consumes it. The Rust struct keeps its name, its fields and its
  `camelCase` serialization, and the event names are unchanged, so the wire contract is untouched.
  There is no ts-rs, typeshare or specta in this crate (stated in the parity test at
  `src/testability/ui_automation.rs:2577`), so nothing regenerates from the Rust side and no generated
  artifact can drift.
- **Rust API path change.** `agentscommander_lib::commands::loops::emit_loop_change` becomes
  `agentscommander_lib::loops::events::emit_loop_change`, and `LoopEventPayload` moves with it. A
  repository wide search finds no other consumer of either symbol in any crate of the workspace, so
  nothing outside the two edited files can break. The library is internal to this app.
- **No config, schema, file format or persisted state is touched.**

### 7.3 Security

No new surface. `emit_loop_change` is not a `#[tauri::command]` and is not reachable from the
frontend; it was `pub` in a `pub` module before and is `pub` in a `pub` module after, so its effective
visibility is unchanged. No new capability, no new IPC entry point, no change to what is emitted or to
who receives it.

---

## 8. Implementation order

Each step leaves the tree in a state the next one can check.

1. Create `src-tauri/src/loops/events.rs` with the content of Section 5.1, and nothing else. It ends
   at the closing brace of `capitalize_reason`, 69 lines. If an earlier round left a
   `#[cfg(test)] mod tests` at the end of it, delete that module.
2. Add `pub mod events;` to `src-tauri/src/loops/mod.rs` (Section 5.2).
3. Apply the five edits to `src-tauri/src/commands/loops.rs` (Section 5.3).
4. Apply the two edits to `src-tauri/src/loops/scheduler.rs` (Section 5.4).
5. Create `src-tauri/tests/loops_layering.rs` with the content of Section 5.5, verbatim and
   unformatted. It is already rustfmt clean; running rustfmt over it changes nothing, reflowing it by
   hand changes the bytes this plan certified.
6. From `src-tauri`: `cargo check --all-targets`.
7. From `src-tauri`: `cargo clippy --all-targets -- -D warnings`.
8. From `src-tauri`: `cargo fmt --check`. It exited 0 on the tree before this change and the new file
   is rustfmt clean as written, so a diff here means step 5 was reformatted.
9. From `src-tauri`: `cargo test --lib --bins --tests`. The new guard is an integration test target,
   so it runs under `--tests`.
10. From the repo root: `npm run typecheck` and `npm test`. Both must pass unchanged; the frontend is
    not edited, and these run because CI runs them.
11. Regenerate the arc record (Section 9.4).
12. Verify the graph shape and the levels (Section 9.5).
13. Verify criterion 2c with the three `rg` commands of Section 9.5, then review
    `git diff -- src-tauri/module-arcs.txt` against Section 7.1: exactly 1 line removed and 3 added,
    and no others. **Adding `tests/loops_layering.rs` must not change that diff at all**; if it does,
    stop and report, because the instrument was run with the wrong flags (Section 2.5).
14. Commit the four source files, the new test file, `src-tauri/module-arcs.txt` and this plan. Delete
    the emitted graph file. **Never commit a graph:** it is about 4.9 MB, it carries the absolute path
    of the machine that produced it, and it is CRLF sensitive.

    **This plan needs `git add -f`, and round 2 was committed without it.** `.gitignore` line 11
    ignores `plans/`, so a plain `git add plans/1252-break-loops-scheduler-cycle.md` does nothing and
    `git status` stays clean while the file is silently left out. Measured: commit `7778f67b` carries
    the five source and record files and **not** this plan, and `git ls-files` does not list it,
    although every older plan in that directory is tracked. Run
    `git add -f plans/1252-break-loops-scheduler-cycle.md` and confirm with `git show --stat` that the
    plan is in the commit before reporting the step done.

If step 6, 7, 8 or 9 fails, fix it before continuing. If step 12 or 13 disagrees with the numbers in
Section 9, **stop and report**; do not adjust the plan's numbers to match the output.

---

## 9. Tests and acceptance criteria

### 9.1 What must be green

| Command | Working directory | Expectation |
|---|---|---|
| `cargo check --all-targets` | `src-tauri` | clean |
| `cargo clippy --all-targets -- -D warnings` | `src-tauri` | clean; the two import edits in Section 5.3 exist precisely to keep it clean |
| `cargo test --lib --bins --tests` | `src-tauri` | full suite green, including the new test in 9.3 |
| `npm run typecheck` | repo root | clean |
| `npm test` | repo root | full vitest suite green |
| `npm run test:debt` | repo root | clean; this change adds no ignored or placeholder test |

`test:debt` scans `src-tauri/tests/*.rs` as well as `src-tauri/src/**.rs`, so it does read the new
guard. It reports `#[ignore]` attributes and placeholder bodies (`todo!()`, `unimplemented!()`, a
`panic!("TODO...")`); the guard has none, so it stays clean.

Three existing tests scan the Rust tree and were checked against this change before it was specified.
None of them is expected to move, and any of them going red means something outside this plan changed:

- `session::selection` `production_selection_and_lifecycle_sources_have_one_owner` scans every `.rs`
  under `src/` but only fires on the four `session_*` event literals and six manager mutator
  signatures. `loops/events.rs` emits `"loop_event"` and `"ac_project_refresh_requested"` and contains
  none of them.
- `tests/pty_writer_inventory.rs` scans every `.rs` under `src/` for `write_with_permit(`,
  `backend.write(`, `route_guard.write(` and `for_route_guard`. `loops/events.rs` contains none.
- `loops::scheduler` `scan_once_calls_archived_candidate_filter` reads `scheduler.rs` and asserts the
  relative order of two lines inside `scan_once`, which this change does not touch.

The guard of Section 5.5 is the fourth tree-scanning test and the only one this change adds. It scans
`src/loops/` only, so it cannot interact with the three above, and none of them reads `tests/`.

### 9.2 No behavioural test is added

`emit_loop_change` has no test today, `commands/loops.rs` has no test module at all, and the emitter
needs a live `AppHandle` to exercise. Writing a first behavioural test for it would mean building a
Tauri harness, which is a larger change than the fix and outside the scope this dispatch set. The
behavioural guarantee here is that the moved code is byte identical and its call sites are unchanged.

### 9.3 One structural guard is added, and what it can and cannot prove

The instrument that would catch a reintroduced arc is run by hand and is deliberately not wired to CI,
so a guard inside the suite is the only thing that fires without somebody remembering to look. That
guard is `src-tauri/tests/loops_layering.rs`. **Its content is Section 5.5 and is not repeated here.**
This section is the reasoning behind it, which review has now reopened twice, and it is the part to
read before touching the matcher.

#### 9.3.1 What the guard is, and the sentence this plan got wrong

A net over the **spellings** a dependency can be written in. Not a proof that the dependency cannot
return. It matches text and does not resolve names, so a spelling it does not know about passes it.

Round 2 of this plan opened this section with "without a test nothing stops this cycle coming back".
**That sentence was false, and its falseness is what made the round-2 guard dangerous**: the guard was
believed to carry more than it could, so a green result was read as a proof. The guard now says so
about itself, in the module header of Section 5.5 and again in the `SCOPE` paragraph of its own
failure message, and it names the authoritative check: the cycle detector of Section 9.4, whose
`coverage.graphShape.cyclicSccs` must stay at 1. A green guard means "no known spelling is present".
It never means "the cycle is impossible".

#### 9.3.2 Why the round-2 matcher was withdrawn

It filtered with `production_source(path).contains("commands::loops")`. **An import that groups two or
more items does not contain that substring, because the braces are in the way.** Inserting

```rust
use crate::commands::{loops::LoopCronPreview, session::CreateSelectionIntent};
```

into the production region of `scheduler.rs` was measured, on the real tree, against every guard the
change had:

| Guard | Measured result |
|---|---|
| round-2 structural test | **passes** |
| `cargo fmt --check` | exit 0 |
| `cargo clippy -- -D warnings` | exit 0 |
| the detector | **`cyclicSccs = 2`**, both modules back in one `sccId` |

The cycle #1252 removed came back whole and nothing red went red.

Two further facts make that worse rather than better. **The single item form was covered by accident**:
rustfmt strips the redundant braces from `use crate::commands::{loops::X};` and normalizes it to the
direct spelling, and that accident disappears with two items, where the braces are required. And
`loops/delivery.rs`, the sibling module, already names `commands::session` at lines 92, 367 and 383 and
`commands::pty` at line 160, so consolidating those into one grouped import and adding `loops::` while
passing through is an ordinary refactor, not sabotage.

#### 9.3.3 What the round-3 matcher does instead

It reads the use-tree instead of searching for one of its renderings. Four steps, all in Section 5.5:

1. **Normalize.** Collapse whitespace runs to a single space, then delete the space on both sides of
   `::`, `{`, `}` and `,`. Every rendering of the same use-tree, reflowed by rustfmt or written with
   spaces around the separators, becomes the same text. This is the same technique
   `tests/pty_writer_inventory.rs` already adopted in this repo after a cosmetic reflow silently
   dropped a real write site from its inventory; the round-2 guard did not reuse it.
2. **Anchor.** Find each occurrence of `commands::` that is not the tail of a longer identifier, so
   `subcommands::` and `my_commands::` are not matches.
3. **Extract the children.** If a brace group follows, walk it balanced, split on its own top level
   commas, and take the leading identifier of each item; otherwise take the leading identifier
   directly. `loops::{a, b}` yields `loops`, `loops as l` yields `loops`, `*` yields `*`, `self`
   yields `self`. An unclosed group returns an error and fails the test loudly, because a scanner that
   cannot delimit its input must say so rather than return an empty result.
4. **Compare against an allowlist.** `ALLOWED_COMMAND_CHILDREN` is `["pty", "session"]`, the two
   references `loops::delivery` already carried before #1252, neither of them in a cycle. Everything
   else fails, and `loops` fails with its own message.

Separately, `use crate::commands as <name>;` is refused outright rather than followed. Renaming the
group puts `commands::loops` in reach under a name no text scan can resolve, so the rename is the
offence. The check is anchored on the path punctuation in front of `commands`, so English prose about
commands does not trip it.

**Every row below was measured** by compiling Section 5.5 verbatim and running it against a copy of
`src/loops/` with the spelling injected into the production region of `scheduler.rs`:

| Spelling | Result |
|---|---|
| `crate::commands::loops::emit_loop_change(...)` | red, #1252 message |
| `use crate::commands::{loops::A, session::B};` **(the one proven live)** | red, #1252 message |
| the same grouped import reflowed across four lines by rustfmt | red, #1252 message |
| `use crate::commands::{loops::{a, b}, session::c};` | red, #1252 message |
| `use crate::commands::loops as l;` and `{loops as l, session}` | red, #1252 message |
| `crate :: commands :: loops :: f()` | red, #1252 message |
| `super::super::commands::loops::...` and bare `commands::loops::...` | red, #1252 message |
| a `#[cfg(test)]` helper mid file, then the grouped import below it | red, #1252 message |
| `use crate::commands as c;` then `c::loops::...` | red, rename message |
| `use crate::commands::{self, session};` | red, membership |
| `use crate::commands::*;` | red, membership, observed `["*", "pty", "session"]` |
| a new unlisted child, `use crate::commands::terminal_snapshot::Snapshot;` | red, membership |
| `crate::commands::pty::...` removed from `delivery.rs` | red, membership, observed `["session"]` |
| the tree as this plan leaves it | **green**, observed exactly `["pty", "session"]` |
| `crate::subcommands::loops::f()`, `my_commands::loops::f()` | no match, correctly |

#### 9.3.4 Why the guard moved out of `events.rs`, and the second defect that closed

The round-2 guard lived inside the tree it scanned, so it had to remove itself from its own reading
before scanning. It did that by cutting each file at its **first** `#[cfg(test)]` and discarding
everything after. **Any production code below a mid file test helper was therefore invisible to it.**
That was not exploitable on the tree as it stands, because all four files under `src/loops/` carry a
single terminal `#[cfg(test)]` (`delivery.rs` 519 of 722, `non_stop_watchdog.rs` 368 of 630,
`scheduler.rs` 554 of 704, `events.rs` 71 of 146), and it stops being true the first time somebody
adds a test helper in the middle of a file.

The guard does not patch that cut. **It removes the reason for it.** Integration test targets are
separate leaf crates that the instrument marks `enabled: opts.includeTests`, and the record is emitted
with `includeTests: false` (Section 2.5), so `src-tauri/tests/` contributes no arc and no module.
Placed there, the guard is outside the tree it scans and never reads itself, so it reads whole files:
test regions included, nothing cut, nothing to get wrong.

Reading `#[cfg(test)]` regions makes the guard **stricter than the detector**, which ignores them. That
is the safe direction, and it is chosen deliberately: an over-strict guard produces a red somebody
argues with, an under-strict one produces a green somebody believes. The cost is stated rather than
hidden: no source under `src/loops/`, test regions and comments included, may spell a child of
`commands` outside the allowlist. Measured on the current tree, the only occurrences are the four
production paths in `delivery.rs`, so the cost is zero today.

#### 9.3.5 Membership, not counting

The guard asserts that the observed set of command children **equals** `["pty", "session"]`, rather
than counting occurrences or only looking for bad ones. This reuses the method that criterion 3 already
uses in Section 9.5, for the same reason: an equal count is not an equal set.

Equality catches the failure a denylist cannot. **A set that shrinks is the more dangerous failure**: it
means the scan stopped seeing references it used to see, because `commands` was renamed or moved or
because the matcher was narrowed, and a guard that observes nothing passes everything. That is exactly
how a green result stops meaning anything, so the guard proves it is still alive on every run by naming
what it must still see. `assert!(!files.is_empty(), ...)` does the same job one level up.

#### 9.3.6 What the guard does not cover, and how that list is maintained

**This is the part that is expected to grow, and growing it is not a plan change.**

The module header of Section 5.5 carries a numbered `KNOWN UNCOVERED SPELLINGS` list, seeded with five
entries and ending with `6. (append here: ...)`. Each entry names a way to reach the command surface
from `src/loops/` that the scan provably does not see: re-export laundering through a third module,
macro generated paths, `#[path = "..."]` modules and `include!` that move source outside the scanned
directory, and runtime indirection wired up elsewhere.

**Review is expected to find more, and the ones it finds are declared, not hidden.** When a reviewer
demonstrates a spelling that reaches the command surface and still passes, `dev-rust` appends one entry
to that list and nothing else. **Appending an entry is part of the review loop for #1252. It does not
require the plan to be reopened and does not invalidate this plan's digest.** Widening the matcher to
cover a newly found spelling is the same: if it is a spelling, it belongs in the matcher or in the
list, and either way it stays inside this section.

The one thing that does require reopening the plan is a finding that the guard cannot be a text scan at
all. If that turns up, say so rather than building something more elaborate that still cannot carry it.

#### 9.3.7 The failure messages

The `WHY` and `INSTEAD` paragraphs of the #1252 message are carried over **word for word** from the
round-2 text that review approved. Nothing in them is reworded. One paragraph is inserted before
`OFFENDING FILES`:

```
SCOPE: this is a net over the spellings of that reference, not a proof that it cannot return. It
matches text and does not resolve names, so a spelling it does not know about passes it; the ones it
is known to miss are listed at the top of this file. The authoritative check is the cycle detector,
whose `coverage.graphShape.cyclicSccs` must stay at 1.
```

That paragraph is the correction of the false promise in 9.3.1, and it sits in the failure message
rather than only in a doc comment because a doc comment is not printed when a test fails. The other two
messages, for a renamed group and for a moved set, are new and follow the same shape: what happened,
why the rule exists, what to do instead, which files.

**Two prose constraints survive from round 2, for the same reason as before.** Nothing in `src/loops/`
may spell a child of `commands` outside the allowlist, comments included, which is why the `events.rs`
header of Section 5.1 names the Tauri command layer in words. And the guard itself may name the
forbidden path as freely as it likes, because `src-tauri/tests/` is outside both the scanned directory
and the arc record.

### 9.4 Regenerating the arc record

From the repository root, with

```
VAULT = repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust
GRAPH = an absolute path OUTSIDE the working tree, e.g. %TEMP%\ac-1252\graph.json
```

```
node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "<GRAPH>" --quiet
npm run record:arcs -- --graph "<GRAPH>"
```

Then delete `<GRAPH>`.

- **The detector exits 1 while any cycle remains, and writes the graph anyway. After this fix it will
  still exit 1**, because the 89 module knot survives. Only exit 3 means no graph was written. Do not
  read that 1 as a failed change.
- Every flag above is part of the measurement. `02-module-arc-record.mjs` refuses a graph whose
  `rootPath` last segment is not `src-tauri`, or whose `crateDiscovery`, `includeTests` or `excludes`
  differ. Do not add flags.
- Emit outside the working tree. `src-tauri/module-arcs.txt` is pinned to LF in `.gitattributes`; do
  not defeat that.
- **Never diff the instrument's suggested cut between runs.** It is one of several valid minimum
  feedback arc sets and its membership is not unique. Compare arc sets and SCC membership, which is
  what every criterion below does.

### 9.5 Acceptance criteria

Every number below was produced by simulating this exact change over the frozen graph, whose arc set
is identical to `module-arcs.txt` at `f15f59a4` (Section 2.4). Verify with:

```
node "<VAULT>/Levelization/02-levelize.mjs" rank "<GRAPH>"
```

reading `coverage.graphShape` and the `modules[]` entries.

| # | Criterion | Before | After, required |
|---|---|---|---|
| 1 | `coverage.graphShape.cyclicSccs` | 2 | **1** |
| 2a | `agentscommander_lib::loops::scheduler -> agentscommander_lib::commands::loops` in `src-tauri/module-arcs.txt` | present, line 638 | **absent** |
| 2b | `no_loops_source_reaches_into_the_command_surface` in `src-tauri/tests/loops_layering.rs` (Sections 5.5 and 9.3) | does not exist | **green**, and `src-tauri/src/loops/events.rs` carries no test module |
| 2c | Where the three moved symbols are defined | all three in `src/commands/loops.rs`, at lines 57, 319 and 354 | **all three in `src/loops/events.rs`, none in `src/commands/loops.rs`, and exactly one definition of each in all of `src-tauri/src`** |
| 3 | Modules with `sccSize > 1`, and their count | one SCC of 89, one of 2 | **exactly one SCC, with exactly 89 members** |
| 4a | `agentscommander_lib::commands::loops` | rank `[40]`, level 4, `sccSize` 2 | **rank `[50]`, level 5, `sccSize` 1** |
| 4b | `agentscommander_lib::loops::scheduler` | rank `[40]`, level 4, `sccSize` 2 | **rank `[40]`, level 4, `sccSize` 1** |
| 4c | `agentscommander_lib::loops::events` | absent | **rank `[30]`, level 3, `sccSize` 1** |
| 5 | Test suites of Section 9.1 | green | **green**, and `module-arcs.txt` regenerated and committed |
| 6 | Arc record diff | | **exactly the 1 removal and 3 additions of Section 7.1, and nothing else** |

**Criterion 2 is the three conditions 2a, 2b and 2c together, and all three must hold.** 2a alone
cannot carry it: the arc record is produced by an instrument with a known blind spot (`src/lib.rs:1178`
writes `loops::scheduler::LoopScheduler::new()` with no `crate::` prefix and no arc is recorded for
it), so an arc absent from the record is not by itself proof that the dependency is gone. 2b **narrows**
the spelling hole at the source level, which is all a text scan can do and is exactly what Section 9.3
now says out loud, and **2c closes the duplication hole**: a "move" that left a copy of the emitter
behind in `commands::loops` would satisfy 2a and 2b and still be wrong, because the two copies would
drift and the layering claim would be false.

**2b changed in round 3 and only 2b.** The test it names is a different file with a different matcher;
the property it asserts is the same property, asserted over more spellings and over whole files. What
it is worth is stated in Section 9.3.1 rather than assumed.

Verify 2c from `src-tauri` with three commands. Each expected result was produced by applying the
first four edits of Section 5.3 to the current file and counting; **the comment of edit 5 was worded so
that none of the three counts moves**, which is why it says "the emitter" and not the symbol name:

```
rg -n "pub struct LoopEventPayload|pub fn emit_loop_change|fn capitalize_reason" src
```
Exactly **three** lines, all three in `src/loops/events.rs`. Any hit in `src/commands/loops.rs`, or a
fourth hit anywhere, fails the criterion. The command scans `src`, so the new guard under
`src-tauri/tests/` is out of its reach and cannot contribute a fourth hit; run it as written and do not
widen it to the repository root.

```
rg -n "LoopEventPayload|capitalize_reason" src/commands/loops.rs
```
**No output** (`rg` exits 1). Both names must be gone from the origin file, not merely unused.

```
rg -c "emit_loop_change" src/commands/loops.rs
```
Exactly **6**: one `use crate::loops::events::emit_loop_change;` and the five unchanged call sites.
Not 7, which would mean a definition stayed behind, and not 5, which would mean the import is missing
and the file does not compile. **The comment of Section 5.3 edit 5 sits on the line above that import
and does not contain the symbol, so this count stays at 6.** If it reads 7, check that comment before
looking anywhere else.

The same simulation confirms `AcLoopSummary`, `Emitter`, `serde_json` and `uuid` all fall to zero
occurrences in `src/commands/loops.rs`, while `Serialize` and `LoopCronPreview` survive, and the file
goes from 360 lines to 309.

**The 308 versus 309 line question, closed with a measurement.** The file was 360 lines at `f15f59a4`
and measures 308 after the first four edits of Section 5.3, one short of the 309 this plan predicted in
round 2. That one line is informative and never was a gate. The comment of edit 5 adds a line, so the
file lands at **309** and the sentence above is now true as written.

Criterion 4 is satisfied by 4a and 4b together: `commands::loops` at level 5 and `loops::scheduler` at
level 4 are **different levels**, and both are declared here rather than merely asserted to differ.

**Informative, not gates.** These will move and are recorded so nobody has to wonder whether they
should have: `modules` 182 to 183, `arcs` 972 to 974, `sites` 3504 to 3505, `sccs` 93 to 95,
`arcsInsideScc` 453 to 451, `arcsBetweenSccs` 519 to 523, `quotientArcs` 170 to 176. The `sites` drop
of 1 alongside 3 additions is the `AcLoopSummary` symbol leaving the `use` list in `commands/loops.rs`
(Section 5.3 edit 2), which costs that arc one of its 26 sites while leaving the arc itself in place.
`sccId` values are indices and may renumber; criterion 3 is written over membership, not over ids, for
that reason.

**None of those numbers moves because of `src-tauri/tests/loops_layering.rs`.** It is an integration
test target, the instrument marks those `enabled: opts.includeTests`, and the record is emitted with
`includeTests: false` (Section 2.5). Measured on the tree as it stands: `module-arcs.txt` holds zero
arcs from `tests/` while `src-tauri/tests/` holds 20 files. If the arc diff of criterion 6 shows
anything attributable to the new file, the instrument was run with the wrong flags; re-read Section 9.4
before touching anything else.

**If criterion 3 fails, stop and report.** A knot that is no longer 89 members means something outside
this plan's scope was touched.

### 9.6 Report to the tech lead

State: the verdict; the shape chosen and its justification by cost and layering; the arc enumeration
actually observed in the diff, compared against Section 7.1; the observed values for every criterion,
with 2a, 2b and 2c reported separately and 2c quoting the actual output of its three commands; and the
detector's exit code with the note that 1 is expected. If anything refuses to run, report it with its
reason rather than working around it.

Three additions for round 3:

- **Report `src/commands/loops.rs` at 309 lines**, and confirm that `rg -c "emit_loop_change"` on it
  still reads 6 with the comment of Section 5.3 edit 5 in place.
- **Report the `KNOWN UNCOVERED SPELLINGS` list as it stands when the work is handed over**, including
  any entry review caused to be appended. An empty round of review is worth stating too.
- **Do not report the guard as proof that the cycle cannot return.** Report it as green, and report
  `coverage.graphShape.cyclicSccs` separately as the check that carries criterion 1. Section 9.3.1
  exists because those two were previously reported as one thing.
