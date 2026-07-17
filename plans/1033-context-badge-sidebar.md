# Plan Contract: #1033 - the context regex field and the CTX badge (frontend)

**Status: READY_FOR_IMPLEMENTATION**
**Issue:** #1033, child of epic #1031
**Branch:** `feature/1033-context-badge-sidebar`, cut from `main` = `d1b4a52dc590bc7428e9969220942a496802d9bc`
**Revision:** 1, authored and certified in one pass (lite path). Classification re-checked against the stated bar in §10.2 and it holds.

> **#1032 landed and is invisible.** It emits `session_context` and nobody listens. Every byte of this issue is TypeScript, CSS and one placeholder helper. `dev-webpage-ui` implements it and owns every file under `src/`.
>
> **Three of the "copy this" instructions are traps: followed faithfully, each ships a defect.** None is visible by reading the code, and each is refuted below by a probe rather than an argument.
>
> 1. **Copy the #529 normalizer** (`settings-save.ts:15-20`): its `.trim()` deletes a pattern's leading whitespace, which is the column-2 anchor. The badge **fails open** and every normal case keeps working (§2.6).
> 2. **Copy the #529 placeholder and hint**: there, blank means "use the default shown". Here, **blank means the feature is off**, and there is no fallback pattern anywhere in the backend (§2.7).
> 3. **"Mirror the #882 path exactly"**: #882's value rides on `Session` from the backend. This one does not, and the engine emits **only on change**, so a listener alone leaves a reloaded sidebar reading `CTX N/A` **forever** (§2.3, §4.4). This is the one I would have shipped.
>
> A fourth is not a trap but a gap: the pattern #1032 tells me to ship verbatim is **correct** and **cannot be typed on a keyboard** (§2.8).

---

## 1. Issue and objective

Give the user the reading #1032 already produces:

1. A per-agent **context regex** field in Settings, Coding Agents, so the scrape can be configured at all.
2. A **`CTX N%`** badge on the sidebar's coding-agent sessions, so the reading is visible.

**The percentage is a signal for a human. It never drives an action.** No click target, no threshold, no PTY write, ever.

---

## 2. Evidence and current-state gap

Everything below was verified by me against `d1b4a52`. Where I ran a probe, the probe is named and its output is quoted.

### 2.1 The backend contract is frozen, shipped, and verified by hash

```
git cat-file blob main:plans/1032-context-scrape-vt100-mirror.md | sha256sum
→ 3f415ffe8aeb4ad298fa2ef0509c705a1bc6d54abcab20e7bbb4e47e3327cd11
```

Matches the tech-lead's certification hash. `plans/1032…md` §5.5 is normative for this plan and I make no contract decisions.

Verified on `main`, in the source, not quoted from the issue:

| Piece | Location at `d1b4a52` |
|---|---|
| `AgentConfig.context_regex: Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]` | `config/settings.rs:79-80` |
| `ContextUsagePayload { session_id, percent: Option<u8> }`, **no `skip_serializing_if` on `percent`** (comment `:73-76` says why) | `pty/context_scrape/mod.rs:69-78` |
| Event `session_context`, emitted through `ScraperSink` | `lib.rs:456` |
| Command `get_session_context(app, session_id) -> Result<Option<u8>, String>` | `commands/pty.rs:457-463` |
| Command registered | `lib.rs:2174` |
| `last_reading` = `entry.last_emitted`, *"`None` covers both 'no reading' and 'not registered', which are the same thing to the badge"* | `mod.rs:195-203` |
| `SAMPLE_INTERVAL` = **5s** | `mod.rs:26` |
| Pattern compile: Rust `regex` 1.12.3, 1 MiB size limit, **capture group 1 mandatory** (`captures_len() < 2` rejects) | `pty/context_scrape/pattern.rs:36-54` |

**The frontend surface is empty.** `git grep -nE "session_context|sessionContext|contextRegex|getSessionContext|onSessionContext|context_regex" -- src/` returns **exit 1, zero lines**. All of it is mine.

#### 2.1.1 An instrument warning for whoever works this branch next

**Plain `grep -r` in this environment is wrapped and lies by omission.** It summarises (`121 matches in 16 files`), truncates, and dropped the line I was looking for; `grep -rnF 'emit("session_context"' src-tauri/src/` returned **nothing** while `git grep -nF` returned `lib.rs:456`. Same worktree, same second. Every enumeration in this plan was taken with `git grep` or a direct read. Do not verify anything here with bare `grep`.

### 2.2 The issue's coordinates are stale, and #1028 is why (re-anchored)

I re-anchored every coordinate. The drift is real, but **its cause is not what the dispatch says, and the difference matters for whoever re-reads #1031 next.**

The dispatch says #1031's family "cites `4acadfe` in its Provenance but the refs were recycled from a memo at `f123b03`, 130 commits stale". For **this** issue's most load-bearing coordinate that is **not** what happened. Measured, by reading `types.ts` out of each commit:

| SHA | `export interface Session` | `agentId` |
|---|---|---|
| `f123b03` (the memo) | `:17` | `:31` |
| **`4acadfe`** (the SHA #1031 claims) | `:32` | **`:46`** |
| `57b1f13` (#1028) | `:70` | **`:84`** |
| `d1b4a52` (this branch) | `:70` | `:84` |

**The issue's `types.ts:46-53` was exactly right at the `4acadfe` it cites.** It was not recycled from `f123b03`, where the same field sits at `:31`. **#1028 invalidated it afterwards**, adding 38 lines above `Session` (`RepoDirtyByPath`, `:40-58`, among them).

So the failure mode here is an issue that aged, not an issue that lied, and the remedy is the same either way: re-anchor against `d1b4a52`, which is what the table below does. I am recording the correction because "the refs are recycled and 130 commits stale" invites the next reader to distrust every #1031 coordinate on principle, and at least this one was accurate when written.

| The issue / epic says | Actually at `d1b4a52` | Note |
|---|---|---|
| `types.ts:46-53` (`Session.agentId`/`agentLabel`) | **`types.ts:84-85`**, `Session` spans **`:70-98`** | drifted 38 lines |
| `SettingsModal.tsx:2190-2202` (#529 input) | **holds exactly**, hint at `:2203-2206` | |
| `SettingsModal.tsx:433` `agentList()`, `:560` `setSettings`, `:651` profiles | **all hold** | |
| `settings-save.ts:16-21` (normalizer) | fn is **`:15-20`**; applied in the chain at **`:95-98`** | off by one |
| `session-status.ts:12-15` / `:16-31` | **holds exactly** | |
| `ipc.ts:762-773` (`onSessionIdle`/`Busy`) | **`:767-777`** | |
| `App.tsx:654-665` (listeners) | **holds exactly** | |
| `sessions.ts:357-370` (`setSessionWaiting`) | **holds exactly** | |
| `SessionItem.tsx:369-389` (badge slot), `:45-49` (resolver) | **both hold exactly** | |
| `SettingsModal.tsx` / `settings-save.ts` paths | **`src/sidebar/components/…`**, not `src/components/…` | the issue omits the dir |

The three render sites, re-anchored: **`SessionItem.tsx:373`** (`agent-badge`) and **`:383`** (`profile-badge`); **`RootAgentBanner.tsx:407`**; **`ProjectPanel.tsx:2686`**.

### 2.3 **TRAP 3 (header).** The event is emit-on-change, so a listener alone is **not** sufficient

This is the single most important fact in this plan and it is not in the issue.

`mod.rs:190` registers a session with `last_emitted: None`, and #1032's §4.1.1 gate emits only `if reading != last_emitted`. **A session already sitting at 42% emits nothing more until it changes.** A sidebar that starts listening after that point hears silence and renders `CTX N/A` **indefinitely**, while the terminal plainly shows 42%.

`App.tsx`'s `onMount` awaits settings (`:537`), a repo search (`:565`) and the session list (`:576`) before it reaches the listener block at `:654`. The scraper starts in `setup` at `lib.rs:696` and ticks every 5s regardless.

**So `get_session_context` is mandatory, not an optimisation.** #1032 shipped it for exactly this and said so (`mod.rs:195-196`: *"for the snapshot command"*). The ordering rule is §4.4.

### 2.4 `setSessions` is a wholesale replace, and it has exactly one production caller

`sessions.ts:318-320` is a bare `setState("sessions", sessions)`. No field preservation. `setProfileOutdated`'s comment (`:385-387`) warns about precisely this: *"never use setSessions for this (a wholesale replace would reset the frontend-only pendingReview field)"*.

Production callers of `setSessions`: **`App.tsx:577` only** (once, at mount). Everything else is a test or `ui-harness.tsx:240`. So the wipe is real but it happens **before** the `:654` listener block, which is why the #882 path never noticed it. §4.2 sidesteps it entirely rather than relying on that ordering.

### 2.5 The store already has the exact shape this needs

`SessionsState.lastActivityBySessionId: Record<string, number>` (`types.ts:833`), initialised `{}` (`sessions.ts:26`), getter `:439-441`, setter `markActivity` `:506-508` (`setState("lastActivityBySessionId", (prev) => ({ ...prev, [sessionId]: performance.now() }))`), read in `ProjectPanel.tsx:2125`.

A **session-id-keyed, frontend-only, event-fed map on `SessionsState`, structurally immune to `setSessions`.** That is this feature, exactly. §4.2 copies it.

The miss-versus-null question it raises is also already answered in this codebase: `RepoDirtyByPath` (`types.ts:40-58`, #1028) documents a missing key and an explicit `null` collapsing to one rendering **by design**. §4.2 copies that discipline and its comment style.

### 2.6 **TRAP 1 (header).** Copying the #529 normalizer fails the badge OPEN. Measured

`normalizeAgentInstructionsFilename` (`settings-save.ts:15-20`) does `agent.instructionsFilename?.trim()` and **persists the trimmed value**. For a filename that is correct. For a regex it is destructive: **leading whitespace is load-bearing in a pattern.** A user who writes the column-2 anchor as two literal spaces rather than `^ {2}` has that anchor silently deleted at save.

Probe (`rxp`, real `regex` 1.12.3, real 1 MiB limit):

```
stored  = "  Context [░█]+ (\\d{1,3})%"
trimmed = "Context [░█]+ (\\d{1,3})%"     <-- what a copied #529 normalizer persists

  real statusline (truth=0)     stored -> Some(0)    trimmed -> Some(0)
  ATTACK: typed in input box    stored -> None       trimmed -> Some(99)
```

The normal case keeps working, so nothing looks broken. The typed-in-box attack of #1032's §2.11.3 goes from **rejected to accepted**. This is the failure mode the epic calls unshippable, introduced by faithfully following the instruction to copy the template.

**Decided: the `contextRegex` normalizer drops empty-or-whitespace and stores every kept value BYTE-FOR-BYTE, untrimmed.** A deliberate, stated divergence from `instructionsFilename`, `configSeed.dest` and `backend.image`, all three of which trim. The "no sentinel" half of the contract is unaffected.

### 2.7 **TRAP 2 (header).** The #529 placeholder and hint semantics invert here

The template pairs `placeholder={defaultInstructionsFilename(agent.command)}` (`:2198`) with the hint *"Leave blank to use the default shown"* (`:2203-2206`). That is true there: the backend really does fall back to the derived default.

**Here, blank means the feature is OFF.** `mod.rs`'s table: never configured, forever, emits nothing. There is no fallback pattern anywhere in the backend. A hint saying "leave blank to use the default shown" would be a plain lie, and it is the sentence the implementer is most likely to copy along with the markup.

**Decided: keep a placeholder (it is the only in-product example of the shape), and write the hint fresh.** §5.4 fixes the copy verbatim.

### 2.8 **GAP (not a trap: the instruction is right).** The pattern this plan ships cannot be typed, and weakening it is measurably worse

§4.5 of #1032 records the Claude pattern as `^ {2}Context [░█]+ (\d{1,3})%`. `░` is U+2591 and `█` is U+2588. **Neither is on a keyboard.** With the field as the only affordance, the feature is unconfigurable for Claude, which is the agent the user asked for.

The obvious escape is a typeable character class. Measured, same rig:

| Row | `[░█]+` (#1032 §4.5, verbatim) | `\S+` (typeable) |
|---|---|---|
| `  Context ░░░░░░░░░░ 0% │ Usage █░░░░░░░░░ 8% (resets in 3h 22m)` | `Some(0)` | `Some(0)` |
| `  Context ░░░░ 0%` (cols=40) | `Some(0)` | `Some(0)` |
| `  Context ████ 10…` (truncated at 100) | `None` | `None` |
| `❯ The row says Context ██████████ 99% right now` | `None` | `None` |
| `  Context left until auto-compact: 15%` (stock Claude) | `None` | `None` |
| **`  Context is 42% of the budget`** (prose at column 2) | **`None`** | **`Some(42)`** |

**The glyph class is load-bearing and the dispatch is right to ship the pattern verbatim.** It rejects a word-shaped bar; `\S+` accepts one. So the typeability problem must be solved with an affordance, not by relaxing the pattern.

**Decided: a "Use suggested pattern" button** (§5.4), filling the field from a command-keyed helper. ~8 LOC on the `settings-add-btn` precedent (`SettingsModal.tsx:1890-1897`).

### 2.9 Client-side regex validation is wrong in **both** directions. Measured

`handleSave` has a validation gate (`currentValidationError()`, `SettingsModal.tsx:1120`), so "validate the regex before saving" is the natural next thought. It cannot be done in JavaScript. `RegExp` and Rust's `regex` are different grammars, and I measured both sides rather than asserting it:

| Pattern | JS `new RegExp` | Rust `regex` 1.12.3 |
|---|---|---|
| `^ {2}Context [░█]+ (\d{1,3})%` | accepts | accepts, `captures_len=2` |
| `^ {2}.*· Context (\d{1,3})% used` | accepts | accepts, `captures_len=2` |
| `(?P<pct>\d{1,3})` | **SyntaxError** | **accepts** |
| `(?=.*Context)(\d+)` | accepts | **rejects** (parse error) |
| `(a)\1` | accepts | **rejects** (parse error) |

A JS gate would **block** valid patterns and **pass** invalid ones. Either behaviour is worse than none.

**Decided: #1033 ships no regex validation.** An unusable pattern renders `CTX N/A` and its reason reaches `app.log` only, exactly as #1032 §5.5 froze (*"I pasted a regex and nothing happened"*). Surfacing real compile errors needs a backend command; that is a separate issue and this plan does not block it (§10.3).

Both shipped patterns compile with `captures_len=2`, so they clear `pattern.rs:44`'s mandatory-capture gate. Verified, not assumed.

### 2.10 #1043 is right about the Codex pattern's stated reason, and the real reason is sharper

#1032 §4.5 says ` used` is what excludes the row's second percentage (`weekly 83% left`). Measured against `  Ready · Context 0% used · weekly 83% left`:

| Pattern | Result |
|---|---|
| `^ {2}.*· Context (\d{1,3})% used` (shipped) | `Some(0)` |
| `^ {2}.*· Context (\d{1,3})%` (` used` removed) | `Some(0)` |
| `^ {2}.*(\d{1,3})% used` (`· Context ` removed) | `Some(0)` |
| **`^ {2}.*(\d{1,3})%` (both removed)** | **`Some(3)`** |

On the real row **either literal alone pins the capture**; ` used` is not what excludes `weekly 83%`. Strip both and greedy `.*` slides to the last `%` and captures **`3`** out of `83`: a wrong number, not a miss. And on a reordered row (`Context 0% left · weekly 83% used`) the shipped pattern returns `None` while dropping ` used` returns the correct `Some(0)`, so ` used` is the more brittle of the two.

**The transferable rule, and the reason this plan does not let anyone invent a pattern: a literal adjacent to the capture is what pins where the capture lands.** The patterns ship verbatim; §5.4's copy states this reason and not #1032 §4.5's.

### 2.11 The a11y requirements do not survive contact with the surface

- **`prefers-reduced-motion` occurs in zero CSS files** (`git grep -n "prefers-reduced-motion" -- "*.css"` is empty). "Honour reduced motion" presumes an animation. §4.6 ships none, which satisfies it by construction rather than by a media query for an animation that does not exist.
- **`role="meter"` requires `aria-valuenow`.** There is no valid `aria-valuenow` for `N/A`. A meter with no value is invalid ARIA, so the unavailable state **must not be a meter** (§4.5). The only `aria-value*` precedent in the repo is `main/App.tsx:250-265` (`role="separator"` plus the `data-ac-testid`/`data-ac-role`/`data-ac-state` trio), and §5.3 follows its shape.
- **A live region here would be an accessibility defect, not a feature.** The value refreshes on a 5s cadence (`mod.rs:26`); an `aria-live` badge would interrupt a screen reader roughly every 5 seconds, forever, with a number that drives nothing. §4.6 ships no live region. The issue's "live regions only on significant transitions" is honoured by there being no significant transition to announce (§4.6 ships no thresholds).

### 2.12 The gap

Nothing in `src/` knows the feature exists.

---

## 3. Scope

### In

1. `contextRegex?: string` on the TS `AgentConfig`, plus its input, suggest button, hint and save-time normalizer.
2. `suggestedContextRegex(command)` and the two pattern constants.
3. The IPC surface: `SessionContextPayload`, `onSessionContext`, `PtyAPI.getSessionContext`.
4. `contextPercentBySessionId` on `SessionsState`, its setter, its hydrate-if-absent seeder, and its getter.
5. The `App.tsx` listener plus the mount-time hydration (§4.4).
6. `session-context.ts` (pure projection) and `ContextBadge.tsx` (shared markup), wired into the three sites.
7. One CSS block.

### Out

- **Any backend change.** The contract is frozen (§2.1). If this plan seems to need one, stop and report; it does not.
- **Regex validation and compile-error surfacing** (§2.9, §10.3).
- **Threshold styling and any threshold at all** (§4.6).
- **The terminal window's `StatusBar.tsx`.** Separate Tauri window, own store, second listener path. Not free. Decide separately.
- **Cross-project / `.ac/project-settings.json`:** #1034.
- Notifications, toasts, gauges, history graphs, hiding/filtering configs.

---

## 4. The decided solution

### 4.1 Shape

```
lib.rs:456  emit "session_context" {sessionId, percent: number|null}
  → onSessionContext(cb)                     src/shared/ipc.ts   [new, beside onSessionIdle]
  → listener in onMount, pushed to unlisteners   src/sidebar/App.tsx
  → sessionsStore.setSessionContext(id, percent)
  → state.contextPercentBySessionId[id]
  → contextBadgeText(percent)                session-context.ts  [pure projection]
  → <ContextBadge percent testId />          ContextBadge.tsx    [shared markup]
  → SessionItem · ProjectPanel · RootAgentBanner

commands/pty.rs:457  get_session_context
  → PtyAPI.getSessionContext(id)             src/shared/ipc.ts   [new, beside getScreenSnapshot]
  → mount-time hydrate, AFTER the listener, never clobbering it  (§4.4)
```

### 4.2 State lives in a session-keyed map, not on `Session`

**Decided: `contextPercentBySessionId: Record<string, number | null>` on `SessionsState`.** Exact precedent §2.5.

Rejected: a field on `Session`. `Session` mirrors a Rust struct field-for-field, and #1032 deliberately did **not** put `percent` there. A TS-only field on it would make the interface lie about the wire, and it would sit in the blast radius of `setSessions`'s wholesale replace (§2.4). The map has neither problem and is already the house style for this exact case.

**Two absent-ish states, one rendering, stated the way `RepoDirtyByPath` states it:**

| Store state | Means | Renders |
|---|---|---|
| key missing | no event yet, and no snapshot yet | `CTX N/A` |
| `null` | the engine says unavailable | `CTX N/A` |
| `0` | **a real reading of zero** | `CTX 0%` |

`0` is a third, distinct value and must survive the trip. **A `??`-to-`||` slip turns a true `0%` into `N/A` and is invisible on screen.** §9 pins it.

**This does not reintroduce the third state #1032 §5.5 forbids.** That rule governs the **payload type**, which is `percent: number | null` and never `percent?: number` (§5.1). At the store layer, missing and `null` collapse to one rendering, so "unavailable is exactly one thing" holds where the user can see it.

### 4.3 The badge is a shared component, not three copies

`ProfileOutdatedBadge.tsx` is the precedent and it is exact, down to the problem: *"Shared by SessionItem, the ProjectPanel replica rows, and RootAgentBanner so the badge looks and behaves identically everywhere a coding-agent session can drift"*, with a `testId?: string` prop because each surface names itself. `ContextBadge.tsx` copies that shape. Triplicating the ARIA markup across three files is how the three drift.

The projection stays in its own pure module (`session-context.ts`), mirroring `session-status.ts`, so §9 can assert totality without rendering anything.

### 4.4 Hydration: listener first, then hydrate, never clobber

§2.3 makes hydration mandatory. The order is not free:

- **Hydrate then listen** loses any event landing in the gap. An event only fires on change, so "lost" means wrong until the *next* change, which may be never.
- **Listen then hydrate, unconditionally** lets a slow `await` resolve with a stale snapshot **after** a fresher event already landed, and overwrite it.

**Decided: register the listener first; then hydrate only sessions whose key is absent.**

```ts
unlisteners.push(
  await onSessionContext(({ sessionId, percent }) => {
    sessionsStore.setSessionContext(sessionId, percent);
  })
);

// #1033 - snapshot-seed every agent session no event has spoken for yet. The engine
// emits ONLY on change (pty/context_scrape/mod.rs), so a session already sitting at a
// value when this window mounted will never send one, and a listener alone leaves it
// reading N/A forever. Registered AFTER the listener, and hydrateSessionContext is a
// no-op on an existing key, so a slow invoke cannot overwrite a fresher event.
// Absent-key check, never a truthiness check: a hydrated `null` and a hydrated `0`
// are both answers. try/catch mirrors the repo-search fan-out at :564-567 so one
// rejected invoke cannot abort the rest of onMount.
try {
  await Promise.all(
    sessionsStore.sessions
      .filter((s) => s.agentId)
      .map(async (s) => {
        sessionsStore.hydrateSessionContext(s.id, await PtyAPI.getSessionContext(s.id));
      })
  );
} catch {}
```

`hydrateSessionContext` is a no-op when the key exists. It is the only thing standing between the user and a permanent `N/A` on a window reload.

**`Promise.all`, not a sequential `for`/`await`:** the fan-out is one map lookup per session behind an IPC hop (`commands/pty.rs:457-463`), and it sits at the tail of an `onMount` that has already awaited settings, a repo search and the session list. Serialising it buys nothing and delays the first paint of every badge by the sum of the round-trips. **The `try` wraps the whole `Promise.all`,** so one rejected invoke costs the batch its unsettled hydrations, not `onMount`; every session it misses still shows `CTX N/A` and self-corrects on its next change. That is the same trade `:564-567` already makes for repos.

**Placement: inside the existing `onMount`, after the `:654-665` listener block.** Sessions are already loaded at `:577`. Sessions with `agentId === null` (plain shells) are never registered by the backend and are skipped here, so the fan-out is bounded by agent sessions.

### 4.5 Markup: the meter only exists when there is a value

`role="meter"` requires `aria-valuenow` (§2.11), so the two states are structurally different elements, not one element with a nullable attribute:

| State | Element |
|---|---|
| `percent` is a number | `<span class="ctx-badge" role="meter" aria-valuenow={p} aria-valuemin={0} aria-valuemax={100} aria-valuetext={`Context ${p}% used`} aria-label="Context window used">` |
| `null` or missing | `<span class="ctx-badge unavailable">` with **no** `role`, **no** `aria-value*` |

Both carry the same `title` (§5.5's tooltip constant), the same `data-ac-testid`, `data-ac-role="status"`, and `data-ac-state` of `"reading"` or `"unavailable"`.

### 4.6 No thresholds, no colour semantics, no animation, no live region

**Decided: the badge is inert text.**

The issue permits threshold styling under constraints; it does not require it. I am shipping none, and the reason is not minimalism:

- **AC has no basis for a number.** The epic states the reading is *"one rounded integer, unknown denominator"*, with `context_window_size` unavailable to a scrape. Any threshold I pick is invented, and §10.4 of #1032 is a six-entry record of what inventing costs here.
- It makes "do not depend on colour alone" true by construction (there is no colour).
- It makes "honour reduced motion" true by construction (there is no motion, and no CSS file in the repo has ever needed the query).
- It makes "live regions only on significant transitions" true by construction (no transition is significant, and a 5s live region is an active defect, §2.11).

If the user wants a reminder level later, it is a clean follow-up: one class, one CSS block, and a threshold **the user chooses**, labelled an AgentsCommander reminder level.

### 4.7 The ProjectPanel search-text invariant gets a stated exception

`ProjectPanel.tsx:1118-1125` (#733/#515) documents a real invariant: *"a badge that is visible is always matchable"* by the sidebar filter, so the row render and `replicaSearchText` (`:1142`) cannot diverge. Repo badges honour it (`sessionRepoSearchText`, `:1108-1111`).

**Decided: `CTX` is NOT added to `replicaSearchText`, and this is an exception, not an oversight.** Every other badge in that row is stable identity (agent label, profile letter, repo/branch). This one changes every 5 seconds. Making it matchable would make a filtered list **add and drop rows on its own**, with no user input, as numbers tick. The invariant protects discoverability of identity; a volatile gauge is not identity.

---

## 5. Affected surfaces

### 5.1 `src/shared/types.ts`

| Where | Change |
|---|---|
| `AgentConfig`, after `backend?` at **`:225`** | `contextRegex?: string;` plus a doc comment: best-effort recognition pattern, **absent means the badge is off** (no fallback default, unlike `instructionsFilename`), stored verbatim and never trimmed (§2.6), Rust-`regex` grammar and not JS (§2.9). Mirrors Rust field order (`settings.rs:79-80`, before `backend`) |
| new, beside the `Session` types | `export interface SessionContextPayload { sessionId: string; percent: number \| null }`. **`percent: number \| null`, never `percent?: number`** (§5.5 of #1032, load-bearing; `mod.rs:73-76` is the reason) |
| `SessionsState`, after `lastActivityBySessionId` at **`:833`** | `contextPercentBySessionId: Record<string, number \| null>;` plus the §4.2 miss/null/`0` comment in `RepoDirtyByPath`'s style (`:40-58`) |

### 5.2 `src/shared/ipc.ts`

| Where | Change |
|---|---|
| `PtyAPI`, after `getScreenSnapshot` at **`:275-276`** | `getSessionContext: (sessionId: string) => transport.invoke<number \| null>("get_session_context", { sessionId }),` |
| after `onSessionBusy` at **`:773-777`** | `export function onSessionContext(callback: (data: SessionContextPayload) => void): Promise<UnlistenFn> { return transport.listen<SessionContextPayload>("session_context", callback); }` |

### 5.3 New files

| File | Contents |
|---|---|
| `src/sidebar/components/session-context.ts` | **Two exports.** (1) `contextBadgeConfigured(agents, agentId): boolean`, the one visibility gate for all three surfaces (§5.8). (2) `contextBadgeText(percent: number \| null \| undefined): string` → `` `CTX ${percent}%` `` or `"CTX N/A"`. **Pure. Total.** Doc comment mirroring `session-status.ts:12-15`: editing a value here changes pixels only; no behavior reads it. State plainly that it is deliberately **not** injective (`null` and `undefined` both map to `CTX N/A`, because unavailable is exactly one thing), and that the `percent === null \|\| percent === undefined` test is deliberate: `0` is a real reading (§4.2) |
| `src/sidebar/components/ContextBadge.tsx` | `Component<{ percent: number \| null \| undefined; testId?: string }>`, default export, §4.5's two elements. Docstring modelled on `ProfileOutdatedBadge.tsx`: cite #1033, name the three sharing surfaces, and say the badge is a signal and never a control (no `onClick`, no `<button>`, ever) |

### 5.4 `src/sidebar/components/SettingsModal.tsx` and its helpers

| Where | Change |
|---|---|
| `src/shared/profile-utils.ts`, after `defaultInstructionsFilename` at **`:499-505`** | Two exported constants and `suggestedContextRegex(command: string): string \| null`, reusing `executableTokenBasename` (`:452`) exactly as `defaultInstructionsFilename` does. **Returns `null` for anything that is not Claude or Codex** (see below) |
| `SettingsModal.tsx:33` | add `suggestedContextRegex` to the existing `profile-utils` import |
| `SettingsModal.tsx`, after the hint at **`:2206`** | the field, the suggest button, the hint (copy below) |
| `settings-save.ts`, beside the other normalizers (**`:15-60`**) | `normalizeAgentContextRegex`, **which does not trim** (§2.6) |
| `settings-save.ts:98` | add `.map(normalizeAgentContextRegex)` to the chain |

**The two constants, verbatim from #1032 §4.5 (do not edit, do not re-derive):**

```ts
export const CLAUDE_CONTEXT_REGEX = String.raw`^ {2}Context [░█]+ (\d{1,3})%`;
export const CODEX_CONTEXT_REGEX = String.raw`^ {2}.*· Context (\d{1,3})% used`;
```

**`suggestedContextRegex` returns `null` for Gemini and everything else, and that is a deliberate divergence** from `defaultInstructionsFilename`, which falls back to `"AGENTS.md"`. A filename fallback that is wrong costs a renamed file. A *pattern* fallback that is wrong costs a silently wrong percentage. #1032's capture covered `claude.exe` 2.1.212 and `codex.exe` 0.144.4 and nothing else; there is no evidence for a Gemini row, so no guess ships. No suggestion means the button does not render; the field still accepts anything the user types.

**The normalizer, and the one line that matters:**

```ts
/**
 * #1033 - normalize the optional context regex. Mirrors
 * normalizeAgentInstructionsFilename's CONTRACT (drop the key rather than persist
 * an empty-string sentinel, since Rust's skip_serializing_if = "Option::is_none"
 * does not omit Some("")) but deliberately NOT its trim.
 *
 * A regex's leading whitespace is load-bearing: a user who writes the column-2
 * anchor as two literal spaces instead of `^ {2}` has it silently deleted by a
 * trim, and the pattern then matches agent prose anywhere on the row. Measured
 * against regex 1.12.3: trimming turns the typed-in-the-input-box false positive
 * from None into Some(99) while the real statusline keeps reading correctly, so
 * the damage is invisible in every normal case. Whitespace-only is still dropped
 * (it cannot compile: no capture group), so the sentinel rule is unaffected.
 */
function normalizeAgentContextRegex(agent: AgentConfig): AgentConfig {
  if (agent.contextRegex && agent.contextRegex.trim()) return agent; // kept BYTE-FOR-BYTE
  const { contextRegex: _drop, ...rest } = agent;
  return rest;
}
```

**The markup**, on the `:2190-2202` template plus the `settings-add-btn` precedent (`:1890-1897`):

```tsx
<label class="settings-field">
  <span class="settings-label">Context badge pattern</span>
  <input
    class="settings-input"
    value={agent.contextRegex ?? ""}
    onInput={(e) => updateAgent(i(), "contextRegex", e.currentTarget.value)}
    placeholder={suggestedContextRegex(agent.command) ?? ""}
    data-ac-testid={`settings.agentRow.${i()}.contextRegex`}
    data-ac-role="textbox"
    spellcheck={false}
  />
</label>
<Show when={suggestedContextRegex(agent.command)}>
  {(suggested) => (
    <button
      class="settings-add-btn"
      onClick={() => updateAgent(i(), "contextRegex", suggested())}
      data-ac-testid={`settings.agentRow.${i()}.contextRegex.suggest`}
      data-ac-role="button"
    >
      Use suggested pattern
    </button>
  )}
</Show>
<div class="settings-hint">
  Best-effort pattern AC runs over what this agent draws in its terminal, to show
  a CTX badge on its sessions. Capture group 1 is the percentage. The reading can
  be unavailable, stale or absent, and a high one does not mean the session needs
  restarting. <strong>Leave blank for no badge.</strong>
</div>
```

**Copy rules the implementer may not quietly "improve":**

- **No "leave blank to use the default shown".** Blank is off (§2.7).
- **The button exists because the suggested pattern contains `░` and `█`, which cannot be typed** (§2.8). Do not delete it as decoration, and do not "simplify" the pattern to typeable characters: `\S+` reads `  Context is 42% of the budget` as `Some(42)`, measured.
- **No threshold, no limit, no Anthropic wording** anywhere in this copy.
- `updateAgent`'s signature is `(index, field: keyof AgentConfig, value)` (`:553-557`); the new field widens `keyof AgentConfig` automatically. Keying by **row index** here is correct and is not the thing #1031 warns about: the warning is against keying by `command`, and `"command": "claude"` matching two agents is exactly why `suggestedContextRegex(agent.command)` is fine (a many-to-one **default** lookup, explicitly legitimate) while the stored value stays per-row.

### 5.5 The badge tooltip, shared by both states

```
Context window in use, read from what the agent draws in its terminal.
Best-effort: it can be unavailable, stale or absent. A high reading does not
mean this session must be restarted.
```

Lives as one exported constant in `ContextBadge.tsx`. It is the accessible tooltip the issue requires and it is the only place the honesty requirement is user-visible, so it is not optional.

### 5.6 `src/sidebar/stores/sessions.ts`

| Where | Change |
|---|---|
| `:26`, after `lastActivityBySessionId: {}` | `contextPercentBySessionId: {},` |
| beside the `:439-441` getter | `get contextPercentBySessionId() { return state.contextPercentBySessionId; }` |
| beside `markActivity` (`:506-508`) | `setSessionContext(sessionId, percent)` and `hydrateSessionContext(sessionId, percent)` (§4.4; the latter returns early when the key exists) |

Neither setter touches `state.sessions`, so `setProfileOutdated`'s #592 warning (`:385-387`) does not apply and `setSessions` cannot reach them.

### 5.7 `src/sidebar/App.tsx`

| Where | Change |
|---|---|
| `:22-23` import block | add `onSessionContext` |
| after the `:654-665` listener block | §4.4's listener and hydration loop, `unlisteners.push(...)` exactly as its neighbours do |

### 5.8 The three render sites

| File:line at `d1b4a52` | Change |
|---|---|
| `SessionItem.tsx`, after `profile-badge` at **`:389`** | `<Show when={ctxVisible()}><ContextBadge percent={ctxPercent()} testId={`session.${props.session.id}.contextBadge`} /></Show>`. Resolvers go beside `sessionAgentLabel` (`:45-49`), which already reads `settingsStore.current?.agents?.find(a => a.id === props.session.agentId)` (`:48`), so `settingsStore` needs no new import (`:10`) |
| `RootAgentBanner.tsx`, after `profile-badge` at **`:416`** | same, keyed off `rootSession()` (`:35-37`); `agentLabel()` (`:70-76`) already has the identical lookup |
| `ProjectPanel.tsx`, after `profile-badge` at **`:2687`** | same, keyed off `session()` (`:2401`). **Do not touch `replicaSearchText`** (§4.7) |

**The visibility gate is ONE shared resolver, not three copies.** It lives in `session-context.ts` (§5.3) and takes its inputs as arguments, exactly as `profileDisplayLabel(cfg, agents, agentId, letter)` does, because the three surfaces reach their session three different ways: `props.session` in `SessionItem`, `rootSession()` in `RootAgentBanner`, `session()` in `ProjectPanel`. A gate written against `props.session` compiles in exactly one of them.

```ts
// #1033 - no regex configured for this session's agent => no badge at all. Not N/A,
// not a chip, no visual noise. One resolver for all three surfaces, mirroring #548's
// rule for the profile tooltip: no second resolver, or the three drift.
export function contextBadgeConfigured(
  agents: AgentConfig[] | undefined,
  agentId: string | null | undefined
): boolean {
  if (!agentId) return false;
  return !!agents?.find((a) => a.id === agentId)?.contextRegex?.trim();
}
```

Callers, one line each. `settingsStore.current` is a signal, so a save (which calls `settingsStore.refresh()`, `SettingsModal.tsx:1144`) makes the badge appear or disappear with no reload:

```ts
// SessionItem.tsx, beside sessionAgentLabel (:45-49)
const ctxVisible = () => contextBadgeConfigured(settingsStore.current?.agents, props.session.agentId);
const ctxPercent = () => sessionsStore.contextPercentBySessionId[props.session.id];

// RootAgentBanner.tsx, beside agentLabel (:70-76)
const ctxVisible = () => contextBadgeConfigured(settingsStore.current?.agents, rootSession()?.agentId);
const ctxPercent = () => { const r = rootSession(); return r ? sessionsStore.contextPercentBySessionId[r.id] : undefined; };

// ProjectPanel.tsx, beside liveAgentLabel/profileBadge (:2494-2495)
const ctxVisible = () => contextBadgeConfigured(settingsStore.current?.agents, session()?.agentId);
const ctxPercent = () => { const s = session(); return s ? sessionsStore.contextPercentBySessionId[s.id] : undefined; };
```

**The `.trim()` here is a visibility test only and never touches a stored or transmitted value** (§2.6 forbids that). It exists so a legacy file hand-edited to `"contextRegex": "   "` does not show a permanent `N/A`.

**A missing key reads `undefined`, and that is why `ContextBadge` accepts `number | null | undefined`** rather than `number | null`: `contextPercentBySessionId[id]` on an unhydrated session is `undefined`, not `null`, and §5.3's projection collapses both to `CTX N/A` (§4.2).

All three sit inside their surface's existing meta row, so the badge inherits its gates: it is hidden during voice recording (`SessionItem.tsx:367`, `RootAgentBanner.tsx:396`), exactly like `agent-badge` and `profile-badge` are today. That is a pre-existing cosmetic quirk of the row and this plan does not change it.

### 5.9 `src/sidebar/styles/sidebar.css`

One block after `.profile-badge` (**`:745-759`**), copying its shape (`inline-flex`, `min-width`, `padding: 0 5px`, `border-radius: 3px`, `font-size: 9px`, `line-height: 1`, `white-space: nowrap`) with `.ctx-badge` on the neutral `var(--sidebar-border)` / `var(--sidebar-fg-dim)` pair `.agent-badge` (`:729-738`) already uses, and `.ctx-badge.unavailable` at reduced opacity. **No `@keyframes`, no `transition`, no colour that encodes a value** (§4.6). `.session-item-meta` is already `flex-wrap: wrap; gap: 6px` (`:676-683`), so it needs no change.

---

## 6. Required behavior, edge cases, failure behavior

### 6.1 Required

- **`CTX N/A` when there is no reading. Never `0` for unknown.** `0` renders `CTX 0%` because it is a real reading (§4.2, and §6.3 of #1032 pins why the engine cannot tell a true `0%` from claude-hud's null fallback).
- **The badge is a signal, never a control.** `<span>`, never `<button>`. No `onClick`, no `onKeyDown`, no `tabindex`, no cursor change. Nothing reads the projection.
- **No regex configured, no badge at all.**
- **Accept decreases.** No monotonicity, no smoothing, no "Compacting" inferred from a fall or an absence.
- **The stored pattern is byte-for-byte what the user typed** (§2.6).

### 6.2 Edge cases

| Case | Behavior | Why |
|---|---|---|
| No regex configured | **No badge** | §5.8's gate |
| Regex configured, no event yet | `CTX N/A` | key missing (§4.2) |
| Engine says unavailable | `CTX N/A` | explicit `null` |
| Real reading of `0` | **`CTX 0%`** | `0` is a value, not an absence (§4.2) |
| Reading drops 80 → 12 (post-compact) | `CTX 12%` | decreases accepted |
| Sidebar window reloads while a session sits at 42% | **`CTX 42%`** | hydration (§4.4); **without it, `N/A` forever** (§2.3) |
| Snapshot resolves after a fresher event | Event wins | `hydrateSessionContext` no-ops on an existing key (§4.4) |
| Regex cleared and saved | Badge disappears | engine emits `null` once **and** the gate goes false; both agree |
| Regex invalid or has no capture group | `CTX N/A`, reason in `app.log` only | §2.9, frozen by §5.5 |
| Pattern with literal leading spaces | Works | the normalizer does not trim (§2.6) |
| Two concurrent sessions | Independent | map keyed by session id |
| Plain shell (`agentId === null`) | No badge, not hydrated | never registered by the backend |
| Inactive placeholder rows (`id` starts `inactive-`) | No badge | `makeInactiveEntry` (`sessions.ts:57-60`) has no `agentId` |
| Session ends | Its key is stale but the row is gone | no cleanup needed; bounded by sessions per window lifetime, same as `lastActivityBySessionId` |
| Voice recording on the row | Whole meta row hides, badge with it | pre-existing (`SessionItem.tsx:367`) |
| Screen reader | Reads text plus `aria-valuetext`; **never interrupts** | no live region (§2.11) |

### 6.3 Failure behavior

Every failure renders `CTX N/A` and nothing else. A rejected pattern, a dead engine, an unmanaged scraper (`commands/pty.rs:459-461` returns `Ok(None)`), a failed `getSessionContext` (wrap it so a rejected promise cannot break the rest of `onMount`, exactly as `App.tsx:564-567` and `SettingsModal.tsx:1146-1149` already `try {} catch {}` their fan-outs), a session the engine never registered: all indistinguishable, all `N/A`, all harmless.

---

## 7. Compatibility and security impact

### Compatibility

- **No backend change. No IPC breaking change.** The event and command already ship.
- **Existing settings files load unchanged** and gain no key: `contextRegex` is optional in TS, `#[serde(default, skip_serializing_if)]` in Rust, and the normalizer drops it when unset.
- **`SettingsModal.automation.test.ts` cannot break on the new row.** Verified: the file selects **only** by `data-ac-testid` and contains no `querySelectorAll`, no `toHaveLength`, no `.length ===`, and no positional or `nth` selector. A new testid is invisible to it. Run it anyway (§8).
- **`Session` is untouched**, so nothing that consumes `SessionAPI.list()` moves.

### Security

- **The regex is user-supplied and is only ever a regex**, compiled by the already-shipped backend under a 1 MiB limit with no backtracking (`pattern.rs:27-30`). #1033 neither compiles nor executes it.
- **No agent output reaches the frontend through this path.** The payload is one `u8` or `null`. #1032 deliberately keeps captured text out of `app.log`; this plan keeps it out of the DOM by never receiving any.
- **The reading is never persisted.** It lives in a store map for the window's lifetime.
- **`aria-valuetext` and the badge text interpolate a `number`,** never a string from the agent, so there is no injection surface in the tooltip or the meter.

---

## 8. Implementation order

1. **`types.ts`**: `contextRegex`, `SessionContextPayload`, `contextPercentBySessionId`. Inert.
2. **`ipc.ts`**: `getSessionContext`, `onSessionContext`. Inert.
3. **`profile-utils.ts`**: the two constants and `suggestedContextRegex`, plus §9.1. **Gate: the constants must be byte-identical to §5.4. Diff them against this plan, do not retype them.**
4. **`session-context.ts`**: `contextBadgeText`, plus §9.2.
5. **`settings-save.ts`**: `normalizeAgentContextRegex` and the chain entry, plus §9.3. **Gate: §9.3's untrimmed test must be red if `.trim()` is added to the kept value.**
6. **`SettingsModal.tsx`**: the field, the button, the hint. **Gate: `SettingsModal.automation.test.ts` green.**
7. **`sessions.ts`**: the state key, getter, and the two setters, plus §9.4.
8. **`ContextBadge.tsx`** + **`sidebar.css`**, plus §9.5.
9. **`App.tsx`** + the three sites, plus §9.6.

Steps 1 to 5 are invisible. The field becomes usable at 6. The badge first appears at 9.

---

## 9. Tests and objective acceptance criteria

**Fixture discipline, inherited and non-negotiable.** Row fixtures are **copied from #1032's capture** (§2.11 of that plan), never hand-written. **No fixture may contain `▓`** (U+2593): it never occurs in the real output, and the issue's own probe fixtures used it and tested fiction. The real glyphs are `░` (U+2591) and `█` (U+2588).

### 9.1 `profile-utils.test.ts`

| Test | Asserts |
|---|---|
| `the_claude_suggestion_is_the_captured_pattern` | `suggestedContextRegex("claude")` is byte-identical to `CLAUDE_CONTEXT_REGEX`, and the string **contains `░` and `█` and no `▓`** |
| `the_codex_suggestion_is_the_captured_pattern` | same for `codex` |
| `a_docker_wrapped_claude_still_resolves` | a `command` whose basename resolves through `executableTokenBasename` still returns the Claude pattern, mirroring `defaultInstructionsFilename`'s own cases |
| **`an_unknown_agent_gets_no_suggestion`** | `suggestedContextRegex("gemini")` and `("nonsense")` are **`null`**, not `AGENTS.md`-style fallbacks. **Pins §5.4's deliberate divergence**: a wrong pattern is a wrong number, not a renamed file |

### 9.2 `session-context.test.ts`, pure

| Test | Asserts |
|---|---|
| `a_reading_renders_as_a_percentage` | `contextBadgeText(42)` → `"CTX 42%"` |
| **`zero_is_a_reading_not_an_absence`** | `contextBadgeText(0)` → **`"CTX 0%"`**. The highest-value test in this file: it is the only thing that stays red if anyone writes `percent ? … : "CTX N/A"` |
| `null_and_undefined_both_render_unavailable` | both → `"CTX N/A"`. Pins that unavailable is exactly one thing |
| `the_projection_is_total` | every value in `[0, 1, 50, 99, 100, null, undefined]` returns a non-empty string and never throws |

And the visibility gate, in the same file because it is the same module (§5.8):

| Test | Asserts |
|---|---|
| `a_configured_agent_is_visible` | `contextBadgeConfigured([{id:"a", contextRegex:"x", …}], "a")` → `true` |
| `an_agent_with_no_regex_is_not_visible` | same agent without `contextRegex` → `false` |
| **`a_whitespace_only_regex_is_not_visible`** | `contextRegex: "   "` → `false`. The hand-edited-file case (§5.8); without it that agent shows a permanent `N/A` |
| `a_null_agentId_is_not_visible` | `contextBadgeConfigured(agents, null)` → `false`. Plain shells |
| `an_unknown_agentId_is_not_visible` | an id absent from `agents` → `false`, no throw |
| **`the_gate_keys_by_id_and_not_by_command`** | two agents sharing `command: "claude"`, only one with a `contextRegex` → `true` for that id, **`false` for the other**. Pins #1031's hard rule at the only place #1033 could break it |

### 9.3 `settings-save.test.ts`

| Test | Asserts |
|---|---|
| `a_set_pattern_round_trips` | a normal pattern survives the merge unchanged |
| **`a_pattern_with_literal_leading_spaces_is_not_trimmed`** | `"  Context [░█]+ (\d{1,3})%"` persists **byte-for-byte**, leading spaces intact. **Red the moment anyone copies #529's `.trim()`.** The single most important test in this plan (§2.6) |
| `an_empty_pattern_is_dropped_not_persisted_as_a_sentinel` | `contextRegex: ""` → the key is **absent** from the merged agent |
| `a_whitespace_only_pattern_is_dropped` | `"   "` → key absent |
| `an_agent_without_the_field_is_unchanged` | no key in, no key out; nothing gains a sentinel |

### 9.4 `sessions` store

| Test | Asserts |
|---|---|
| `an_event_sets_a_sessions_reading` | `setSessionContext(id, 42)` → map holds `42` |
| `two_sessions_never_cross` | two ids, two values, independent |
| `a_null_is_stored_as_null_not_dropped` | `setSessionContext(id, null)` → the key exists and holds `null` |
| **`hydrate_never_clobbers_a_value_an_event_already_set`** | `setSessionContext(id, 43)` then `hydrateSessionContext(id, 42)` → still **`43`** (§4.4) |
| `hydrate_seeds_a_session_no_event_has_spoken_for` | `hydrateSessionContext(id, 42)` on an empty map → `42` |
| **`hydrate_treats_a_stored_zero_as_spoken_for`** | `setSessionContext(id, 0)` then `hydrateSessionContext(id, 42)` → still **`0`**. Red if the guard is a truthiness check instead of a key-presence check |
| `setSessions_cannot_wipe_a_reading` | set a reading, call `setSessions([...])`, reading survives (§2.4/§4.2) |

### 9.5 `ContextBadge.test.tsx`

| Test | Asserts |
|---|---|
| `a_reading_is_a_meter_with_a_value` | `role="meter"`, `aria-valuenow="42"`, `aria-valuemin="0"`, `aria-valuemax="100"`, `aria-valuetext` present, text `CTX 42%` |
| **`the_unavailable_state_is_not_a_meter`** | `percent={null}` → **no `role`**, **no `aria-valuenow`**. Pins §2.11: a meter without a value is invalid ARIA |
| **`the_badge_is_not_a_control`** | rendered element is not a `<button>`, has **no** `onclick`, no `tabindex`, no `href`. Pins #1031's one hard rule at the DOM |
| `both_states_carry_the_tooltip` | `title` is the §5.5 constant in both states |
| `no_live_region` | **no `aria-live`** on either state (§2.11) |

### 9.6 Wiring

| Test | Asserts |
|---|---|
| `no_regex_configured_renders_no_badge` | agent session, agent has no `contextRegex` → **the testid is absent entirely**. Not `N/A`, not an empty chip |
| `a_configured_agent_with_no_reading_renders_N_A` | regex set, no event → `CTX N/A` |
| `an_event_paints_the_badge` | regex set, emit `{sessionId, percent: 42}` → `CTX 42%` |
| **`an_event_with_percent_null_paints_N_A`** | `{sessionId, percent: null}` → `CTX N/A`, **not** `CTX 0%` |
| `the_badge_appears_when_a_regex_is_saved` | agents updated in `settingsStore` → badge appears with no reload (`SettingsModal.tsx:1144`) |
| `two_concurrent_sessions_show_independent_values` | two rows, two values, no bleed |

### 9.7 Acceptance criteria, objective

1. Regex round-trips: set, save, reload shows it; cleared, the key is **absent** from the persisted JSON.
2. A settings file with no `contextRegex` loads unchanged and saves with no sentinel.
3. A pattern with leading spaces is stored with its leading spaces.
4. No regex, no badge, no `N/A`, no chip.
5. A session with a reading shows `CTX N%`; one without shows `CTX N/A`; a true `0` shows `CTX 0%`.
6. Neither state is clickable and neither triggers anything.
7. Two concurrent sessions are independent.
8. **A sidebar reload against a live session at 42% shows `CTX 42%`, not `N/A`** (§2.3; the criterion the issue does not have and the feature is broken without).
9. Clicking **Use suggested pattern** on a Claude row fills the field with a pattern containing `░`/`█` that the user could not have typed.
10. `SettingsModal.automation.test.ts` green. `npm test` and `npx tsc --noEmit` green.

**Verification note (`npm test`).** Get a baseline count on `main` **before** blaming this branch for any red. The Rust suite here has a known flake population; treat the TS suite with the same suspicion and compare k/n on both sides.

---

## 10. Verdict

### Status: READY_FOR_IMPLEMENTATION

Authored and certified in one pass, as dispatched. Every coordinate re-anchored against `d1b4a52` by me. Every claim about the pattern grammar, the trim and the glyphs came from a probe against `regex` 1.12.3; the two findings that reading caught (§10.1 items 2 and 6) are marked as such, because which instrument found which is the useful part of the record.

### 10.1 What checking changed, and which instrument caught which

| # | The instruction or assumption | What checking it said | Caught by |
|---|---|---|---|
| 1 | "copy the #529 normalizer" (`settings-save.ts:15-20`) | **Trap.** Its trim turns the typed-in-box false positive from `None` into `Some(99)` while the real statusline still reads correctly. Fails **open**, invisibly (§2.6) | probe |
| 2 | "placeholder = derived default", hint "leave blank to use the default shown" | **Trap.** Blank is **off** here; there is no fallback pattern in the backend. The copy inverts (§2.7) | reading `mod.rs` |
| 3 | "#1032 §4.5 records both patterns verbatim, use them" | **Right, and now I know why.** `[░█]` rejects `  Context is 42% of the budget`; `\S+` reads it as `Some(42)`. But the pattern **cannot be typed**, so it needs the suggest button or the feature is unconfigurable for Claude (§2.8) | probe |
| 4 | (unstated) validate the regex before saving | **Impossible in JS.** Wrong in both directions, measured on both sides (§2.9) | probe, both sides |
| 5 | #1043: "#1032 §4.5's reason for the Codex pattern is wrong" | **Confirmed, and sharper.** Either literal alone pins the capture; strip both and greedy `.*` captures `3` out of `83` (§2.10) | probe |
| 6 | "mirror the #882 path exactly" | **Insufficient.** #882's value rides on `Session` from the backend; this one does not, so a listener alone leaves a reloaded sidebar at `N/A` forever (§2.3, §4.4) | **reading the frozen backend** |

**Item 6 is the one I would have shipped, and the instrument column is the point: a probe did not catch it. Reading did.**

That is not a rebuttal of #1032's §10.4 (*"reading the crate carefully caught none of the six"*), it is the other half of the same lesson. #1032's six were **mechanisms built on unverified claims about an external thing** (a crate, a TUI, a PTY), and only running the real thing could refute them. Item 6 is the opposite kind: a claim about **our own frozen contract's emission semantics**, sitting in plain text in `mod.rs:190` and in #1032's own §4.1.1 table (*"Unchanged → no emit"*). No probe was needed and none would have been aimed there, because the thing that hid it was not complexity. It was the word **frozen**.

"The backend contract is frozen and shipped, mirror it" is an invitation to read the *types* and skip the *behaviour*. The types are perfect: `percent: number | null` says everything about what a payload means and nothing about **when one arrives**. I mirrored #882 correctly, against a contract I had verified by hash, and would have shipped a badge that reads `N/A` forever on a window reload.

**The transferable rule, and it is the one I would defend without evidence: a frozen contract still has to be read for when it speaks, not just what it says.** Match the instrument to the claim. External thing, unverified claim: probe it. Our own contract, and an assumption you did not notice you made: read it, and read the parts the summary told you not to worry about.

### 10.2 Lite holds, and here is the bar it was checked against

- **Both halves have an exact named precedent.** Field: #529 (`SettingsModal.tsx:2190-2202`) plus its normalizer's contract. Badge: #882 (`session_idle` → `ipc.ts` → `App.tsx` → store → projection → CSS), `ProfileOutdatedBadge.tsx` for the shared component, `lastActivityBySessionId` for the state, `RepoDirtyByPath` for the miss/null discipline, `sessionProfileBadge` for a resolver across the same three sites.
- **No new abstraction, dependency, schema or protocol.** No dependency. The contract is frozen and shipped. The store key copies a store key that exists.
- **The backend contract is frozen and shipped**, verified by blob hash.

**It is lite, and it is not a copy job.** Six instructions or assumptions needed checking; three were traps that ship a defect if followed (§10.1 items 1, 2 and 6), one was a correct instruction with an unusable consequence (item 3), one was an idea that had to be killed (item 4), and one was a peer's correction that held (item 5). I am flagging the gap between "lite" and "mechanical" rather than letting `READY_FOR_IMPLEMENTATION` imply the second.

### 10.3 What I deliberately did not build

- **No thresholds.** AC has no denominator and therefore no basis for a number (§4.6). A follow-up with a **user-chosen** level, labelled an AgentsCommander reminder level, is clean.
- **No regex validation** (§2.9). If the user finds "I pasted a pattern and nothing happened" as bad as #1032 §5.5 predicts, the fix is a backend `validate_context_regex` command returning `pattern.rs:36`'s existing `Result<_, String>`, which is a Rust issue and not this one.
- **No terminal-window badge.** Second window, second store, second listener path.
- **`CTX` is not in `replicaSearchText`** (§4.7), a stated exception to #733/#515 because a 5s-volatile value would make a filtered list rearrange itself with no user input.

### 10.4 The one thing I want the implementer to carry

**`0` is a reading and `N/A` is not a `0`.** Three separate places can destroy that with a single idiomatic character: `percent ? …` in the projection, `||` instead of `??` at a call site, and a truthiness guard in `hydrateSessionContext`. All three keep every normal case working and are invisible on screen, which is exactly the shape of the trim defect in §2.6. §9.2, §9.4 and §9.6 exist to be red when any of them happens; do not "simplify" them green.
