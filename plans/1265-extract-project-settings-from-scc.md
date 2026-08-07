# Implementation Plan: #1265 Extract `commands::project_settings` from the 89 module cyclic SCC

Status: READY_FOR_IMPLEMENTATION

**Recertified by the architect after review.** The first certification was written in one pass;
`dev-rust` verified it against the real tree and `dev-rust-grinch` re-attacked it, between them
finding five blocking defects in the guard and two executable steps that did not work. `dev-rust`
closed all seven. Recertification rebuilt the laboratory from scratch rather than reviewing that
work, confirmed every closure independently, and found and closed one more: the guard could still be
walked around from inside `src/web/`, through the emitter's sibling. Section 10 is the full log, and
Section 10.5 is this round. **The direction of the cut, the placement, the four arcs and every
acceptance number are unchanged across all three rounds**; every defect was in the guard, which is
where this plan says the durable value is.

`dev-rust` implements from this document; `dev-rust-grinch` re-attacks the guard afterwards. This is
the complete cold-start specification: the exact file contents to write, the exact commands to run,
the exact numbers to expect, and the exact arcs the change adds and removes. Nothing is left to
decide.

**One line of dissent, as the dispatch invites.** The objective is fixed and this plan implements it
as given. It is worth recording that removing this module thins the knot by one out of 89 and does
not change any level except its own, so the structural return is small; the durable return is the
guard of Section 9.3 and the placement precedent it sets. That is not an argument to pick a different
target, and no part of this plan acts on it.

---

## 1. Issue and objective

**Issue:** #1265, https://github.com/mblua/AgentsCommander/issues/1265. It stays **OPEN**. No commit
message, PR body or comment may contain `Closes #1265` or any other closing keyword.

**Branch:** `refactor/1265-extract-project-settings-wg11`, already created, already checked out.
**Base:** `origin/main` @ `5168310b2a63149de3b846e9e45bdb4dcea696fe`.

**Objective.** Take `agentscommander_lib::commands::project_settings`
(`src-tauri/src/commands/project_settings.rs`) out of the crate's single cyclic SCC by removing the
arc

```
agentscommander_lib::commands::project_settings -> agentscommander_lib::web::commands
```

and keeping the opposite arc `web::commands -> commands::project_settings` exactly as it is.

**This is a structural change with no behavioural change.** Every event the app emits today, with the
same name, the same payload, the same recipients and the same ordering, it must still emit after the
change.

---

## 2. Evidence and current state

### 2.1 The knot, re-verified here

The tech lead's measurement was re-derived independently for this plan by running Tarjan over the
974 arcs of the committed record `src-tauri/module-arcs.txt`. Both agree:

```
modules with at least one arc 173 | unique arcs 974 | sccs 85 | cyclicSccs 1
cyclic SCC sizes: [89]
sccSize(agentscommander_lib::commands::project_settings) = 89
sccSize(agentscommander_lib::web::commands)              = 89
sccSize(agentscommander_lib::web::broadcast)             = 1
```

`modules 173` counts only modules that appear as an endpoint of some arc; the levelizer reports a
larger `modules` figure because it also counts isolated modules, which the record cannot carry. That
difference is informative and is not a gate anywhere in this plan.

### 2.2 The four arcs that touch the target

Lines 384, 385, 386 and 959 of the record:

```
agentscommander_lib::commands::project_settings -> agentscommander_lib::config::project_settings
agentscommander_lib::commands::project_settings -> agentscommander_lib::web::broadcast
agentscommander_lib::commands::project_settings -> agentscommander_lib::web::commands
agentscommander_lib::web::commands              -> agentscommander_lib::commands::project_settings
```

`config::project_settings` and `web::broadcast` are both `sccSize 1`, outside the knot. The mutual
pair with `web::commands` is the whole of what holds the target inside it.

### 2.3 Levels today

Measured with the same condensation, and matching the levelizer output the tech lead supplied:

| module | level | sccSize |
|---|---|---|
| `agentscommander_lib::commands::project_settings` | 2 | 89 |
| `agentscommander_lib::web::commands` | 2 | 89 |
| `agentscommander_lib::config::project_settings` | 1 | 1 |
| `agentscommander_lib::web::broadcast` | **0** | 1 |

**The two sides of the pair share a level because they share an SCC.** Acceptance criterion 4, that
they end on distinct levels, is unsatisfiable while the cycle exists and is satisfied by the removal
itself. It is a consequence of the cut working, not a separate goal to chase.

### 2.4 The two call sites, counted in the source

**`commands::project_settings -> web::commands` is one call site.**
`src-tauri/src/commands/project_settings.rs:44` calls, fully qualified and with no `use`:

```rust
crate::web::commands::broadcast_all(
    &app,
    broadcaster.inner(),
    PROJECT_GROUPS_UPDATED_EVENT,
    &payload,
)
```

Searched across the whole of `src-tauri/src`: that is the only reference from this file to
`web::commands`, and `crate::web::commands::broadcast_all` appears exactly once in the crate.

**`web::commands -> commands::project_settings` is four call sites**, all in
`src-tauri/src/web/commands.rs`, at lines 756, 765, 767 and 771, all fully qualified:

```
756  crate::commands::project_settings::get_project_groups_inner(&path)?
765  crate::commands::project_settings::update_project_groups_inner(&path, config)?
767  crate::commands::project_settings::project_groups_updated_payload(&path, &result)
771  crate::commands::project_settings::PROJECT_GROUPS_UPDATED_EVENT
```

A fifth reference exists at line 940, a grouped `use` of two of those symbols, but it is inside the
file's `#[cfg(test)] mod tests` and therefore contributes no arc: the record is emitted with
`includeTests: false`, which `scripts/02-module-arc-record.mjs` gates on and refuses to relax.

### 2.5 `broadcast_all` is shared infrastructure parked in a command surface

`src-tauri/src/web/commands.rs:851-860`:

```rust
/// Emit event to both Tauri windows and WebSocket clients.
pub fn broadcast_all(
    app: &tauri::AppHandle,
    broadcaster: &WsBroadcaster,
    event: &str,
    payload: &Value,
) {
    let _ = tauri::Emitter::emit(app, event, payload.clone());
    broadcaster.broadcast_event(event, payload);
}
```

It carries no `#[tauri::command]`, it is not a `BrowserProjectCommand` variant, it is not reachable
from the wire, and it dispatches nothing. It emits on two transports. It has **ten** call sites: nine
inside `web/commands.rs` (lines 267, 285, 395, 403, 484, 503, 536, 622 and 768) and the one in
`commands/project_settings.rs:44`, plus one test at line 1179 of the same file.

**So the cause of this arc is where the emitter lives, not how it is called.** `commands` is the
Tauri IPC surface, `web::commands` is the browser IPC surface, and a Tauri command reaching sideways
into the browser dispatcher in order to announce a change is an inverted dependency independently of
what the graph says about it.

### 2.6 What the instrument does and does not record

Load bearing for Sections 5.5, 7 and 9, all established by reading the committed record against the source:

- **References under `#[cfg(test)]` are not recorded**, and neither are integration test targets.
  `src-tauri/tests/` holds 21 files and contributes zero arcs to the 974. That is why the guard of
  Section 9.3 lives there and may name the forbidden path as freely as it likes.
- **`mod` declarations create no arc.** `src/web/mod.rs` gains `pub mod event_broadcast;` and that
  line adds nothing to the record.
- **`use self::` and `use super::` ARE resolved, and so is `super::` in an expression path.**
  `src/web/mod.rs:24` writes `use self::broadcast::WsBroadcaster;` and arc 953 `web -> web::broadcast`
  exists; `src/web/commands.rs:12` writes `use super::broadcast::WsBroadcaster;` and arc 974
  `web::commands -> web::broadcast` exists. Both are production code and neither spells `crate::`.
  **This corrects the dispatch's premise**, which stated that rewriting a reference as
  `super::super::…` deletes the arc. It does not.

  **The `use`-versus-expression question is now closed too, and it was open when this plan was first
  written.** `dev-rust-grinch` built a fixture and ran the real detector
  (`01-rust_module-dependency-cycles.mjs`) over it. Both forms are recorded:

  ```
  fx6lib::commands::expr_super -> fx6lib::commands       [kind=path line=2]
  fx6lib::commands::use_super  -> fx6lib::commands       [kind=use  line=1]
  fx6lib::commands::anchored   -> fx6lib::web::commands  [kind=path line=2]
  fx6lib::commands::expr_bare  -> (NO ARC)
  ```

  What is measured absent is only the **fully unanchored** path, consistent with `src/lib.rs:1178`
  constructing `loops::scheduler::LoopScheduler::new()` with no matching arc among the 974, and with
  `src/web/mod.rs:75` calling `embedded::embedded_static_handler` with no `web -> web::embedded` arc.
  From `commands::project_settings` that spelling is not reachable without first creating a recorded
  arc towards `commands`, so it is bounded on both sides: the blind spot is real, narrow, and named.
  **This narrows nothing about the need for the guard**, whose reason is Section 9.3.2 and never
  rested on that one spelling.
- **Do not evade the detector.** Every reference this plan introduces is written `use crate::…`. An
  arc must disappear because the call is gone, never because the spelling changed.

---

## 3. Scope

### 3.1 In scope

- `src-tauri/src/web/event_broadcast.rs` (new)
- `src-tauri/src/web/mod.rs`
- `src-tauri/src/web/commands.rs`
- `src-tauri/src/commands/project_settings.rs`
- `src-tauri/tests/project_settings_layering.rs` (new; the guard, Section 9.3)
- `src-tauri/module-arcs.txt` (regenerated)
- This plan file.

### 3.2 Out of scope, and the hard prohibitions

- **The knot is untouchable.** Do not modify anything inside it and do not tidy anything adjacent to
  it opportunistically. If its membership moves by anything other than the target leaving, something
  outside scope was touched: stop and report.
- **`broadcast_all_r` and `broadcast_all_to_managed` stay in `web::commands`, and this is a closed
  decision.** Moving them too was simulated and also yields a knot of 88 with identical membership,
  so it is not refused on structural grounds. It is refused because it deletes
  `commands::ac_discovery -> web::commands`, an arc nobody asked to remove, in service of tidiness
  rather than of the objective. `broadcast_all_r` is generically parameterised over `R: Runtime` and
  resolves its broadcaster through `try_state`; `broadcast_all` takes one explicitly. They are not
  duplicates. **Do not reopen this during implementation or review.**

  **And nothing enforces it, which is declared rather than fixed.** After this change there are two
  dual-transport emitters in two different modules, with identical `let _ = tauri::Emitter::emit(…)`
  bodies. The duplication test of Section 5.5 watches the name `broadcast_all`, and criterion 8a is
  written about that name. The day somebody moves `broadcast_all_r` down "for symmetry", no test in
  this repository goes red and `commands::ac_discovery -> web::commands` disappears silently. It is
  entry 11 of `KNOWN UNCOVERED SPELLINGS`. Fixing it would mean guarding a module this issue does not
  touch, which is scope #1265 did not ask for.
- **No new arc from `commands::project_settings` into any module inside the knot.** Trading this arc
  for another one is not a fix.
- No behavioural change, no new feature, no signature change, no frontend change, no change to
  `src/shared/types.ts`.
- **`I` (instability) justifies nothing here and must not appear in the implementation report.** With
  the cycle present, both sides' `Ce`/`Ca` include the very arc being deleted, so the ordering hint
  is computed over a graph containing the thing being removed. The instrument's own note says it must
  not derive a code movement. Cost and layering are the reasons; `I` is not one.

---

## 4. The decided solution

**Move `broadcast_all` down into a new module that both sides depend on:
`agentscommander_lib::web::event_broadcast`, at `src-tauri/src/web/event_broadcast.rs`.**

One item moves there verbatim: the `broadcast_all` function, with its doc comment. Its own test moves
with it. `commands::project_settings` and `web::commands` then both import it and both depend
downward, and the arc `commands::project_settings -> web::commands` disappears because the call to
that path is gone.

### 4.1 Why this direction, by cost

**One call site against four**, counted in the source and confirmed against the graph's edge records:

| direction | arc deleted | call sites to rewrite | shape |
|---|---|---|---|
| **chosen** | `commands::project_settings -> web::commands` | **1** (`commands/project_settings.rs:44`) | one fully qualified call, no `use` |
| rejected | `web::commands -> commands::project_settings` | **4** (`web/commands.rs:756, 765, 767, 771`) plus a grouped `use` of two more symbols at line 940 | four fully qualified calls across two dispatcher arms |

The rejected direction also moves four of the six items in a 101 line module
(`PROJECT_GROUPS_UPDATED_EVENT`, `project_groups_updated_payload`, `get_project_groups_inner`,
`update_project_groups_inner`), leaving behind only the two `#[tauri::command]` wrappers, against one
function in the chosen direction. Cheaper to cut, and less code in motion, on both counts.

### 4.2 Why this direction, by layering

Cost alone would not settle it. Layering settles it the same way, from three independent angles.

1. **The domain must not depend on the surface it is announced through.** `broadcast_all` is not a
   command (Section 2.5). It is the emitter both transports share, parked inside one of them. The
   rejected direction would instead move the project-settings domain helpers out from under the Tauri
   command surface and leave the emitter where it is, which fixes the arc while leaving the actual
   inversion in place.
2. **The rejected direction breaks the pattern the file already follows.** `web/commands.rs` reaches
   into `crate::commands::*` for shared `*_inner` helpers in six further places for
   `commands::ac_discovery` alone (lines 741, 779, 790, 801, 814, 825). `web::commands ->
   commands::project_settings` is that established shape; `commands::project_settings ->
   web::commands` is the one anomaly. Cutting the anomaly leaves the codebase consistent; cutting the
   pattern leaves this one module special for no reason a reader could reconstruct.
3. **The resulting levels differ, and only one of them is right.** Measured on the condensation:

   | | `commands::project_settings` | `web::commands` |
   |---|---|---|
   | today | 2 | 2 |
   | **chosen** (cut the out-arc) | **2** | **3** |
   | rejected (cut the in-arc) | 3 | 2 |

   The chosen cut leaves the Tauri command **below** the browser dispatcher. The rejected cut leaves
   it **above**: the desktop IPC surface depending on the browser IPC surface, which is the inversion
   written the other way round. Both satisfy criterion 4; only one of them means the right thing.

**Both directions produce cyclicSccs 1, a knot of 88, membership identical minus exactly the target,
and `sccSize(target) = 1`.** That was simulated for each and is stated here so nobody re-derives it
hoping topology will break the tie. It does not. Cost and layering do.

### 4.3 Why this placement, and the proof it cannot be absorbed

`web::event_broadcast` is a new module whose only in-crate dependency is `web::broadcast`, for the
`WsBroadcaster` type in the signature.

**Proof that the knot cannot absorb it.** A module joins a cyclic SCC only if it can reach a member of
that SCC and be reached from it. `web::event_broadcast` has exactly one out-arc, to `web::broadcast`.
**`web::broadcast` has zero out-arcs**: measured over the 974, it never appears on the left of the
separator, so the set of modules reachable from it is empty. Therefore the set reachable from
`web::event_broadcast` is `{web::broadcast}`, which contains no knot member, so no cycle through it is
possible. This is not an appeal to the candidate not currently sharing an SCC with the knot; it is the
reachability computation, and it holds for any future arc **into** `web::event_broadcast` as well,
because absorption needs a path out. Simulation confirms the conclusion: `sccSize 1`, level 1.

**And here is the premise that can fail, stated in the word that matters.** The paragraph above is
about arcs *into* the module, and that is the direction that is safe. **The proof rests on
`web::event_broadcast` having no OUTGOING arc into the knot, and on `web::broadcast` continuing to
have no outgoing arc at all.** Neither is a theorem about the future; both are measurements of
today's tree. `dev-rust-grinch` measured what one added outgoing arc costs, simulated over the arc
record:

```
AFTER (this plan applied)                        arcs=976  cyclicSccs=1  knot=88
                                                 sccSize(commands::project_settings)=1
                                                 sccSize(web::event_broadcast)=1

AFTER + web::event_broadcast -> session::manager  arcs=977  cyclicSccs=1  knot=90
                                                 sccSize(commands::project_settings)=90

AFTER + web::broadcast -> session::manager        arcs=977  cyclicSccs=1  knot=91
                                                 sccSize(commands::project_settings)=91
```

One `use` in the new 56 line file undoes #1265 completely and leaves the knot **larger than the 89 it
started at**. `web::broadcast` is not a frozen leaf either: it has zero outgoing arcs and **ten
incoming** ones, so it is a live module that people edit.

**That is why the guard of Section 5.5 has a second test.**
`the_emitter_home_names_nothing_but_the_websocket_fan_out` asserts by equality that
`src/web/event_broadcast.rs` names `web` under `crate::`, `broadcast` under `web::` and nothing but
its own `broadcast_all` under `super::`. Without it the only thing watching this premise would be a
detector run by hand, while the compiler's `error[E0603]` already covers the other side of the cut. A
guard that watches the side the compiler watches and not the side nothing watches is a guard pointed
at the wrong module.

**The third equality is not symmetry, and recertification added it.** The first two anchors were
measured leaving one live spelling open: the dispatcher is the emitter's **sibling**, so from inside
`src/web/` it is reachable as `super::commands` with no `web::` token anywhere. That is not an exotic
form. It is the idiom the neighbouring file already uses, at `src/web/commands.rs:12`
(`use super::broadcast::WsBroadcaster;`), so it is the first thing a reader of that directory would
copy, and it rebuilds the whole cycle: `commands::project_settings -> web::event_broadcast ->
web::commands -> commands::project_settings`. Measured green under two anchors, red under three
(probes P29 to P34 of Section 9.3.4). `commands::project_settings` needs no equivalent anchor and has
none: it is not a sibling of anything under `web`, so every path from there into the dispatcher must
spell `web` followed by `::`, or rename a group, which is refused by name.

**Why not `web::broadcast` itself**, which would have been a one line diff with zero arcs added since
`commands::project_settings -> web::broadcast` already exists. Refused, and this is a closed decision:
`web::broadcast` is the WebSocket fan-out and contains no reference to Tauri at all. Hosting
`broadcast_all` there gives the WebSocket transport an `AppHandle` and an `Emitter`, so the module
named for one transport starts emitting on the other. That changes the host's role, which the dispatch
forbids, and it is the same trade #1252 refused when it declined to put a Tauri emitter into
`config::loops`. **The minimal diff is a symptom of good placement, never the target**, and here it is
a symptom of the wrong one.

**Why not inside `commands::`**, where the target's own helpers live: any such module would be a peer
of the surface being decoupled rather than a layer below it, and the point is that the emitter belongs
below both surfaces.

`web::event_broadcast` lands at **level 1**, below `commands::project_settings` at 2 and
`web::commands` at 3, and above `web::broadcast` at 0.

### 4.4 Accepted cost

The chosen shape removes one arc and adds three, against the "ideal one line diff" of the refused
`web::broadcast` placement. Section 7 enumerates all four. None of the three added arcs points into
the knot, none creates a cycle, and all three were verified by simulation before this plan was
written.

---

## 5. Affected surfaces: exact files and symbols

Every file in this section was written, compiled, formatted and run before this plan was certified.
`rustfmt --check --edition 2021` exits 0 on each of them **as written**, so do not reformat them; a
diff from `cargo fmt --check` means the file was retyped rather than copied.

### 5.1 New file: `src-tauri/src/web/event_broadcast.rs`

Create with exactly this content, 56 lines. The body of `broadcast_all` is moved verbatim from
`src/web/commands.rs`; do not reformat, reorder or improve it. The test moves with the function
because it is that function's test, and leaving it behind would make `web::commands` the home of a
test for a symbol it no longer defines.

```rust
//! The dual-transport event emitter: one call reaches both the Tauri windows and
//! the connected WebSocket clients.
//!
//! #1265: this module exists so a Tauri command never has to reach sideways into
//! the browser command dispatcher just to announce a change. The dispatcher and
//! `commands::project_settings` both depend downward on this module, which owns
//! the emitter, so neither of them depends on the other in order to emit.
//!
//! It sits beside `web::broadcast` rather than inside it because `web::broadcast`
//! is the WebSocket fan-out and knows nothing about Tauri. Handing it an
//! `AppHandle` and an `Emitter` would make the WebSocket transport depend on the
//! desktop one, which trades one layering inversion for another.

use serde_json::Value;

use crate::web::broadcast::WsBroadcaster;

/// Emit event to both Tauri windows and WebSocket clients.
pub fn broadcast_all(
    app: &tauri::AppHandle,
    broadcaster: &WsBroadcaster,
    event: &str,
    payload: &Value,
) {
    let _ = tauri::Emitter::emit(app, event, payload.clone());
    broadcaster.broadcast_event(event, payload);
}

#[cfg(test)]
mod tests {
    use super::broadcast_all;
    use crate::web::broadcast::{WsBroadcaster, WsOutMsg};
    use serde_json::{json, Value};

    #[test]
    fn broadcast_all_sends_to_explicit_websocket_broadcaster() {
        let managed = WsBroadcaster::new();
        let explicit = WsBroadcaster::new();
        let mut receiver = explicit.subscribe();
        let app = tauri::Builder::default()
            .any_thread()
            .manage(managed)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let payload = json!({ "path": "C:/project", "archived": false });

        broadcast_all(app.handle(), &explicit, "project_archive_changed", &payload);

        let event = match receiver.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        };
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"], payload);
    }
}
```

The `use crate::web::broadcast::WsBroadcaster;` is written anchored on `crate::`, not as
`super::broadcast::…`, so the arc it creates is unambiguous in the record. Section 2.6 measured that
`super::` is in fact resolved, so this began as a readability rule; the guard now enforces it, and
Section 9.3.3 says why that matters here and not in the guarded module.

**The test module imports by name and does not glob its parent, and that is load bearing.** The
obvious `use super::*;` is refused by the guard's third equality, for a reason that has nothing to do
with tests: written at module level in this file, `use super::*;` pulls the children of `crate::web`
into scope, `commands` among them, under no name a text scan can follow. It is indistinguishable by
text from the same glob inside `mod tests`, so the glob is refused outright rather than
context-guessed. Naming `broadcast_all` costs one line and closes it. This was measured: probe P30 of
Section 9.3.4. `WsBroadcaster` and `Value` move into the test module's own imports for the same
reason, and both name children already on the allowed tables, so no observed set moves.

### 5.2 `src-tauri/src/web/mod.rs`

Add one line, keeping the list alphabetical. `rustfmt`'s `reorder_modules` sorts these regardless of
visibility, so `event_broadcast` goes **after** `embedded`, not after `commands`:

```rust
pub mod auth;
pub mod broadcast;
pub mod commands;
mod embedded;
pub mod event_broadcast;
```

This adds no arc: `mod` declarations are not recorded (Section 2.6).

### 5.3 `src-tauri/src/web/commands.rs`

Two edits. The nine call sites of `broadcast_all` at lines 267, 285, 395, 403, 484, 503, 536, 622
and 768 are **not** touched: they are unqualified calls and resolve through the new import.

1. **After line 10**, add the import with its comment. `crate::web::event_broadcast` sorts after
   `crate::session::manager`, so this position is what `reorder_imports` already wants:

   ```rust
   use crate::config::settings::SettingsState;
   use crate::pty::manager::PtyManager;
   use crate::session::manager::SessionManager;
   // #1265: keep private. A `pub use` here would re-expose the emitter under the
   // dispatcher's own path and let the deleted arc come back as an import.
   use crate::web::event_broadcast::broadcast_all;
   ```

   **Why the comment is required.** Because this import is not `pub use`, `web::commands` no longer
   defines the symbol and only holds it privately, so writing `crate::web::commands::broadcast_all`
   from anywhere else fails to compile with `error[E0603]`. That is a real second line of defence and
   a fragile one: a future `pub use` on this line removes it in complete silence, with no test and no
   reviewer prompt. A reader has no way to know a visibility keyword is load bearing here unless the
   line says so. Copy the two comment lines verbatim; both fit rustfmt's 100 column default.

2. **Delete lines 850 to 860**, which is `broadcast_all` together with its doc comment **and the
   blank line above it**, and **delete lines 1166 to 1187**, which is the test
   `broadcast_all_sends_to_explicit_websocket_broadcaster` **and the blank line above it**. The test
   moves to Section 5.1. **Do not touch `broadcast_all_to_managed` (line 832), `broadcast_all_r`
   (line 847) or the test `broadcast_all_r_sends_to_managed_websocket_broadcaster` (line 1147)**;
   they stay, and Section 3.2 says why.

   **The adjacent blank line is part of each deletion and this is not cosmetic.** Line 850 is blank
   and line 861 is blank; line 1166 is blank and line 1188 is blank. Deleting only 851-860 and
   1167-1187 leaves **two consecutive blank lines** in each place, and `rustfmt` collapses two blank
   lines into one (`blank_lines_upper_bound` defaults to 1 and is enforced even though the option
   itself is unstable). Measured with the repository's own `rustfmt --check --edition 2021`: it emits
   a diff and exits 1. **Step 8 of Section 8 would then fail, and it would fail with the wrong
   diagnosis**, because this plan reads a `cargo fmt --check` diff as "the file was retyped rather
   than copied". Deleting 851-861 and 1167-1188 instead is equally correct; what must not happen is
   deleting the item without one of its surrounding blank lines.

`use super::broadcast::WsBroadcaster;` at line 12 stays: `WsState.broadcaster` and the
`try_state::<WsBroadcaster>` inside `broadcast_all_to_managed` still need it, so the arc
`web::commands -> web::broadcast` is unaffected. `WsOutMsg` and `Value` stay imported in the test
module: the surviving `broadcast_all_r` test uses both.

The file goes from 1471 to **1441** lines: `1471 + 3` for the import and its two comment lines,
`- 11` for the function, its doc comment and one blank line, `- 22` for the test and one blank line.
That is informative and is not a gate. (The 1471 was verified on the real tree. Note that
PowerShell's `Measure-Object -Line` reports 1340 for this file because it does not count empty lines;
use `[System.IO.File]::ReadAllLines(...).Count` or `rg -c ''`.)

### 5.4 `src-tauri/src/commands/project_settings.rs`

Two edits, and the whole file is 102 lines afterwards.

1. **After line 6**, add the import, alphabetical among the `crate::web::` imports:

   ```rust
   use crate::web::broadcast::WsBroadcaster;
   use crate::web::event_broadcast::broadcast_all;
   ```

2. **Line 44**, drop the `crate::web::commands::` prefix from the call. The argument list, the
   line breaks and the trailing comma are unchanged; rustfmt keeps this call multi-line because
   collapsed it is 85 columns against `fn_call_width`'s default of 60, which was measured rather than
   assumed:

   ```rust
       let result = update_project_groups_inner(&path, config)?;
       let payload = project_groups_updated_payload(&path, &result);
       broadcast_all(
           &app,
           broadcaster.inner(),
           PROJECT_GROUPS_UPDATED_EVENT,
           &payload,
       );
       Ok(result)
   ```

   **No blank line is introduced before `Ok(result)`.** The rest of the file, the two
   `#[tauri::command]` wrappers, the three `pub(crate)` items and the whole `#[cfg(test)] mod tests`,
   is untouched.

### 5.5 New file: `src-tauri/tests/project_settings_layering.rs`

The structural guard. Section 9.3 is the reasoning behind it and the evidence that it is alive; this
section is the content to write. **Create it with exactly these 1403 lines.**

**This is the third version.** The 785 line version the architect first certified was re-attacked by
`dev-rust-grinch`, who measured five ways it reported green while the dependency was live, and
`dev-rust` rewrote it to 1317 lines against those measurements. **Recertification then measured one
more**, the sibling spelling of Section 4.3, and closed it with a third anchor. Section 10 is the log
of what changed and why; Section 9.3.4 is the probe table that proves each fix. Do not restore either
earlier version.

Verified on `rustc 1.93.1` in a laboratory crate before this section was written, twice: by
`dev-rust` at 1317 lines and again by the architect at 1403 during recertification. In both runs
`rustfmt --check --edition 2021` exits 0 on it as written, `cargo clippy --all-targets -- -D
warnings` is clean, and all three tests pass against a **copy of the real `src-tauri/src/`** (188
files) carrying this change, in 1.40 s and 1.42 s respectively. So do not reformat it and do not
reflow it by hand. The file is pure ASCII (measured: zero bytes above 127) with LF endings, so no
encoding or line ending question arises on Windows.

**It now guards three things, not two.** `project_settings_names_no_web_module_above_it` is the
original: the Tauri command may not name the browser dispatcher.
`the_emitter_home_names_nothing_but_the_websocket_fan_out` is new and is the one Section 4.3 depends
on: the emitter module may not name anything but `web::broadcast`, because the non-absorption
argument fails on an **outgoing** arc from that module and nothing else in the repository would go
red. It asserts three equalities, one per anchor, and Section 9.3.3 explains why the third exists for
this module alone. `the_dual_transport_emitter_is_defined_exactly_once` is the original criterion 8
check, with a definition matcher that now sees a generic copy.

**Once this file exists, it is the canonical copy and this section is a snapshot.** Section 9.3.5
invites reviewers to append entries to the guard's `KNOWN UNCOVERED SPELLINGS` list, and the first
appended entry makes the file and this section diverge. That is expected and correct: the file runs,
this section does not. **Append to `src-tauri/tests/project_settings_layering.rs` and leave this
section alone.** The guard's own module header says the same thing, where a reader who never opens
this plan will find it.

```rust
//! #1265 layering guard: `crate::commands::project_settings` may not name the
//! browser command dispatcher `crate::web::commands`, and the emitter module
//! `crate::web::event_broadcast` may not name anything but the WebSocket
//! fan-out.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1 with the guarded module at
//! `sccSize 1`. A green result here means "no known spelling is present", never
//! "the cycle is impossible".
//!
//! WHY IT GUARDS TWO MODULES AND NOT ONE. #1265 took
//! `commands::project_settings` out of the knot by moving the emitter down into
//! `web::event_broadcast`, and the argument that the emitter module cannot be
//! absorbed by the knot rests on it having exactly one outgoing arc, to
//! `web::broadcast`, which itself has none. **That premise fails on an outgoing
//! arc, not an incoming one.** A single `use` in `src/web/event_broadcast.rs`
//! pointing at any knot member puts the guarded module straight back into the
//! knot and leaves the knot larger than it was before the change, and the
//! project-settings assertions below stay green throughout, because that file
//! never changed. So the emitter module is guarded too, by the same matcher
//! under three anchors: `crate::`, `web::` and `super::`.
//!
//! The third anchor is not symmetry, it is the emitter's neighbourhood. The
//! dispatcher `web::commands` is the emitter's SIBLING, so from inside
//! `src/web/` it is reachable as `super::commands` with no `web::` token
//! anywhere, which the first two anchors cannot see. That spelling is the idiom
//! the neighbouring file already uses (`src/web/commands.rs:12` writes
//! `use super::broadcast::WsBroadcaster;`), so it is the first thing a reader
//! of that directory would copy. `commands::project_settings` needs no such
//! anchor: it is not a sibling of anything under `web`, so every path from
//! there into the dispatcher must spell `web` followed by `::`, or rename a
//! group, which is refused by name.
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
//! Comments and the bodies of string and character literals are removed before
//! anything is matched: neither can be a dependency, neither may hide a path
//! from the scan, and neither may feed one to it.
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: the three `ALLOWED_*` tables are the whole contract,
//! and the spellings the scan is known to miss are listed below instead of being
//! left unsaid.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that reaches the browser command dispatcher from the guarded module and still
//! passes this file, it is appended here. Appending an entry is part of
//! reviewing #1265 and is expected; it changes nothing else.
//!
//! **This file is the canonical copy.** Section 5.5 of
//! `plans/1265-extract-project-settings-from-scc.md` quotes it verbatim, but that
//! quote is a snapshot taken when the plan was certified. The first appended
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
//!      `pub use crate::web::commands::broadcast_all;` and the guarded module
//!      imports from there. No `web::commands` token appears in the scanned
//!      files. The detector still catches it: the laundering module gains the
//!      arc, the guarded module reaches the knot through it, and the knot grows
//!      instead of shrinking.
//!   2. Macro-generated paths. A `macro_rules!` defined elsewhere, or any
//!      procedural macro, whose expansion contains the path. The text is not in
//!      the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `include!`. A file textually included from outside the module tree is
//!      pulled in without a `mod` declaration, so walking the tree does not
//!      reach it.
//!   4. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in the dispatcher and which is wired together
//!      outside the guarded module. No path text appears in the scanned files.
//!   5. `concat!` and friends. `concat!("crate::web", "::commands")` builds the
//!      path text out of fragments none of which contains the anchor, and the
//!      bodies of those literals are removed before the scan in any case.
//!   6. A `mod x;` declaration nested inside an inline `mod y { ... }` block.
//!      rustc resolves it against the inline module's own directory and this
//!      resolver does not, so it would scan a file rustc does not compile.
//!      **It used to pass silently whenever a file happened to exist at the
//!      path this resolver looks in; that was measured.** It is now refused:
//!      `module_body` rejects the whole file with a hard failure naming it. The
//!      spelling is still uncovered in the sense that the reference is not
//!      read, but it can no longer be read as green.
//!   7. NTFS alternate data streams. `#[path = "carrier.rs:evil"]` compiles from
//!      a stream that carries code the resolver does open by path, but a `mod`
//!      declaration hidden inside a stream of another file is not reachable.
//!      Git stores only the main stream, so a clone has no `:evil` and the build
//!      fails rather than hiding anything.
//!   8. Laundering through the PARENT module, FROM THE GUARDED MODULE ONLY.
//!      `commands/mod.rs` re-exports the dispatcher and
//!      `commands/project_settings.rs` reaches it as `super::<name>`, in a
//!      `use` declaration or in an expression path. No `web::` token appears
//!      there at all, and that file is read under two anchors only, so nothing
//!      matches. Measured green in both forms. **The emitter module is not
//!      exposed this way**: it is read under `super::` as well, so the same
//!      laundering from `src/web/event_broadcast.rs` is refused. The detector
//!      does catch it, in both forms, also measured: the arc
//!      `commands::project_settings -> commands` appears, and
//!      `web::commands -> commands::project_settings` closes the cycle, so the
//!      knot grows. See the note above about what that is worth.
//!   9. Aliasing the crate root or a parent, other than the two spellings
//!      `aliases_a_module_group` knows. `use crate as c;` and `use crate::web as
//!      w;` are refused by name; a rename reached some other way is not.
//!  10. A path assembled across a `cfg` boundary in a way the resolver
//!      over-reads into but the equality tables do not distinguish. This
//!      resolver scans both arms of a platform module, so a forbidden reference
//!      in either arm is caught, but which arm rustc compiled is not known here
//!      and the failure message cannot say.
//!  11. `broadcast_all_r` moving. After #1265 there are two dual-transport
//!      emitters in two modules: `broadcast_all` in `web::event_broadcast` and
//!      `broadcast_all_r` in `web::commands`. Section 3.2 of the plan closes the
//!      decision to leave `broadcast_all_r` where it is, because moving it would
//!      delete `commands::ac_discovery -> web::commands`, an arc nobody asked to
//!      remove. **Nothing in this file or in the suite enforces that.** The day
//!      somebody moves it "for symmetry", no test goes red.
//!  12. A reference inside a `#[cfg(test)]` region holding an equality up on its
//!      own. Whole files are read, test regions included, while the detector
//!      ignores them. Everywhere else that makes this guard stricter, which is
//!      the safe direction; here it makes it laxer. Measured: deleting the
//!      production `use crate::web::broadcast::WsBroadcaster;` from the emitter
//!      module leaves both of its equalities satisfied, because the test
//!      module's own `use crate::web::broadcast::{WsBroadcaster, WsOutMsg};`
//!      names the same child. **It is not exploitable as it stands**, because
//!      that deletion does not compile: the type is in `broadcast_all`'s
//!      signature. It is written down because the shrinking-set argument is the
//!      thing somebody will be trusting on the day it stops being true. The
//!      same asymmetry is recorded in `loops_layering.rs` for #1252.
//!  13. (append here: one entry per spelling a reviewer proves still passes)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Every `(file, child)` reference the guarded module is allowed to make into a
/// child of `crate::web`, sorted.
///
/// `broadcast` is the `WsBroadcaster` type in the signature of the
/// `update_project_groups` command and predates #1265. `event_broadcast` is the
/// emitter #1265 moved below both surfaces, and it is listed **because it must
/// be there**: this is an equality, so if that import silently disappears the
/// assertion fails rather than passing quieter.
///
/// The pair is the contract, not the child on its own. Keying on the child alone
/// would make the observed set a union over every scanned file, so a reference
/// added to a future submodule of this module would leave the set unmoved and
/// pass. Adding a row here is a deliberate decision to accept a new dependency
/// from this command onto a new part of the web transport.
const ALLOWED_WEB_REFERENCES: [(&str, &str); 2] = [
    ("src/commands/project_settings.rs", "broadcast"),
    ("src/commands/project_settings.rs", "event_broadcast"),
];

/// Every `(file, child)` reference the emitter module is allowed to make under
/// `crate::`, sorted.
///
/// One row. The emitter module's whole in-crate dependency is the `WsBroadcaster`
/// type in `broadcast_all`'s signature, and the non-absorption argument of the
/// plan's Section 4.3 is exactly the claim that this stays true.
const ALLOWED_EMITTER_CRATE_REFERENCES: [(&str, &str); 1] = [("src/web/event_broadcast.rs", "web")];

/// Every `(file, child)` reference the emitter module is allowed to make into a
/// child of `crate::web`, sorted.
///
/// Two equalities rather than one path, and this is deliberate: the `crate::`
/// table above pins the first segment and this one pins the second, and together
/// they admit `crate::web::broadcast` and nothing else. Expressing the contract
/// as one joined `web::broadcast` string would have needed a second matcher that
/// recurses through brace groups, where `children_under` already handles
/// `use crate::web::{broadcast::A, commands::B}` and `use crate::{web::A, x::B}`
/// correctly under each anchor. Reusing the audited matcher twice is worth more
/// than a prettier table.
const ALLOWED_EMITTER_WEB_REFERENCES: [(&str, &str); 1] =
    [("src/web/event_broadcast.rs", "broadcast")];

/// Every `(file, child)` reference the emitter module is allowed to make under
/// `super::`, sorted.
///
/// **This anchor exists only for the emitter module, and the asymmetry is the
/// point.** The dispatcher `web::commands` is the emitter's SIBLING: from inside
/// `src/web/`, `super::commands` reaches it without the text `web::` appearing
/// anywhere, so neither of the two tables above sees it. That is not an exotic
/// spelling, it is the idiom the neighbouring file already uses:
/// `src/web/commands.rs:12` writes `use super::broadcast::WsBroadcaster;`.
/// `commands::project_settings` needs no such anchor because it is not a sibling
/// of anything under `web`: every path from there into `web` must spell `web`
/// followed by `::`, or rename a group, which is refused separately.
///
/// The one allowed row is the test module reaching its own parent for the
/// function under test. **A glob is deliberately not allowed.** `use super::*;`
/// written at module level would pull `crate::web`'s children, `commands`
/// included, into scope under no name this scan could follow, and it is
/// indistinguishable by text from the same glob inside `mod tests`. That is why
/// Section 5.1 imports by name instead of globbing its parent.
const ALLOWED_EMITTER_SUPER_REFERENCES: [(&str, &str); 1] =
    [("src/web/event_broadcast.rs", "broadcast_all")];

/// The child #1265 removed, called out separately so its failure carries the
/// explanation of the cycle rather than the generic allowlist message.
const FORBIDDEN_WEB_CHILD: &str = "commands";

const ANCHOR: &str = "web::";
const CRATE_ANCHOR: &str = "crate::";
const SUPER_ANCHOR: &str = "super::";

/// The module this guard is written about, as path segments below `crate`.
const GUARDED_MODULE: [&str; 2] = ["commands", "project_settings"];

/// The module #1265 created to hold the emitter, as path segments below `crate`.
const EMITTER_MODULE: [&str; 2] = ["web", "event_broadcast"];

/// The emitter #1265 moved, and the one file that may define it.
///
/// The name only. `defines_emitter` decides what counts as a definition, because
/// `fn broadcast_all(` as a literal needle misses `fn broadcast_all (` and, more
/// to the point, misses a generic copy `fn broadcast_all<R: Runtime>(`, which is
/// the exact shape of the sibling `broadcast_all_r` that stays behind.
const EMITTER_NAME: &str = "fn broadcast_all";
const EMITTER_HOME: &str = "src/web/event_broadcast.rs";

/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Literals {
    Keep,
    Drop,
}

/// Replace every comment, and optionally every string or character literal, with
/// a single space, leaving code behind.
///
/// A comment is whitespace to the Rust lexer, so `web /* x */ ::commands` is the
/// same path as `web::commands`; collapsing whitespace alone would leave that
/// spelling intact and break the anchor. Tracking literals is what makes comment
/// removal correct at all: `"https://host"` carries a `//` that would otherwise
/// blank the rest of its line. Dropping literal bodies additionally stops prose
/// or a string from holding the observed set at its expected value after the real
/// references are gone, which is the failure the equality below exists to catch.
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
/// crate::web::{commands::broadcast_all, broadcast::WsBroadcaster};` does not
/// contain the text `web::commands` at all: the braces are in the way. Reflowed
/// across lines by rustfmt it does not contain it either. After normalization
/// every one of those forms is the same text and the use-tree can be read.
///
/// `U+200E` and `U+200F` are replaced first because Rust's lexer treats them as
/// whitespace and `char::is_whitespace` does not, so `split_whitespace` would
/// leave `web<U+200E>::commands` intact and the anchor would never match a path
/// rustc compiles without a warning. They are the only two characters where the
/// two definitions disagree; `U+0085`, `U+2028` and `U+2029` are covered.
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
/// out, as in `use crate::web as w;`, `use crate::web::{self as w};` or
/// `use crate as c;`.
///
/// After such a rename `w::commands::...` or `c::web::commands::...` reaches the
/// forbidden module under a name no text scan can follow, so the rename itself is
/// refused instead of followed. Anchored on the path punctuation in front of
/// `web` so that English prose about the web does not trip it, and on `use crate`
/// rather than bare `crate` for the same reason.
fn aliases_a_module_group(body: &str) -> bool {
    [
        "::web as ",
        "{web as ",
        ",web as ",
        "web::{self as ",
        "use crate as ",
    ]
    .iter()
    .any(|spelling| body.contains(spelling))
}

/// The leading identifier of a use-tree item: `commands` from `commands::{a, b}`,
/// from `commands as c` and from `commands`. A non-identifier item such as `*` is
/// returned as itself, so a glob is reported rather than silently dropped.
///
/// A leading `r#` is dropped first: `r#commands` is the raw-identifier spelling
/// of `commands` and names the same module, but reading it literally stops at the
/// `#` and reports the child as `r`, so the reference would be caught by the
/// equality assertion instead of by the #1265 message that explains it.
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
/// as `commands::{a, b}, broadcast::c` yields two items and not three.
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
/// `anchor` is `web::` for the dispatcher question and `crate::` for the emitter
/// module's own dependencies. Both go through the same brace-group handling, so
/// `use crate::{web::A, session::B}` reports `web` and `session` under
/// `crate::`, while `use crate::web::{broadcast::A, commands::B}` reports
/// `broadcast` and `commands` under `web::`.
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

/// Whether `body`, which must be scrubbed and normalized, defines the emitter.
///
/// The needle is the name, and what follows it decides. `fn broadcast_all_r(`
/// is not a definition of `broadcast_all`, so a following identifier character
/// disqualifies the hit; `fn broadcast_all (` and `fn broadcast_all<R: Runtime>(`
/// are definitions, so whitespace is skipped and both `(` and `<` count. A
/// generic copy is the shape that matters here: it is exactly how the sibling
/// `broadcast_all_r` is written, so it is the shape a copy would most naturally
/// take.
fn defines_emitter(body: &str) -> bool {
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(EMITTER_NAME) {
        let after = from + offset + EMITTER_NAME.len();
        let next = body[after..].trim_start().chars().next();
        if matches!(next, Some('(') | Some('<')) {
            return true;
        }
        from = after;
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
/// this resolver reads it as a child of the file. It used to resolve anyway,
/// to a different file, whenever one happened to exist at the path it looks in,
/// and then reported green having read the wrong file. A scanner that cannot
/// tell which file it should be reading has to say so.
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
///   Trying the subdirectory first picks the file rustc does not compile
///   whenever both exist, which is the whole of a benign-decoy attack. Since
///   `module_body` refuses declarations nested in inline blocks, the case where
///   rustc would use the module directory cannot reach this function.
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

/// The files rustc compiles for `module` and every module below it, resolved by
/// walking `mod` declarations down from the crate root.
///
/// The walk carries a frontier rather than a single file, because a segment can
/// be declared more than once under opposite `cfg`s and this resolver keeps both
/// arms. An error at any step is propagated rather than skipped: a module that
/// cannot be located is the one case where reading nothing must not look like
/// reading nothing forbidden.
fn sources_of(module: &[&str]) -> Result<Vec<PathBuf>, String> {
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
/// of it would refuse to answer the question it was called to answer. Its
/// children are lost, and that is stated rather than hidden.
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
/// `web` and `anchored` are `(file, child)` pairs under `web::` and under
/// `crate::` respectively; `aliases` is the files that rename a module group.
struct Observation {
    web: Vec<(String, String)>,
    anchored: Vec<(String, String)>,
    relative_up: Vec<(String, String)>,
    aliases: Vec<String>,
}

/// Read every file of `module` and report what it names.
///
/// A file reached through the module tree is a file rustc compiles, so a `scrub`
/// failure on one of them is fatal here and says so: it is source the compiler
/// reads and this scan could not.
fn observe(module: &[&str]) -> Observation {
    let files = sources_of(module).unwrap_or_else(|reason| {
        panic!(
            "the module {module:?} could not be resolved from the module tree, so this scan \
             proves nothing: {reason}\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: this guard exists to prove that a specific \
             dependency is absent. If the module cannot be located, the guard has read nothing \
             and must say so rather than pass. Rename or move the module and this message names \
             the file whose `mod` declaration no longer resolves; update GUARDED_MODULE or \
             EMITTER_MODULE, or the declaration, to match."
        )
    });
    assert!(
        !files.is_empty(),
        "the module {module:?} resolved to no files at all; the scan proves nothing"
    );

    let mut web = Vec::new();
    let mut anchored = Vec::new();
    let mut relative_up = Vec::new();
    let mut aliases = Vec::new();
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
        web.extend(name(children_under(&body, ANCHOR)));
        anchored.extend(name(children_under(&body, CRATE_ANCHOR)));
        relative_up.extend(name(children_under(&body, SUPER_ANCHOR)));
        if aliases_a_module_group(&body) {
            aliases.push(relative.clone());
        }
    }
    web.sort();
    web.dedup();
    anchored.sort();
    anchored.dedup();
    relative_up.sort();
    relative_up.dedup();
    Observation {
        web,
        anchored,
        relative_up,
        aliases,
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

/// #1265: `commands::project_settings` used to call
/// `crate::web::commands::broadcast_all`, which put the Tauri command and the
/// browser dispatcher in a mutual pair and held the command inside an 89 module
/// cyclic SCC. The emitter moved to `web::event_broadcast` so both surfaces
/// depend downward on it.
///
/// This test lives in `src-tauri/tests/`, which is a separate leaf crate the
/// detector marks `enabled: opts.includeTests` and the record is emitted with
/// `includeTests: false`. It therefore adds no arc and no module, is outside the
/// tree it reads, and never has to excise itself from its own scan. Whole files
/// are read, `#[cfg(test)]` regions included, which is stricter than the
/// detector: a false red is argued about, a false green is believed.
#[test]
fn project_settings_names_no_web_module_above_it() {
    let seen = observe(&GUARDED_MODULE);
    let observed = seen.web;
    let alias_offenders = seen.aliases;

    let dispatcher_offenders: Vec<String> = observed
        .iter()
        .filter(|(_, child)| child == FORBIDDEN_WEB_CHILD)
        .map(|(file, _)| file.clone())
        .collect();
    let allowed = expected(&ALLOWED_WEB_REFERENCES);
    let unlisted_offenders: Vec<String> = observed
        .iter()
        .filter(|pair| !allowed.contains(pair))
        .map(|(file, _)| file.clone())
        .collect();

    assert!(
        dispatcher_offenders.is_empty(),
        "commands::project_settings must not reference web::commands.\n\
         \n\
         WHY: `web::commands` is the browser IPC dispatcher and this module is a \
         Tauri IPC command. Two transport surfaces must not depend on each other, \
         and the dispatcher already depends on this module for its \
         `get_project_groups_inner`, `update_project_groups_inner`, \
         `project_groups_updated_payload` and `PROJECT_GROUPS_UPDATED_EVENT`. \
         Issue #1265 removed the one call going the other way, \
         `crate::web::commands::broadcast_all`, because that mutual pair was the \
         only thing holding this module inside the crate's 89 module cyclic SCC. \
         Any reference from here puts it back in.\n\
         \n\
         INSTEAD: emit through `crate::web::event_broadcast::broadcast_all`, which \
         the dispatcher and this module both depend on downward. If you need \
         something from the dispatcher that is not an emission, it belongs in a \
         module below both of them, never in either one.\n\
         \n\
         SCOPE: this is a net over the spellings of that reference, not a proof \
         that it cannot return. It matches text and does not resolve names, so a \
         spelling it does not know about passes it; the ones it is known to miss \
         are listed at the top of this file. The authoritative check is the cycle \
         detector, whose `coverage.graphShape.cyclicSccs` must stay at 1 with \
         `sccSize(agentscommander_lib::commands::project_settings) = 1`.\n\
         \n\
         OFFENDING FILES: {}",
        dispatcher_offenders.join(", ")
    );

    assert!(
        alias_offenders.is_empty(),
        "commands::project_settings must not rename the web module group or the \
         crate root.\n\
         \n\
         WHY: `use crate::web as <name>;`, `use crate::web::{{self as <name>}};` \
         and `use crate as <name>;` each put every module under `web`, \
         `web::commands` included, within reach under a name this scan cannot \
         follow. Following it would mean resolving names, which a text scan does \
         not do, so the rename is refused instead.\n\
         \n\
         INSTEAD: import the item you need by its real path, so this guard and \
         the cycle detector can both see it.\n\
         \n\
         OFFENDING FILES: {}",
        alias_offenders.join(", ")
    );

    assert_eq!(
        observed,
        allowed,
        "the set of web modules named from commands::project_settings moved.\n\
         \n\
         FILES NAMING SOMETHING UNLISTED: {}\n\
         \n\
         Each entry is a (file, child) pair, because the file is half of the \
         rule. Naming an allowed child from a different file of this module's \
         subtree is still a new dependency, so it fails here even though the set \
         of children on its own would not have moved.\n\
         \n\
         A LARGER SET means this command reached further into the web transport. \
         That is a decision, not a detail: remove it, or add its pair to \
         ALLOWED_WEB_REFERENCES and say in the commit why the new dependency is \
         acceptable.\n\
         \n\
         A SMALLER SET is the more dangerous failure, and it is why this is an \
         equality and not a denylist. `event_broadcast` is listed because #1265 \
         put it there: if that import silently disappears, the emitter has been \
         reached some other way and the reason this module is out of the cycle \
         has changed without anybody saying so. A shrinking set also means the \
         scan may have stopped seeing references it used to see, and a guard that \
         observes nothing passes everything. Comments and literal bodies are \
         removed before the scan so no amount of prose can hold this set up while \
         the real references disappear.",
        unlisted_offenders.join(", ")
    );
}

/// #1265 Section 4.3: the emitter module cannot be absorbed by the knot because
/// its only outgoing arc goes to `web::broadcast`, which has none of its own.
///
/// **That is a claim about outgoing arcs, and this test is the only thing that
/// holds it.** Measured on the arc record: adding one arc from
/// `web::event_broadcast` to any knot member takes the knot from 88 back to 90
/// and puts `commands::project_settings` back inside it, leaving the crate worse
/// than before #1265, while every assertion in the test above stays green,
/// because that module's own file did not change.
///
/// Two equalities, one per anchor. `crate::` pins the first segment and `web::`
/// pins the second, and together they admit `crate::web::broadcast` and nothing
/// else. The alias check is here for the same reason it is above: a renamed
/// group is a path this scan cannot follow.
#[test]
fn the_emitter_home_names_nothing_but_the_websocket_fan_out() {
    let seen = observe(&EMITTER_MODULE);
    let alias_offenders = seen.aliases;

    assert!(
        alias_offenders.is_empty(),
        "web::event_broadcast must not rename the web module group or the crate \
         root; see the same assertion for commands::project_settings.\n\
         \n\
         OFFENDING FILES: {}",
        alias_offenders.join(", ")
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_EMITTER_CRATE_REFERENCES),
        "the set of crate modules named from web::event_broadcast moved.\n\
         \n\
         WHY THIS MATTERS MORE THAN IT LOOKS: #1265 is only correct while this \
         module cannot reach the cyclic SCC. It has one in-crate dependency, the \
         `WsBroadcaster` type in `broadcast_all`'s signature, and that is what \
         makes the non-absorption argument of the plan's Section 4.3 true. One \
         `use` from here into any module of the knot puts \
         `commands::project_settings` back inside it and leaves the knot LARGER \
         than it was before #1265, and no other test in this repository would go \
         red.\n\
         \n\
         INSTEAD: if this module needs something else, that something belongs \
         below it, and the plan's Section 4.3 has to be rewritten before the \
         dependency is added. Adding a row to ALLOWED_EMITTER_CRATE_REFERENCES is \
         a decision about the crate's shape, not a detail.\n\
         \n\
         A SMALLER SET is a failure too: `web` is listed because the emitter's \
         signature needs `WsBroadcaster`, so if it disappears the module has \
         changed shape or the scan has stopped seeing it."
    );

    assert_eq!(
        seen.web,
        expected(&ALLOWED_EMITTER_WEB_REFERENCES),
        "the set of web modules named from web::event_broadcast moved.\n\
         \n\
         `broadcast` is the WebSocket fan-out and is the only child of `web` this \
         module may name. `commands` here would be the same cycle #1265 removed, \
         written from the other end: the emitter would depend on the dispatcher \
         that depends on the command that depends on the emitter.\n\
         \n\
         See the crate-anchored assertion above for why a change here is a \
         decision about the crate's shape."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_EMITTER_SUPER_REFERENCES),
        "the set of names reached by `super::` from web::event_broadcast moved.\n\
         \n\
         WHY THIS ANCHOR EXISTS AT ALL: the dispatcher `web::commands` is this \
         module's SIBLING. From inside src/web/, `use super::commands::X;` \
         reaches it without the text `web::` appearing anywhere, so neither of \
         the two assertions above can see it, and it would rebuild the #1265 \
         cycle in silence. This is not an exotic spelling: the neighbouring \
         file writes `use super::broadcast::WsBroadcaster;` at line 12, so it \
         is the first thing a reader of src/web/ would copy.\n\
         \n\
         INSTEAD: name what you need by its `crate::` path, the way the two \
         production imports in this module already do. Then the arc it creates \
         is visible to the record, to the two assertions above, and to anyone \
         reading the file.\n\
         \n\
         A GLOB FAILS HERE ON PURPOSE. `use super::*;` at module level pulls \
         `crate::web`'s children, `commands` included, into scope under no name \
         a text scan can follow, and it is indistinguishable by text from the \
         same glob inside `mod tests`. The test module therefore imports \
         `broadcast_all` by name, which is the one row this table allows.\n\
         \n\
         `commands::project_settings` has no equivalent assertion because it is \
         not a sibling of anything under `web`: every path from there into the \
         dispatcher must spell `web` followed by `::`, or rename a group, which \
         is refused separately."
    );
}

/// #1265 criterion 8: the emitter moved, it was not copied. Two copies would
/// drift and the layering claim would be false while every arc assertion still
/// passed.
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
fn the_dual_transport_emitter_is_defined_exactly_once() {
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
                if defines_emitter(&normalized(&code)) {
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
             and cannot claim the emitter is defined exactly once.\n\
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
             **Do not add an extension filter to `every_file_under`**: rustc decides what to \
             compile from the module tree, a filter decides from the name, and production \
             code lives in the gap between the two.\n\
             \n\
             FILES THE COMPILER READS THAT COULD NOT BE DELIMITED: {fatal:?}"
        );
    }

    assert_eq!(
        homes,
        vec![EMITTER_HOME.to_string()],
        "the dual-transport emitter must be defined exactly once, in {EMITTER_HOME}.\n\
         \n\
         WHY: #1265 moved `broadcast_all` out of the browser command dispatcher \
         so that a Tauri command would stop depending on it. A move that left a \
         copy behind satisfies every arc assertion and is still wrong: the two \
         copies drift, and the claim that this module is the only home of the \
         emitter stops being true.\n\
         \n\
         WHAT COUNTS AS A DEFINITION: the name `broadcast_all` followed, after \
         any whitespace, by `(` or `<`. The generic form is included on purpose: \
         `fn broadcast_all<R: Runtime>(...)` is a copy of this emitter and it is \
         the exact shape of the sibling `broadcast_all_r` that stays in \
         `web::commands`, so it is the shape a copy would most naturally take.\n\
         \n\
         INSTEAD: keep one definition. If a second transport needs a variant, \
         give it a different name and a reason, in {EMITTER_HOME} beside this \
         one.\n\
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

**Required behaviour: byte for byte identical observable behaviour.** This is a move, not a rewrite.

| Property | Requirement |
|---|---|
| Emission targets | `tauri::Emitter::emit` to all windows, then `WsBroadcaster::broadcast_event` to all WS clients, unchanged |
| Emission order | Tauri first, WebSocket second, unchanged |
| Event name for this command | `project_groups_updated`, from `PROJECT_GROUPS_UPDATED_EVENT`, unchanged |
| Payload | `project_groups_updated_payload`, `{ "projectPath": …, "config": … }`, unchanged |
| Signature of `broadcast_all` | `(&tauri::AppHandle, &WsBroadcaster, &str, &Value)`, unchanged |
| The nine other callers in `web/commands.rs` | same arguments, same order, unchanged |
| `broadcast_all_r`, `broadcast_all_to_managed` | untouched, still in `web::commands` |

**Edge cases, all preserved as they are today, none to be "fixed" in this change:**

- **A Tauri emit that fails** is discarded by `let _ = …`. It is not logged, not retried and not
  propagated, and the caller cannot observe it. Keep it exactly as written.
- **A WebSocket client whose queue is full** is dropped by `WsBroadcaster::broadcast_event`'s
  `retain(|tx| tx.try_send(…).is_ok())`. That behaviour lives in `web::broadcast` and this change does
  not touch it.
- **A payload that fails to clone** cannot occur: `serde_json::Value` clone is infallible.
- **`update_project_groups` failing before the emit** returns early through the `?` on
  `update_project_groups_inner`, so no event is emitted. Unchanged: the `?` stays where it is.

**Failure behaviour, preserved.** Do not add logging, error propagation or a return value while
moving this code. Changing failure behaviour inside a structural fix makes the change unreviewable
against its own acceptance criteria.

---

## 7. Compatibility, security, and the complete arc enumeration

### 7.1 Arcs added and removed

Four lines change in `src-tauri/module-arcs.txt`, and these are all of them. Every one was produced by
simulating this exact change over the committed record and re-running Tarjan.

**Removed (1):**

```
agentscommander_lib::commands::project_settings -> agentscommander_lib::web::commands
```

Currently line 386. Cause: the only call to that path is gone.

**Added (3):**

```
agentscommander_lib::commands::project_settings -> agentscommander_lib::web::event_broadcast
agentscommander_lib::web::commands              -> agentscommander_lib::web::event_broadcast
agentscommander_lib::web::event_broadcast       -> agentscommander_lib::web::broadcast
```

| Added arc | Cause | Why it is safe |
|---|---|---|
| `commands::project_settings -> web::event_broadcast` | `use crate::web::event_broadcast::broadcast_all;` in `commands/project_settings.rs` | Points from level 2 down to level 1. The target has one out-arc, to a module with none, so it cannot reach the knot and no cycle is possible. |
| `web::commands -> web::event_broadcast` | the same import in `web/commands.rs` | Points from inside the knot down to a level 1 module outside it. Absorption would need a path back out of `web::event_broadcast` into the knot, and there is none. |
| `web::event_broadcast -> web::broadcast` | `use crate::web::broadcast::WsBroadcaster;` in the new module, required by the signature | The same dependency `web::commands` already had for the same type. `web::broadcast` has zero out-arcs, so this arc cannot carry anything into a cycle. |

**No added arc points into the knot**, and no added arc points at `web::commands` from anywhere.

Sorted positions in the regenerated record, for reviewing the diff: current line 386 disappears; the
`commands::project_settings` line takes position **386**; the `web::commands` line lands at **975**
and the `web::event_broadcast` line at **976**. Net: **974 arcs to 976**.

**Adding `src-tauri/tests/project_settings_layering.rs` must not change that diff at all.** Integration
test targets are separate leaf crates the instrument marks `enabled: opts.includeTests`, and the
record is emitted with `includeTests: false`. Measured on the current tree: `module-arcs.txt` holds
zero arcs from `tests/` while `src-tauri/tests/` holds 21 files. If the arc diff shows anything
attributable to the new test file, the instrument was run with the wrong flags; re-read Section 9.2
before touching anything else.

### 7.2 Compatibility

- **Frontend: no change, and none is permitted.** No event name, payload shape or serialization moves.
  `broadcast_all` is not a `#[tauri::command]` and is not reachable from the wire, so no IPC surface
  changes on either transport.
- **Rust API path change.** `agentscommander_lib::web::commands::broadcast_all` becomes
  `agentscommander_lib::web::event_broadcast::broadcast_all`. A repository-wide search finds exactly
  one consumer outside the defining file, `commands/project_settings.rs:44`, which this change edits.
  Nothing else in the workspace names it. The library is internal to this app.
- **The old path stops compiling on purpose.** After the move, `web::commands` holds the symbol
  through a private `use`, so `crate::web::commands::broadcast_all` fails with `error[E0603]` rather
  than silently resolving. That is the backstop Section 5.3 edit 1 documents.
- **No config, schema, file format or persisted state is touched.**

### 7.3 Security

No new surface. `broadcast_all` was `pub` in a `pub` module and is `pub` in a `pub` module after, and
it is not a command on either transport. No new capability, no new IPC entry point, no change to what
is emitted or to who receives it. The move narrows one path (`web::commands::broadcast_all` becomes
private) and widens none.

---

## 8. Implementation order

Each step leaves the tree in a state the next one can check.

1. Create `src-tauri/src/web/event_broadcast.rs` with the content of Section 5.1 and nothing else.
2. Add `pub mod event_broadcast;` to `src-tauri/src/web/mod.rs` (Section 5.2), after `mod embedded;`.
3. Apply the two edits to `src-tauri/src/web/commands.rs` (Section 5.3).
4. Apply the two edits to `src-tauri/src/commands/project_settings.rs` (Section 5.4).
5. Create `src-tauri/tests/project_settings_layering.rs` with the content of Section 5.5, verbatim.
   It is already rustfmt clean; reflowing it by hand changes the bytes this plan certified.
6. From `src-tauri`: `cargo fmt --check`. **This runs first, before the compiler.** It exited 0 on
   the tree before this change and every new file is rustfmt clean as written, so a diff here means
   a file was retyped rather than copied. It is the cheapest step and it is the one that catches the
   copy error, which is the error that would otherwise waste a full compile.
7. From `src-tauri`: `cargo check --all-targets`.
8. From `src-tauri`: `cargo clippy --all-targets -- -D warnings`.
9. From `src-tauri`: `cargo test --lib --bins --tests`. The new guard is an integration test target,
   so it runs under `--tests`. Measured baseline in Section 9.1.
10. From the repo root: `npm run typecheck`, `npm test` and `npm run test:debt`. All three must pass
    unchanged; the frontend is not edited, and these run because CI runs them.
11. **Prove the guard is alive**, in the two parts of Section 9.3.4. They prove different things and
    neither substitutes for the other.

    **11a, the visibility backstop.** Put `crate::web::commands::broadcast_all(...)` back into the
    production region of `src-tauri/src/commands/project_settings.rs` and run
    `cargo test --test project_settings_layering`. **The expected result is a compilation failure,
    `error[E0603]`**, because after the move `web::commands` holds the symbol behind a private `use`
    (Section 7.2). **That is the visibility backstop and it is NOT the guard going red**: the guard's
    binary never linked and never ran. Record it as what it is, then remove the probe.

    **11b, the guard itself.** Put a forbidden reference that **does** compile into the same
    production region:

    ```rust
    crate::web::commands::broadcast_all_r(&app, PROJECT_GROUPS_UPDATED_EVENT, &payload);
    ```

    `broadcast_all_r` is `pub`, takes `(&tauri::AppHandle<R>, &str, &Value)`, and `app` is in scope,
    so this compiles. It is the closest surviving shape to the call that #1265 removed and it goes
    through the same `web_children` path. Run `cargo test --test project_settings_layering` and
    confirm the guard **fails with the #1265 message and names
    `src/commands/project_settings.rs`**. Then remove the probe and confirm green.

    If emitting twice during the probe is unwelcome, the authorised equivalent is
    `let _ = std::any::type_name::<crate::web::commands::WsState>();`. `WsState` is `pub` in
    `web/commands.rs`. Do not ask again; either is approved.

    Use `cargo test --test project_settings_layering` and not `cargo test --tests` for this loop: it
    targets the one binary and takes seconds, where `--tests` builds and runs all 22 integration
    targets. Step 9 already covers the full suite.

    If 11b does not go red, **stop and report**: a guard that cannot fail is worth nothing and the
    rest of the verification is void.

12. **Confirm both probes are out of the tree before measuring anything.** Run
    `git diff -- src-tauri/src/commands/project_settings.rs` and confirm it shows **only** the two
    edits of Section 5.4: the added import and the dropped `crate::web::commands::` prefix. Nothing
    else. Regenerating the arc record with a probe still in place produces a record that contains the
    very arc this change removes. Criterion 5 would catch it, but this step exists so nothing depends
    on that.
13. Regenerate the arc record (Section 9.2).
14. Verify the graph shape and the levels (Section 9.5), then review
    `git diff -- src-tauri/module-arcs.txt` against Section 7.1: exactly 1 line removed and 3 added,
    and no others.
15. Commit the four source files, the new test file, `src-tauri/module-arcs.txt` and this plan.
    Delete the emitted graph. **Never commit a graph:** it is about 4.9 MB, it carries the absolute
    path of the machine that produced it, and it is CRLF sensitive.

    **This plan needs `git add -f`, and it runs BEFORE the commit.** `.gitignore` line 11 ignores
    `plans/`, so a plain `git add plans/1265-extract-project-settings-from-scc.md` does nothing and
    `git status` stays clean while the file is silently left out. That is measured, and it is exactly
    what happened to `plans/1252-break-loops-scheduler-cycle.md`: `git show --stat 7778f67b` lists
    five files and no plan; that plan only entered the repository later, in `71831b4`. The order for
    this step is therefore:

    ```
    git add -f plans/1265-extract-project-settings-from-scc.md
    git add <the four source files> src-tauri/tests/project_settings_layering.rs src-tauri/module-arcs.txt
    git commit -m "..."
    git show --stat
    ```

    The `git show --stat` is part of this step, not a follow-up: **confirm the plan is in the commit
    before reporting the step done.**

16. Stay on `refactor/1265-extract-project-settings-wg11`. Never touch `main`, do not open a PR, do
    not merge, do not use `--admin`. **Issue #1265 stays OPEN**; no closing keyword anywhere.

If step 6, 7, 8, 9 or 10 fails, fix it before continuing. If step 11b does not go red, **stop and
report**: a guard that cannot fail is worth nothing and the rest of the verification is void.

**If step 14 disagrees with Section 7.1, revert `src-tauri/module-arcs.txt` before reporting, then
stop.** `git checkout -- src-tauri/module-arcs.txt`. Do not adjust the plan's numbers to match the
output, and do not leave the regenerated record sitting in the tree: a modified arc record is exactly
what criterion 6 fails on, so leaving it there means the next person reads a criterion 6 failure that
has nothing to do with the disagreement being reported. Report the disagreement on a clean tree.

---

## 9. Tests and acceptance criteria

### 9.1 What must be green

| Command | Working directory | Expectation |
|---|---|---|
| `cargo check --all-targets` | `src-tauri` | clean |
| `cargo clippy --all-targets -- -D warnings` | `src-tauri` | clean |
| `cargo fmt --check` | `src-tauri` | clean |
| `cargo test --lib --bins --tests` | `src-tauri` | full suite green, including all three new tests of Section 5.5 |
| `npm run typecheck` | repo root | clean |
| `npm test` | repo root | full vitest suite green |
| `npm run test:debt` | repo root | clean; this change adds no ignored or placeholder test |

**Measured baseline, on this branch, before the change.** "Green after" only means something if
somebody measured green before, so these were measured by `dev-rust` on
`refactor/1265-extract-project-settings-wg11` at `5168310` with a warm `target/`, and they are the
baseline criterion 7 is compared against:

| Command | Measured before the change |
|---|---|
| `cargo fmt --check` | **exit 0**, ~2 s |
| `cargo check --all-targets` | **exit 0**, 34 s |
| `cargo test --lib --bins --tests` | **exit 0**, 355 s: **3330 passed, 0 failed, 22 ignored** in the lib, plus all 21 integration targets green |

The slowest integration targets are `terminal_snapshot_host` (11.3 s), `pty_lifecycle_regression`
(10.1 s), `cli_ui_automation` (6.9 s) and `cli_role_experiment` (6.2 s); the rest of the 355 s is
compilation. `loops_layering` runs in 0.01 s, so the new guard, which does more I/O (its second test
reads the 188 files and 9.5 MB under `src/`), stays negligible next to that. With a cold `target/`,
add the full Tauri build, which is tens of minutes. `cargo clippy --all-targets -- -D warnings`,
`npm run typecheck`, `npm test` and `npm run test:debt` were not timed.

**The 22 ignored tests are pre-existing and are not this change's business.** There are also open
issues for tests that fail under load (#1261, #1258, #1256, #1255, #1254, #1253, #1248, #1241 and
others). If a failure appears at the end of implementation, **identify it against that list before
calling it a regression from this change**; a flake from that set is not evidence about #1265 either
way, and reporting it as one wastes the review.

`test:debt` scans `src-tauri/tests/*.rs` as well as `src-tauri/src/**.rs`, so it does read the new
guard. It reports `#[ignore]` attributes and placeholder bodies (`todo!()`, `unimplemented!()`, a
`panic!("TODO…")`); the guard has none, so it stays clean. The `panic!` calls inside the guard carry
real failure messages and are not placeholders.

The one way the guard could have tripped `test:debt` was checked in the scanner's source rather than
assumed. `scripts/check-test-debt.mjs` masks Rust files with `maskCommentsAndStrings(source, {
singleQuote: false })`, and the guard's `scrub` contains four `'"'` character literals whose bare
double quote would have inverted the masker's string state and mangled everything after it. It does
not: `maskSource` has a dedicated Rust character-literal branch (`rustCharLiteralStop`) that consumes
`'"'`, `'\\'` and `'\''` correctly and returns -1 for lifetimes such as `&'static str`, so the
guard's `Result<String, &'static str>` signatures are safe too. Both of the guard's tests have
`assert!`/`assert_eq!` bodies, so `hasExecutableRustBody` classifies them as executable, and neither
carries `#[ignore]`. Run it anyway; this paragraph says why it is expected to pass, not that it may
be skipped.

Four existing tree-scanning tests were checked against this change and none is expected to move:

- `src-tauri/tests/loops_layering.rs` scans `src/loops/` only, which this change does not touch.
- `src-tauri/tests/pty_writer_inventory.rs` walks all of `src/` but matches only four spellings,
  verified in its source: `write_with_permit(`, `backend.write(`, `route_guard.write(` and
  `for_route_guard`. None appears in `web/event_broadcast.rs`.
- `session::selection`'s `production_selection_and_lifecycle_sources_have_one_owner` fires on four
  `session_*` event literals and six manager mutator signatures; `web/event_broadcast.rs` contains
  none of them.
- `loops::scheduler`'s `scan_once_calls_archived_candidate_filter` reads `scheduler.rs`, untouched.

The three tests of Section 5.5 are the fifth, sixth and seventh tree-scanning tests and the only ones
this change adds. Measured in the laboratory against a copy of the real `src-tauri/src/`, all three
together run in **1.40 s**, so they are negligible against the 355 s baseline above.

### 9.2 Regenerating the arc record

From the repository root, with

```
VAULT = repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust
GRAPH = an absolute path OUTSIDE the working tree, e.g. %TEMP%\ac-1265\graph.json
```

```
node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "<GRAPH>" --quiet
npm run record:arcs -- --graph "<GRAPH>"
```

Then delete `<GRAPH>`.

- **The detector exits 1 while any cycle remains, and writes the graph anyway. After this fix it will
  still exit 1**, because the 88 module knot survives. Only exit 3 means no graph was written. Do not
  read that 1 as a failed change.
- Every flag above is part of the measurement. `scripts/02-module-arc-record.mjs` refuses a graph
  whose `target.rootPath` last segment is not `src-tauri`, or whose `crateDiscovery`, `includeTests`
  or `excludes` differ, with exit 3. Do not add flags.
- Emit outside the working tree. `src-tauri/module-arcs.txt` is pinned to LF in `.gitattributes`; do
  not defeat that.
- **Never diff the instrument's suggested cut between runs.** It is one of several valid minimum
  feedback arc sets and its membership is not unique. Compare arc sets and SCC membership, which is
  what every criterion below does.

### 9.3 The structural guard, and what it can and cannot prove

The instrument that would catch a reintroduced arc is run by hand and is deliberately not wired to
CI, so a guard inside the suite is the only thing that fires without somebody remembering to look.
That guard is `src-tauri/tests/project_settings_layering.rs`. **Its content is Section 5.5 and is not
repeated here.** This section is the reasoning behind it, and it is the part to read before touching
the matcher.

#### 9.3.1 What the guard is

A net over the **spellings** a dependency can be written in. Not a proof that the dependency cannot
return. It matches text and does not resolve names, so a spelling it does not know about passes it. A
green result means "no known spelling is present". It never means "the cycle is impossible".

The authoritative check is the cycle detector of Section 9.2, whose `coverage.graphShape.cyclicSccs`
must stay at 1 with `sccSize(commands::project_settings) = 1`. The guard says so about itself, in its
module header and again in the `SCOPE` paragraph of its own failure message, where a doc comment
would not be printed. **Report the guard's green and the detector's numbers as two separate things.**

#### 9.3.2 Why arc absence alone is not enough

`src-tauri/module-arcs.txt` is produced by an instrument with a measured blind spot: `src/lib.rs:1178`
constructs `loops::scheduler::LoopScheduler::new()` with no `crate::` prefix and no corresponding arc
exists among the 974. An arc absent from the record is therefore not by itself proof that the
dependency is gone.

Section 2.6 narrows the shape of that blind spot and the narrowing is stated rather than hidden:
`use self::` and `use super::` **are** resolved, measured twice on production code. The dispatch's
claim that rewriting the call as `super::super::…` would delete the arc does not hold for those forms.
What is measured missing is a fully unanchored path. **This changes nothing about the need for the
guard**, which is not built around that one spelling: it is built around the classes of Section 9.3.3,
and probe P5 of Section 9.3.4 shows it catches the `super::super::` form anyway.

#### 9.3.3 What the matcher does, and the three classes it closes

The dispatch names three ways text approximation loses to what the compiler resolves. The guard closes
each one structurally rather than by adding a substring.

1. **The use-tree.** `use crate::web::{broadcast::WsBroadcaster, commands::broadcast_all};` does not
   contain the text `web::commands` at all: the braces are in the way, and rustfmt may reflow it
   across four lines. The guard collapses whitespace, deletes the space either side of `::`, `{`, `}`
   and `,`, then walks the brace group balanced, splits on its own top level commas and takes the
   leading identifier of each item. A leading `r#` is stripped first, so `r#commands` is read as
   `commands` and fails with the message that explains the cycle rather than with the generic one.
   Renaming the whole group with `use crate::web as w;` is refused outright rather than followed,
   because following it would mean resolving names.
2. **Conditional compilation.** The guard is an integration test target outside the tree it reads, so
   it never has to excise itself and never cuts a file at its first `#[cfg(test)]`. It reads whole
   files, test regions included. That is stricter than the detector, which ignores them, and
   strictness is the safe direction: a false red is argued about, a false green is believed.
3. **The module tree.** **The guard does not walk a directory.** It resolves files by following `mod`
   declarations down from `src/lib.rs`, honouring `path = "…"` in both `#[path]` and
   `#[cfg_attr(…, path = …)]`, collecting **every** declaration of a segment rather than the first,
   and handling both the `x.rs` and `x/mod.rs` forms. This is the one place the guard goes further
   than the #1252 precedent, which listed `#[path]` relocation as an uncovered spelling; probes P11,
   P12 and P20 to P23 of Section 9.3.4 measure it closed.

   **It is not rustc and Section 9.3.7 is the list of where it differs.** The first version claimed
   flatly that "the set of files is the set rustc compiles", and that claim was false in three
   measured ways at once: the `#[path]` candidate order was inverted, `cfg_attr` was invisible, and
   only the first of several `mod` declarations of one name was followed. It now over-reads where
   over-reading is safe and refuses where it is not. A `mod` declaration that resolves to no existing
   file, to two existing files, or that sits inside an inline `mod` block, is a hard failure naming
   the file, never a skip, so an unresolvable tree cannot produce a quiet green.

Separately, **comments and the bodies of string and character literals are removed before anything is
matched**. A comment is whitespace to the Rust lexer, so `web /* x */ ::commands` is the same path;
and prose must not be able to hold the observed set at its expected value after the real references
are deleted. A comment, literal or brace group that never closes is an error rather than a truncated
result: a scanner that cannot delimit what it is reading must say so.

**Membership, not counting.** The guard asserts that the observed set of `(file, child)` pairs
**equals** `[("src/commands/project_settings.rs", "broadcast"), ("src/commands/project_settings.rs",
"event_broadcast")]`. An equal count is not an equal set, and **a set that shrinks is the more
dangerous failure**: `event_broadcast` is listed because #1265 put it there, so if that import
silently disappears the assertion fails instead of passing quieter. `assert!(!files.is_empty(), …)`
and the resolver's hard failure do the same job one level up: an empty scan cannot pass.

The pair, and not the child alone, is the contract. Keying on the child would make the observed set a
union over every scanned file, so a reference added to a future submodule would leave the set unmoved
and pass. That exact defect was found and fixed in the #1252 guard after review; this one is written
with the fix from the start.

**The second test**, `the_emitter_home_names_nothing_but_the_websocket_fan_out`, is the one Section
4.3 depends on and the one the first version did not have: the non-absorption argument fails on an
**outgoing** arc from `web::event_broadcast`, and until this test existed nothing in the repository
watched that. Three equalities under three anchors, `crate::`, `web::` and `super::`.

**Why three anchors and not two, and why only for this module.** The first two admit
`crate::web::broadcast` and nothing else, expressed as one equality pinning the first segment and one
pinning the second. That was measured leaving a live spelling open: `web::commands` is this module's
**sibling**, so `use super::commands::…;` from inside `src/web/` reaches the dispatcher with no
`web::` token anywhere and no `crate::` either, and it rebuilds the whole cycle. It is not an exotic
form. `src/web/commands.rs:12` writes `use super::broadcast::WsBroadcaster;`, so `super::` is the
idiom of that directory and the first thing a reader would copy. The third equality allows exactly
one pair, the test module reaching its own parent for `broadcast_all`, and refuses everything else
including a glob: `use super::*;` at module level pulls `crate::web`'s children into scope under no
followable name, and text cannot tell it apart from the same glob inside `mod tests`, so Section 5.1
imports by name instead.

`commands::project_settings` gets no such anchor, and the asymmetry is the argument rather than an
oversight: it is not a sibling of anything under `web`, so every path from it into the dispatcher
must spell `web` followed by `::`, or rename a group, which `aliases_a_module_group` refuses by name.
Adding a `super::` equality there would guard nothing and would forbid `super::` reaching the rest of
`crate::commands`, which is not this issue's business.

**Two equalities under two anchors rather than one joined `web::broadcast` string, and this was
reviewed and kept.** `children_under` already walks brace groups correctly under each anchor, so
`use crate::web::{broadcast::A, commands::B}` fails on the `web::` anchor and
`use crate::{web::A, session::B}` fails on the `crate::` one. A joined path would have needed a
second matcher recursing through brace groups, with its own probes. The two forms were compared for
gaps during recertification and none was found that the joined form would close and the paired form
would not: the sibling hole above is invisible to both, which is why it took a third anchor and not a
different table shape.

**The third test** closes the duplication hole criterion 8 names: a "move" that left a copy of
`broadcast_all` behind would satisfy every arc assertion and still be wrong, because the two copies
would drift and the layering claim would be false. It reads every file under `src/`, filtered by
nothing, and asserts the list of files defining the emitter **equals** `["src/web/event_broadcast.rs"]`.
No entry fails as loudly as two.

#### 9.3.4 Proving the guard is alive: 37 probes, all measured

Every row below was measured before this plan was certified, by compiling Section 5.5 verbatim and
running it against a **copy** of `src-tauri/src/` carrying this change plus the injected spelling.
**This is not a prediction.** `dev-rust` re-runs the liveness procedure on the real tree at step 11
of Section 8; `dev-rust-grinch` re-attacks with spellings this table does not contain.

**Read the third column before running any of these.** The probes were measured as a **text scan
over a copied tree, where nothing is compiled**. On the real tree, `cargo test` compiles the library
first, and four of these spellings do not compile. That does not invalidate them as scan probes: it
means the observable result of running them through `cargo test` is a build error, not a guard
verdict. **If you run one of the "scan only" rows and see a compilation failure, that is the row
behaving as documented, not the guard broken.** This column is the correction that keeps the next
person from reporting a working guard as broken.

| # | Injected spelling | Measured result | On the real tree |
|---|---|---|---|
| P0 | the tree as this plan leaves it | **green**, observed exactly the two allowed pairs | compiles |
| P1 | `crate::web::commands::broadcast_all(…)` restored | red, #1265 message, names `src/commands/project_settings.rs` | **scan only**: `error[E0603]`, the path is private after the move (Section 7.2). This is step 11a |
| P2 | `use crate::web::{broadcast::WsBroadcaster, commands::broadcast_all};` | red, #1265 message | **scan only**: same `E0603` |
| P3 | the same grouped import reflowed over four lines with spaces around `::` | red, #1265 message | **scan only**: same `E0603` |
| P4 | `use crate::web::r#commands::broadcast_all;` | red, **#1265 message**, not the generic one | **scan only**: same `E0603` |
| P5 | `super::super::web::commands::broadcast_all(…)` | red, #1265 message | **scan only**: same `E0603` |
| P6 | `crate::web /* detour */ ::commands::broadcast_all(…)` | red, #1265 message | **scan only**: same `E0603` |
| P7 | `use crate::web as w;` | red, rename message | compiles (unused-import warning only) |
| P8 | `use crate::web::*;` | red, membership | compiles |
| P9 | `use crate::web::event_broadcast::broadcast_all;` **deleted** | red, membership (the shrinking set) | **scan only**: the nine surviving calls no longer resolve |
| P10 | `mod project_settings;` renamed in `commands/mod.rs` | red, resolver abort naming the missing declaration | **scan only**: breaks the whole crate |
| P11 | `#[path = "elsewhere.rs"] pub mod project_settings;` with the forbidden reference at the destination | red, #1265 message, names `src/commands/elsewhere.rs` | compiles if the file is actually moved |
| P12 | the reference hidden in a declared submodule `src/commands/project_settings/extra.rs` | red, #1265 message, names that file | compiles |
| P13 | the forbidden path written **inside a comment** | **green**, correctly: a comment is not a dependency | compiles |
| P14 | a second `pub fn broadcast_all()` in `src/decoy.rs` | red, emitter-once, lists both files | compiles |
| P15 | the definition renamed to `emit_everywhere` | red, emitter-once, observed `[]` | **scan only**: breaks the nine call sites |

**The rows above were measured against the 785 line guard.** Every one of them was re-measured
against the 1317 line guard of Section 5.5 and none changed verdict. The rows below are new and were
measured by `dev-rust` on that version, in a laboratory crate carrying a fixture tree in the
post-change shape. **Each one exists because a fix needs a probe: a guard change believed rather
than measured is the same mistake as a green believed rather than measured.**

| # | Injected spelling | Measured result | Closes |
|---|---|---|---|
| P16 | a `.md` under `src/` with a stray `"`, **not** in the module tree | **green**, correctly: prose cannot define anything, and the run also proves `crate_sources()` walked the real 188 file tree to decide it | N4 |
| P17 | `use crate::commands::project_settings;` in `src/web/event_broadcast.rs` | red, **emitter-home crate equality**, names `src/web/event_broadcast.rs` | **B2** |
| P18 | `use crate::web::commands as _d;` in `src/web/event_broadcast.rs` | red, **emitter-home web equality**, names `src/web/event_broadcast.rs` | **B2** |
| P19 | `#[path = "extra.rs"] mod extra;` with **both** `src/commands/extra.rs` and `src/commands/project_settings/extra.rs` present | red, **two-candidate refusal**, names the declaring file and both candidates | **B3** |
| P20 | the same `#[path]` with only `src/commands/extra.rs` present, carrying the forbidden reference | red, #1265 message, **names `src/commands/extra.rs`** (the file rustc compiles; the old order read the decoy and passed) | **B3** |
| P21 | `pub mod outer { pub mod inner; }` in the guarded module, with a benign decoy at the path the old resolver looked in | red, **nested-mod refusal**, names `src/commands/project_settings.rs` | **B4** |
| P22 | `#[cfg_attr(windows, path = "win_extra.rs")] mod extra;` with the forbidden reference at the destination | red, #1265 message, names `src/commands/win_extra.rs` | **B5** |
| P23 | two `mod platform;` under opposite `cfg`s, `unix_impl.rs` benign and `win_impl.rs` forbidden | red, #1265 message, **names `src/commands/win_impl.rs`**: both arms are scanned | **B5** |
| P24 | `use crate::web<U+200E>::commands as _d;` | red, #1265 message, names the file | **N1** |
| P25 | a generic copy `pub fn broadcast_all<R>(_r: &R) {}` in `src/decoy.rs` | red, **emitter-once**, lists `src/web/event_broadcast.rs` and `src/decoy.rs` | **N3** |
| P26 | `use crate::web::{self as _w};` | red, **rename message**, not the generic membership one | **N5** |
| P27 | a `.rs` file the resolver reaches whose string is never closed | red, **"FILES THE COMPILER READS THAT COULD NOT BE DELIMITED"**, names `src/broken.rs` | N4 |
| P28 | the tree as this plan leaves it, over a **copy of the real `src-tauri/src/`** (188 files) | **green**, all three tests, 1.40 s | the resolver rewrite as a whole |

P28 is the one that matters most for the rewrite: B3, B4 and B5 all make the resolver stricter, and
a stricter resolver that refuses the real tree would be worse than the defect it fixes. It does not:
every declaration in the crate resolves, no file is nested in an inline block, and no `path` value
has two candidates.

**The rows below were added at recertification.** The architect rebuilt the laboratory independently:
a fresh copy of the real `src-tauri/src/` (188 files), the change applied from Sections 5.1 to 5.4 as
written, and the guard extracted verbatim out of Section 5.5 of this document rather than taken from
`dev-rust`. P29 is the one that found something: two anchors were not enough, and the third exists
because of it.

| # | Injected spelling | Measured result | Closes |
|---|---|---|---|
| P29 | `use super::commands as _d;` in `src/web/event_broadcast.rs` | **green under two anchors** with the cycle live; **red under three**, emitter `super::` equality, names the file | the sibling hole |
| P30 | `use super::*;` at module level in the emitter | red, emitter `super::` equality, observed child `*` | the glob that motivates Section 5.1's named imports |
| P31 | `use super::broadcast::WsBroadcaster;` replacing the `crate::` form in the emitter | red, emitter `super::` equality | makes Section 5.1's `crate::`-anchored rule executable instead of advisory |
| P32 | `super::commands::broadcast_all_r::<tauri::Wry>` as an expression path in the emitter | red, emitter `super::` equality | the sibling hole, in expression position |
| P33 | the emitter's test module loses `use super::broadcast_all;` | red, emitter `super::` equality, observed `[]` | the shrinking set, third anchor |
| P34 | `use super::super::commands::project_settings as _d;` in the emitter | red, emitter `super::` equality, observed children `commands` and `super` | reaching `crate::commands` without spelling `crate::` |
| P35 | the guarded module's fully qualified call restored, with the third anchor present | red, #1265 message, names `src/commands/project_settings.rs` | proves the new anchor did not disturb the first test |
| P36 | the emitter loses its production `use crate::web::broadcast::WsBroadcaster;` | **green**, and correctly so only because the deletion does not compile: the test module's own import of the same child holds both equalities up | nothing; declared as entry 12 of `KNOWN UNCOVERED` |
| P37 | the tree as this plan leaves it, over the architect's independent copy of the real `src-tauri/src/` | **green**, all three tests, 1.42 s, with `rustfmt --check` 0 and `clippy -D warnings` clean on all four touched files | the recertification as a whole |

P36 is reported as a probe that found a limit rather than a fix. It is the `#[cfg(test)]` asymmetry
`loops_layering.rs` already records for #1252: whole files are read while the detector ignores test
regions, which is stricter everywhere except here, where a test import can hold an equality up after
the production one is gone. It is not exploitable as it stands, because `broadcast_all`'s signature
needs the type, so the deletion fails `cargo check` first. It is written down because the
shrinking-set argument is the thing somebody will be trusting on the day it stops being true.

**P29 is the reason this section says 37 and not 28.** Two anchors were measured green with
`commands::project_settings -> web::event_broadcast -> web::commands -> commands::project_settings`
live in the code, which is the same class of failure grinch found in B2 and one layer further in.
Sections 4.3 and 9.3.3 carry the argument; this row carries the measurement.

**P0 to P28 were all re-run against the 1403 line guard and none changed verdict**, so the third
anchor is additive: the 36 rows that were red or green before are red or green now, for the same
reason and with the same message.

To run a "scan only" row as a scan rather than as a build, copy `src-tauri/` elsewhere, inject the
spelling into the copy and point a laboratory crate at it with `autobins = false` and a `[lib] path`
pointing away from the copied tree. That is how P16 to P28 were produced. It is not required by any
step of Section 8.

**The liveness procedure for step 11 of Section 8 is two probes, not one, and Section 8 states them
in full.** In short: 11a is P1 and its real-tree result is `error[E0603]`, which proves the
visibility backstop and proves nothing about the guard, because the guard's binary never ran. 11b
injects a forbidden reference that compiles, `crate::web::commands::broadcast_all_r(&app,
PROJECT_GROUPS_UPDATED_EVENT, &payload)`, and that is the one that must produce the guard's red
naming `src/commands/project_settings.rs`.

**Why this correction exists.** The original procedure named P1 alone. P1 cannot compile after the
move, by this plan's own Section 7.2, so `cargo test` would abort in the library and the guard's
binary would never link. The run would fail, which looks like the probe worked, while the guard had
in fact never been exercised. A guard whose red has not been seen on the tree it is protecting is an
assumption, not a check, and a probe that cannot distinguish "the guard fired" from "the crate did
not build" does not remove that assumption.

#### 9.3.5 What the guard does not cover, and how that list is maintained

**This is the part that is expected to grow, and growing it is not a plan change.**

The module header of Section 5.5 carries a numbered `KNOWN UNCOVERED SPELLINGS` list, now twelve
entries ending with `13. (append here: …)`: re-export laundering through a third module,
macro-generated paths, `include!`, runtime indirection, `concat!`-built paths, a `mod x;` nested
inside an inline `mod y { … }`, NTFS alternate data streams, laundering through the **parent**
module, module-group aliasing beyond the spellings the matcher knows, `cfg`-arm attribution,
`broadcast_all_r` moving, and a `#[cfg(test)]` reference holding an equality up on its own.

**Entry 12 was added at recertification and entry 8 was narrowed.** Entry 12 is probe P36: the
emitter's test-module import of `crate::web::broadcast` satisfies both of that module's first two
equalities on its own, so deleting the production import does not shrink the observed set. It is not
exploitable, because the deletion does not compile, and it is recorded because the shrinking-set
argument is load bearing everywhere else in this guard. Entry 8 previously said parent laundering was
invisible to the guard; that is now true of `commands::project_settings` only. The emitter module is
read under `super::` as well, so the same laundering from `src/web/event_broadcast.rs` is refused.

**Four entries were added and one rewritten as a result of review, and two candidate entries were
closed in the matcher instead.** Entry 8 (parent laundering) and the rest are declarations; U+200E
and U+200F between `web` and `::`, and the generic spelling of the emitter definition, were cheap
enough to close, so they became probes P24 and P25 rather than list entries. **Entry 6 was rewritten
because it was false**: it promised that a `mod` nested in an inline block "cannot pass silently",
and it was measured passing silently whenever a file happened to exist at the path the old resolver
looked in. It now describes what the resolver does, which is refuse the file.

**Two entries state a limit rather than a spelling, and both are deliberate.** Entry 11 records that
nothing enforces Section 3.2's closed decision to leave `broadcast_all_r` in `web::commands`: after
this change there are two dual-transport emitters in two modules, the duplication test watches the
name `broadcast_all`, and the day somebody moves the sibling "for symmetry" no test goes red. Entry
10 records that this resolver reads both arms of a `cfg` split and cannot say which one the compiler
took.

**And a standing note now sits above the list**: several entries say "the detector still catches
it", which is true and measured, but the entire reason this file exists is that the detector is run
by hand and is not wired to CI. An entry the detector catches is still uncovered here.

**Review is expected to find more, and the ones it finds are declared, not hidden.** When a reviewer
demonstrates a spelling that reaches the dispatcher and still passes, `dev-rust` appends one entry to
that list and nothing else. **Appending an entry is part of the review loop for #1265. It does not
require this plan to be reopened and does not invalidate its digest.** Widening the matcher to cover a
newly found spelling is the same: if it is a spelling, it belongs in the matcher or in the list.

**That last sentence only holds because the file, not this plan, is the canonical copy.**
`src-tauri/tests/project_settings_layering.rs` is what runs; Section 5.5 is a verbatim snapshot taken
when the plan was certified. The first appended entry makes the two diverge, and that divergence is
the intended behaviour, not drift to be reconciled. **Append to the file. Do not edit Section 5.5 to
match, and do not read a difference between them as a defect.** The guard's own module header carries
the same statement, so a reader who never opens this plan is told as well.

The one thing that does require reopening the plan is a finding that the guard cannot be a text scan
at all. If that turns up, say so rather than building something more elaborate that still cannot carry
it.

#### 9.3.6 Two costs of reading everything, both accepted deliberately

**Unreadable prose is not a failure; unreadable source is.** The third test reads every file under
`src/` with no extension filter, which is correct and is not to be changed (`every_file_under` says
why). The consequence is that it hands non-Rust files to `scrub`, and `scrub` cannot delimit
arbitrary text: a Markdown file with an odd number of `"`, an unmatched `/*`, or non-text bytes
produces an `Err`. As first written, that `Err` became an anonymous `panic!` from inside the scanner,
so an unrelated edit to a README would have turned the layering guard red with a message that never
mentions #1265.

Measured on the real tree: `src-tauri/src/` holds 188 files, of which **three are not `.rs`** -
`src/api/README.md` (88 double quotes), `src/config/root_agent_defaults/agency-agents-roles/SKILL.md`
(8) and `src/config/root_agent_defaults/role-skill-boundary-audit/SKILL.md` (4). All three have even
parity today and none contains `/*`, so all three scrub cleanly. **That is a coincidence, not an
invariant**, and the next edit to any of them is a coin flip.

**The split is by what the compiler reads, not by file extension.** A file this scan cannot delimit
is fatal only when the module tree reaches it, because that is exactly when it is source rustc
compiles and this scan failed to read it, which is where a second definition would survive. A file
under `src/` that the module tree does not reach cannot define anything, so it is not a failure. The
module tree walk that answers this, `crate_sources`, runs **only** when something was undelimitable,
so the normal path never pays for it.

Both directions were measured on the 1317 line guard, and this is probes P16 and P27:

- A `.md` with a stray `"`, not in the module tree, over a **copy of the real 188 file tree**:
  green, and reaching green required `crate_sources` to walk that whole real tree successfully.
- A `.rs` file the resolver reaches whose string is never closed: red, naming it under
  `FILES THE COMPILER READS THAT COULD NOT BE DELIMITED`.

**The duplication with `tests/loops_layering.rs` is accepted, and this is a closed decision.**
`loops_layering.rs` is 569 lines and already carries `normalized`, `leading_segment`,
`split_top_level`, `aliases_the_command_group`, `relative_of` and the same
`ANCHOR`/`ALLOWED_*`/`FORBIDDEN_*` shape. This guard is 1317 lines that reimplement all of it and add
the module-tree resolver, the emitter-home assertions and the resolver's refusals. That is roughly
1900 lines of near-duplicate scanner across two integration test crates, and it is real.

It is accepted rather than fixed because integration tests are separate crates: sharing would need a
`tests/common/` module or an auxiliary crate, which is more scope than #1265 asked for and would put
a refactor of an existing guard inside a structural change that is supposed to move one function.
**Do not reopen this during implementation or review.** When a third guard of this shape appears the
conversation becomes unavoidable, and it gets its own issue then, not this one.

#### 9.3.7 What the resolver refuses, and why refusing is the answer

Three of the five blocking findings were the same defect wearing different clothes: the resolver
believed it was reading the files rustc compiles, and in each case it read a different file and
reported green. The header of Section 5.5 now says plainly that it is not rustc. Where it can
over-read safely it does, and where it cannot it refuses:

| Situation | Old behaviour | New behaviour |
|---|---|---|
| `#[path = "x.rs"]` with a file at both `<file-dir>/x.rs` and `<module-dir>/x.rs` | tried the module directory first and took it; rustc takes the file directory, so it read the decoy | hard failure naming the declaring file and both candidates |
| `#[path = "x.rs"]` with only the rustc candidate present | resolved through the second candidate by luck | resolves through the first candidate by rule |
| `#[cfg_attr(<cond>, path = "x.rs")]` | the literal `#[path` was not found, so it fell back to the default candidates while rustc compiled the `cfg_attr` file | the `path` key is matched, so both forms resolve |
| `mod x;` declared twice under opposite `cfg`s | took the first declaration, so a Windows build scanned the Unix file | every declaration is collected and every resolved file is scanned |
| `mod x;` inside an inline `mod y { ... }` block | resolved to the wrong file whenever one existed there, silently | the whole file is refused, with a message naming it |
| `x.rs` and `x/mod.rs` both present | took `x.rs` | hard failure; rustc rejects that tree outright |

**Over-reading is the safe direction and refusing is the honest one.** `cfg` is not evaluated here,
so both arms of a platform module are read even though only one compiles: a forbidden reference in
either arm is caught, at the cost of a false red if somebody puts one in the arm this platform does
not build. That trade is the file's existing doctrine, that a false red is argued about and a false
green is believed. Where over-reading is not available, because two candidate files exist and only
one is compiled, guessing is what produced the defect in the first place, so the scan says it cannot
tell instead.

### 9.4 No behavioural test is added

`broadcast_all` already has a behavioural test, `broadcast_all_sends_to_explicit_websocket_broadcaster`,
and it moves with the function to `src/web/event_broadcast.rs` (Section 5.1). It is byte identical
apart from its imports. The behavioural guarantee here is that the moved code is unchanged and its
call sites are unchanged, which criterion 8 and the moved test together establish.

### 9.5 Acceptance criteria

Every number below was produced by re-running Tarjan over the committed record with this exact change
applied to the arc set. Verify with:

```
node "<VAULT>/Levelization/02-levelize.mjs" rank "<GRAPH>"
```

reading `coverage.graphShape` and the `modules[]` entries.

**Criterion 1 is adapted and the standard one does not apply here.** This SCC does not disappear, it
thins. Reading the surviving cycle as a failure is the mistake this criterion exists to prevent.

| # | Criterion | Before | After, required |
|---|---|---|---|
| 1 | `coverage.graphShape.cyclicSccs` | 1 | **1**. It does **not** drop to 0 |
| 2 | Knot size, and its membership | 89 | **88**, membership identical to the previous set **minus exactly** `agentscommander_lib::commands::project_settings`, compared set to set. An equal count is not an equal set |
| 3 | `sccSize(agentscommander_lib::commands::project_settings)` | 89 | **1** |
| 4a | level of `agentscommander_lib::commands::project_settings` | 2 | **2** |
| 4b | level of `agentscommander_lib::web::commands` | 2 | **3** |
| 4c | `agentscommander_lib::web::event_broadcast` | absent | **level 1, `sccSize` 1** |
| 5 | Arc record diff | | **exactly the 1 removal and 3 additions of Section 7.1, and nothing else** |
| 6 | Arc record regenerated and committed | | `git status` empty on the final tree **and** `git show --stat` lists `plans/1265-extract-project-settings-from-scc.md`. Both, see below |
| 7 | Suites of Section 9.1 | green | **green**, including all three tests of Section 5.5, against the measured baseline of Section 9.1 |
| 8a | `rg -n "fn broadcast_all\s*[(<]" src` from `src-tauri` | one line, in `src/web/commands.rs` | **one line, in `src/web/event_broadcast.rs`**, and no other |
| 8b | `rg -n "web::commands" src/commands/project_settings.rs` | one line (44) | **no output**, `rg` exits 1 |
| 8c | the nine surviving call sites of `broadcast_all(` in `src/web/commands.rs`, **enumerated** | lines 267, 285, 395, 403, 484, 503, 536, 622, 768, plus the definition at 852 and its test at 1179 | **exactly lines 267, 285, 395, 403, 484, 503, 536, 622 and 768**, listed and compared line by line, with 852 and 1179 gone |
| 8d | the guard's `the_dual_transport_emitter_is_defined_exactly_once` | does not exist | **green** |
| 8e | the guard's `the_emitter_home_names_nothing_but_the_websocket_fan_out` | does not exist | **green** |

**Criterion 4 is satisfied by 4a and 4b together**, and both levels are stated rather than merely
asserted to differ: `commands::project_settings` at 2 and `web::commands` at 3 are different levels,
and the command ends up **below** the dispatcher, which is the direction Section 4.2 chose the cut for.

**Criterion 6 needs both halves, and the `git status` half alone is the accident it exists to
prevent.** `plans/` is ignored by `.gitignore` line 11, so `git status` comes back empty whether the
plan was committed or not. That is precisely how `7778f67b` left the #1252 plan out in silence. So
`git status` empty carries the arc record and nothing else, and `git show --stat` naming the plan
file carries the plan. Report them as two observations.

**Criterion 8 is the five conditions 8a to 8e together.** 8a proves there is exactly one definition
and where it is, and its pattern allows `(` or `<` after the name because a generic copy
`fn broadcast_all<R: Runtime>(…)` is the shape a copy would most naturally take, being the shape of
the surviving sibling `broadcast_all_r`. 8b proves the origin no longer names the dispatcher at all,
not merely that the arc vanished. **8c is an enumeration and not a count**, which it used to be: a
count of 9 is reached just as well by losing one call site and gaining another, and every other
criterion in this plan is written over membership for exactly that reason. 8d and 8e are the same
properties asserted inside the suite, so they keep holding after this plan is closed.

**Which criteria discriminate, and which cannot be faked.** Say this in the report rather than
listing eight greens as if they weighed the same:

- **Criterion 1 is satisfied without doing anything.** `cyclicSccs` is 1 before and 1 after. It
  belongs here as the warning that it must **not** drop to 0, and it is **not evidence that the work
  was done**.
- **Criterion 4a is the same.** The level of `commands::project_settings` is 2 before and 2 after.
- **Criterion 8b is satisfiable cosmetically**, by the grouped-import spelling of probe P2, which
  contains no `web::commands` text while leaving the arc intact. It is only sound because the guard
  backs it, so do not report it as independent evidence.
- **The four that discriminate and cannot be faked are 2, 3, 4b and 5**: knot membership set to set,
  `sccSize(target)` 89 to 1, `web::commands` moving from level 2 to 3, and the arc diff being exactly
  one removal and three additions. `dev-rust-grinch` reproduced all four independently and they came
  out exact.

**Informative, not gates.** These will move and are recorded so nobody has to wonder whether they
should have: modules with at least one arc 173 to **174**, unique arcs 974 to **976**, total SCCs 85
to **87**. `sccId` values are indices and may renumber, which is why criterion 2 is written over
membership and not over ids. Line counts: `src/commands/project_settings.rs` 101 to **102**,
`src/web/commands.rs` 1471 to **1441**, `src/web/event_broadcast.rs` **56** new,
`src-tauri/tests/project_settings_layering.rs` **1317** new.

**If criterion 2 fails, stop and report.** A knot that is not exactly the old set minus the target
means something outside this plan's scope was touched.

### 9.6 Report to the tech lead

State: the verdict; the direction chosen with its call-site counts; the placement with the
reachability proof **and the outgoing-arc premise it rests on**; the arc enumeration actually
observed in the diff, compared against Section 7.1; the observed value for every criterion, with 8a
to 8e reported separately and quoting the actual output of their commands; the detector's exit code
with the note that 1 is expected; and the `KNOWN UNCOVERED SPELLINGS` list as it stands at handover,
including any entry review caused to be appended. An empty round of review is worth stating too.

**Separate the criteria that discriminate from the ones that do not.** Section 9.5 says which are
which. Reporting eight greens as one block hides that criteria 1 and 4a hold before the change as
well.

**Do not report the guard as proof that the arc cannot return.** Report it as green, and report
`cyclicSccs`, the knot membership and `sccSize` separately as the checks that carry criteria 1, 2 and
3. Section 9.3.1 exists because those two have previously been reported as one thing.

**Report step 11a and step 11b separately.** 11a is a compilation failure and proves the visibility
backstop; 11b is the guard's red and is the only one of the two that proves the guard is alive.
Reporting them as one "the probe worked" is the defect Section 9.3.4 was rewritten to prevent.

**If a suite failure appears, identify it against the known-flaky list before calling it a
regression.** Section 9.1 carries the measured baseline and the issue numbers.

**Do not mention `I` or the instrument's `suggestion` anywhere in the report.** Section 3.2 says why.

---

## 10. Enrichment log: what changed after certification, and why

The architect certified this plan in one pass. `dev-rust` then verified it against the real tree and
`dev-rust-grinch` re-attacked it; the architect then recertified by rebuilding the laboratory rather
than reviewing their work. **Nothing here changes the direction of the cut, the placement, the four
arcs or the acceptance numbers**; every one of those was reproduced independently three times and
came out exact. What changed is two executable steps that did not work, and the guard, which reported
green in six measured ways while the dependency was live: five found by `dev-rust-grinch` (10.2) and
one found at recertification (10.5).

Sections 10.1 to 10.4 are the `dev-rust` and `dev-rust-grinch` round; Section 10.5 is
recertification. Each edit that broke the digest did so on purpose: the plan is only frozen once the
architect signs the version that runs.

### 10.1 Steps that did not work as written

| Finding | What was wrong | Now |
|---|---|---|
| **The liveness probe could not compile.** Found independently by `dev-rust`. | Section 9.3.4 named P1, `crate::web::commands::broadcast_all` restored, as the step 11 procedure. Section 7.2 of the same plan says that path stops compiling with `error[E0603]` after the move. `cargo test` would abort in the library, the guard's binary would never link, and the failing run would read as if the probe had worked while the guard had never been exercised. | Step 11 is 11a and 11b. 11a expects the `E0603` and is labelled the visibility backstop. 11b injects `crate::web::commands::broadcast_all_r(...)`, which is `pub` and compiles, and is the one that must produce the guard's red naming the file. Section 9.3.4 also marks which of the 16 original probes are scan-only on a real tree. |
| **Deleting the stated ranges breaks `cargo fmt --check`.** Found independently by `dev-rust` (E2) and `dev-rust-grinch` (B1), by different routes. | Section 5.3 said to delete 851-860 and 1167-1187. Lines 850, 861, 1166 and 1188 are all blank, so each deletion left two consecutive blank lines, and rustfmt collapses them. Measured: `rustfmt --check --edition 2021` exits 1. Step 8 would fail **with the wrong diagnosis**, since the plan reads a fmt diff as "the file was retyped rather than copied". | Delete 850-860 and 1166-1187. The line count is corrected from 1446 to **1441** in Sections 5.3 and 9.5. |

### 10.2 The guard: five ways it reported green with the dependency live

All five were measured by `dev-rust-grinch` against the 785 line version, in a standalone
`rustc` harness for the pure functions and against real `rustc` fixtures for the module tree. Every
fix below has a probe in Section 9.3.4, and every probe was measured by `dev-rust` against the 1317
line version.

1. **It guarded the wrong module.** Nothing scanned `src/web/event_broadcast.rs`, the module the
   whole non-absorption argument of Section 4.3 rests on. One `use` from there into any knot member
   takes the knot from 88 to 90 and puts the target back inside it, leaving the crate worse than
   before #1265, with every existing assertion green. The guard was watching the side the compiler
   already watches with `E0603`. **Fixed** by a third test asserting, by equality under two anchors,
   that the emitter module names `web` under `crate::` and `broadcast` under `web::` and nothing
   else. Probes P17, P18. Section 4.3 now states the premise in the word that matters, which is
   *outgoing*.
2. **The `#[path]` candidate order was inverted.** The resolver tried the module subdirectory before
   the file's own directory; rustc uses the second. With a file at both, it read the benign one and
   passed while the forbidden reference lived in the file rustc compiles. **Fixed**: the file's
   directory first, and two existing candidates is a hard failure rather than a choice. Probes P19,
   P20.
3. **`KNOWN UNCOVERED` item 6 was false.** It promised that a `mod x;` nested in an inline
   `mod y { ... }` block "cannot pass silently"; it was measured passing silently whenever a file
   existed at the path the resolver looked in. **Fixed** by refusing the whole file with a hard
   failure, and the entry was rewritten to describe what happens instead of promising what does not.
   Probe P21.
4. **`#[cfg_attr(..., path = ...)]` was invisible.** The resolver matched the literal text `#[path`,
   so it fell back to the default candidates while rustc compiled the file the `cfg_attr` names.
   **Fixed** by matching the `path` key. Probe P22.
5. **Only the first `mod x;` of a name was followed.** The standard per-platform module is two
   declarations under opposite `cfg`s, so on Windows the guard scanned the Unix file while reporting
   that it reads what the compiler compiles. This one needs no malice at all. **Fixed** by collecting
   every declaration and scanning every file that resolves; `cfg` is not evaluated, so both arms are
   read. Probe P23.

### 10.3 Smaller guard changes, and the two that were closed instead of declared

- **U+200E and U+200F** are lexical whitespace to rustc and are not `char::is_whitespace`, so
  `web<U+200E>::commands` compiled without a warning and the anchor never matched. Closed in
  `normalized`. Probe P24.
- **The emitter definition matcher** looked for the literal `fn broadcast_all(`, which misses
  `fn broadcast_all (` and misses a **generic** copy, the exact shape of the sibling
  `broadcast_all_r`. Closed by matching the name and requiring `(` or `<` after it; criterion 8a's
  pattern was widened to match. Probe P25.
- **`use crate::web::{self as w};`** renames the whole group and used to fail with the generic
  membership message instead of the rename one. Added to `aliases_a_module_group`, along with
  `use crate as c;`, which the new `crate::` anchor made relevant. Probe P26.
- **Prose under `src/` could turn the guard red.** A stray `"` in any `.md` made `scrub` fail and the
  emitter test panic with a message that never mentions #1265. Section 9.3.6 has the resolution: a
  file this scan cannot delimit is fatal only when the module tree reaches it. Probes P16 and P27.
- **Criterion 8c was a raw count** in a plan that argues everywhere that an equal count is not an
  equal set. It is now an enumeration of the nine call-site lines.
- **Criterion 6 did not prove what it said.** `git status` empty is satisfied whether or not the plan
  was committed, because `plans/` is ignored, which is the `7778f67b` accident itself. It now needs
  `git show --stat` as well.
- **Step 14 had no abort criterion.** It now reverts `src-tauri/module-arcs.txt` before reporting.
- **Criteria 1 and 4a hold before the change**, and 8b is satisfiable cosmetically. Section 9.5 now
  says so and names 2, 3, 4b and 5 as the four that discriminate.

### 10.4 What was checked and found correct

Worth recording, because it bounds everything above. Both reviewers reproduced the structural work
independently and it came out exact: 974 arcs, 173 modules, 85 SCCs, `cyclicSccs` 1, knot 89 before;
976, 174, 87, 1, knot 88 after, with `leaving = [commands::project_settings]` and `entering = []`.
`web::broadcast` has zero outgoing arcs and ten incoming. `broadcast_all` occupies exactly 851-860.
The 11 occurrences of `broadcast_all(` are nine calls plus the definition plus its test. There is no
consumer outside `src-tauri/src`, measured across `src/`, `tests/` and `scripts/`. The record's
sorted insertion positions, 386, 975 and 976, were verified against the file. Two premises the arc
enumeration depends on were verified empirically rather than assumed: `mod` declarations create no
arc (`web -> web::embedded` is absent despite `mod embedded;` and a use of it), and `#[cfg(test)]`
references create none (`web::commands -> pty::git_watcher` is absent despite the import at line
945). `git show --stat 7778f67b` confirms the #1252 plan was left out of its own commit.

### 10.5 Recertification: what the architect verified, and the one thing it added

**Method: rebuilt rather than reviewed.** A fresh laboratory crate, a fresh copy of the real
`src-tauri/src/` (188 files), the change applied from Sections 5.1 to 5.4 exactly as written, and the
guard **extracted verbatim out of Section 5.5 of this document** rather than taken from `dev-rust`'s
scratchpad. Everything below is a measurement on that tree.

**Confirmed closed, independently:**

- **B1.** Lines 850, 861, 1166 and 1188 of `src/web/commands.rs` are all blank, as reported. The
  corrected ranges produce **1441 lines** exactly, and `rustfmt --check --edition 2021` exits 0 on
  all four touched files. The old ranges would have left two consecutive blank lines at both sites.
- **B3, B4, B5.** Re-measured from scratch with probes written here, not `dev-rust`'s: two `#[path]`
  candidates both present is refused naming both; only the rustc candidate present resolves to it and
  the forbidden reference is found there; a `mod` nested in an inline block is refused; both `cfg`
  arms of a duplicated `mod platform;` are scanned, so the reference is found whichever arm carries
  it; `cfg_attr(..., path = ...)` resolves. Nine probes, nine expected verdicts.
- **The measurement the tech lead asked to have revalidated: the stricter resolver does not refuse
  the real tree.** `cargo clippy --all-targets -- -D warnings` clean, `rustfmt --check` 0, and all
  three tests green in 1.42 s against the 188 file copy. Confirmed.
- **The two-anchor decision for B2 is accepted as implemented.** The paired form and the joined
  `web::broadcast` form were compared for gaps and none was found that the joined form would close
  and the paired form would not. `children_under` already walks brace groups under each anchor, so
  both `use crate::web::{broadcast::A, commands::B}` and `use crate::{web::A, session::B}` fail. A
  joined path would have needed a second matcher with its own probes, for no coverage.

**What recertification found that both earlier rounds missed, and closed: the sibling spelling.**

B2 fixed the module the guard watches. It did not fix the **directions** it watches from. Measured on
the 1317 line guard: `use super::commands as _d;` in `src/web/event_broadcast.rs` passes **green**
while `commands::project_settings -> web::event_broadcast -> web::commands ->
commands::project_settings` is live in the code. The dispatcher is the emitter's sibling, so from
inside `src/web/` it is reachable with neither a `crate::` nor a `web::` token, and both of B2's
anchors are blind to it. This is not an adversarial curiosity: `src/web/commands.rs:12` writes
`use super::broadcast::WsBroadcaster;`, so `super::` is the idiom of that directory.

**Closed with a third anchor**, `super::`, on the emitter module only, allowing exactly one pair, the
test module reaching its own parent for `broadcast_all`. A glob fails there deliberately, which is
why Section 5.1 now imports by name instead of writing `use super::*;`. `commands::project_settings`
gets no such anchor and the asymmetry is argued in Section 9.3.3 rather than left as an oversight.
Guard 1317 to **1403** lines. Probes **P29 to P37**. All 28 earlier probes were re-run and none
changed verdict.

**One limit found and declared rather than fixed:** entry 12 of `KNOWN UNCOVERED`, probe P36. The
emitter's test-module import holds its first two equalities up on its own, so deleting the production
import does not shrink the observed set. Not exploitable, because that deletion does not compile.

**Corrected in this round:** Section 5.1's test module imports by name; Section 4.3 states the
three-anchor contract and why the third exists; Section 9.3.3 carries the sibling argument and the
paired-versus-joined comparison; Section 9.3.4 grows to 37 probes; Section 9.3.5 carries entry 12 and
narrows entry 8 to the guarded module. **The direction of the cut, the placement, the four arcs and
every acceptance number are untouched, for the third time.**
