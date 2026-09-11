# #1925 Phase 4: the Settings checkbox for remote blocking-menu patterns

Status: READY_FOR_IMPLEMENTATION

- Parent issue: https://github.com/mblua/AgentsCommander/issues/1925. Child issue: created by `ac-tech-lead-v4`; its number arrives in your Step 8 handoff.
- Branch: `feature/<child-issue>-frontend-checkbox`, exactly as named in the Step 8 handoff, cut from `origin/main` at this phase's Step 8.
- Drift baseline: `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, where this plan was verified. Every line number below is pinned to it. It is not the branch point.
- Class: `patterned`. It mirrors the `npmUpdateNotificationsEnabled` checkbox exactly. Owner: `ac-dev-webpage-ui-v4`.
- Depends on: phase 2, which adds `AppSettings.remote_blocking_menus_enabled`, serialized `remoteBlockingMenusEnabled`, default `true`. Parallel with: phase 3. Phases 5 and 6 depend on this phase.
- Contract: consumer side of the phase 2 settings-schema change. No new IPC command.
- Delivery: commit on the phase branch and report the head SHA to `ac-tech-lead-v4`. Push only as the Step 8 handoff says. `ac-tech-lead-v4` opens and lands the PR and collects the exact-head CI evidence.

## 1. Objective

The repo owner decided the download flag must have a checkbox in Settings. Add the field to the TypeScript settings type, the checkbox in the General section beside the npm update checkbox, and the field to every full `AppSettings` fixture so type checking stays green.

This phase lands before the download (phase 6) so that no state of `main` has the default-on request without a switch. Until phase 6 lands, the checkbox saves and reloads the flag but nothing reads it: no request exists yet.

## 2. Before any write (mandatory, from the repo root)

1. Branch and base:
   ```
   git fetch origin main
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD origin/main
   git status --porcelain
   ```
   The branch equals the Step 8 name and matches `^feature/[1-9][0-9]*-frontend-checkbox$`; both SHAs are equal; status prints nothing.
2. Landed dependency (phase 2): `grep -c 'pub remote_blocking_menus_enabled: bool' src-tauri/src/config/settings.rs` prints `1`. Otherwise stop.
3. Drift:
   ```
   git cat-file -e "b4f7c9de7f15b75f8f12c034c8356f73babd0a8d^{commit}" || git fetch --deepen=200 origin main
   git diff --name-only b4f7c9de7f15b75f8f12c034c8356f73babd0a8d "$(git merge-base HEAD origin/main)"
   ```
   Expected entries: files of phases 1 and 2 of #1925 and, if it landed, phase 3 (which changes `package.json`). If the list names a file in section 3, `package-lock.json`, `tsconfig.json`, `vite.config.*`, or a `package.json` change that is not phase 3's two `check:served-paths` scripts, stop and report it to `ac-tech-lead-v4`.
4. State the environment risk in writing to `ac-tech-lead-v4` before touching code: Windows host; your local Node version (CI `frontend-regression` uses Node 22); the clone is shallow; `core.autocrlf=true` checks `src/` files out CRLF, but the section 7 greps are unanchored and the section 8 diffs compare commits, so CRLF does not change them; any other local risk you see.

## 3. Exact files and symbols

| File | Anchor at the drift baseline | Change |
|---|---|---|
| `src/shared/types.ts` | `npmUpdateNotificationsEnabled: boolean;` at `:685` | add `remoteBlockingMenusEnabled: boolean;` on the next line |
| `src/sidebar/components/SettingsModal.tsx` | the `npmUpdateNotificationsEnabled` `<label>` block at `:2102-2113` | add the new `<label>` block directly after it |
| `src/shared/testing/ui-harness.tsx` | `:181` | add `remoteBlockingMenusEnabled: true,` on the next line |
| `src/sidebar/components/AgentPickerModal.test.tsx` | `:185` | same |
| `src/sidebar/components/CodingAgentQuickConfiguration.test.ts` | `:124` | same |
| `src/sidebar/components/OnboardingModal.test.ts` | `:125` | same |
| `src/sidebar/components/SettingsModal.test.ts` | `:86` | same |
| `src/sidebar/components/settings-save.test.ts` | `:100` | same |
| `src/sidebar/components/SettingsModal.automation.test.ts` | `:207` | same, plus one new test |

Each fixture anchor line is `npmUpdateNotificationsEnabled: true,` inside a builder that returns a full `AppSettings`. Leave the other `npmUpdateNotificationsEnabled` lines in `settings-save.test.ts` (`:279-392`) and the `settings({...})` partial overrides alone: they are merge inputs, not full fixtures. If `npm run typecheck` still reports a missing `remoteBlockingMenusEnabled` in some other place, add the key there too and list that path in your report.

## 4. Decisions (binding for this phase)

- D1. Field name `remoteBlockingMenusEnabled`, type `boolean`, default `true` in every fixture, matching the Rust default.
- D2. The checkbox goes in the General section, directly after the npm update checkbox, and copies that block exactly: a `settings-checkbox-field` label, a `settings-checkbox` input, `checked={settings.data!.remoteBlockingMenusEnabled}`, `onChange` calling `updateField("remoteBlockingMenusEnabled", e.currentTarget.checked)`, and `data-ac-testid="settings.general.remoteBlockingMenusEnabled"`.
- D3. Visible label text, exactly: `Download blocking-menu pattern updates from GitHub`. The docs phase quotes this string, so do not reword it.
- D4. No new IPC call, store, API wrapper or save path: the existing draft save carries every `AppSettings` key.

## 5. Required behavior

The block to add after `SettingsModal.tsx:2113`:

```tsx
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.remoteBlockingMenusEnabled}
            onChange={(e) =>
              updateField("remoteBlockingMenusEnabled", e.currentTarget.checked)
            }
            data-ac-testid="settings.general.remoteBlockingMenusEnabled"
          />
          <span>Download blocking-menu pattern updates from GitHub</span>
        </label>
```

- Toggling the box changes the draft's `remoteBlockingMenusEnabled`, and Save sends it through `SettingsAPI.saveDraft` like every other General checkbox.
- On failure the existing save error handling applies unchanged. No new message.

## 6. Tests

In `SettingsModal.automation.test.ts`, add one test directly after the existing test `round-trips npmUpdateNotificationsEnabled through the General update-notify checkbox (#609)` (`:2018-2048`). Copy its shape: `render`, `settle`, `byTestId`, the mocked `SettingsAPI`.

- `round-trips remoteBlockingMenusEnabled through the General download checkbox (#1925)`:
  1. Render `SettingsModal` with the default fixture and `await settle()`.
  2. `byTestId<HTMLInputElement>("settings.general.remoteBlockingMenusEnabled")`: its `closest("label")?.textContent` contains `Download blocking-menu pattern updates from GitHub`, and `.checked` is `true`.
  3. Set `.checked = false`, dispatch `change` with `bubbles: true`, `await settle()`, click `settings.save`, `await settle()`.
  4. `const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];` then `saved?.remoteBlockingMenusEnabled` is `false` and `saved?.npmUpdateNotificationsEnabled` is `true`. The npm key is the control: it proves the new box does not drive the old flag.
  5. `dispose()`.

## 7. Verification (from the repo root, report exit codes and summary lines)

```
npm run typecheck
npx vitest run src/sidebar/components/SettingsModal.automation.test.ts src/sidebar/components/SettingsModal.test.ts src/sidebar/components/settings-save.test.ts src/sidebar/components/AgentPickerModal.test.tsx src/sidebar/components/CodingAgentQuickConfiguration.test.ts src/sidebar/components/OnboardingModal.test.ts
npm test
```

- `npm run typecheck` exits 0.
- The focused vitest run reports 0 failed, and the new test name appears in its output.
- `npm test`: CI runs it in `frontend-regression` under the #480 known-debt guard. Locally, report the summary; any failure outside the known #480 unhandled WebSocket rejection signature is a blocker.
- `grep -c 'settings.general.remoteBlockingMenusEnabled' src/sidebar/components/SettingsModal.tsx` prints `1`.
- `grep -c 'Download blocking-menu pattern updates from GitHub' src/sidebar/components/SettingsModal.tsx` prints `1`.

## 8. Acceptance criteria

1. Everything in section 7 holds.
2. `git diff --name-only "$(git merge-base HEAD origin/main)" HEAD` lists only the 9 paths in section 3, plus any path added under the typecheck rule and reported by name (ignore `plans/`).
3. `git diff "$(git merge-base HEAD origin/main)" HEAD -- src/sidebar/components/SettingsModal.tsx` adds exactly one `<label>` block of 12 lines and removes nothing.

## 9. Preserve

- The `npmUpdateNotificationsEnabled` checkbox, its label text, its test id and its position.
- `updateField` and the save pipeline, unchanged.
- No change to `menuGuardEnabled`: it keeps no frontend reference (explicitly out of scope).

## 10. Recovery

Restore only paths this phase changed that still hold this run's output (`git restore --source=HEAD -- <path>`). No `git reset`, `git clean` or repository-wide restore. Report external changes to `ac-tech-lead-v4`.
