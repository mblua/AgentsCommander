# Plan #1951: platform-specific Default Shell path guidance (Git Bash on Windows)

Status: READY_FOR_IMPLEMENTATION
Issue: `#1951` (OPEN) — "fix(settings): platform-specific shell path guidance and Git Bash recommendation"
Repository: `repo-AgentsCommander`
Base: `main` = `origin/main` = `7236767b55baccb8baab1eab91ce91b6a607bf33`; branch `fix/1951-platform-shell-guidance`
Band: Lite 1-25. Plan author: `ac-dev-webpage-ui-v4`. Plan reviewer: the Grinch reviewer assigned by the coordinator.
Task class: copy-only UI change in one component. No IPC, no Rust, no settings schema, no persistence, no new platform API.

## 1. Objective and evidence

The red invalid-Default-Shell hint in Settings > General must adapt to the running platform:

- Windows: `for example: C:\Program Files\Git\bin\bash.exe`, plus an explicit `bash.exe` (Git Bash) recommendation.
- macOS / Linux: a native POSIX example, `/bin/bash`.

| Fact | Where (all measured at `7236767b`) |
|---|---|
| Hardcoded PowerShell example to replace | `src/sidebar/components/SettingsModal.tsx:1850-1851` |
| Warning trigger + hint element (must stay) | `SettingsModal.tsx:1845-1853`, testid `settings.general.defaultShell.warning`, class `settings-hint settings-hint-error` |
| Validation to preserve | `isPlausibleCompleteExecutablePath`, `SettingsModal.tsx:121-130`: separator-only shape check, no filesystem, never blocks saving |
| Existing platform mechanism | `src/shared/platform.ts:8-13` — `isWindows` (UA-based, issue `#777`, the repo's only OS flag; guards `typeof navigator`) |
| Existing platform-conditioned copy precedent | `src/sidebar/components/WorkgroupGroupsModal.tsx:293-299` (`<Show when={!isWindows}>` hint) |
| Backend shell defaults | `src-tauri/src/config/settings.rs:935-939`: Windows `powershell.exe`, otherwise `/bin/bash` |
| Git Bash path is already the product's Windows rule | `src-tauri/src/config/session_context.rs:31` (`use C:\Program Files\Git\bin\bash.exe ...`); `docs/agents/host-platform-rules.md:11` |
| No other consumer pins the old copy | repo-wide grep `Not a complete executable path` matches only `SettingsModal.tsx:1850`; no snapshot or `SettingsModal.*` test asserts it |
| jsdom is deterministically non-Windows | jsdom 25.0.1 `navigator.userAgent` here = `Mozilla/5.0 (win32) AppleWebKit/537.36 (KHTML, like Gecko) jsdom/25.0.1`; the token `win32` does **not** match `/Windows/i` (measured with `node -e` against the installed jsdom) |
| Windows-mocked test precedent | `src/sidebar/components/ProjectPanel.repo-browse.test.tsx:1-22` (file-local `vi.mock("../../shared/platform", ...)`, comment explains why a separate file) |

Discovery method: codebase-memory graph first (this working copy indexed at `2026-09-11T03:42:25Z`, 23,242 nodes / 154,274 edges) located `isWindows` and its consumers. The `.test.tsx` files are excluded from that fast index by design (`check_index_coverage` status `excluded`, `fast-pattern`), so they were read directly; the production files cited above returned `no_recorded_issue`.

## 2. Decided solution

### 2.1 Source change — `src/sidebar/components/SettingsModal.tsx` (the only application edit)

1. Line 3 becomes:

```tsx
import { isTauri, isWindows } from "../../shared/platform";
```

2. After `isPlausibleCompleteExecutablePath` (after line 130) add two module-level literals:

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

The ternary keeps exactly one text node, so `textContent` is the copy byte for byte and tests can assert it without whitespace normalization. `Show` is already imported; nothing else in the file changes.

### 2.2 Copy, as it renders

Windows:

```
Not a complete executable path. Enter the complete path to the shell executable, for example: C:\Program Files\Git\bin\bash.exe. We recommend bash.exe from Git Bash.
```

macOS / Linux / unknown:

```
Not a complete executable path. Enter the complete path to the shell executable, for example: /bin/bash.
```

### 2.3 Why these examples, and the fallback

- Windows: the issue's required literal, and the same path the product already mandates for Windows shell work (`session_context.rs:31`, `docs/agents/host-platform-rules.md`). The recommendation sentence names both `bash.exe` and Git Bash, which is what the issue asks for; it does not claim PowerShell is wrong for other uses.
- macOS / Linux: `/bin/bash` is AgentsCommander's own non-Windows default (`settings.rs:938`), exists on both systems, and the platform docs already treat Linux and macOS as one class (`host-platform-rules.md:11`). Decision: **no `isMac` branch.** zsh is the macOS login shell but is not AC's default; adding a macOS UA flag would add shared surface no other code uses. If the reviewer requires `/bin/zsh` on macOS, that is one extra constant plus one test case, not a redesign.
- Fallback: `isWindows` is `false` when `navigator` or the UA token is unavailable (`platform.ts:12-13`), so unknown environments get the POSIX example and never the Windows one.

## 3. Behavior and edge cases

- Trigger unchanged: warning shows only while `isPlausibleCompleteExecutablePath` is false (empty/whitespace or no `/` or `\`), updates live while typing, never disables Save.
- Configured value untouched: the input still binds `settings.data.defaultShell`; nothing changes in `updateField`, save, or Rust defaults.
- Desktop app (WebView2 on Windows): UA carries `Windows NT ...` → Windows copy. WKWebView/WebKitGTK → POSIX copy.
- WS / remote browser mode: the copy follows the **client** browser UA (Chrome on Windows shows the Git Bash advice), matching the existing UA-based precedent in `WorkgroupGroupsModal` for host-side behavior. There is no host-OS signal for Settings today: `BlockerReport.platform` (`types.ts:1611`) exists only inside workgroup-delete diagnostics, and deriving the host from `settingsFilePath` would be a brittle heuristic. Adding a host-platform IPC is out of scope and not requested.
- Backslashes render literally; in the JS literal they are escaped (`\\`). No quoting/clipboard behavior exists.
- Longer Windows sentence wraps inside the existing block `.settings-hint`; no CSS, testid, or DOM-structure change.
- macOS gets the POSIX copy by the explicit decision in 2.3.

## 4. Tests

### 4.1 Extend `src/sidebar/components/SettingsModal.default-shell.test.tsx` (real platform module → POSIX branch)

- Add a `#1951` note to the file-top comment: the real `isWindows` is false under jsdom, the Windows branch lives in the sibling file.
- In the existing test, right after `expect(byTestId(r.root, "settings.general.defaultShell.warning")).toBeTruthy();` (line 78), capture the element and assert the exact POSIX copy:

```tsx
      const warning = byTestId(r.root, "settings.general.defaultShell.warning")!;
      expect(warning.textContent).toBe(
        "Not a complete executable path. Enter the complete path to the shell executable, for example: /bin/bash."
      );
      expect(warning.textContent).not.toContain("Git Bash");
      expect(warning.textContent).not.toContain("C:\\Program Files\\Git");
```

The rest of the test (live clear/reappear, Save enabled, label text) is unchanged.

### 4.2 New `src/sidebar/components/SettingsModal.default-shell.windows.test.tsx` (mocked → Windows branch)

```tsx
// @vitest-environment jsdom
import { vi } from "vitest";
// #1951 - Windows variant of the Default Shell warning. jsdom's user agent here
// is "Mozilla/5.0 (win32) ... jsdom/25.0.1": "win32" does not match
// shared/platform's /Windows/i, so the real module can never report the Windows
// branch. Replace the module for THIS FILE ONLY; a module mock replaces it
// wholesale, so all three exports are needed.
// SettingsModal.default-shell.test.tsx keeps the real module and covers POSIX.
vi.mock("../../shared/platform", () => ({
  isTauri: false,
  isBrowser: true,
  isWindows: true,
}));

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

// The exact Windows copy, byte for byte (#1951).
const WINDOWS_WARNING =
  "Not a complete executable path. Enter the complete path to the shell executable, for example: C:\\Program Files\\Git\\bin\\bash.exe. We recommend bash.exe from Git Bash.";

describe("SettingsModal Default Shell warning on Windows (#1951)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function byTestId<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
    return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
  }

  it("shows the Git Bash example and recommendation for a bare shell name", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ defaultShell: "powershell.exe" }));
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    const r = renderWithFakeTransport(
      () => <SettingsModal section="general" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(byTestId(r.root, "settings.general.defaultShell.warning")).toBeTruthy()
      );
      const warning = byTestId(r.root, "settings.general.defaultShell.warning")!;
      expect(warning.textContent).toBe(WINDOWS_WARNING);
      expect(warning.textContent).toContain("for example: C:\\Program Files\\Git\\bin\\bash.exe");
      expect(warning.textContent).toContain("bash.exe from Git Bash");
      expect(warning.textContent).not.toContain("powershell");
      const save = byTestId<HTMLButtonElement>(r.root, "settings.save")!;
      expect(save.disabled).toBe(false);
    } finally {
      r.cleanup();
    }
  });
});
```

A separate file, not a `vi.mock` in the extended file: the mock is file-wide and would replace the platform module for every test in that file and its import graph; the sibling file keeps the real-module POSIX path covered, following the `ProjectPanel.repo-browse.test.tsx` precedent.

## 5. Verification (focused; no GUI, no images, no manual input)

```
cd repo-AgentsCommander
npx vitest run src/sidebar/components/SettingsModal.default-shell.test.tsx src/sidebar/components/SettingsModal.default-shell.windows.test.tsx
npm run typecheck
```

Expected: both files green, typecheck clean. Acceptance mapping:

| Acceptance criterion (issue #1951) | Proof |
|---|---|
| Windows example matches the requested path and recommendation | 4.2 asserts the full Windows string, incl. `for example: C:\Program Files\Git\bin\bash.exe` and the Git Bash recommendation |
| Other platforms never display the Windows example/recommendation | 4.1 asserts the exact POSIX string and `not.toContain("Git Bash")` / `not.toContain("C:\\Program Files\\Git")` |
| Existing invalid-path warning trigger remains | 4.1 keeps the live appear/clear/reappear + Save-enabled assertions unchanged; 4.2 shows the warning for the bare name `powershell.exe` |
| Focused verification covers platform variants | One real-module file (POSIX) + one mocked file (Windows); both deterministic on any host |

## 6. Scope, rollback, delivery

In scope: `src/sidebar/components/SettingsModal.tsx`, `src/sidebar/components/SettingsModal.default-shell.test.tsx`, new `src/sidebar/components/SettingsModal.default-shell.windows.test.tsx`, this plan.

Out of scope: `src/shared/platform.ts` (no `isMac`); Rust defaults (`settings.rs`); the validator (`isPlausibleCompleteExecutablePath`); `src-tauri/`; CSS; docs/CHANGELOG/versioning; host-platform IPC.

Rollback: revert the commit for the component + tests; the copy is presentation-only and no persisted value or contract changes.

Commit for implementation: `#1951 fix(settings): platform-specific default shell guidance` — one commit with the three files, then run the verification in section 5.
