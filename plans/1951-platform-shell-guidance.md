# Plan #1951: platform-specific Default Shell path guidance (Git Bash on Windows)

Status: READY_FOR_IMPLEMENTATION
Issue: `#1951` (OPEN), "fix(settings): platform-specific shell path guidance and Git Bash recommendation"
Repository: `repo-AgentsCommander`
Base: `main` = `origin/main` = `7236767b55baccb8baab1eab91ce91b6a607bf33`; branch `fix/1951-platform-shell-guidance`
Band: Lite 1-25. Author: `ac-dev-webpage-ui-v4`. Reviewer: the Grinch reviewer assigned by the coordinator.
Task class: copy-only UI change in one component. No IPC, no Rust, no settings schema, no persistence, no test files.

## 1. Objective and evidence

The red invalid-Default-Shell hint in Settings > General must adapt to the running platform:

- Windows: `for example: C:\Program Files\Git\bin\bash.exe`, plus an explicit `bash.exe` (Git Bash) recommendation.
- macOS / Linux: native POSIX example `/bin/bash`.

| Fact | Where (measured at `7236767b`) |
|---|---|
| Hardcoded PowerShell example to replace | `src/sidebar/components/SettingsModal.tsx:1850-1851`; repo-wide grep `Not a complete executable path` matches only this file, and no test or snapshot pins the copy |
| Warning trigger + element, unchanged | `SettingsModal.tsx:1845-1853`, testid `settings.general.defaultShell.warning`, class `settings-hint settings-hint-error` |
| Validation to preserve | `isPlausibleCompleteExecutablePath`, `SettingsModal.tsx:121-130`: separator-only shape check, no filesystem, never blocks saving |
| Existing platform mechanism | `src/shared/platform.ts:8-13`: `isWindows`, UA-based (`#777`), guards `typeof navigator`; consumer precedent `WorkgroupGroupsModal.tsx:293-299` |
| Git Bash path is already the product's Windows rule | `src-tauri/src/config/session_context.rs:31` (`use C:\Program Files\Git\bin\bash.exe ...`); `docs/agents/host-platform-rules.md:11` |
| POSIX example is the backend's non-Windows default | `src-tauri/src/config/settings.rs:935-939`: Windows `powershell.exe`, otherwise `/bin/bash` |
| jsdom is deterministically non-Windows | jsdom 25.0.1 UA here = `Mozilla/5.0 (win32) AppleWebKit/537.36 (KHTML, like Gecko) jsdom/25.0.1`; `win32` does not match `/Windows/i` (measured) |

Discovery method: codebase-memory graph first on this working copy (indexed `2026-09-11T03:42:25Z`, 23,242 nodes / 154,274 edges); the cited production files returned `no_recorded_issue`, and the test file (excluded from the fast index by design) was read directly.

## 2. Decided solution

### 2.1 The only application edit: `src/sidebar/components/SettingsModal.tsx`

1. Line 3 becomes:

```tsx
import { isTauri, isWindows } from "../../shared/platform";
```

2. After `isPlausibleCompleteExecutablePath` (after line 130) add:

```tsx
/** #1951 - the invalid Default Shell warning is platform-specific. Windows
 *  names Git Bash's bash.exe, the shell the Windows host-platform rules
 *  require (src-tauri/src/config/session_context.rs); every other platform
 *  names /bin/bash, the backend's non-Windows default
 *  (src-tauri/src/config/settings.rs). Pure literals: no filesystem access. */
const WINDOWS_DEFAULT_SHELL_HINT =
  "Not a complete executable path. Enter the complete path to the shell executable, for example: C:\\Program Files\\Git\\bin\\bash.exe. We recommend bash.exe from Git Bash.";
const POSIX_DEFAULT_SHELL_HINT =
  "Not a complete executable path. Enter the complete path to the shell executable, for example: /bin/bash.";
```

3. The warning body (lines 1845-1853) becomes:

```tsx
        <Show when={!isPlausibleCompleteExecutablePath(settings.data?.defaultShell ?? "")}>
          <div
            class="settings-hint settings-hint-error"
            data-ac-testid="settings.general.defaultShell.warning"
          >
            {isWindows ? WINDOWS_DEFAULT_SHELL_HINT : POSIX_DEFAULT_SHELL_HINT}
          </div>
        </Show>
```

The ternary keeps one text node, so the rendered `textContent` is the copy byte for byte. `Show` is already imported; nothing else changes.

### 2.2 Copy, as it renders

Windows:

```
Not a complete executable path. Enter the complete path to the shell executable, for example: C:\Program Files\Git\bin\bash.exe. We recommend bash.exe from Git Bash.
```

macOS / Linux / unknown:

```
Not a complete executable path. Enter the complete path to the shell executable, for example: /bin/bash.
```

### 2.3 Platform semantics

- Windows: `isWindows` true (WebView2 UA carries `Windows NT ...`) -> Git Bash path and recommendation. The recommendation sentence names both `bash.exe` and Git Bash, and the path matches the product's existing Windows shell rule.
- macOS / Linux: one POSIX branch, `/bin/bash`, which is AC's own non-Windows default and exists on both systems; no `isMac` flag is added because `/bin/zsh` is not AC's default and a macOS flag would have no other consumer.
- Fallback: `isWindows` is false when `navigator` or the UA token is unavailable, so unknown environments get the POSIX copy and never the Windows one.
- WS / remote browser mode: the copy follows the client browser UA, matching the existing UA-based precedent for host-side behavior (`WorkgroupGroupsModal`). Settings has no host-OS signal (`BlockerReport.platform` is delete-diagnostic-only, `src/shared/types.ts:1611`), and adding a host-platform IPC is out of scope.
- Untouched: warning trigger, testid, classes, validator, live typing behavior, Save enablement, and the configured `defaultShell` value.

## 3. Verification

```
cd repo-AgentsCommander
npx vitest run src/sidebar/components/SettingsModal.default-shell.test.tsx
npm run typecheck
```

The existing test must stay green unchanged: it renders the modal under jsdom (real `isWindows` false), so it exercises the POSIX branch and proves the trigger, live clear/reappear, and Save-enabled behavior are preserved. No test is added or modified; the Windows branch and both exact literals are verified by reviewer inspection of section 2.1 (the jsdom UA cannot produce the Windows branch).

Acceptance mapping:

| Acceptance criterion (issue #1951) | Proof |
|---|---|
| Windows example matches the requested path and recommendation | Exact constants in 2.1 and 2.2; reviewer inspects the Windows literal |
| Other platforms never display the Windows example/recommendation | The POSIX constant contains neither; existing jsdom test runs that branch |
| Existing invalid-path warning trigger remains | Existing test unchanged; trigger/validator untouched |
| Focused verification covers platform variants | Existing test (POSIX runtime) + typecheck + reviewer inspection of the one ternary (Windows) |

## 4. Scope and rollback

In scope: `src/sidebar/components/SettingsModal.tsx` and this plan.
Out of scope: all test files; `src/shared/platform.ts` (no `isMac`); Rust defaults and `src-tauri/`; CSS; docs/CHANGELOG/versioning; host-platform IPC.

Rollback: revert the component commit; the change is presentation-only and no persisted value or contract changes.

Implementation commit: `#1951 fix(settings): platform-specific default shell guidance` — one commit with the component change, then run section 3.
