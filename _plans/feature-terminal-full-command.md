# Feature — Terminal: show full launch command in StatusBar

**Branch**: `feature/terminal-full-command` (already created from `main`)
**Repo**: `repo-AgentsCommander`
**Scope**: Frontend only — no Rust/backend changes.
**Version bump**: `0.7.3` → `0.7.4` (patch)

---

## 1. Requirement

The Terminal window's bottom StatusBar currently shows only the shell binary (e.g. `claude-mb`) plus terminal dimensions (`151x50`). The user wants the **full launch command** instead — binary + all arguments exactly as the session was initialized.

**Before**: `claude-mb  151x50`
**After**:  `claude-mb --dangerously-skip-permissions --effort max`

If the command doesn't fit on one line, it must truncate with `…` and expose the full text via a native `title` tooltip on hover.

Additionally:
- The `cols x rows` block is removed from the StatusBar.
- The `({shell})` suffix in the Terminal Titlebar is removed (now redundant, since the full command is visible in StatusBar).

---

## 2. Affected files

| # | File | Change type |
|---|------|-------------|
| 1 | `src/terminal/stores/terminal.ts` | Extend store: add `activeShellArgs` signal + widen `setActiveSession` signature |
| 2 | `src/terminal/App.tsx` | Update all 7 `setActiveSession` call sites to thread `shellArgs` |
| 3 | `src/terminal/components/StatusBar.tsx` | Replace shell-only render with full command + tooltip; remove termSize block |
| 4 | `src/terminal/components/Titlebar.tsx` | Remove ` ({activeShell})` suffix |
| 5 | `src/terminal/styles/terminal.css` | Add ellipsis truncation rules for the new `status-bar-command` class |
| 6 | `src-tauri/tauri.conf.json` | Bump `version` to `0.7.4` |
| 7 | `src-tauri/Cargo.toml` | Bump `version` to `0.7.4` |

No changes needed to:
- `src/shared/types.ts` — `Session.shellArgs: string[]` already exists (line 11).
- Any Rust source under `src-tauri/src/` — the backend already emits `shellArgs` as part of `Session`.
- `src/sidebar/**` — out of scope.

---

## 3. Detailed changes

### 3.1 `src/terminal/stores/terminal.ts`

**Current full file (40 lines) is shown for context**:

```ts
import { createSignal } from "solid-js";

const [activeSessionId, setActiveSessionId] = createSignal<string | null>(null);
const [activeSessionName, setActiveSessionName] = createSignal<string>("");
const [activeShell, setActiveShell] = createSignal<string>("");
const [activeWorkingDirectory, setActiveWorkingDirectory] = createSignal<string>('');
const [termSize, setTermSize] = createSignal<{ cols: number; rows: number }>({
  cols: 0,
  rows: 0,
});

export const terminalStore = {
  get activeSessionId() { return activeSessionId(); },
  get activeSessionName() { return activeSessionName(); },
  get activeShell() { return activeShell(); },
  get activeWorkingDirectory() { return activeWorkingDirectory(); },
  get termSize() { return termSize(); },

  setActiveSession(id: string | null, name?: string, shell?: string, workingDirectory?: string) {
    setActiveSessionId(id);
    if (name !== undefined) setActiveSessionName(name);
    if (shell !== undefined) setActiveShell(shell);
    if (workingDirectory !== undefined) setActiveWorkingDirectory(workingDirectory);
  },

  setTermSize(cols: number, rows: number) {
    setTermSize({ cols, rows });
  },
};
```

**Exact edits**:

**(a)** After line 5 (`const [activeShell, setActiveShell] = createSignal<string>("");`), insert a new signal for args:

```ts
const [activeShellArgs, setActiveShellArgs] = createSignal<string[]>([]);
```

**(b)** Inside `terminalStore` (after the `activeShell` getter at line 19-21), add a new getter:

```ts
  get activeShellArgs() {
    return activeShellArgs();
  },
```

**(c)** Change the signature of `setActiveSession` (line 29) and its body (lines 30-34). New signature inserts `shellArgs?` immediately after `shell?` to match the field order of `Session` in `src/shared/types.ts`:

```ts
  setActiveSession(
    id: string | null,
    name?: string,
    shell?: string,
    shellArgs?: string[],
    workingDirectory?: string
  ) {
    setActiveSessionId(id);
    if (name !== undefined) setActiveSessionName(name);
    if (shell !== undefined) setActiveShell(shell);
    if (shellArgs !== undefined) setActiveShellArgs(shellArgs);
    if (workingDirectory !== undefined) setActiveWorkingDirectory(workingDirectory);
  },
```

The optional-undefined-skip pattern is preserved for every field, so a rename-only call (`setActiveSession(id, name)`) continues to leave shell / shellArgs / workingDirectory untouched.

**Final file (post-edit) — dev can use this as the authoritative target**:

```ts
import { createSignal } from "solid-js";

const [activeSessionId, setActiveSessionId] = createSignal<string | null>(null);
const [activeSessionName, setActiveSessionName] = createSignal<string>("");
const [activeShell, setActiveShell] = createSignal<string>("");
const [activeShellArgs, setActiveShellArgs] = createSignal<string[]>([]);
const [activeWorkingDirectory, setActiveWorkingDirectory] = createSignal<string>('');
const [termSize, setTermSize] = createSignal<{ cols: number; rows: number }>({
  cols: 0,
  rows: 0,
});

export const terminalStore = {
  get activeSessionId() {
    return activeSessionId();
  },
  get activeSessionName() {
    return activeSessionName();
  },
  get activeShell() {
    return activeShell();
  },
  get activeShellArgs() {
    return activeShellArgs();
  },
  get activeWorkingDirectory() {
    return activeWorkingDirectory();
  },
  get termSize() {
    return termSize();
  },

  setActiveSession(
    id: string | null,
    name?: string,
    shell?: string,
    shellArgs?: string[],
    workingDirectory?: string
  ) {
    setActiveSessionId(id);
    if (name !== undefined) setActiveSessionName(name);
    if (shell !== undefined) setActiveShell(shell);
    if (shellArgs !== undefined) setActiveShellArgs(shellArgs);
    if (workingDirectory !== undefined) setActiveWorkingDirectory(workingDirectory);
  },

  setTermSize(cols: number, rows: number) {
    setTermSize({ cols, rows });
  },
};
```

---

### 3.2 `src/terminal/App.tsx`

All `setActiveSession` call sites need to thread `shellArgs` in the new positional slot (between `shell` and `workingDirectory`). There are **8 call sites** total: 4 "setting" (with real session data), 3 "clearing" (with empty values), and 1 "rename" (only id+name).

| Line(s) | Current | Replace with |
|---------|---------|--------------|
| **40** | `terminalStore.setActiveSession(session.id, session.name, session.shell, session.workingDirectory);` | `terminalStore.setActiveSession(session.id, session.name, session.shell, session.shellArgs, session.workingDirectory);` |
| **43** | `terminalStore.setActiveSession(null, "", "", "");` | `terminalStore.setActiveSession(null, "", "", [], "");` |
| **54** | `terminalStore.setActiveSession(active.id, active.name, active.shell, active.workingDirectory);` | `terminalStore.setActiveSession(active.id, active.name, active.shell, active.shellArgs, active.workingDirectory);` |
| **57** | `terminalStore.setActiveSession(null, "", "", "");` | `terminalStore.setActiveSession(null, "", "", [], "");` |
| **74** | `terminalStore.setActiveSession(null, "", "", "");` | `terminalStore.setActiveSession(null, "", "", [], "");` |
| **80-85** | multi-line `setActiveSession(session.id, session.name, session.shell, session.workingDirectory)` | insert `session.shellArgs,` between `session.shell,` and `session.workingDirectory` |
| **93-98** | multi-line `setActiveSession(session.id, session.name, session.shell, session.workingDirectory)` | same — insert `session.shellArgs,` |
| **123** | `terminalStore.setActiveSession(id, name);` | **UNCHANGED** (rename-only; shell/shellArgs/workDir stay as-is by design) |

**Exact replacement for the two multi-line call sites**:

**(a)** Lines 80-85 become:
```tsx
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell,
              session.shellArgs,
              session.workingDirectory
            );
```

**(b)** Lines 93-98 become:
```tsx
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell,
              session.shellArgs,
              session.workingDirectory
            );
```

**No other changes to App.tsx.** The `onSessionRenamed` handler at lines 121-125 stays exactly as-is.

---

### 3.3 `src/terminal/components/StatusBar.tsx`

**(a)** Extend the top import (line 1) to include `createMemo`:

```tsx
import { Component, Show, createMemo, onCleanup } from "solid-js";
```

**(b)** Inside the `StatusBar` component, add a derived memo for the full command. Insert it immediately after the `isProcessing` definition (after line 11), before the `handleMicDown` handler:

```tsx
  const fullCommand = createMemo(() => {
    const shell = terminalStore.activeShell;
    const args = terminalStore.activeShellArgs;
    if (!shell) return "";
    return args.length > 0 ? `${shell} ${args.join(" ")}` : shell;
  });
```

**(c)** Replace the entire current block at lines 60-69 (the shell `<Show>` and the termSize `<Show>`):

**Current (lines 60-69)**:
```tsx
        <Show when={terminalStore.activeShell}>
          <div class="status-bar-item">
            <span class="status-bar-accent">{terminalStore.activeShell}</span>
          </div>
        </Show>
        <Show when={terminalStore.termSize.cols > 0}>
          <div class="status-bar-item">
            {terminalStore.termSize.cols}x{terminalStore.termSize.rows}
          </div>
        </Show>
```

**Replace with**:
```tsx
        <Show when={fullCommand()}>
          <div class="status-bar-item status-bar-command">
            <span class="status-bar-accent" title={fullCommand()}>
              {fullCommand()}
            </span>
          </div>
        </Show>
```

Notes:
- The `status-bar-command` modifier class is new (CSS rules added in §3.5).
- The `title` attribute provides the native tooltip showing the full command when the text is truncated. The browser/WebView renders the tooltip after a short hover delay; no custom component needed.
- The termSize `<Show>` is removed entirely; no call site references it (keep the `setTermSize` method in the store — it's still called by `TerminalView` to track pixel-to-cell size, even though the StatusBar no longer renders it).

**No other changes in StatusBar.tsx.**

---

### 3.4 `src/terminal/components/Titlebar.tsx`

**Replace lines 87-97** (the `<Show when={terminalStore.activeSessionName}>` block that currently renders name + ` (shell)`):

**Current (lines 87-97)**:
```tsx
        <Show
          when={terminalStore.activeSessionName}
          fallback={<span>Terminal</span>}
        >
          <span class="titlebar-session-name">
            {terminalStore.activeSessionName}
          </span>
          <Show when={terminalStore.activeShell}>
            <span> ({terminalStore.activeShell})</span>
          </Show>
        </Show>
```

**Replace with** (inner `<Show>` removed):
```tsx
        <Show
          when={terminalStore.activeSessionName}
          fallback={<span>Terminal</span>}
        >
          <span class="titlebar-session-name">
            {terminalStore.activeSessionName}
          </span>
        </Show>
```

That's the entire Titlebar change. No other lines touched.

---

### 3.5 `src/terminal/styles/terminal.css`

**Implementation note** (2026-04-20): this section originally documented a nested-flex-with-inline-block ellipsis pattern (Option B). Per grinch §9.1 MEDIUM and tech-lead direction, the implementer (dev-webpage-ui) chose **Option A** — simpler, more predictable, single-element ellipsis. The actual CSS shipped is below; `vertical-align: bottom` (grinch §9.2) is intentionally omitted because it has no effect with the block-level pattern.

Extend the existing `.status-bar-left` rule (currently `display: flex; align-items: center; gap: var(--spacing-md);`) with `min-width: 0;` and `flex: 1 1 auto;` so it can shrink inside the `space-between` parent. Then, after `.status-bar-accent`, insert one new rule for the command wrapper:

```css
.status-bar-left {
  display: flex;
  align-items: center;
  gap: var(--spacing-md);
  min-width: 0;
  flex: 1 1 auto;
}

/* Full launch command in status bar: ellipsis + tooltip on overflow.
   Option A (per grinch §9.1): apply ellipsis directly to the command wrapper
   as a plain block (overrides inherited display:flex from .status-bar-item).
   Single-element ellipsis is more predictable than nested flex+inline-block. */
.status-bar-command {
  display: block;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
```

Rationale:
- `.status-bar-left`: `min-width: 0; flex: 1 1 auto;` — default `min-width: auto` blocks shrinking below content width (blocks ellipsis); `flex: 1 1 auto` gives it all remaining space inside `.status-bar` (the right side, `.status-bar-actions`, keeps its intrinsic width).
- `.status-bar-command { display: block; ... }` — overrides the `display: flex` inherited from `.status-bar-item`. With a single block element carrying the ellipsis triad (`overflow: hidden; text-overflow: ellipsis; white-space: nowrap;`), there is no nested flex+inline-block circular-sizing concern. The child `<span class="status-bar-accent">` is a plain inline child that inherits `white-space: nowrap` and stays inside the clipped parent.

Touching `.status-bar-left` is safe: only one rule defines it and there is no other selector that overrides these props (grinch §9.9 verified).

---

### 3.6 Version bump (`0.7.3` → `0.7.4`)

**`src-tauri/tauri.conf.json`** — change line 4:
```json
"version": "0.7.3",
```
to
```json
"version": "0.7.4",
```

**`src-tauri/Cargo.toml`** — change line 3:
```toml
version = "0.7.3"
```
to
```toml
version = "0.7.4"
```

**Note to dev**: `__APP_VERSION__` (used in both Titlebars) is auto-injected by `vite.config.ts:14` from `tauriConf.version`, so no hardcoded TSX edit is needed — the Vite define picks up the new value on next build. `package.json` currently reads `0.7.1` and is NOT used at runtime for version display; leave it alone unless tech-lead explicitly asks for sync.

**Cargo.lock**: After bumping `Cargo.toml`, run `cargo check` (or `cargo build`) inside `src-tauri/` so `Cargo.lock` is updated with the new version stamp. Commit the `Cargo.lock` change.

---

## 4. Dependencies

**None.** No new npm packages, no new Rust crates, no new Tauri capabilities. Only `solid-js` (already imported) — the new `createMemo` symbol is exported from the existing `solid-js` import.

---

## 5. Edge cases & notes

1. **Empty args array** (native shells: `powershell.exe`, `bash`, `cmd`, `pwsh` with no flags): `activeShellArgs` is `[]`, so `fullCommand()` returns just the shell name with no trailing space, no empty parens. Matches the spec.

2. **Very long single argument** (e.g. a prompt passed via `--prompt "…"`): rendered verbatim via `args.join(" ")`. No quoting or shell-escaping — this is a display-only label, not an executable string. If the user sees an unquoted space inside a long arg, that's a cosmetic limitation, not a bug. The `title` tooltip shows the exact same string, so the user can still read the full argument by hovering.

3. **Args containing special characters** (quotes, backslashes, unicode): rendered raw. SolidJS's default text interpolation (`{fullCommand()}`) safely escapes HTML entities, so no XSS risk even if an arg contains `<`, `>`, `&`, etc.

4. **Very narrow terminal window**: the ellipsis triggers only when the command's natural width exceeds the space available inside `.status-bar-left`. Because `.status-bar-left` now uses `flex: 1 1 auto; min-width: 0;`, and `.status-bar-actions` (voice + clear buttons) sits on the right with intrinsic width, the command block gets whatever space remains. Under extreme narrowing (e.g. 200px wide) the command may be reduced to `c…`; that's acceptable — the tooltip still carries the full text.

5. **Voice recording / transcribing states activate** while a long command is visible: the `status-bar-recording` / `status-bar-processing` items appear to the right of `status-bar-command` and consume their own space. Since the command block has `flex: 0 1 auto` (default; `.status-bar-item` does NOT get `flex-grow: 1`), those sibling items push the command to truncate further. Correct behavior.

6. **DETACHED badge** appears before the command block. It has a fixed intrinsic width (~75px with the "DETACHED" text + color), so it never truncates and never blocks the command from showing.

7. **Session switch while truncated**: `createMemo` re-evaluates whenever `activeShell` or `activeShellArgs` changes. The new full command is rendered immediately, tooltip updates atomically. No stale state possible.

8. **Rename event** (`onSessionRenamed` at App.tsx line 121-125): only `id` and `name` are passed to `setActiveSession`. The new signature leaves `shell`, `shellArgs`, and `workingDirectory` untouched when the corresponding argument is `undefined`, so a rename does NOT accidentally zero out the shell args. This is the whole reason the store uses the optional-undefined-skip pattern — do NOT change it.

9. **`setActiveShellArgs` stays private** to the store module. Do NOT export it; all external mutation goes through `setActiveSession`. This mirrors how `setActiveShell`, `setActiveSessionName`, etc. are already gated.

10. **Control characters in args** (`\n`, `\t`, `\r`): whitespace is collapsed in the inline span by `white-space: nowrap`, so the visible command is single-line regardless. The raw string is still passed to the native `title` attribute; WebView2 on Windows will render multi-line tooltips when the value contains `\n`. Acceptable — the tooltip is a debugging aid, and multi-line is arguably clearer for prompt-style args (e.g. `--prompt "line1\nline2"`). No code change needed. (Added per grinch §9.3.)

---

## 6. What the dev must NOT do

- Do NOT change any Rust source under `src-tauri/src/`. The backend already emits `shellArgs`; no serde / IPC / type changes needed.
- Do NOT change `src/shared/types.ts`. `Session.shellArgs: string[]` is already in place (line 11).
- Do NOT touch anything under `src/sidebar/**`. The sidebar's session list does not render the launch command, and that behavior is explicitly out of scope.
- Do NOT add a custom tooltip component. Native `title` attribute is specified and sufficient.
- Do NOT rework `setTermSize` or its call sites. The method stays in the store (it's still used by `TerminalView`'s fit logic for PTY resize); only the StatusBar's render of `termSize.cols x rows` is removed.
- Do NOT bump `package.json` version. It's currently out-of-sync (`0.7.1`) but not used at runtime, and fixing it is out of scope for this feature.
- Do NOT create new CSS tokens / variables / theme files. Reuse `.status-bar-accent` + new `.status-bar-command` modifier only.
- Do NOT refactor surrounding code in the touched files. Minimum-diff principle applies.

---

## 7. Manual verification checklist (post-implementation)

Dev should verify each of these before handing off:

- [ ] Sessions with args (e.g. `claude-mb --dangerously-skip-permissions --effort max`): StatusBar shows the full command; Titlebar shows `Name` only (no `(shell)` suffix).
- [ ] Native shells without args (`powershell.exe`, `cmd`, `bash`): StatusBar shows just the binary name; no trailing space, no empty parens.
- [ ] `cols x rows` block is gone from the StatusBar.
- [ ] Resizing the Terminal window narrow: command ellipsis-truncates; hovering the truncated text shows the native tooltip with the full command.
- [ ] Voice recording active while a long command is visible: command truncates further to keep the mic / cancel / clear buttons visible; tooltip still shows full command.
- [ ] DETACHED mode: badge shows, command shows after it, actions still work.
- [ ] Renaming an active session: session name updates in titlebar; StatusBar command stays unchanged (NOT cleared).
- [ ] Switching between sessions: StatusBar command updates instantly to the new session's shell + args.
- [ ] Destroying the last session: StatusBar command disappears; StatusBar shows only the actions row (or nothing if no session).
- [ ] Progressive narrow-window test: resize Terminal to 600px, 400px, 300px, 200px. At each width, verify the command either fully fits OR truncates with `…`. It must NEVER overflow into the actions column, and must NEVER disappear entirely (unless reduced below the DETACHED badge width, which is acceptable). (Added per grinch §9.7.)
- [ ] `npx tsc --noEmit` passes with no new errors.
- [ ] `cd src-tauri && cargo check` passes after the version bump.
- [ ] Titlebar shows `v0.7.4` in both Sidebar and Terminal windows after `npm run tauri dev` (dev server must be restarted — see §8.7).

---

## 8. Reviewer enrichments (dev-webpage-ui, 2026-04-20)

All line numbers in §1–§7 verified exact against HEAD of `feature/terminal-full-command`. The 8 `setActiveSession` call sites listed in §3.2 are the ONLY occurrences in `src/`. `setTermSize` is still called by `TerminalView.tsx:39, 181, 254` — confirms the plan's assertion that the method must remain in the store even after StatusBar drops its render. The `__APP_VERSION__` injection at `vite.config.ts:14` reads `tauriConf.version`, so bumping `src-tauri/tauri.conf.json` propagates to BOTH `src/sidebar/components/Titlebar.tsx:6-7` AND `src/terminal/components/Titlebar.tsx:5-6` without any TSX edit.

### 8.1 SolidJS reactivity gotcha (critical)

The `createMemo` in §3.3(b) accesses `terminalStore.activeShell` and `terminalStore.activeShellArgs` INSIDE the memo body. That is correct — the getters resolve lazily, so reactivity is tracked per access.

**Do NOT refactor** the memo to destructure the store at the top of the component body, e.g.:
```tsx
// BROKEN — do not do this
const { activeShell, activeShellArgs } = terminalStore;
const fullCommand = createMemo(() => { ... });
```
That evaluates the getters ONCE at component setup time and kills tracking. The existing `isRecording`, `isProcessing`, `handleClearInput` helpers use the correct lazy-access pattern (`terminalStore.activeSessionId` resolved at call time) — stay consistent.

### 8.2 Array-signal reference equality

`setActiveShellArgs(newArray)` triggers the memo only when the array REFERENCE changes. The backend emits a fresh `Vec<String>` → JS `Array` on every IPC serialization, so session switches naturally produce new references. **Do NOT** mutate the existing array via `.push()` / `.splice()` / `.sort()` — SolidJS signals compare by reference and in-place mutation won't re-fire the memo. The plan's §5.9 ("setActiveShellArgs stays private") already enforces this, but be explicit if future code ever adds args-editing UI.

### 8.3 TypeScript safety net for the signature change

Inserting `shellArgs?: string[]` between `shell?: string` and `workingDirectory?: string` is a positional-breaking change. `npx tsc --noEmit` WILL catch any missed caller:
- Old 4-arg call `setActiveSession(id, name, shell, workDir)` where `workDir: string` now lands in the `shellArgs?: string[]` slot → type error (`string` not assignable to `string[]`).
- The 3-arg rename call at `App.tsx:123` is unaffected (no 4th arg passed; `shell`, `shellArgs`, `workingDirectory` all stay `undefined` and skip the setter — exactly the desired rename semantics).

Grep-verified: `App.tsx` is the ONLY external caller of `setActiveSession`. All 8 call sites enumerated in §3.2 are accounted for. Safe.

### 8.4 Version bump requires dev-server restart

`vite.config.ts:14` reads `tauriConf.version` at Vite **startup** time (not hot-reload, not runtime). After bumping `src-tauri/tauri.conf.json`:
- If `npm run tauri dev` is already running, the new version will NOT appear until the dev server is restarted (`npm run kill-dev` then `npm run tauri dev` per project rules — see `CLAUDE.md` "CRITICAL — Running the App").
- Post-bump verification of `v0.7.4` in either titlebar requires a fresh server.

Adjusted §7 checklist to reflect this.

### 8.5 Role-boundary note (src-tauri/ config edits)

The `dev-webpage-ui` role doc restricts Rust backend CODE modifications (`src-tauri/src/`). Version bumps to `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` are **manifest/metadata** edits and fall within frontend scope. `cargo check` to refresh `Cargo.lock` is a build-step invocation, not a source change. No conflict with dev-rust's domain. The plan's §6 explicitly excludes `src-tauri/src/` modifications, which is consistent with my role.

### 8.6 Minor UX notes (informational, NOT actions)

- **Always-on `title` tooltip**: native `title` renders on hover regardless of whether the text actually overflows. A short fully-visible command (`bash`) still shows a redundant tooltip. Acceptable per spec; a conditional-tooltip implementation (JS measuring `scrollWidth > clientWidth`) is **out of scope**.
- **Cursor over truncated text**: the `<span>` defaults to the text I-beam (selectable text), matching the current `activeShell` span behavior. Do NOT add `cursor: default` or `cursor: help` — stay minimum-diff. Flag as a potential follow-up if users ask for a different feel.
- **Sibling overflow under extreme narrowing** (<200px window): only `.status-bar-command` truncates gracefully; DETACHED / recording / transcribing items just clip since they lack `text-overflow: ellipsis`. Not a bug — spec calls out ellipsis only on the command block.
- **Monospace for the command?** Shell commands are traditionally rendered in monospace. Current statusbar uses `--font-ui` (Geist/Outfit) for the shell name already, so the new command block inherits the same. Keep as-is for minimum diff; trivial single-rule override later if tech-lead wants monospace.

---

## 9. Grinch adversarial review (dev-rust-grinch, 2026-04-20)

Verified against HEAD of `feature/terminal-full-command`. Cross-checked every line reference in §1-§8 and the dev-webpage-ui enrichment. All line numbers and code snippets match the actual source. The dev-webpage-ui pass caught the biggest reactivity traps; my job is to find what's still missing.

### 9.1 Nested ellipsis-in-flex — pattern is non-standard (MEDIUM)

**What**: The proposed CSS creates **three nested flex containers** with ellipsis applied to an inline-block flex item two levels deep:

```
.status-bar               (display: flex, space-between)
  .status-bar-left        (display: flex, + plan adds: flex: 1 1 auto; min-width: 0)
    .status-bar-command   (inherits display: flex from .status-bar-item, + plan adds: min-width: 0; overflow: hidden)
      .status-bar-accent  (plan adds: display: inline-block; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap)
```

**Evidence**: `terminal.css:262-266` — `.status-bar-item { display: flex; align-items: center; gap: 4px; }`. The plan's `.status-bar-command` modifier class never overrides this, so the outer ellipsis container is itself a flex container, and `.status-bar-accent` is a flex child with `display: inline-block`.

**Why it matters**: The canonical flex-ellipsis pattern applies `overflow: hidden; text-overflow: ellipsis; white-space: nowrap` **directly to a flex item** (with its parent having `min-width: 0`), not to an inline-block nested inside a flex item. In Chromium/WebView2, nested flex+inline-block ellipsis usually works — but the `max-width: 100%` resolution against a flex container whose intrinsic width is itself content-driven is a known fragile case (circular sizing). If it fails under specific viewport widths, the text will either clip without `…` or stop shrinking.

**Fix**: Simplify to one of these patterns (either is fine):

Option A — apply ellipsis directly to `.status-bar-command`, skip the nested span rules:
```css
.status-bar-command {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  display: block;            /* override inherited display: flex from .status-bar-item */
}
.status-bar-command > .status-bar-accent {
  /* no extra rules needed — inherits color from existing .status-bar-accent */
}
```

Option B — keep `.status-bar-command` as a flex container but move ellipsis properties onto the direct accent child and drop `max-width: 100%`:
```css
.status-bar-command {
  min-width: 0;
  overflow: hidden;
  flex: 0 1 auto;
}
.status-bar-command > .status-bar-accent {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
```

Option A is the minimum-diff and most predictable choice. Whatever pattern the dev chooses, the verification checklist (§7) must include an **explicit narrow-window test at multiple widths** (e.g. resize Terminal to 400px, 300px, 200px, 150px) to confirm ellipsis actually fires.

---

### 9.2 `vertical-align: bottom` is a no-op in this layout (LOW)

**What**: Plan §3.5 adds `vertical-align: bottom` to `.status-bar-command > .status-bar-accent` with the rationale "prevents a small baseline jitter that inline-block can introduce against sibling text."

**Why**: The accent span is a child of `.status-bar-command`, which inherits `display: flex` from `.status-bar-item`. Flex items ignore `vertical-align` — flex alignment is controlled by `align-items` / `align-self`. There is no baseline jitter to fix because there are no inline siblings to align against (the `.status-bar-command` has exactly ONE child). The rule is harmless but misleading.

**Fix**: Drop the `vertical-align: bottom` line. If the dev chooses §9.1 Option A (plain block), the rule is also irrelevant because there are no inline-level siblings in the flow.

---

### 9.3 Control characters in args — title tooltip + ellipsis truncation interaction (LOW)

**What**: Plan §5.2 covers long args and §5.3 covers quotes/backslashes/unicode (HTML escaping via SolidJS text interpolation). Neither explicitly addresses **control characters** (newline `\n`, tab `\t`, carriage return `\r`) that may appear in an arg.

**Why it matters**:
- The span uses `white-space: nowrap`, which collapses consecutive whitespace (including `\n`, `\t`) to a single space in rendered text. Safe for display, but the displayed command may look different from the actual launch command.
- The `title={fullCommand()}` attribute passes the raw string (including `\n`) to the native OS tooltip. WebView2 on Windows does render multi-line tooltips when the `title` value contains `\n`. This means the hover tooltip may expand to multiple lines for commands that contain embedded newlines in args (rare but possible for prompt-style args: `--prompt "line1\nline2"`).

**Fix**: Add to §5 a new edge case entry: *"Args containing control characters (`\n`, `\t`, `\r`) will have whitespace collapsed in the inline span (due to `white-space: nowrap`) but may render multi-line in the native `title` tooltip on Windows/WebView2. Acceptable — the tooltip is a debugging aid and multi-line is arguably clearer."* No code change required.

---

### 9.4 Prod/stage Tauri configs exist but aren't mentioned (INFO)

**What**: Plan §3.6 and §8.5 mention only `tauri.conf.json`. The repo also has:
- `src-tauri/tauri.prod.conf.json` (lines 1-5 — `productName`, `identifier`, `mainBinaryName` only)
- `src-tauri/tauri.stage.conf.json` (lines 1-5 — same shape)

Referenced by `package.json` scripts:
```
"build:prod": "... tauri build --config src-tauri/tauri.prod.conf.json"
"build:stage": "... tauri build --config src-tauri/tauri.stage.conf.json"
```

**Why it matters**: Tauri's `--config <file>` flag merges the supplied file on top of `tauri.conf.json`. Since neither `tauri.prod.conf.json` nor `tauri.stage.conf.json` defines a `version` field, the bumped version from `tauri.conf.json` propagates correctly to both profiles. **No bug — but the plan should briefly acknowledge these files exist so a future reader doesn't wonder whether they need parallel bumps.**

**Fix**: Add one sentence to §3.6: *"`src-tauri/tauri.prod.conf.json` and `src-tauri/tauri.stage.conf.json` are profile overlays that do NOT define a `version` field — they inherit the bumped version from `tauri.conf.json` via Tauri's config-merge behavior. No edits needed."*

---

### 9.5 Backend `Session.shell_args` has no `#[serde(default)]` — not introduced by this plan but worth a note (INFO)

**What**: `src-tauri/src/session/session.rs:47` defines `pub shell_args: Vec<String>` without `#[serde(default)]`. Same at line 99 (`SessionInfo`). Same at `src-tauri/src/config/sessions_persistence.rs:19` (`PersistedSession`).

**Why it matters**: If a legacy persisted session TOML from a pre-shellArgs version is loaded, deserialization fails and that session is dropped. This is **pre-existing behavior and out of scope for this plan** — but since the plan now elevates `shellArgs` from "backend-only field" to "user-visible rendering on every session," a legacy-session load failure would become visibly different (the StatusBar would show no command for a session that silently disappeared during load). Worth mentioning as context.

**Fix**: Nothing in this plan. Mention to tech-lead that if legacy TOML compatibility is ever needed, the three `shell_args` field sites would need `#[serde(default)]`. NOT a blocker for this feature.

---

### 9.6 `activeShellArgs` initial state vs rename-before-first-switch (INFO, covered)

**What**: Verified scenario — a sidebar-created session gets renamed before the Terminal window ever switches to it. The rename fires `onSessionRenamed` in Terminal/App.tsx (line 121-125), which calls `setActiveSession(id, name)`. But `id` only matches `terminalStore.activeSessionId` (guard at line 122), so if the session isn't active, the rename is a no-op for the Terminal store. 

**Why it works**: The plan's §5.8 already correctly identifies the rename case as untouched. The `activeSessionId !== renamedId` guard at line 122 prevents the rename from mutating ANY state for non-active sessions. No issue — this is fully handled.

**No fix needed.** Flagged for completeness.

---

### 9.7 Verification checklist §7 needs one addition (LOW)

**What**: §7 checklist covers many cases but does not include an **explicit narrow-window progressive-resize test** at multiple widths to validate §9.1's ellipsis behavior.

**Fix**: Add to §7:
```
- [ ] Progressive narrow-window test: resize Terminal to 600px, 400px, 300px, 200px.
      At each width, verify the command either fully fits OR truncates with `…`.
      It must NEVER overflow into the actions column, and must NEVER disappear
      entirely (unless reduced to below the DETACHED badge width, which is acceptable).
```

---

### 9.8 Signature-change grep verification (confirmed clean, no action needed)

**What**: Independently verified §8.3's claim. Grep across `src/` for `setActiveSession` returns exactly the 8 call sites in `src/terminal/App.tsx` plus the 1 definition in `src/terminal/stores/terminal.ts`. No other files (sidebar, shared, browser, guide) call `setActiveSession`. The positional reordering is TypeScript-caught for any missed migration. Confirmed safe.

**No fix needed.** Flagged as independent verification.

---

### 9.9 CSS scoping — no theme or sidebar file can fight `.status-bar-left` (confirmed clean, no action needed)

**What**: Tech-lead raised concern about global `.status-bar-left` changes. Verified via grep:
- `.status-bar` selector family appears ONLY in `src/terminal/styles/terminal.css` and `src/terminal/components/StatusBar.tsx`.
- No sidebar CSS (`src/sidebar/styles/**`), no browser CSS (`src/browser/styles/**`), no guide CSS (`src/guide/styles/**`), and no theme/light-mode rules touch `.status-bar-left` or `.status-bar-accent`.

**No fix needed.** Confirmed scope is clean.

---

## Grinch Review Summary

| # | Finding | Severity |
|---|---------|----------|
| 9.1 | Nested flex+inline-block ellipsis pattern is fragile | MEDIUM |
| 9.2 | `vertical-align: bottom` is a no-op in this flex layout | LOW |
| 9.3 | Control chars in args — whitespace collapse + multi-line `title` | LOW |
| 9.4 | Prod/stage Tauri configs merge cleanly — plan should acknowledge | INFO |
| 9.5 | Backend `shell_args` lacks `#[serde(default)]` — pre-existing, context only | INFO |
| 9.6 | Rename-before-first-switch is already handled (§5.8) | INFO (no action) |
| 9.7 | §7 checklist missing progressive narrow-window resize test | LOW |
| 9.8 | Signature change grep confirmed clean | INFO (no action) |
| 9.9 | `.status-bar-left` global change confirmed scope-safe | INFO (no action) |

### Verdict

**APPROVED with MEDIUM finding** — the plan is thorough and safe enough to proceed. Finding 9.1 (nested flex ellipsis) is not a blocker because the pattern **may work** in Chromium/WebView2 despite being non-canonical; however the dev should either simplify the CSS per 9.1 Option A OR expand the verification checklist (9.7) to catch ellipsis failures empirically. Findings 9.2-9.3 are small tightening opportunities. 9.4-9.6 and 9.8-9.9 are informational / already-handled.

No BLOCKER or HIGH findings. No rework required before Step 6 (implementation). Dev should incorporate 9.1 and 9.7 at minimum; 9.2/9.3/9.4 at their discretion.
