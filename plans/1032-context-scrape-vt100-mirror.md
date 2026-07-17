# Plan Contract: #1032 - context % scrape off the vt100 session mirror (backend engine)

**Status: READY_FOR_IMPLEMENTATION**
**Issue:** #1032, child of epic #1031
**Branch:** `feature/1032-context-scrape-vt100-mirror`, cut from `main` = `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe`
**Revision:** 4, certified at round 3 of 3. Integrates `dev-rust` round 3 (verdict: implement cold, no open decision) and `dev-rust-grinch` round 3 (H1/H2/H3, acceptance condition met by §6.4.1).

> **Retracted-in-place** is kept: both reviewers said it let them check the delta rather than re-derive it. Revision 4 retracts: §6.4's `Unqueryable` disposition (**H1**), §6.4's map-growth claim (**H3**), §6.1.1's guard corollary, §2.13's count, and §6.4's lock-order citation.
>
> **Every mechanism in this plan has now been run against the real thing by someone other than its author.** That is the only thing that has ever caught a defect here (§10.4), and it is why revision 4 is certifiable where revisions 1-3 were not.

---

## 1. Issue and objective

Produce a per-session, best-effort reading of a coding agent's context-window usage (`0..=100`, or unavailable), by running a **per-agent, user-configured regex** over the plain de-ANSI'd rows of the `vt100` screen mirror AC already maintains for every session.

Deliver **only the backend engine**. The badge and every TypeScript surface are #1033. The move of the config to `.ac/project-settings.json` is #1034.

**The percentage is a signal for a human. It never drives an action.**

---

## 2. Evidence and current-state gap

### 2.1 The issue's coordinates are 130 commits stale (holds)

Evidence copied from `dev-rust`'s memo, written at **`f123b03`**, 130 commits behind the `4acadfe` the issue claims. `git show f123b03:src-tauri/src/pty/output.rs` puts `get_screen_snapshot` at `216`, exactly what the issue cites; at `4acadfe` it is at `274`. Claims survive; coordinates do not. Both reviewers re-checked the corrected map across three rounds and it lands.

#### 2.1.1 Corrections to revision 1's own coordinates (all fixed, re-verified)

`Cargo.toml:28` not `:29`; insert the settings field **before `:72`** (`:72` is the `#[serde]` attribute, `:73` is `pub backend:` - inserting at `:73` splits them and does not compile); `instructions_filename` is attribute `:63`, field `:64`; `pty/mod.rs:8` (alphabetical); mock impl `:3882`; the scroll clear is `grid.rs:549` → `:47` → `row.rs:13`, not `grid.rs:534`/`:570`.

### 2.2 "12 impls" is a grep count, not an impl count (holds)

12 `fn get_screen_snapshot` lines; **8** are `PtyBackend` impls. See §2.13 for why this section aged badly.

### 2.3 What already exists (verified)

Key entries: `screen_parsers` `output.rs:28`; `register_session` `:109-116` (`Parser::new(rows, cols, 0)`, no scrollback); fed `:157-167`; `remove_session` `:226-236`; `get_screen_snapshot` `:274-285`; `get_pty_size` `:287-291`; `git_watcher.rs` sampler precedent (`:12`, `:64-84`, `:132-154`); `create_session_inner` `session.rs:945`, spawn `:1640`, Err arm `:1642-1647`; `SettingsState` `settings.rs:2184`, built `lib.rs:526`, managed `lib.rs:604`; `vt100 = "0.15"` at `Cargo.toml:28`.

**Load-bearing for §6.4:**

| Piece | Location |
|---|---|
| Per-session child-liveness oracle | `LocalProcessBackend::probe_child`, `local_backend.rs:981-993` |
| Its Windows implementation | `probe_child_liveness`, `local_backend.rs:1016-1045` |
| `ChildLiveness { Alive, Exited{code,success}, Unqueryable(String), Gone }` | `spawn_diagnostics.rs:257-266` |
| A per-session poller that already calls it | `spawn_diagnostics::watch_child`, `:1009-1049`, spawned at `local_backend.rs:906-908` |
| Container teardown on natural exit | `handle_bridge_exit` `:380` → `close_transport` `:435-448` → `remove_session_state` `:436`/`:466-472` (**drops the parser at `:469`**) + `mark_exited` `:445` |
| **The test seam for a real ConPTY + real `PtyInstance`** | `startup_gate_tests`, `local_backend.rs:1605-1641` (`openpty` `:1620-1627`, instance `:1631-1639`, `child: None` `:1634`) |
| Free-function-over-fields precedent | `resize_instance` `local_backend.rs:97`; `hand_over_held_size` `:200` |

### 2.4 `vt100::Screen::row_wrapped()` is real, and it is nowhere in production

The find holds: public at `screen.rs:604-611`, set only at `grid.rs:679`, cleared by `Grid::set_size` via the unconditional `row.resize` (`grid.rs:79` → `row.rs:73-76`).

**RETRACTED (rev 1): the design built on it.** The capture measured twelve widths: the statusline never hard-wraps. The join it enabled produced a wrong number (grinch's F1). Deleted in revision 2.

**RETRACTED (rev 2): the reason for keeping the flags exposed.** §2.4 kept them *"because the §9.2 tests pin the crate's behavior"* while §9.2's drop list removed the only test that read one, *"because no production path reads them (§2.4)"* - the two sections cited each other for opposite conclusions (G6). They were not free either: `row_wrapped(r)` is `visible_rows().nth(r)` over a `skip`/`take`/`chain` iterator (`grid.rs:119-125`, `:138-140`), so building the vector is **O(N²)** - ~465 iterator steps per sample at 30 rows, for a field with no reader and no test. Grinch reproduced the consequence (`clippy -D warnings`: `fields wrapped and cols are never read`, halting step 4) and notes that §2.4 found the **cause** its own finding was only the symptom of.

**Decided (rev 3): `wrapped`, `cols` and the `ScreenRows` struct are all gone.** `row_wrapped` appears nowhere in production; both reviewers confirmed.

**The whole arc is the plan's most useful record and is kept deliberately:** a real find, correctly read, verified against the crate, praised - and every line of code it motivated is now deleted, because nobody checked whether the hazard it addressed existed.

### 2.5 The two backends do not share a fanout (holds)

`SessionIoFanout::new` builds `screen_parsers` fresh per call (`output.rs:105`), called twice in production: `local_backend.rs:650`, `container_backend.rs:211`. Disjoint. The trait method is necessary.

### 2.6 RETRACTED (rev 1). `AppSettings` is in managed state, and **both** whole-settings writers write through

Revision 1's *"no `AppSettings` in Tauri managed state... a settings disk read per tick... the only affordable option"* was false in every clause. `SettingsState = Arc<tokio::sync::RwLock<AppSettings>>` (`settings.rs:8`, `:2184`), built `lib.rs:526`, managed `lib.rs:604`, read by `create_session_inner` itself at `session.rs:977`/`:1073`/`:1239`/`:1411`. **A grep for a type name is not a search for a managed state** - it is reached through the alias.

**Corrected in revision 3:**

- Revision 2 cited `persist_protected_settings_update_with_saver` (`config.rs:205-216`) as "the shape" of a function it was not. The real draft path is `save_settings_draft` (`:169`) → `persist_settings_draft_update` (`:248`) → `persist_settings_draft_update_with_saver` (`:255-280`): `settings.write().await` (`:260`), `let written = save(&draft)?` (`:277`), `*s = written.clone()` (`:278`).
- **Both whole-settings writers write through**, so the plan predicts nothing about #1033's choice: `update_settings` → `persist_protected_settings_update_with_saver` (`config.rs:206-217`) does `*s = written.clone()` at `:211-215`. **`agents` is not in the protected-restore list** (`:262-274`), so `agents[].context_regex` survives to both `save` and `*s`. The agent-config surface uses `SettingsAPI.update` **today** (`CodingAgentQuickConfiguration.test.ts:315`, `:350`, `:390`).

### 2.7 `Session.agent_id` exists; keying by `id` is the codebase's existing rule (`session.rs:3099`)

### 2.8 `regex` adds no supply chain; `Cargo.lock` gains exactly one line

788 `[[package]]` before and after; `regex 1.12.3` already vendored. **RETRACTED (rev 1): "`Cargo.lock` does not change".** It gains `"regex",` on the `agentscommander-new` dependency array, so revision 1's `git diff --exit-code` gate halted step 1 on the correct outcome. Gate corrected in §8.

### 2.9 The gap

No context signal exists today.

### 2.10 Facts inherited from the epic

The row is **claude-hud**, hard-pinned to an absolute path ending `…/0.0.10/dist/index.js`; the cache also holds an orphaned `0.3.0`. "claude-hud's format" means **0.0.10's** format and changes silently if the pin is rewritten. The `statusLine` hook approach was rejected by the epic; not reopened.

### 2.11 The round-1 live capture (real bytes)

Probe reproduces AC's mirror exactly (`portable-pty 0.8.1` + `vt100 0.15.2`, `Parser::new(rows,cols,0)`, `process()` on raw ConPTY chunks) against real `claude.exe` v2.1.211 and `codex.exe` 0.144.4.

**The capture's bytes are literally this accessor's bytes**, verified independently by both reviewers: `contents_between` with `start_row == end_row` and `start_col < end_col` **is implemented as** `self.rows(start_col, end_col-start_col).nth(start_row)` (`screen.rs:222-227`). Same call, same code path.

1. **The narrow-wrap hazard does not exist.** Measured at 120, 80, 60, 40, 30, 24, 23, 22, 21, 20, 19, 18 cols. claude-hud pre-wraps *itself* at ` | ` / ` │ ` segment boundaries only; `Context <bar> <pct>` is one segment and is never split. The issue's "real design work" is not real.
2. **Truncation is the real hazard.** Overflow truncates with `…`, never wraps. At a true 100%, cols=20 renders `  Context ████ 10…`; `Context [░█]+ (\d+)` returns **10**. **Live at cols=120 too**, because Claude's render budget is narrower than `cols`. **Mitigation: require the literal `%` after the digits.** Truncation eats right-to-left, so removing any digit necessarily removed `%` first - grinch confirms this is structural, not lucky.
3. **The glyph anchor is refuted.** Reproduced by *typing into the input box*: `❯ The row says Context ██████████ 99% right now` matches while the truth is `0%`. **`▓` never occurs** - only `░` and `█`; the issue's own probe fixtures use `▓` and test fiction.
4. **`null != 0` confirmed on real bytes.** Fresh session sends `used_percentage: null` with a real `context_window_size: 1000000`; claude-hud computes `0/1000000` and prints `0%`, **byte-identical** to a true 0%.
5. **Row position is not stable and the row is never last.** `09 → 11 → 14 → 18` on width alone; `⏸ manual mode on` always below it. Absolute-index, "last row" and "second-to-last" anchoring all refuted. `Context` starts at **column 2**, always.
6. **Codex is real and worse than described.** `  Ready · Context 0% used · weekly 83% left`. Glyphless; **a second `%` on the row**; `Ready · ` is a mutable prefix.
7. **Absence states:** `/help`, slash autocomplete, `@` autocomplete, `disableAllHooks` all remove the row (verified live). Untrusted workspace: untested, not refuted. **Permission prompts: NOT VERIFIED LIVE.** Two user-config lines delete the anchor (`lineLayout: "compact"`; `showContextBar: false`) - source-read only.

### 2.12 The round-2 exit-grid capture. **The row survives, and nothing signals the death**

| Exit | Provider | Status | Context row on the frozen grid |
|---|---|---|---|
| `/exit` clean | Claude | `code: 0` | **survives verbatim** |
| external kill (crash surrogate) | Claude | `code: 1` | **survives verbatim** |
| `Ctrl+D` | Claude | `code: 129` | absent - **not** an exit-time clear |
| external kill | Codex | `code: 1` | **survives verbatim** |
| `/quit` | Codex | `0xC000013A` | **survives verbatim** |

Clean `/exit`: child gone at t=16098 ms; grid sampled at 16.5/17.5/19/22/24 s, **every sample identical, ~8 s after the process died**. Claude Code never uses the alternate screen, does not clear, and does not restore.

**The `Ctrl+D` result is not an exit clear.** Ctrl+D swaps the statusline for `  Press Ctrl-D again to exit` *before* exiting - §2.11.7's absence class, inherited incidentally. **The killed path can only ever preserve the row, because a killed process cannot repaint.**

**PTY EOF never arrives either.** `eof_at=None` in **every** run, 8 s past a confirmed `code: 0`. The rig matches AC where it counts (master held alive `local_backend.rs:852`, reader cloned `:839`, break on `Ok(0)` `:916-917`). **Nothing signals the death from any direction, so any mechanism must poll.** `dev-rust` retracted its own round-1 *"the read thread just ends on EOF"* on this evidence, unprompted: *"an inference from `:917`'s `Ok(0) => break` without checking whether ConPTY ever delivers the `Ok(0)`. It does not."*

*Caveat, stated: ~8 s observed, not minutes; Windows/ConPTY only. Unix PTY EOF semantics differ and are untested by anyone.*

**A frozen grid presents a perfectly well-formed row** - glyph present, `%` present, column 2, last match on the grid - that passes **every** defence in §4.5. Those rules separate prose from statusline; **none separates live from dead. Liveness is not a regex problem.**

#### 2.12.1 Q2: column 2 is reachable, via the input box's wrapped continuation lines

The round-1 capture asserted *"Claude indents transcript output by 2 as well"* with no capture. Measured: **the conclusion holds, the stated mechanism does not.**

```
[06]|❯ aaaaaaaaaa bbbbbbbbbb … kkkkkk|
[07]|  Context ██████████ 99% tail|
[10]|  Context ░░░░░░░░░░ 0% │ Usage ██░░░░░░░░ 16% …|
```

Row 07 is a wrapped continuation, starts with exactly two spaces, and matches `^ {2}Context [░█]+ (\d+)%`; truth is row 10 = `0%`. `❯` decorates only the **first** visual row. Two bounds that survive: the *single-row* input case does **not** defeat the anchor (`❯` at column 0), and tool output is gutter-indented (content at column 5). Assistant transcript prose: **not verified live**; it cannot change the answer, since input wrapping needs no model turn.

**The expert retracted its own round-1 advice:** persistence does not defend against this, because a wrapped input line persists as long as the user leaves it in the box. That independently confirms §4.6's rejection from a third angle.

### 2.13 RETRACTED: my round-1 report's "`mark_exited` has exactly two production callers". **No count is stated here**

I ran `grep ... | head -20` and read the truncated output as the complete list. **That is precisely §2.2's error** - reading a grep count as an enumeration - committed by me, four sections later, in the report that told the tech-lead a peer's disposition was falsified.

**The count is now dropped entirely, on the tech-lead's call, and he is right.** Three parties have produced three numbers for it (mine: 2; `dev-rust`: 20 `.mark_exited(` sites, noting *"§2.13 is itself the retraction of a miscount, and it miscounts by one"*; grinch: 29 grep lines, ~9 production sites, *"not 2, not 21"*). **It is not load-bearing, and it has been wrong three times in a section whose subject is a wrong count.** State the scoped claim, cite the mechanism, cite no total.

**The scoped claim, verified independently by both reviewers, and it is stronger than revision 3 stated:**

> **Nothing observes a *local* session's natural exit.**

`dev-rust` corrected two of revision 3's own rows: `phone/mailbox.rs:7176` and `:9367` are **test fixture builders**, not startup restore (both sit in helpers that create a session, force it to a requested status, `.unwrap()` throughout, and return `(session.id, session.token)`; `mailbox.rs`'s real `#[cfg(test)]` is at `:6510`, above both). So the production non-container surface is **only** the two explicit stops that `kill()` first (`resource_monitor.rs:82`, `session.rs:2252` → `kill` at `:2246`) and the two startup-restore sites (`lib.rs:1592`, `:1720`). Grinch reached the same place from its own enumeration: *"every one replays a status AC already recorded, observes a container, or kills first. **None observes a local child dying.**"*

*(A trap for the next reader, from `dev-rust`: `mailbox.rs` contains `#[cfg(test)]` inside a **string literal** at `:6686` as part of a source-scrape guard, so a naive grep for scope markers in this file misleads.)*

**The general claim *"AC is blind to natural exits"* is false: the container backend is not blind.** `close_transport` (`:435-448`) drops the parser (`:436` → `:469`) and calls `mark_exited` (`:445`). **This is why §6.4's fix is small** - the gap is local-only, and the container backend is the reference behaviour, not a second site to change. §5.3's bare container delegation is right, and both reviewers confirmed it.

---

## 3. Scope

### In

1. A rows accessor on `SessionIoFanout`.
2. One `PtyBackend` trait method returning `ScreenRowsRead`, its 8 impls, and one `PtyManager` forwarder. **The local impl gates on child liveness (§6.4).**
3. `pty/context_scrape/`: three narrow source/sink traits, pattern resolution and compile cache, matching, per-session state, timer sampler.
4. A `context_regex` field on `AgentConfig`, `#[serde(default)]`.
5. Registration at the spawn chokepoint.
6. One typed IPC event (`session_context`) and one snapshot command (`get_session_context`).

### Out

- **Every TypeScript surface**: **#1033** (tech-lead's role-boundary call). §5.5 is normative so it can be mirrored without a decision.
- The Settings field and the sidebar badge: **#1033**. `.ac/project-settings.json`: **#1034**.
- Any automatic action at any threshold. Ever.
- **The general product gap: AC does not notice that a local agent died.** #1032 does not fix it, does not depend on it, and does not make it worse. See §10.3.

---

## 4. The decided solution

### 4.1 Shape

```
create_session_inner (commands/session.rs:945)
  after a successful PtyManager::spawn, for a session with Some(agent_id):
    scraper.register_session(id, agent_id)         [try_state; absent scraper = feature off]
                                                   [fresh entry: last_emitted = None]

ContextScraper (new, pty/context_scrape/)
  holds: Arc<dyn ScreenRowsSource>, Arc<dyn ContextPatternSource>, Arc<dyn ContextEventSink>
         and NOTHING else. No AppHandle.           [§6.1]
  own thread + own tokio runtime + shutdown token  [GitWatcher shape, git_watcher.rs:64-84]

  every SAMPLE_INTERVAL (5s):
    if registered.is_empty() { return }            [§6.1.1 - before patterns()]
    patterns = source.patterns().await             [ONE RwLock read; BoxFuture, §6.1]

    // Snapshot ids first: the loop must not mutate `registered` while iterating it,
    // and must not write last_emitted to an entry it is about to remove (dev-rust).
    let ids: Vec<(Uuid, String)> = registered snapshot;
    let mut over: Vec<Uuid> = vec![];

    for (id, agent_id) in ids {
      let (reading, session_over): (Option<u8>, bool) = match patterns.get(&agent_id) {
        None                         => (None, false),   [not configured: no lock, no rows, no compile]
        Some(p) if compile_failed(p) => (None, false),   [no lock, no rows]
        Some(p) => {                                     [recompile ONLY if p changed since last tick]
          match source.get_screen_rows(id) {             [short lock, rows cloned out, released]
            ScreenRowsRead::Rows(rows)  => (extract(&regex, &rows), false),
            ScreenRowsRead::Unavailable => (None, false),   [NOT a statement about the session - H1]
            ScreenRowsRead::SessionOver => (None, true),    [the session is over - prune]
          }
        }
      };
      if reading != last_emitted[id] {               [ONE gate for every state - §4.1.1]
        sink.emit(ContextUsagePayload { id, reading });
        last_emitted[id] = reading;
      }
      if session_over { over.push(id); }
    }
    registered.retain(|id, _| !over.contains(id));   [prune AFTER the loop]
```

#### 4.1.1 One equality gate covers every state

**RETRACTED (rev 2): "pattern = None -> skip entirely".** Grinch (G3): skip means no emit, so clearing the regex left the badge showing `42` **forever**, for a feature the user had just turned off - the exact class this plan calls unshippable, on a path marked decided, landing on the very journey §4.2 was rewritten to fix (typing a regex passes **through** empty and invalid states).

**Decided:** there is no skip. Every state produces a `reading: Option<u8>`, and **one** gate decides the emit. `last_emitted: Option<u8>` starts at `None` on registration, because `None` **is** what the badge already shows.

`dev-rust` on why this is the right shape: it makes §6.1.1's "no event when unconfigured" hold **by construction** rather than by a special case - `None != None` is false - *"exactly the property revision 2's `skip entirely` tried to get and broke G3 getting."*

| Case | reading | last_emitted | Emit? | Prune? |
|---|---|---|---|---|
| Never configured, forever | `None` | `None` | **no** | no |
| Configured, matches 42 | `Some(42)` | `None` | yes, `42` | no |
| Pattern cleared | `None` | `Some(42)` | **yes, `null`** | no |
| Pattern edited to something invalid | `None` | `Some(42)` | **yes, `null`** | no |
| Row stops matching (`/help`) | `None` | `Some(42)` | yes, `null` | no |
| **Child alive but unqueryable** | `None` | `Some(42)` | **yes, `null`** | **no - H1** |
| **Child `Exited`/`Gone`, session unknown** | `None` | `Some(42)` | **yes, `null`** | **yes** |
| Unchanged | `Some(42)` | `Some(42)` | no | no |
| Decrease | `Some(12)` | `Some(80)` | yes, `12` | no |

### 4.2 Pattern resolution: a compile cache keyed by the pattern string

**Revision 1 decided resolve-at-spawn on a false premise (§2.6); the tech-lead acked it citing that same premise and explicitly unbound me from the ack.** The property worth protecting was never a disk read - settings are in memory. It is **no `Regex::new` per tick per session**.

**Decided: register the `agent_id` at spawn; resolve the pattern *string* per tick from `SettingsState`; compile only when that string changes.** The decisive fact is **the respawn requirement**: under resolve-at-spawn, first contact is *paste the regex, see nothing*, because every watched session is already running. The counter-argument (every other `AgentConfig` field is spawn-time-resolved) fails because those are **inherently** spawn-time - you cannot change a running child's env. The regex is a read-side concern.

**The registration point survives §2.6's collapse.** `create_session_inner` (`session.rs:945`) sits above the backend split and calls `PtyManager::spawn` for both backends (`:1640`), so one registration covers local and container with **zero backend plumbing**. Both reviewers confirmed the chokepoint (7 production callers) and no register-before-parser race. `dev-rust`'s sharpening, adopted: **the correct claim is "the parser exists before the first sample", and the proof is the 5s gap, not the ordering** (`notify_waiters()` fires at `container_backend.rs:341` before `register_session` at `:346-347`).

**`try_state`, not `state` (F5).** Tauri 2.10.3's `state::<T>()` panics when unmanaged; `session_test_app` (`session.rs:4045-4053`) manages two types and `create_session_inner_holds_a_spawn_mark_until_the_pty_exists` (`:4295`) reaches the registration on a successful spawn. `commands/pty.rs:97` is the exact idiom (one of 25 sites). **Absent scraper = feature off.**

**Wording (`dev-rust`):** there is **no `settings` binding in scope at `session.rs:1648`**. Registration needs only `agent_id`, which is in scope (`:1549`, `:1595`).

#### 4.2.1 RETRACTED (rev 2): the F7 registration-reset paragraph. **AC never respawns with the same `Uuid`**

`SessionManager::create_session` (`manager.rs:43-53`) is the **only** creation path, takes **no id parameter**, and mints `let id = Uuid::new_v4();` (`:54`). The wake path (`lib.rs:1803-1819`) never passes `ps.id` and reads the new id back at `:1822`. AC says it out loud at `lib.rs:1912-1913`: `// PB-4: pass &info.id (the newly-live session's UUID), NOT ps.id (the stale prior-run UUID).`

**`register_session` can never meet an existing entry.** The `GitWatcher::invalidate_session_cache` precedent does not transfer (`git_repos` **is** rewritten on a live id; nothing rewrites a context reading on a live id).

**Decided: the paragraph and its test are deleted, not re-justified.** Keeping the reset as "insurance" would preserve a mechanism with no verified problem, which is the precise habit this plan exists to record. **Three of us moved an unverified claim forward** - grinch inferred it, the tech-lead relayed it as a requirement, I adopted it as fact and built a test on it. Grinch retracted its own premise unprompted, in the lead position of its own report.

### 4.3 A per-session accessor, not a batch (holds)

The scraper iterates its own map, so "zero cost when unconfigured" is satisfied before any lock. `dev-rust` confirmed the lock discipline is **compiler-enforced**: `get_screen_rows` is a **sync** `fn` over a `std::sync::Mutex` returning owned data, and a sync fn has no `await` to hold a guard across.

**Implementer watch-item (grinch), in the contract:** write `let rows = { source.get_screen_rows(id) };` so the guard drops. The natural `if let Some(rows) = ....lock().unwrap().get_screen_rows(id) { ...await... }` holds the temporary to the end of the block; `MutexGuard` is `!Send` but the tick runs under `rt.block_on`, which does not require `Send`, **so it would compile.** `clippy::await_holding_lock` catches it (§9.5.7); do not rely on the gate alone. **The §6.4 gate itself is structurally immune to this trap - see §6.4.2.**

### 4.4 Scan physical rows, bottom-up, first match wins

**RETRACTED (rev 1): wrap-joined logical rows.** Grinch re-ran its F1 screen under revision 2's rules and got `Some(42)`: **the deletion is verified clean, not conceded**, and grinch agrees deletion beat its own verified rightmost-match fix.

**Decided:**
- **Physical rows only.** No join. `wrapped[]` gone (§2.4).
- **Bottom-up; first match from the bottom wins.** The capture's "last match on the grid". Measured, not inferred: prose and transcript always render above the statusline; only `⏸ manual mode on` and blanks render below it (§2.11.5).
- **Extraction:** capture group 1, parsed as `u8`, rejected unless `0..=100`. No capture group 1 → rejected at compile time.

### 4.5 Anchoring: the rules live in the user's regex, and the engine ships none of them

**RETRACTED (rev 1): "the regex is a complete defense for Claude".** Refuted on real bytes (§2.11.3).

**The structural fact that keeps this section short:** the regex is **per-agent user configuration**. `%`-required, column-2-anchored and glyph-required are all expressible *in the pattern*. The engine implements **none** of them. The capture's rules are **documentation for #1033's placeholder** and cost this issue nothing.

| Agent | Pattern | Every element load-bearing |
|---|---|---|
| Claude (claude-hud 0.0.10, `lineLayout: expanded`) | `^ {2}Context [░█]+ (\d{1,3})%` | `^ {2}` rejects single-row input prose (`❯ ` at col 0); `[░█]` only (never `▓`); trailing `%` fails truncation closed |
| Codex 0.144.4 | `^ {2}.*· Context (\d{1,3})% used` | ` used` excludes the second `%` (`weekly 83% left`); no anchor on the mutable `Ready · ` |

Do not anchor `$` after `%`: `write_contents` (`row.rs:98-135`) strips trailing blank cells, but a submitted-prompt row was observed padded to column 120.

**The residual, measured (§2.12.1):** when the statusline is **suppressed** and a column-2 row carrying a matching bar is on the grid, the engine reads it. The reachable route is the **input box's wrapped continuation lines**, fully user-controlled, needing no model turn. It self-corrects on the next tick once the statusline returns.

Accepted rather than defended against: the epic already accepted this class (*"a stale or wrong number, not an error"*); it is transient and self-correcting, unlike §6.4's defect; and with the statusline absent **the grid carries no information that distinguishes it from prose**, so every mechanism against it is undecidable from the input available.

### 4.6 What I rejected, and why

**RETRACTED (rev 1): the row-budget contingency.** Refuted (§2.11.5).

**Cross-sample persistence: rejected, confirmed from three independent angles.** The reproduced false positive sits *in the input box* and is not transient, so a stability gate accepts it exactly as it accepts the statusline. Grinch reached the same place from the other side and reproduced it. **The expert retracted its own round-1 recommendation** for the same reason. Persistence separates *scrolling* prose, which bottom-up already handles.

**`────`-border anchoring: rejected for #1032, recorded for a successor.** It would close the residual, but it is **Claude-shaped** (Codex has no `────`), so it needs a second config field against the epic's single-field opt-in - **and the capture observed the border without testing an anchored scan against it.** Grinch endorses rejecting it on **"untested"** rather than "unwanted", from the agent that reproduced what building on an observation costs.

### 4.7 Sampling interval: 5s (holds, measured twice)

Matches `git_watcher.rs:12`. Grinch measured a full sample at **203 µs** on AC's default 30×120: fifty sessions at 5s is **0.2% of one core**. The `probe_child` gate adds **~0.9 µs** per configured session per tick (§6.4.3). Both reviewers endorse keeping the honest "argued by comparison" label for the sampler CPU itself; `dev-rust` would decline a benchmark for it.

---

## 5. Affected surfaces

### 5.1 New files

| File | Contents |
|---|---|
| `pty/context_scrape/mod.rs` | `ContextScraper`, `ScreenRowsSource`, `ContextPatternSource`, `ContextEventSink`, `ScreenRowsRead`, `SAMPLE_INTERVAL`, `ContextUsagePayload` |
| `pty/context_scrape/pattern.rs` | `ContextPattern` (compiled + source string), `compile(&str) -> Result<ContextPattern, String>`, 1 MiB size limit, capture-group-1 validation |
| `pty/context_scrape/rows.rs` | `extract(&ContextPattern, rows: &[String]) -> Option<u8>`. **Pure: no locks, no vt100, no Tauri** |

### 5.2 Modified

| File:line at `4acadfe` | Change |
|---|---|
| `Cargo.toml:28` | add `regex = "1"` after `vt100` |
| `pty/mod.rs:8` | `pub mod context_scrape;` (alphabetical) |
| `pty/output.rs:285` | after `get_screen_snapshot`: `pub fn get_screen_rows(&self, id) -> Option<Vec<String>>`. `let (_rows, cols) = screen.size();` (the call `get_pty_size:290` makes), then `screen.rows(0, cols).collect()`. `.ok()?` on the mutex, matching `:275`. Sync; clones out; releases. **The fanout stays 2-state: it knows nothing about children. The 3-state mapping is the backend's job (§6.4.1)** |
| `pty/local_backend.rs` (new free fn, beside `resize_instance:97`) | `fn screen_rows_if_child_alive(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, fanout: &SessionIoFanout, id: Uuid) -> ScreenRowsRead`. **Free over the fields so a test can drive it against a real ConPTY child** (`dev-rust`; precedent `resize_instance:97`, `hand_over_held_size:200`) |
| `pty/backend.rs:135` | `fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead;` after `get_pty_size`. **Doc: the three states and what each means for the caller (§6.4.1)** |
| `pty/manager.rs:270` | inherent forwarder. **`kind_for_session(id)` failing means the route is gone ⇒ `ScreenRowsRead::SessionOver`**, not `Unavailable` (grinch verified every route removal is preceded by parser removal) |
| `config/settings.rs:**72**` | **insert BEFORE `:72`**: `#[serde(default, skip_serializing_if = "Option::is_none")] pub context_regex: Option<String>` |
| `commands/session.rs:1648` | after the Err arm (`:1642-1647`): if `agent_id.is_some()`, `app.try_state::<Arc<ContextScraper>>()` → `register_session(id, agent_id)` |
| `commands/pty.rs:440` | `#[tauri::command] pub fn get_session_context(...) -> Result<Option<u8>, String>`, `try_state` |
| `lib.rs:696` | after `app.manage(pty_mgr.clone())`: build the **three** adapters, `ContextScraper::new(...)`, `.start(shutdown_for_setup.clone())`, `app.manage(...)`. Mirrors `GitWatcher` `:656-661`. **Must be after `lib.rs:604`'s `.manage(settings)`** |
| `lib.rs:2069` | register `get_session_context` |

### 5.3 The 8 `PtyBackend::get_screen_rows` impls (complete; confirmed across three rounds)

| File:line | Impl | Body |
|---|---|---|
| `pty/local_backend.rs:1358` | `LocalProcessBackend` (`:1120`) | one call to `screen_rows_if_child_alive(&self.ptys, &self.fanout, id)` |
| `pty/container_backend.rs:1182` | `ContainerTransportBackend` (`:1088`) | `match self.fanout.get_screen_rows(id) { Some(r) => Rows(r), None => SessionOver }`. **No liveness gate: `close_transport` drops the parser before anyone could read it (§2.13), so parser-absent IS the container's liveness oracle** |
| `pty/manager.rs:371` | `RecordingBackend` (`:324`) | `SessionOver` |
| `pty/manager.rs:462` | `DelayedSpawnBackend` (`:417`) | `SessionOver` |
| `commands/session.rs:3922` | `FailingSpawnBackend` (`:3882`) | `SessionOver` |
| `commands/session.rs:4024` | `GatedSpawnBackend` (`:3969`) | `SessionOver` |
| `commands/ac_discovery.rs:2642` | `FlippingLiveBackend` (`:2610`) | `SessionOver` |
| `phone/mailbox.rs:6574` | `MailboxMockPtyBackend` (`:6538`) | `SessionOver` |

`session_test_app` (`session.rs:4045-4053`) needs **no** change, because §4.2 uses `try_state`.

### 5.4 (removed - TypeScript is #1033)

### 5.5 The IPC contract, normative for #1033

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsagePayload {
    pub session_id: String,
    /// 0..=100, or None when unavailable. NEVER 0 for "unknown" (§6.3).
    /// Deliberately NO `skip_serializing_if`: `None` MUST serialize as an
    /// explicit `"percent": null`, not as an absent key.
    pub percent: Option<u8>,
}
```

- **Event:** `session_context`. **Command:** `get_session_context`, args `{ sessionId: string }`, returns `Option<u8>`.
- **`Some`:** `{"sessionId":"3f2a…","percent":42}` · **`None`:** `{"sessionId":"3f2a…","percent":null}` · **Command:** `42` or `null`
- **TS #1033 must write:** `interface SessionContext { sessionId: string; percent: number | null }`; `getSessionContext: (sessionId: string) => Promise<number | null>`; `onSessionContext(cb)`

**Why `skip_serializing_if` is forbidden here:** with it, `None` omits the key and TS must type `percent?: number`, silently re-introducing *absent* as a third state beside `null` and `0`, in a feature whose one hard rule is that unavailable is exactly one thing. The codebase distinguishes these deliberately: `Session.agent_id` has no skip and surfaces as `agentId: string | null` (`types.ts:46`); `PtyOutputPayload.sequence` (`output.rs:48-49`) has the skip and is optional. `AgentConfig.context_regex` (§5.2) **does** take the skip - a settings field where an absent key is correct.

**Known and frozen (grinch):** the engine gives #1033 **no way to distinguish "invalid regex" from "no match"** - both are `percent: null`. Bounded, but the user-visible result is "I pasted a regex and nothing happened, with no error anywhere but `app.log`". #1033 may want to surface compile errors from Settings; that is its call and this contract does not block it.

---

## 6. Required behavior, edge cases, failure behavior

### 6.1 Required

- **The value never drives an action.** No `/clear`, `/compact`, restart, PTY write, or process destruction, at any threshold, ever.

  **RETRACTED (rev 1): "enforced by construction, not by discipline"** - it was discipline; the scraper held `Arc<Mutex<PtyManager>>`. **RETRACTED (rev 2): "two narrow trait objects and nothing else"** - §4.1 listed `AppHandle` as a third field, and `AppHandle` is the universal capability handle: the repo does `app.state::<Arc<Mutex<PtyManager>>>()` in **19 places** (count verified exact by `dev-rust`), including `session.rs:2242` which calls `.kill(uuid)` four lines later at `:2246`, `pty/inject.rs:79`/`:104`/`:115` (writes to PTYs), and `session/auto_close.rs:197` (kills sessions). I narrowed two fields of three and re-made the same absolute claim one level up, about #1031's one hard rule.

  **Decided (rev 3), and grinch confirms it is now true:**

  ```rust
  pub trait ScreenRowsSource: Send + Sync {
      /// Three states, because the oracle behind it has three (§6.4.1).
      fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead;
  }
  pub trait ContextPatternSource: Send + Sync {
      /// BoxFuture, not a sync fn: `blocking_read` panics in a runtime (G2, reproduced).
      fn patterns(&self) -> BoxFuture<'_, HashMap<String, String>>;
  }
  pub trait ContextEventSink: Send + Sync {
      fn emit(&self, payload: ContextUsagePayload);
  }

  pub enum ScreenRowsRead {
      /// The live grid's rows.
      Rows(Vec<String>),
      /// No reading this tick. Says NOTHING about whether the session is over.
      /// Retry next tick, keep the entry. (H1: child alive but unqueryable; parser poisoned.)
      Unavailable,
      /// The session is over. Emit null once, then stop sampling it.
      SessionOver,
  }
  ```

  All three traits implemented in `lib.rs` over `Arc<Mutex<PtyManager>>`, `SettingsState` and `AppHandle`. The scraper holds **three trait objects and no `AppHandle`**. **Grinch went looking for the escape and it is not there:** all three are `Send + Sync` **only**, with **no `Any` supertrait and no `as_any`**, so a `dyn` handle cannot be downcast back to its implementation - contrast `PtyBackend: Any + Send + Sync` with `fn as_any(&self)` (`backend.rs:120-121`), which **is** downcastable. Its verdict: *"G1's fix is real, and it's the first mechanism in this plan to earn its own claim. I falsified it twice; it's now true."* **#1031's one hard rule is structurally enforced.**

  Precedent: `PtyOutputTarget` (`output.rs:58-92`) already wraps `AppHandle` behind `Arc<dyn Fn(PtyOutputPayload) + Send + Sync>` **in the file the accessor goes into**, and ships `noop()` (`:72`) and `from_test_sink()` (`:79`). `GitWatcher` holding an `AppHandle` (`git_watcher.rs:32`, `:147`) is a fine *shape* precedent and not a precedent for the *claim*, because it never claimed to be read-only.

- **`ContextPatternSource` must be async (G2, reproduced).** Revision 2 specified a **sync** `fn patterns(&self)` over `SettingsState`, which is a **tokio** `RwLock` (§2.6 says so itself). A sync fn can only read it via `blocking_read()`, which **panics inside an async context**; §4.1's tick is one (`rt.block_on`). Grinch reproduced: `PANIC: Cannot block the current thread from within a runtime.` `GitWatcher` survives only because its `poll()` is an `async fn` that awaits (`git_watcher.rs:97-98`). **The scraper would have died on tick 1, silently, permanently, for every session**, with §6.5's "silent and inert" holding in its most useless possible sense. **Decided: `BoxFuture`**, the repo's own idiom (`PtyBackend::spawn`, `backend.rs:123`), one file over. `async_trait` is in the lock but used nowhere in `src`. (`dev-rust`, unprompted: *"grinch was right and I was not."*)
- **Accept decreases.** No monotonicity. **Sample on a timer, never in `handle_output`. Key by agent `id`, never `command`. Emit only on change**, through §4.1.1's single gate.

#### 6.1.1 The cost when unconfigured, stated completely, and the leak it does not close

The issue's criterion is *"No regex configured → no event, no state, no cost."*

**Restated:** *no regex configured → no event, no reading, no PTY lock taken, no rows cloned, no regex compiled.* Sessions with `agent_id: None` (plain shells) are never registered. `dev-rust` confirms §4.1.1's single gate makes the "no event" half hold **by construction**, and that the `None` arm never calls `get_screen_rows`, so the rest holds too.

**RETRACTED (rev 3): the guard corollary.** Revision 3 adopted `dev-rust`'s empty-map guard and claimed it restores *"literal zero cost for users running no agent sessions"*. **Grinch reproduced that it does not**, because `prune` lives only in the `Some(p)` arm, so **no pattern ⇒ no `get_screen_rows` ⇒ no prune**:

```
=== C. agent registered, NO regex configured, children EXITED ===
  tick 0..3: registered.len() = 5, 5, 5, 5   --> entries never leave
=== C2. same, WITH a regex configured ===
  after 1 tick: registered.len() = 0         (prune fired)
```

So `registered` **never returns to empty** after the first agent session of the app run. The guard fires only in the window before that, and never again.

**Decided, and the tension is decided rather than left open (grinch: "a decision, not a defect"):**

1. **The guard stays, scoped honestly:** it covers the window from app start until the first agent session. That window is real and the guard is two lines; it is simply not the general property revision 3 claimed.
2. **Entries for unconfigured agents persist until app exit.** ~100 bytes each (`Uuid` + agent id + `Option<u8>` + map overhead), bounded by agent sessions spawned per app run. A week-long run at 100 sessions/day is ~70 KB.
3. **I am not pruning them, and the reason is this plan's own lesson.** Reaching them costs either a `ptys` probe **or** a discarded 30-row clone for every registered session on every tick - purely to reclaim ~100 bytes for a feature that is switched off. Grinch is right that it would break "no PTY lock taken when unconfigured", and it would need a second method on the source trait to avoid the clone. **Building a mechanism to solve a problem measured at ~70 KB is exactly what §10.4 is a record of.** The honest bound is cheaper and true.

**Flagged as a scope decision, not hidden:** this rewrites an acceptance criterion from the issue body. It is the price of deleting the respawn requirement (§4.2). The fallback is resolve-at-spawn plus a named respawn requirement.

### 6.2 Edge cases

| Case | Behavior | Why |
|---|---|---|
| Chunk splits, SGR interleaved | Non-problem | The grid normalizes before any regex runs |
| **Narrow width** | **Matches; the segment becomes its own row** | claude-hud pre-wraps at segment boundaries (§2.11.1) |
| **Truncation (`Context ████ 10…`)** | **N/A** | Only if the pattern requires the trailing `%` (§2.11.2) |
| Alt-screen | Live grid; no match → N/A | `Screen::grid()` returns `alternate_grid` when active (`screen.rs:744`); `Screen::set_size` (`:113-118`) resizes **both** grids |
| Scrolled-off rows | Unreadable | Scrollback is `0` |
| Prose FP, single-row input box | Rejected | Column-2 anchor; `❯` at column 0 |
| Prose FP, column 2, statusline present | Rejected | Bottom-up (§4.4). **Tested by §9.1.4** |
| **Prose FP, wrapped input continuation** | **Wrong number, self-corrects next tick** | Accepted residual (§4.5, §2.12.1). **Pinned by §9.1.6** |
| Statusline suppressed (`/help`, autocomplete, `disableAllHooks`) | N/A | §2.11.7 |
| **Local agent exits naturally** | **`null` once, then pruned** | §6.4 |
| **Local child alive but unqueryable** | **`null`, entry KEPT, retry next tick** | **§6.4.1 - H1** |
| **Container agent exits naturally** | **`null` once, then pruned** | `close_transport` drops the parser (§2.13) |
| **Pattern cleared or made invalid** | **`null` once** | §4.1.1 |
| Two concurrent sessions | Never cross | Mirror keyed by `Uuid`; scraper map likewise. `Regex` is `Sync` and stateless across matches |
| Session never mounted in a terminal | Still produces a reading | `register_session` runs at `local_backend.rs:866` before any frontend involvement |
| Match outside `0..=100` | Rejected, N/A | §4.4 |

**RETRACTED (rev 1): "sample inside the resize window → `null`, never a wrong number".** Grinch reproduced the opposite (`EXTRACT = Some(87)` where revision 1 predicted `null`): the wrap splits wherever the column boundary falls, and claude-hud's real format has a long suffix after the percent, so the first fragment still matches alone. The *mechanism* was right; the *inference* was wrong, and §9.1.3 was cited as pinning it while its fixture split the number. Moot now that the join is gone; recorded because **the reasoning error is the transferable part** and it recurred three more times.

### 6.3 The one place `null != 0` stops (holds; confirmed on real bytes)

**The engine never synthesizes `0`.** Absent row, no match, unparseable, out of range, session over, unqueryable child, poisoned lock: all `null`.

**If the row literally renders `0%`, the engine reports `Some(0)`.** The capture proves this is unavoidable: on a fresh session `used_percentage` is `null` but `context_window_size` is a real 1000000, so claude-hud computes `Math.round(0/1000000*100)` and prints `0%`, byte-identical to a true 0%. Inferring `null` from `Some(0)` would erase a genuine low reading. The epic accepted this; this plan pins where the line falls.

### 6.4 Liveness: gated in the one backend that has the gap

The probe (§2.12) closed §10.1's blocker against "accept and document": the row survives every path that matters, PTY EOF never arrives, and a frozen grid passes every §4.5 defence.

**Three facts, reconciled:** any mechanism must **poll** (§2.12); `watch_child` (`spawn_diagnostics.rs:1009-1049`) **already polls `probe_child` per session** and already sees `Exited` (`:1040-1043`) and only logs; and **the gap is local-only** (§2.13). So revision 2's costing of "a liveness trait method plus 8 impls" was wrong twice: the oracle already exists **and** only one backend needs it.

**Decided: `LocalProcessBackend` gates its rows read on `probe_child`, as a free function so a test can drive it.**

```rust
/// Free over the map and the fanout, like `resize_instance` (`:97`), so the liveness gate
/// can be driven by a test against a real ConPTY child (§9.2).
fn screen_rows_if_child_alive(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    fanout: &SessionIoFanout,
    id: Uuid,
) -> ScreenRowsRead {
    match probe_child_in(ptys, id) {
        ChildLiveness::Alive => match fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            // The child is alive, so the session is NOT over. A missing/poisoned parser
            // here is a desync or a poisoned lock, never a statement about the session.
            None => ScreenRowsRead::Unavailable,
        },
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ScreenRowsRead::SessionOver,
        // H1: "we could not ask" is NOT "the child is dead". Keep the entry, retry.
        ChildLiveness::Unqueryable(_) => ScreenRowsRead::Unavailable,
    }
}
```

#### 6.4.1 RETRACTED (rev 3): `Unqueryable → None → prune`. **H1, reproduced twice**

Revision 3 priced `Unqueryable` as *"a possible flicker to N/A"* and mapped it to the same `None` as a dead child. §4.1's pseudocode then pruned on `None`, and **`prune` removes the entry from `registered`, which only `create_session_inner` ever writes.** The pseudocode wins. Grinch reproduced it (`tickprobe`, §4.1 verbatim):

```
t=0  alive                             registered=[1]  emitted=[(1,Some(42))]
t=10 AV strips rights -> Unqueryable   registered=[]   emitted=[(1,Some(42)), (1,None)]
t=15 alive again, child never died     registered=[]   emitted=[…unchanged]
--> child ALIVE, row says 42%. Badge: None. Still registered? false
```

**A flicker is 5 seconds. This is the rest of the session's life, on a child that never died.** And it is reachable on a **live** child, reproduced against real Windows processes (`gateprobe`): the same running process, two handles - full rights → `Alive`; no `SYNCHRONIZE` → `Unqueryable("WaitForSingleObject failed (os error 5)")`.

*Grinch's bound, stated as it stated it:* **"No lo validé"** that AV/EDR strips `SYNCHRONIZE` in production; it proved the mechanism, not its frequency. It also notes #942's rights-stripping comment is about `GetExitCodeProcess`, which the gate reaches only **after** the child is gone. **That does not soften it:** revision 3 already conceded reachability and priced it, so the dispute is the price, and the price was wrong by the difference between 5 seconds and forever.

**The codebase already made this argument and I overrode it.** `spawn_diagnostics.rs:260-263`: `Unqueryable` is *"**Distinct from `Alive` on purpose**: reporting an unanswerable handle as 'running' is the same ambiguity class as the `ExitStatus { code: 1 }` trap this module exists to kill."* **#942 built a three-valued oracle precisely so "could not ask" is never confused with a definite answer.** Revision 3 collapsed it to two and read the merged value as a definite *dead*, while saying so out loud: *"`watch_child` treats `Unqueryable` as 'keep polling', which is right for diagnostics and wrong here."* **That sentence is the defect. `watch_child` is the one that has it right**, and it is the only consumer that kept the third state third.

**H2 (grinch) is the cause: `Option<Vec<String>>` is a two-state channel for a three-state oracle**, and `None` came to mean four things - session unknown, parser poisoned, child dead, child unqueryable - of which prune is correct for three and destructive for the fourth. **The oracle was never wrong; the seam was too narrow to carry what it knows.** And the same narrowness made the defect **unrepresentable in §9.4 by construction**, because the fake implements `ScreenRowsSource` and so was two-state too. Grinch calls that a new F6 mutation: not a test pinning a fiction, but **a fake whose type makes the failure unsayable**.

**Fixed by `ScreenRowsRead` (§6.1), which closes H1, H2 and my own §10.2 item 4 together:** three states in, three states out, no collapse, and the fake becomes three-state so `a_live_child_that_cannot_be_queried_is_not_pruned` (§9.4) is writable. It is the same move §6.1 made three times this revision: **give the type the shape of the truth.**

*Naming, since the shape was mine to choose:* grinch proposed `RowsResult { Rows, Unavailable, Gone }`. I kept its structure and renamed `Gone` → `SessionOver`, because `ChildLiveness::Gone` already means something narrower (no PTY instance) and both enums sit in the same five lines; and `SessionOver` names what the caller must **do**, which is the distinction the enum exists to carry.

#### 6.4.2 Lock order: verified, and structurally immune to §4.3's own trap

`probe_child` takes the `ptys` guard as a **local inside its own body** and returns `ChildLiveness` **by value**, so the guard drops at the function's return. The `match` scrutinee is an owned value holding no borrow of the map, so no temporary-lifetime extension can carry the guard into an arm. `screen_parsers` is therefore taken strictly after `ptys` is released.

`dev-rust`'s point, adopted verbatim because it is the difference that matters: **this is not "we were careful", it is "it cannot be written wrong here".** §4.3 warns about `if let Some(x) = lock().foo() { ...await... }` holding the temporary to the end of the block; the gate **cannot** have that bug, because the lock is taken inside the callee and the callee returns an owned value rather than a guard.

The wider chain adds no edge: `PtyManager` → (`kind_for_session`) → `ptys` → *released* → `screen_parsers`. `PtyManager` → `ptys` is pre-existing (`commands/session.rs:70` → `kill` → `local_backend.rs:1217`).

**RETRACTED (rev 3): the citation.** Revision 3 cited `local_backend.rs:940-943` as precedent for *"the order stays `ptys → screen_parsers`"*. **That comment documents the opposite order** (*"the query takes `screen_parsers` and RELEASES it before `open_startup_gate` takes `ptys`"*). Both are safe for the reason the comment actually gives - **never nested** - so the conclusion stands and the citation supported it by analogy, not directly. Cited here for the discipline, not the direction.

#### 6.4.3 Cost: measured independently, twice

Revision 3 listed this as a belief I could not check (*"I believe this is free - which is precisely the kind of belief that has been wrong twice"*). It is now a number.

| | `dev-rust` (`ptysbench`, n=20000, release) | grinch (`gateprobe`, n=200000, release) |
|---|---|---|
| **full `probe_child` `ptys` hold** | **729 ns** (p50 700, p95 1000, p99 1200-1300) | **897 ns** |
| `WaitForSingleObject(h, 0)` alone | - | 788 ns |
| lock + `HashMap::get_mut` only | 94 ns (empty write) | in the noise |
| **one keystroke `write` `ptys` hold** | **1868-2350 ns** (p50 1300-1500) | - |
| `probe_child` on an exited child | 1661 ns (adds `GetExitCodeProcess`) | - |

**A single keystroke holds `ptys` 2.6 to 3.2 times longer than the entire probe.** Duty cycle 1.5 × 10⁻⁷ (`dev-rust`) / 0.0009% at 50 sessions (grinch). **Contention was unmeasurable:** `dev-rust` ran keystrokes against a sampler at **1000 Hz, 5000× production**, and *the ordering inverted between runs* (the real-rate row was worst in one and best in the other); grinch hot-looped at ~5,000,000× and moved the write-lock mean only 0.154 → 1.793 µs. **Both refused to dress noise as a delta**, and grinch discarded its own worst-case figures after finding the no-scraper baseline already showed 377 µs outliers. `WaitForSingleObject(handle, 0)` makes it free. **Confirmed.**

#### 6.4.4 Why this shape, and not the alternatives

- **The oracle is inherent to the backend with the gap.** No trait method for liveness, no 8 impls, no container change. Both reviewers confirmed the oracle choice: `probe_child_liveness` (`local_backend.rs:1016-1045`) deliberately does **not** use `try_wait`, because #942 found `WinChild::is_complete` *"swallows a failed `GetExitCodeProcess` and returns 'not exited'"* and its `STILL_ACTIVE` sentinel 259 is a legal exit code. It calls `WaitForSingleObject(handle, 0)` directly. Grinch confirmed both halves against real processes (live → `Alive`, exited → `Exited{code:0}`). **The expert's probe reached the same conclusion from the other end when it abandoned EOF for polling; AC's oracle is the stronger of the two and it was already here.**
- **It does not touch `get_screen_snapshot`, and that is the point.** The obvious general fix - drop the parser when the child dies - would change what `get_screen_snapshot` returns for an exited local session, and that accessor is #955's screen replay, fired on every terminal attach (`commands/pty.rs:413`). Whether a user re-attaching to a dead agent should see its final screen or a black tile is **a product question this issue must not answer by accident**. See §10.3.

**With the gate, the prune fires on natural exit** for configured sessions, §4.1.1 emits `null` once, and the entry leaves the map. Grinch reproduced it (`tickprobe` scenario B): frozen 42% → `null` exactly once → silence → entry gone. **§10.1's blocker is genuinely closed for `Exited`/`Gone`.** (The map-growth corollary for *unconfigured* sessions is **not** closed: §6.1.1.)

**What this does NOT fix, deliberately (§3, §10.3):** AC still does not notice that a local agent died. `SessionStatus` stays stale, the parser still leaks, the sidebar still shows the session as it was. **#1032 makes none of that worse and depends on none of it.**

### 6.5 Failure behavior

Every failure is silent and inert: never breaks a session, never writes to the PTY, never blocks a launch, never raises a dialog.

| Failure | Behavior |
|---|---|
| Pattern does not compile | `reading = None` → §4.1.1 emits `null` if the badge showed a number. Logged **once per pattern change**, not once per tick (the cache makes the failure sticky, so the log keys on the change). No prune |
| Pattern has no capture group 1 | Same; rejected in `pattern.rs::compile` |
| Pattern compiles but never matches | `null`. No log spam |
| `screen_parsers` mutex poisoned | `.ok()?` → fanout `None`; with a live child that is `Unavailable` → `null`, **no prune** (§6.4.1) |
| `ptys` mutex poisoned | `probe_child` uses `unwrap_or_else(|e| e.into_inner())` (`local_backend.rs:982`), so it still answers |
| `PtyManager` mutex poisoned | Tick logs once, returns. Next tick retries |
| `kind_for_session` fails (route gone) | `SessionOver` → `null` once → pruned (grinch: route removal is always preceded by parser removal) |
| Scraper not managed (`try_state` → `None`) | Feature off. No panic (F5) |
| Session killed, or child exited | `SessionOver` → `null` once → pruned |
| **Child alive, liveness unqueryable** | **`Unavailable` → `null`, entry kept, retried every tick until it answers** |

**Catastrophic-regex note:** `regex` 1.x has no backtracking and is linear-time by construction. `RegexBuilder::size_limit` fixed at 1 MiB so a hostile pattern fails compile rather than allocating unboundedly. With the cache, compilation happens once per pattern change.

---

## 7. Compatibility and security impact

### Compatibility

- **Existing settings files deserialize unchanged** (`#[serde(default)]` + `Option` + skip, identical to `instructions_filename`).
- **`Cargo.lock` gains exactly one line**; 788 packages before and after; no new crate.
- **No IPC breaking change.**
- **`PtyBackend` gains a method.** Private in-crate trait, 8 impls, all enumerated.
- **`cargo test --lib` unaffected** (`try_state`).
- **`get_screen_snapshot` is untouched** (§6.4.4), so screen replay behaves exactly as it does today for every session, live or exited.

### Security

- **Read-only by type** (§6.1), and grinch verified the escape is closed: no `Any` supertrait, no `as_any`, so the `dyn` handles cannot be downcast back. Total capability is read rows, read patterns, emit a payload.
- **The reading is never persisted.**
- **The regex is user-supplied and executed as a regex**, never as a command, path, or format string.
- **No captured text is logged.** Only capture group 1, parsed as `u8` and range-checked, leaves `rows.rs`. Agent output cannot reach `app.log` through this path - deliberate, since `app.log` is what users paste into issues.
- **Out of scope, re-confirmed by the capture (`env_has_AC_TOKEN = true`), already routed by the epic:** `AGENTSCOMMANDER_TOKEN` is inherited by the `statusLine` subprocess, today the hard-pinned third-party claude-hud, which makes network calls. Neither worsened nor addressed here.

---

## 8. Implementation order

1. **`Cargo.toml:28`**: `regex = "1"`.
   **Gate (corrected; revision 1's halted on the correct outcome):** `Cargo.lock` gains **exactly one line**, `"regex",` in `agentscommander-new`'s `dependencies`, and the `[[package]]` count is **unchanged at 788**. **If any new `[[package]]` block appears, stop and report.** Do **not** use `git diff --exit-code Cargo.lock`; it returns 1 on a correct run.
2. **`context_scrape/rows.rs`**: `extract`, pure, plus §9.1.
3. **`context_scrape/pattern.rs`**: `ContextPattern`, `compile`, size limit, capture-group validation, plus tests.
4. **`output.rs`**: `get_screen_rows -> Option<Vec<String>>`, plus §9.2's first block.
5. **`backend.rs` + `ScreenRowsRead` + the 8 impls + `manager.rs` forwarder + `screen_rows_if_child_alive`**, plus §9.2's real-ConPTY block.
6. **`settings.rs`**: the field, **before `:72`**, plus §9.3.
7. **`context_scrape/mod.rs`**: the three traits, `ContextScraper`, thread, tick, compile cache, prune-after-loop, emit, plus §9.4.
8. **`lib.rs` + `commands/pty.rs` + `commands/session.rs`**: adapters, construct, manage, start, register, command.

Steps 1-6 are inert. The feature first becomes live at step 8.

---

## 9. Tests and objective acceptance criteria

**Fixture discipline.** §9.1's fixtures are **copied from the capture**, never hand-written: `Row::write_contents` (`row.rs:98-135`) strips trailing blank cells and pads interior gaps, so an invented `"% "` is not what the parser emits. **No fixture may contain `▓`** (§2.11.3). *(Revision 2 wrote this rule and violated its spirit twice on the same page; see §9.6.)*

### 9.1 `rows.rs`, pure

| Test | Asserts |
|---|---|
| `the_real_120_col_row_extracts` | `  Context ░░░░░░░░░░ 0% │ Usage …` → `Some(0)` |
| `truncation_fails_closed_when_the_pattern_requires_percent` | `  Context ████ 10…` (truth 100) → **`None`**. The highest-value test here |
| `truncation_without_the_percent_rule_returns_a_wrong_number` | Same row against `Context [░█]+ (\d+)` → `Some(10)`. **Pins *why* the rule exists**, so a careless pattern edit cannot silently drop it. Grinch: the one test here that survives a careless edit |
| **`the_lowest_matching_row_wins`** | **REWRITTEN (G4).** `["  Context ██████████ 99% (pasted)", "", <capture row 09>]` → bottom-up `Some(0)`; the same fixture top-down gives `Some(99)`. **The only test that distinguishes scan order** |
| `input_box_prose_is_rejected_by_the_column_two_anchor` | Capture row 06 (`❯ ` at col 0) → `None` |
| `a_wrapped_input_continuation_defeats_the_column_two_anchor` | §2.12.1's row 07 → `Some(99)`. **Pins the accepted residual as measured, not as a surprise** |
| `the_85_percent_breakdown_still_extracts` | `  Context █████████░ 87% (in: 12k, cache: 40k) │ …` → `Some(87)`. Catches an end-anchored pattern |
| `all_three_bar_widths_extract` | 10/6/4-block real rows (cols 120/80/40) |
| `zero_and_hundred_extract_despite_a_missing_glyph` | `Context ░░░░░░░░░░ 0%` (no `█`) and `Context ██████████ 100%` (no `░`) |
| `codex_row_extracts_context_not_weekly` | `  Ready · Context 0% used · weekly 83% left` → `Some(0)`, **never `Some(83)`** |
| `an_out_of_range_match_is_rejected` | `Context 999%` → `None` |
| `a_zero_percent_row_reports_zero_not_null` | Pins §6.3 |

### 9.2 `output.rs` and the gate, against the real parser and a real child

Harness: `fanout()` (`output.rs:508-514`), `feed()` (`:527-536`), `session()` (`:538-542`). **`session()` registers at 30×120 (`:540`)**, so a test needing another size must call `register_session` directly or resize first, or it silently tests nothing (`dev-rust`).

| Test | Asserts |
|---|---|
| `get_screen_rows_matches_contents_between` | `rows(0, cols)[r] == contents_between(r, 0, r, cols)` **for valid `r` only** - the `Equal` branch ends in `.unwrap_or_default()`, so an out-of-range row yields `""` where `rows()` has no index (grinch). Pins §2.11's transfer claim |
| `leading_spaces_survive_so_the_column_two_anchor_works` | A row painted at column 2 reads back with both leading spaces. **The column-2 anchor rests on `write_contents`'s gap-padding** |
| `two_sessions_never_cross_rows` | Criterion 1 |
| `get_screen_rows_is_none_for_an_unknown_session` | The fanout's 2-state contract |
| `rows_are_readable_for_a_session_with_no_terminal` | `PtyOutputTarget::noop()`, never resized. Criterion 2 |

**The gate, against a real ConPTY child** (`dev-rust`'s seam, adopted - see §10.2). Extend `startup_gate_tests`' existing helper (`local_backend.rs:1605-1641`, which already opens a real ConPTY at `:1620-1627` and builds a real `PtyInstance` at `:1631-1639`) with a `conpty_with_child()` variant filling the `child: None` gap at `:1634`, plus the real `fanout()`:

| Test | Asserts |
|---|---|
| `rows_are_readable_while_the_child_is_alive` | Real ConPTY + real child + real fanout, feed bytes → `ScreenRowsRead::Rows(..)` |
| `session_over_once_the_child_actually_exits` | Kill the child, await exit → `ScreenRowsRead::SessionOver`. **Red if the probe call is deleted** |

This pins the **real wiring against the real oracle and the real parser**, which the §9.4 fake structurally cannot, since a fake `ScreenRowsSource` sits *above* the gate. It is §10.4's own conclusion applied to the one new mechanism. **Cost, stated:** it spawns a real process, so it is slower and carries the usual real-process flake surface; `startup_gate_tests` already pays that, so it is settled precedent rather than a new argument.

**Not testable in-repo, and stated rather than faked:** the `Unqueryable → Unavailable` arm needs a rights-stripped handle inside a `PtyInstance`. Its *behaviour* is pinned by §9.4's three-state fake; the *real-handle* proof is grinch's `gateprobe`, which reproduced it against live Windows processes and is not a repo test.

**Dropped:** `a_hard_wrapped_statusline_joins_into_one_logical_row`, `a_wrap_between_context_and_the_number_still_matches` (tested the deleted join against a shape that cannot occur), `a_resize_clears_the_wrap_flags` (the flags no longer exist - §2.4).

### 9.3 `settings.rs`

`settings_without_context_regex_deserialize_unchanged` (criterion 6); `context_regex_round_trips_as_camel_case`.

### 9.4 `context_scrape/mod.rs` - pure, with a **three-state** rows fake

Fake `ScreenRowsSource` / `ContextPatternSource` / `ContextEventSink`. No Tauri, no `PtyManager`, no `SettingsState`. The sink is a recording fake holding `Vec<ContextUsagePayload>` (`PtyOutputTarget::from_test_sink`'s shape). **The rows fake returns `ScreenRowsRead`, so H1 is now expressible** - under revision 3's `Option` it was unrepresentable by construction (§6.4.1).

| Test | Asserts |
|---|---|
| `a_session_whose_agent_has_no_pattern_takes_no_lock_and_emits_nothing` | The rows fake records calls: **zero**. Sink: **empty**. Criterion 5 |
| `an_empty_registered_map_never_reads_patterns` | The pattern fake records calls: **zero**. §6.1.1's guard, in the only window it covers |
| `clearing_a_pattern_emits_null_once` | **G3.** 42, then pattern gone → **one** `null`, then silence. Entry kept |
| `a_pattern_edited_to_something_invalid_emits_null_once` | **G3**, second half |
| `an_uncompilable_pattern_logs_once_per_change_not_once_per_tick` | §6.5's sticky-failure rule |
| `a_changed_pattern_is_recompiled_within_one_tick` | The compile cache; the deleted respawn requirement |
| `an_unchanged_pattern_is_not_recompiled` | Compile count stays 1 across 3 ticks |
| `session_over_emits_null_once_and_prunes` | Rows fake → `SessionOver`: one `null`, entry gone, no further reads |
| **`a_live_child_that_cannot_be_queried_is_not_pruned`** | **NEW - H1.** Rows fake: `Rows(42)` → `Unavailable` → `Rows(42)`. Asserts `null` then `42`, **and that the entry survives the `Unavailable` tick.** Red against revision 3's design. **This is the test the old fake's type made impossible to write** |
| `an_unchanged_value_emits_once` | The gate |
| `a_decrease_is_emitted` | 80 then 12 emits 12 |
| `a_second_session_for_the_same_agent_gets_its_own_entry_and_first_emit` | Replaces `re_registering_an_id_re_emits` (§4.2.1). Exercises a thing production does |

### 9.5 Objective acceptance criteria

1. Two concurrent sessions never cross samples. **9.2.3.**
2. A session whose terminal was never mounted still produces a reading. **9.2.5.**
3. Absent/no-match/truncated/out-of-range/session-over/unqueryable renders unavailable, never `0`, and triggers no action. **9.1.2, 9.1.11, 9.1.12, 9.4.8, 9.4.9**; "no action" holds **by type** (§6.1, escape verified closed).
4. A prose line containing `Context 99%` does not move the badge **in the two configurations that are tested by two different tests**: single-row input-box prose (**9.1.5**, the column-2 anchor) and **column-2 prose with the statusline present (9.1.4, bottom-up)**. **Not** when the statusline is suppressed, and **not** for a wrapped input continuation (**9.1.6** pins that as known and measured). The residual is stated in §4.5.
5. No regex configured: no event, no reading, no lock, no compile. **9.4.1**, per §6.1.1 (which also states what *is* paid).
6. Existing settings files without the new field deserialize unchanged. **9.3.1.**
7. **A live child that cannot be queried is never deregistered. 9.4.9.**
8. `cargo clippy --all-targets -- -D warnings` clean for touched files. Catches `await_holding_lock` (§4.3). `ScreenRows` is gone, so G5's `fields never read` cannot recur.
9. `cargo fmt` clean **for touched files only** (the repo is not `rustfmt`-clean at baseline).
10. `cargo test --lib` green. **Not `cargo test`**: it fails at baseline (`rustdoc.exe` missing), pre-existing.

**CPU:** the sampler is argued by comparison, corroborated at 203 µs per sample at 30×120 (50 sessions at 5s = 0.2% of one core). **The gate is measured, not argued: ~0.9 µs `ptys` hold, against a keystroke's own 1.9-2.4 µs** (§6.4.3).

### 9.6 RETRACTED (rev 2): §9.1.4, and the rule it broke

Revision 2's `the_lowest_matching_row_wins` used *"capture's rows 06 and 09 verbatim"*. Grinch ran it against the plan's own pattern: **bottom-up `Some(0)`, top-down `Some(0)`** - row 06 is `❯ The row says…`, so `❯ ` puts `Context` at column 16 and it **never matches**. The test passed under **any** scan order, pinning only the column-2 anchor that §9.1.5 already pinned **with the same fixture**. **§9.5's criterion 4 then named those two as different configurations, counting one configuration twice**, while column-2-prose-with-statusline-present - the case bottom-up is the **sole** defence for - had **no test at all**. Both fixed above.

---

## 10. Verdict

### Status: READY_FOR_IMPLEMENTATION

Round 3 of 3. `dev-rust`: *"Yes. I would implement revision 3 cold, from the plan alone, with no open decision."* Grinch's acceptance condition, stated verbatim in its own verdict: *"If `Unqueryable` stops pruning - via H2's `RowsResult` or any equivalent - then on my evidence this mechanism is sound, and it is the first in this plan that a probe could not break."* **§6.4.1 does exactly that**, and §9.4.9 is the test its type finally permits.

### 10.1 What round 3 changed

| Item | Disposition |
|---|---|
| **H1** (`Unqueryable` prunes a live session, permanently) | **Fixed.** `ScreenRowsRead::Unavailable` never prunes (§6.4.1). New test §9.4.9 |
| **H2** (the seam is 2-state for a 3-state oracle; the fake cannot express H1) | **Fixed by the same enum.** Closes my own §10.2 item 4 with it |
| **H3** (prune unreachable without a pattern; §6.1.1's guard never fires again) | **Guard corollary retracted; the leak is bounded honestly and the tension decided** (§6.1.1). Not pruning ~100 bytes is cheaper than the mechanism to reclaim it |
| §2.13's count | **Dropped.** Three parties, three numbers, in a section about a wrong count. Scoped claim kept and strengthened |
| §6.4's lock-order citation | **Retracted** (it documents the opposite order); the conclusion holds on "never nested" (§6.4.2) |
| `dev-rust`'s §9.2 seam | **Adopted** (§9.2). Real ConPTY child, red if the probe call is deleted |

### 10.2 My five unanswerable questions, answered

Revision 3 listed five things I could not check about my own work. Round 3 answered all five, and this is the reason the plan is certifiable now rather than a round ago.

1. **Can the gate false-prune a live session?** **Yes, permanently.** Reproduced twice. Fixed (§6.4.1).
2. **Does `probe_child` cost the keystroke path anything?** **No, and it is now a number.** 729 ns / 897 ns against a keystroke's own 1868-2350 ns, measured independently by both reviewers, with contention unmeasurable at 5000× and ~5,000,000× production rates (§6.4.3). **My belief was right; it should not have shipped as a belief.**
3. **Is the lock order sound?** **Yes, and it is structurally immune to my own §4.3 trap** (§6.4.2).
4. **Is the gate pinned by any test?** It was not, and it was worse than "only a mock" - the fake's *type* made the failure unsayable. **Now it has a real-ConPTY seam the repo already built for #973** (§9.2), and a three-state fake.
5. **Is gating a general-sounding accessor on liveness the right seam?** **The oracle was right; the seam was too narrow.** `ScreenRowsRead` is the answer.

**Asking them was worth more than any answer I could have given.** Every one came back with a probe attached, and two of them (1 and 4) were defects I would have shipped.

### 10.3 Scope: option 3, as recommended and endorsed

**#1032 gates its own read; a new issue owns the general gap.** `dev-rust` concurs.

- **Option 1 (absorb the liveness fix) is more dangerous than it looks, and §2.13 is why.** The obvious general fix is "remove the parser when the child dies" - exactly what the container backend already does. For local sessions that changes what `get_screen_snapshot` returns for an exited session, and that accessor is #955's screen replay, fired on every terminal attach. **The two backends already disagree about this today**, which is itself worth an issue and not one to settle by picking a side inside a badge issue.
- **Option 2 buys correctness we do not need.** #1032 does not need AC to *notice* the death; it needs to not *report* a dead reading. The second is ~5 lines.

**File the general gap separately.** It is unusually well-specified already: the target behaviour is **what the container backend already does** (`close_transport`: drop state, then `mark_exited`), the oracle exists (`probe_child`), the poller exists (`watch_child`, which detects the exit and discards it), and there is exactly one real design question (what `get_screen_snapshot` should return for an exited session).

### 10.4 The record, which is the thing that freezes

Six retracted mechanisms, one mistake: **a claim nobody verified, plus a test that pins the resulting fiction.**

| # | Mechanism | Justified by | Found by |
|---|---|---|---|
| 1 | `row_wrapped` join (rev 1) | the issue's "genuine hazard" | capture (never wraps) + grinch (wrong number) |
| 2 | §9.1.3 pinning the resize window (rev 1) | an inference about where a wrap splits | grinch (reproduced the opposite) |
| 3 | `a_pruned_session_stops_emitting` (rev 1) | a mock returning `None` | `dev-rust` (production never reaches `None`) |
| 4 | `re_registering_an_id_re_emits` (rev 2) | grinch's F7, relayed by the tech-lead, adopted by me | grinch (retracted its own premise) |
| 5 | §9.1.4 pinning bottom-up (rev 2) | a fixture where the decoy never matches | grinch (passes under any scan order) |
| 6 | **`Unqueryable → prune` (rev 3)** | **my own inference that it was "a flicker"** | **grinch (`tickprobe`: the rest of the session's life)** |

**4 and 5 were added by revision 2, in a §9 whose opening sentence convicts revision 1 of exactly this.** 6 was added by revision 3, in a document whose §10.4 already said all of the above - **while the codebase's own comment (`spawn_diagnostics.rs:260-263`) argued against me in advance, and revision 3 quoted that comment and overrode it in the same paragraph.** Writing the rule down has never once stopped me from breaking it.

And §2.13 is the same reflex in the evidence rather than the design: I convicted the issue of reading a grep count as an enumeration (§2.2), then did it myself four sections later. **Then §2.13's own replacement count was wrong too**, which is why no count survives in it.

**What actually worked, every single time, is a probe that runs the mechanism against the real thing.** Reading the crate carefully caught none of the six, and I read it very carefully. Six were caught by `vt100probe`, `tickprobe`, `gateprobe`, `ptysbench` and a real ConPTY rig. **That is why §9.2 now spawns a real child, and it is the one design decision in this plan I would defend without evidence.**
