# Plan #2030 — the locked-replica sidebar chip renders the lock icon only

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2030
- Band: 1 to 25. Delivery path: Lite (no `Plan-SHA256` freeze in this band).
- Repository: `repo-AgentsCommander`
- Base: `main` @ `33504845e46ada38d0d3685f71472402d22c3e51`
- Branch: `fix/2030-lock-chip-icon-only` (created and pushed from `origin/main` `33504845`)
- Implementation owner: `ac-dev-webpage-ui-v4`
- Adversarial reviewer: `ac-dev-rust-grinch-v4`
- This plan is committed on its own on the branch; `plans/` is gitignored, so that commit needs `git add -f`.

Every line number, fence and grep count below was read at `33504845e46ada38d0d3685f71472402d22c3e51`. The working tree is CRLF and the blobs are LF (`core.autocrlf=true`); every fence below shows the working-tree form.

## 1. Objective

On a replica whose `selectionState === "locked"`, the sidebar tile's `.selection-lock-chip` must show the padlock and nothing else: no `KEEP` text node, no second child. The chip stays a passive, accessible, hover-explained badge, and it gets smaller — that is the point of the change (the user asked to save space).

The chip is rendered once, inside `renderReplicaItem`, so one change reaches all three contexts that draw replica rows: the workgroups section (`ProjectPanel.tsx:3013` → `:2711`), the selected section (`:2953` → `:2711`), and the quick-access strip (`:2924`).

Unchanged by design: the render gate (`selectionState === "locked"`), the `.selection-lock-chip` class, the `title` text, the `data-ac-testid`, the amber border/background/color, the 8px glyph, and the row's chip order.

## 2. Cause — the label, and the space that exists to hold it

`src/sidebar/components/ProjectPanel.tsx:2602-2614` today:

```tsx
                  {/* #1943 - KEEP chip for a locked replica. Only an established
                      `locked` state renders it, so an unknown or invalid state is
                      never drawn as unlocked. The same helper covers every
                      renderReplicaItem call site (workgroups, selected, quick). */}
                  <Show when={replica.selectionState === "locked"}>
                    <span
                      class="selection-lock-chip"
                      title={selectionLockChipTitle(replica, settingsStore.current)}
                      data-ac-testid={lockChipTestId()}
                    >
                      <LockIcon />KEEP
                    </span>
                  </Show>
```

`KEEP` at `:2612` is a bare text node. It is the only reason the chip needs `gap: 3px` to separate icon from label and `padding: 0 5px` around the pair. `src/sidebar/styles/sidebar.css:5030-5056` today:

```css
/* ── #1943 selection lock: KEEP chip, lock bar, conflict review ──
   Ported from the approved v3 stylistic reference
   (.ac/plans/issue_1937/prototypes/index-v3.html, section "Candado de
   selección"). Amber marks protection everywhere; green marks the eligible
   set. Nothing here changes an unrelated theme or class. */

/* The sidebar KEEP chip: only a locked replica renders it. */
.selection-lock-chip {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  height: 16px;
  padding: 0 5px;
  border-radius: 3px;
  border: 1px solid rgba(227, 179, 65, 0.55);
  background: rgba(227, 179, 65, 0.14);
  color: #e3b341;
  font-size: 9px;
  font-weight: 700;
  line-height: 1;
  white-space: nowrap;
}

.selection-lock-chip svg {
  width: 8px;
  height: 8px;
}
```

Deleting the four characters alone would leave a 20px-wide chip with the glyph floating in it, so the label's own spacing declarations go with it. Nothing else is a cause; each moving part has exactly one site:

- `selectionLockChipTitle` — `grep -rn "selectionLockChipTitle" src/` reports 2 lines: the helper `ProjectPanel.tsx:170` and its only call `:2609`.
- `.selection-lock-chip` — `grep -rn "selection-lock-chip" src/` reports 8 lines: the class `ProjectPanel.tsx:2608`, the rule `sidebar.css:5037`, its svg rule `sidebar.css:5053`, and 5 uses inside `ProjectPanel.chip-order.test.tsx` (`:270`, `:277`, `:305`, `:332`, `:388`). No other component or stylesheet touches the chip.
- The word `KEEP` in the sidebar — 13 lines: the render `ProjectPanel.tsx:2612`; five comments (`AgentPickerModal.tsx:39`, `ProjectPanel.tsx:167`, `ProjectPanel.tsx:2602`, `sidebar.css:5030`, `sidebar.css:5036`); five lines in `ProjectPanel.chip-order.test.tsx` (`:248`, `:252`, `:279`, `:315`, `:333`); and two comments in fixtures that are not otherwise modified (`ProjectPanel.modal-refresh.test.tsx:118`, `ProjectPanel.restart-prompt.test.tsx:138`). The only other `KEEP` strings in the sidebar directory are `KEEPALIVE_MS` (`non-stop-watchdog-client.ts:9`, `:54`) and two "KEEPS" test titles in `WorkgroupGroupRail.favorites.test.tsx`; all four are unrelated and stay.
- `LockIcon` — `grep -rn "LockIcon" src/` reports 6 lines: the definition `AgentPickerModal.tsx:42` (its svg already carries `aria-hidden="true"`), three uses inside the picker (`:1461`, `:1733`, `:1761`), and in the sidebar only the import `ProjectPanel.tsx:64` and the chip `:2612`.
- No Rust or crate code knows about the chip: `grep -rn "KEEP" src-tauri/src crates --include=*.rs` matches only unrelated retention constants and prose (`ACTIVITY_KEEP`, "KEEPS").
- `ProjectPanel.order-lock.test.tsx` and `App.order-lock.test.tsx` never mention a chip (`grep -rin "chip"` on them is empty), so they need no change.

## 3. In scope and out of scope

In scope:

- `src/sidebar/components/ProjectPanel.tsx`: the chip's children, the doc comment on `selectionLockChipTitle`, the chip's comment, and one new label accessor.
- `src/sidebar/styles/sidebar.css`: the chip's `gap` and horizontal `padding`, plus the two comments that name the chip.
- `src/sidebar/components/ProjectPanel.chip-order.test.tsx`: the assertions and titles that pin the word `KEEP`.
- `src/sidebar/components/AgentPickerModal.tsx`, `ProjectPanel.modal-refresh.test.tsx`, `ProjectPanel.restart-prompt.test.tsx`: comments only, so the term stops pointing at a label that no longer exists.

Out of scope (a reviewer should reject the PR if any of these appear):

- The picker's own lock UI (`AgentPickerModal.tsx:1461/1733/1761`), `.selection-lock-bar`, the conflict-review panel, and every lock behaviour (gate, saved pair, persistence, bulk apply).
- Any other chip or badge, any other CSS rule, any theme or token, any new component, class modifier, or i18n key.
- `src-tauri/`, stores, transports, and shared types.
- The chip's color, border, radius, height and icon size stay as they are. `font-size`, `font-weight`, `line-height` and `white-space` stay in the rule too: they are inert for an svg and cost nothing, and removing them is unrelated cleanup.

## 4. Decided solution

### 4.1 `ProjectPanel.tsx` — icon-only chip behind one label accessor

Add one accessor next to `lockChipTestId` (`:2448-2449`) so the tooltip and the accessible name cannot drift:

```tsx
          const lockChipTestId = () =>
            `replica.lockChip.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`;
          // #2030 - the chip is icon-only, so its tooltip and its accessible name
          // come from the same accessor; the glyph itself is aria-hidden.
          const lockChipLabel = () => selectionLockChipTitle(replica, settingsStore.current);
```

The helper's doc comment (`:167-169`) becomes:

```tsx
/** #1943 - lock chip label: the tooltip and, since #2030 made the chip
 *  icon-only, the accessible name. Reads the SAVED pair, never the session's
 *  launch-time pair, and falls back to the stored identifier when the provider
 *  is no longer configured instead of dropping the fact. */
```

The chip (`:2602-2614`) becomes exactly:

```tsx
                  {/* #1943 - lock chip for a locked replica; icon-only since #2030.
                      Only an established `locked` state renders it, so an unknown
                      or invalid state is never drawn as unlocked. The same helper
                      covers every renderReplicaItem call site (workgroups,
                      selected, quick). */}
                  <Show when={replica.selectionState === "locked"}>
                    <span
                      class="selection-lock-chip"
                      role="img"
                      aria-label={lockChipLabel()}
                      title={lockChipLabel()}
                      data-ac-testid={lockChipTestId()}
                    >
                      <LockIcon />
                    </span>
                  </Show>
```

`role="img"` is a decision, not decoration: ARIA 1.2 does not allow a name on the `generic` role, which is what a bare `<span>` maps to, so a lone `aria-label` there is not a supported name source. `role="img"` supports it and exposes the chip as one atomic piece of content. `title` stays for the mouse tooltip, so pointer and AT users read the same sentence. No `tabindex`, no `aria-live`, no handler: the chip is a passive badge and must not enter the tab order.

### 4.2 `sidebar.css` — drop the label's spacing

`gap: 3px` (`:5040`) and `padding: 0 5px` (`:5042`) exist only to space the label; with the label gone they are dead space. `padding: 0 3px` remains so the 8px glyph keeps symmetric breathing room inside its 1px border. With `* { box-sizing: border-box }` (`sidebar.css:6`) and `height: 16px`, the chip becomes a 16px square — the same height it has today — holding an 8px glyph. Today's chip is 8px icon + 3px gap + ~22px of 9px/700 text + 10px padding + 2px border, i.e. roughly 45px wide; after the change it is 16px, so a crowded row recovers about 29px.

Post-state of `:5030-5051`:

```css
/* ── #1943 selection lock: lock chip, lock bar, conflict review ──
   Ported from the approved v3 stylistic reference
   (.ac/plans/issue_1937/prototypes/index-v3.html, section "Candado de
   selección"). Amber marks protection everywhere; green marks the eligible
   set. Nothing here changes an unrelated theme or class. */

/* The sidebar lock chip: only a locked replica renders it. Icon-only since
   #2030, so the label's gap and padding are gone. */
.selection-lock-chip {
  display: inline-flex;
  align-items: center;
  height: 16px;
  padding: 0 3px;
  border-radius: 3px;
  border: 1px solid rgba(227, 179, 65, 0.55);
  background: rgba(227, 179, 65, 0.14);
  color: #e3b341;
  font-size: 9px;
  font-weight: 700;
  line-height: 1;
  white-space: nowrap;
}
```

The `.selection-lock-chip svg` rule (`:5053-5056`) is unchanged.

### 4.3 Three comments that still name the old label

- `AgentPickerModal.tsx:39`: "the sidebar KEEP chip draws the same shape" → "the sidebar lock chip draws the same shape".
- `ProjectPanel.modal-refresh.test.tsx:118`: "no KEEP chip is drawn" → "no lock chip is drawn".
- `ProjectPanel.restart-prompt.test.tsx:138`: "no KEEP chip and an actionable lock row" → "no lock chip and an actionable lock row".

The `ProjectPanel.tsx` and `sidebar.css` comments are covered by 4.1 and 4.2.

## 5. Affected files and symbols

Modify — 6 files, no new file, no deletion:

1. `src/sidebar/components/ProjectPanel.tsx` — `selectionLockChipTitle` doc comment (`:167-169`); new `lockChipLabel` accessor after `lockChipTestId` (`:2448-2449`); chip comment and markup (`:2602-2614`).
2. `src/sidebar/styles/sidebar.css` — `.selection-lock-chip` rule and its two comments (`:5030-5051`).
3. `src/sidebar/components/ProjectPanel.chip-order.test.tsx` — comment `:248`, test title `:252`, assertions `:277-287`, test title `:315`, deleted line `:333`.
4. `src/sidebar/components/AgentPickerModal.tsx` — comment `:39` only.
5. `src/sidebar/components/ProjectPanel.modal-refresh.test.tsx` — comment `:118` only.
6. `src/sidebar/components/ProjectPanel.restart-prompt.test.tsx` — comment `:138` only.

Add: `plans/2030-lock-chip-icon-only.md` (this plan, committed alone before the code). No test file is added.
Remove: no file. One assertion line is deleted inside `ProjectPanel.chip-order.test.tsx` (§7.2).

## 6. Required behaviour, edge cases, failure behaviour

Required behaviour:

- Locked replica → the chip renders with exactly one child (the `aria-hidden` svg), its text is whitespace only, and it carries `role="img"`, `aria-label` equal to `title`, and the unchanged `data-ac-testid`.
- Unlocked or unknown `selectionState` → no chip at all, exactly as today.

Edge cases:

- `savedPair: null` → the chip still renders (the lock is a fact about the replica) and the helper's fallback string becomes both tooltip and accessible name. Logic unchanged.
- Unresolvable provider id → the existing fallback (`retired-agent · Profile B`) becomes the accessible name too.
- All three row contexts (workgroups, selected, quick) → shared `renderReplicaItem` body, so one edit covers them.
- Crowded or narrow sidebar → the strip is `display: flex; gap: 4px; flex-wrap: wrap` (`sidebar.css:6737-6741`); a 16px chip is 4px narrower than the old padding-and-gap floor and ~29px narrower than today, so it can only reduce wrapping.
- Assistive technology → previously the visible word `KEEP` was read and the `title` was at best a description; now one stable name (`Protected from bulk changes · …`) is exposed to every AT. No information is lost and no new focus stop appears.
- Locale, direction, zoom and DPI → no text and no directional CSS is involved; the glyph stays an 8px px-sized svg.

Failure behaviour: none new. No IPC, no store write, no persistence, no async path and no error branch is touched.

## 7. Tests

No new test file and no new test case. `src/sidebar/components/ProjectPanel.chip-order.test.tsx` holds 6 tests and is the only suite that asserts the chip's content:

1. `"orders every chip of a maximal quick-access coordinator row"` (`:176`, 12 children) — untouched.
2. `"inserts the KEEP chip at index 7 …"` (`:252`) — the only test whose assertions change, per §7.1.
3. `"falls back to the stored provider id …"` (`:290`) — asserts `title` only; passes unchanged and remains the guard that the label helper still runs.
4. `"draws no KEEP chip …"` (`:315`) — one title correction and one deletion, per §7.2.
5. `"lands the auto-closed pill at index 2 …"` (`:338`) — untouched.
6. `"renders the chip as the only child on a worker row …"` (`:362`) — untouched.

### 7.1 Test 2: pin the icon-only content, and the name that replaces the text

The comment `:248-251` becomes:

```tsx
  // #1943 - the lock chip (icon-only since #2030): one more child than the
  // unlocked row above, inserted right after the profile badge, with every later
  // badge pushed back one slot and keeping its relative order. The chip strip IS
  // the row's only identity, so a silent insertion here would be invisible to
  // typecheck and to CI.
```

The title `:252` becomes `"inserts the icon-only lock chip at index 7 on a locked row and shifts the later badges"`.

`:277-288` becomes:

```tsx
    const chips = strip.querySelectorAll<HTMLElement>(".selection-lock-chip");
    expect(chips).toHaveLength(1);
    // #2030 - icon only: no text node and a single child, so no label can wrap
    // or widen the chip. The padlock is still announced: the svg is hidden and
    // the chip carries the name.
    expect(chips[0].textContent?.replace(/\s+/g, "")).toBe("");
    expect(chips[0].children).toHaveLength(1);
    expect(chips[0].querySelector("svg")).not.toBeNull();
    expect(chips[0].querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    expect(chips[0].getAttribute("role")).toBe("img");
    // The title names the SAVED pair, resolved through the configured label -
    // never the session's launch-time pair - and the accessible name repeats it.
    expect(chips[0].getAttribute("title")).toBe(
      "Protected from bulk changes · Codex · Profile B",
    );
    expect(chips[0].getAttribute("aria-label")).toBe(chips[0].getAttribute("title"));
    expect(children[7].getAttribute("data-ac-testid")).toBe(
      `replica.lockChip.quick.${automationIdPart(wgName)}.${automationIdPart(coordName)}`,
    );
  });
```

### 7.2 Test 4: delete one assertion that the change makes vacuous

`:333` is `expect(strip.textContent).not.toContain("KEEP")`. Once the label is removed, that string can never appear, so the assertion can never fail. The line above it — `:332`, `expect(strip.querySelector(".selection-lock-chip")).toBeNull()` — already pins the stronger fact that no chip element exists. Delete `:333`; keep the comment at `:313-314` and drop the word `KEEP` from the title at `:315`, which becomes `"draws no lock chip, and no unlocked claim, for a state this build does not know"`.

## 8. Acceptance criteria

Objective and checkable. All commands run from the repository root on `fix/2030-lock-chip-icon-only` with a clean tree.

Base dependency: AC 1-3 pin assertions, not absolute line numbers, but their text is quoted from `33504845`. If `main` advances with changes to the three files, re-read them before evaluating.

1. Icon-only content and accessibility: `npx vitest run src/sidebar/components/ProjectPanel.chip-order.test.tsx` exits 0 and reports 6 passed, 0 failed; the assertions of §7.1 prove the chip has one child, empty text, an `aria-hidden` svg, `role="img"`, and `aria-label` equal to `title`.
2. No side effects: `npm run typecheck` exits 0 and `npm test` exits 0. Any failure must be reproduced at `33504845` before it can be attributed to this change.
3. File set: `git diff --name-only main...HEAD` lists exactly `plans/2030-lock-chip-icon-only.md` and the six paths in §5 — no `src-tauri/` path, no new file, no deleted file. `git diff main...HEAD -- src/sidebar/stores src/sidebar/App.tsx src/sidebar/components/ProjectPanel.order-lock.test.tsx src/sidebar/App.order-lock.test.tsx` is empty.
4. The word is gone from the sidebar source: `grep -rn "KEEP" src/sidebar --include=*.tsx --include=*.ts --include=*.css | grep -vE "KEEPALIVE|KEEPS"` prints nothing.
5. Visual confirmation (evidence, not a gate): with the app running, a locked replica's tile shows the amber padlock alone, hovering it shows `Protected from bulk changes · …`, and the chip is visibly narrower. Lock a replica from the picker ("Apply to, optionally with lock") if no row is locked. If the Tauri dev host is unavailable, AC 1 plus the measured arithmetic in §4.2 is the evidence.

## 9. Ordered implementation

1. Commit this plan alone on `fix/2030-lock-chip-icon-only` (`git add -f plans/2030-lock-chip-icon-only.md`). That commit contains no code.
2. Apply §4.1 to `ProjectPanel.tsx`, §4.2 to `sidebar.css`, then the three comment edits of §4.3.
3. Apply §7 to `ProjectPanel.chip-order.test.tsx`.
4. Run AC 1, then AC 2; check AC 3 and AC 4.
5. Commit the six files as one implementation commit and quote the passing test output in the reply to the coordinator.
