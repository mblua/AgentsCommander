# Issue #1347 Plan (Lite): plaintext-secret warning under Gemini API Key and Telegram Bot Token

- Issue: #1347, `Warn in Settings UI that API keys and bot tokens are stored in plaintext, and show the exact settings.json path`
- Branch: `feature/1347-plaintext-secrets-warning`
- Planning base: `f9b8cbd5` (`f9b8cbd548a37fc176efbf6f6b189b3a160f4cfa`, branch head and `main` head, clean tree)
- Delivery path: Lite (one additive serialize-only backend field plus a localized UI notice; no schema, persistence, protocol, or runtime change). Visual app change.
- Plan storage: `plans/1347-plaintext-secrets-warning.md`
- Owners: `dev-rust` (Section 4.1, 4.2, 9 Rust test), `dev-webpage-ui` (Section 4.3 to 4.6, 9 TS tests)
Status: READY_FOR_IMPLEMENTATION

## 1. Objective

In Settings > Integrations, tell the user that the two secret inputs are persisted **unencrypted**, and name the exact file that holds them:

1. Under the `Gemini API Key` input, render a red informational notice.
2. Under the `Bot Token` input of **each** configured Telegram bot, render the same notice.

The notice names the absolute path of the instance `settings.json`. The frontend does not know that path today, so the backend adds it to the snapshot the UI already fetches.

Nothing else changes: no encryption, no ACL hardening, no change to the atomic writer, no change to what `get_settings` sends beyond the one new path field, no change to how the two secrets are stored, read, or consumed. The notice only informs; it never blocks saving and never alters a value.

## 2. Fixed evidence and current-state trace

All line numbers are at the planning base `f9b8cbd5`, verified by direct read.

**Backend**

1. `SettingsSnapshot` is declared at `src-tauri/src/commands/config.rs:198-204`: `#[derive(Debug, Clone, serde::Serialize)]`, `#[serde(rename_all = "camelCase")]`, with `#[serde(flatten)] pub settings: AppSettings` and `pub project_path_resolution: ProjectPathResolution`. Serialize-only; there is no `Deserialize` and no `deny_unknown_fields`.
2. The struct has exactly **one** construction site in the whole crate: `settings_snapshot_from` at `config.rs:390-403` (literal at `:399-402`). Every other reference is a type position or a call: `config.rs:469` (inside `settings_snapshot_helper`), `config.rs:473-474` (`get_settings` Tauri command), `web/commands.rs:382` (the WebSocket `get_settings` route, via `settings_snapshot_helper`), and the tests at `config.rs:2434`, `:2488`, `:2509-2515`, `:2518`, `:3736`, `:3760`, which all go through `settings_snapshot_from` / `settings_snapshot_helper`. Therefore adding a field to the struct cannot break any caller: no other struct literal exists.
3. `settings_snapshot_from` (`config.rs:387-403`) is the single shared builder for both transports. Its current body clones the settings, sets `cleaned.root_token = None` (`:396-398`) so the existing `skip_serializing_if` omits `rootToken`, and attaches the resolution report. Its doc comment at `:387-389` currently begins "Pure snapshot builder".
4. `crate::config::config_dir() -> Option<PathBuf>` is declared at `src-tauri/src/config/mod.rs:185-187`. It projects the cached `instance_location()`; the resolution runs once at first call from `std::env::current_exe()` with a `dirs::home_dir()` fallback (`mod.rs:160-167`). Documented pattern: `<binary_parent_dir>/.<binary_file_stem>/` (`mod.rs:180-184`). `None` only in the fully degraded mode where neither `current_exe()` nor a home dir resolves.
5. `commands::config` **already** depends on `crate::config`: `config.rs:424` calls `crate::config::config_dir().map(|d| d.join("settings.json"))` and `:431` calls `crate::config::settings::refresh_and_decode_project_paths_from_path`. The exact expression this plan needs is already present in this file.
6. `settings_snapshot_helper` (`config.rs:408-470`) accepts a test-injectable `settings_path: Option<PathBuf>`. That parameter is the **reconciliation write target** only (`:422-424`); it is not plumbed into `settings_snapshot_from` and does not represent the production settings location for a client.
7. `AppSettings` (`src-tauri/src/config/settings.rs:271-273`) derives `Serialize, Deserialize` with `rename_all = "camelCase"` and **no** `deny_unknown_fields`, so the `update_settings` / save-draft path silently ignores keys it does not know. This is already relied upon today: the frontend round-trips `projectPathResolution` back into `saveDraft` (Section 2, item 13) and it is dropped harmlessly.
8. The two secrets live in `settings.json` in that config dir, in cleartext: `gemini_api_key: String` (`config/settings.rs:333`) and `TelegramBotConfig.token: String` (`src-tauri/src/telegram/types.rs:8`). No encryption module exists in `src-tauri/src`. (Verified in the #1347 Step 1 investigation report.)

**Frontend**

9. `src/shared/types.ts:872-874`: `export interface SettingsSnapshot extends AppSettings { projectPathResolution: ProjectPathResolution; }`. The `ProjectPathResolution` sibling above it (`:863-868`) models a nullable field as `reconciliationError: ProjectPathReconciliationError | null`, which is this file's convention for a Rust `Option<T>` that is always serialized.
10. `SettingsAPI.get()` (`src/shared/ipc.ts:315-319`) is `transport.invoke<SettingsSnapshot>("get_settings")`.
11. The Gemini API Key field is `src/sidebar/components/SettingsModal.tsx:3696-3707` (input `type="password"` at `:3700`), inside `renderIntegrationsTab` (`:3679`) and inside `<Show when={settings.data!.voiceToTextEnabled}>` (`:3695-3751`).
12. The Bot Token field is `SettingsModal.tsx:3786-3797` (input `type="password"` at `:3790`), inside `<For each={settings.data!.telegramBots || []}>{(bot, i) => ...}` (`:3758-3759`), in a `.settings-button-card` per bot. The `Chat ID` block follows at `:3798-3803`.
13. The modal store is `createStore<{ data: AppSettings | null }>` (`SettingsModal.tsx:644-646`), typed `AppSettings`, **not** `SettingsSnapshot`. `onMount` (`:1001-1016`) does `const [loaded, wsRunning, apiRunning] = await Promise.all([SettingsAPI.get(), ...])` (so `loaded` is typed `SettingsSnapshot`), then `setSettings("data", cloneSettings(loaded))` at `:1011-1013`. `cloneSettings` (`:192-199`) is typed `(AppSettings | null) => AppSettings | null`, so the snapshot-only keys survive at runtime but are erased from the type. Consequently **`settings.data` cannot be used to read a snapshot-only field in a type-safe way**; the value must be read from `loaded` at the `onMount` boundary.
14. The save path is `saveCurrentSettingsDraft` (`SettingsModal.tsx:701-719`): `mergeSettingsForSavePreservingProjects(settings.data, await SettingsAPI.get(), modalSeed())` then `SettingsAPI.saveDraft(nextSettings)`. `mergeSettingsForSavePreservingProjects` (`src/sidebar/components/settings-save.ts:39-69`) returns `{ ...draft, ... }`, so snapshot-only keys ride along and are dropped by serde (item 7). This is the pre-existing behavior for `projectPathResolution`.
15. The Integrations tab body renders only inside `<Show when={settings.data}>` (`SettingsModal.tsx:3903-3913`), with `<Show when={activeTab() === "integrations"}>{renderIntegrationsTab()}</Show>` at `:3909-3911`. `resolveSettingsSection` maps `"integrations"` to that tab (`:154-158`).
16. Red hint styles already exist and are **not** touched: `.settings-hint` base at `src/sidebar/styles/sidebar.css:3253-3259` (11px, `var(--sidebar-fg-dim)`, line-height 1.4, bottom margin), `.settings-hint-error` at `:3274-3280` (`#f87171`) and `html.light-theme .settings-hint-error` at `:3282-3284` (`#b91c1c`), both introduced by #1313. Neither sets any wrapping property.
17. Markup precedent for exactly this notice: `SettingsModal.tsx:1783-1789`, a `<div class="settings-hint settings-hint-error" data-ac-testid="settings.general.defaultShell.warning">` placed after its field's `</label>`, inside the section, with no `role` / `aria-live`. `<div>` is the dominant element for `.settings-hint` in this file (`:572`, `:1958`, `:2022`, `:2061`, `:2189`, `:2431`, `:2454`, `:2602`, `:2881`, `:2909`, `:2941`, `:3176`, and more); a single `<p class="settings-hint">` exists at `:3963`.
18. Kebab-case inline `style` objects are an established convention in this codebase: `ActionBar.tsx:404` `style={{ "max-width": "380px" }}`, `ProjectPanel.tsx:3218` `style={{ "flex-shrink": "0" }}`, `:3540` `style={{ "margin-top": "10px" }}`, `:3591` `style={{ "font-size": "11px", opacity: 0.85 }}`. `SettingsModal.tsx:3763-3765` already uses `style={{ background: bot.color }}`.
19. The UI language of this modal is **English**: `SettingsModal.tsx:3693` `Enable microphone button on sessions`, `:3733` `Auto-execute after transcription`, `:3697` `Gemini API Key`, `:3766` `New Bot`, `:3771` `Remove bot`, `:3787` `Bot Token`. Verified, not assumed. The notice copy is therefore English.
20. Two existing factories are typed `SettingsSnapshot` and list every required field explicitly, so a new **required** field on that interface breaks their typecheck until updated: `src/shared/testing/ui-harness.tsx:202-210` (`settingsSnapshot`, which spreads `baseSettings(...)` and adds `projectPathResolution`) and `src/sidebar/components/SettingsModal.automation.test.ts:71-173` (local `settings()`, which sets `projectPathResolution` at `:166-171`). `baseSettings` (`ui-harness.tsx:106-185`) returns `AppSettings` and is deliberately legacy-shaped (`ui-harness.tsx:187-189`); it is **not** touched.
21. `FakeTransport.resolve(cmd: string, value: unknown)` (`src/shared/testing/fake-transport.ts:38-40`) is untyped, so a test may resolve `get_settings` with any object shape. `SettingsModal.default-shell.test.tsx:60` already resolves it with a bare `baseSettings(...)`.
22. Test harness conventions for a modal component test, from `SettingsModal.default-shell.test.tsx` (the #1313 sibling of this change): `// @vitest-environment jsdom` first line; imports `FakeTransport` from `../../shared/testing/fake-transport` and `baseSettings`/`installBrowserDomStubs`/`renderWithFakeTransport`/`resetUiStoresForTests`/`waitFor` from `../../shared/testing/ui-harness`; `beforeEach` installs DOM stubs and resets UI stores; `afterEach` cleans up and calls `document.body.replaceChildren()`; a local `byTestId` helper querying `[data-ac-testid="..."]`; four fake resolves (`get_settings`, `get_web_server_status`, `get_coding_agent_catalog`, `list_reseedable_agent_commands`); `r.cleanup()` in a `finally`.

## 3. Scope and non-goals

### In scope

- One additive `Option<String>` field on `SettingsSnapshot` (serialize-only), populated in the single shared builder from `crate::config::config_dir()`.
- The mirrored `string | null` field on the TypeScript `SettingsSnapshot` interface, plus the two `SettingsSnapshot`-typed test factories that must list it (evidence item 20).
- One module-scope notice component in `SettingsModal.tsx`, one signal carrying the path, one assignment in the existing `onMount`, and two render sites (Gemini API Key, per-bot Bot Token).
- The minimal tests: one Rust unit test on the new field, one new TSX component test file with two cases.

### Out of scope (do not add any of it)

- Encryption of any kind (DPAPI, keyring, passphrase), Windows DACL hardening, and any change to `write_value_atomic` or `save_settings_value_locked`. These are separately tracked follow-ups.
- Filtering `geminiApiKey` / `telegramBots[].token` out of `settings_snapshot_from` for web-server clients. Separately tracked; it changes the edit contract of this modal and is explicitly not this issue.
- Any notice on the coding-agent profile `env` rows or on `agents[].envs[].value`. The issue asks for the two named secret inputs. If wanted later, it is a separable follow-up that reuses the component added here verbatim, with no change to anything in this plan.
- Any CSS change. `sidebar.css` must not appear in the diff.
- Any `AppSettings` change, any persisted-schema change, any migration, any new Tauri command, any new event, any new dependency, any module move.
- Changing the modal store's type, the `settingsStore` signal type, `cloneSettings`, `mergeSettingsForSavePreservingProjects`, or the save flow.
- `role`, `aria-live`, or any accessibility attribute beyond what the precedent at `SettingsModal.tsx:1783-1789` carries (it carries none).
- Adding `data-ac-testid` to the two `<input>` elements. The tests locate the notices directly by their own testids; the inputs are not typed into.

## 4. Decided solution

### 4.1 Backend: the new snapshot field

In `src-tauri/src/commands/config.rs`, extend the struct at `:198-204` to:

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    #[serde(flatten)]
    pub settings: AppSettings,
    pub project_path_resolution: ProjectPathResolution,
    /// #1347 - absolute path of the instance `settings.json` this snapshot was
    /// built for, so the UI can name the file that holds the plaintext secrets.
    /// `None` only when `config_dir()` itself is unresolvable (no `current_exe()`
    /// and no home dir); there is no `skip_serializing_if`, so the key is always
    /// present on the wire and is `null` in that degraded mode.
    pub settings_file_path: Option<String>,
}
```

Fixed details:

- No `skip_serializing_if`. The key is always emitted (`settingsFilePath: "..."` or `settingsFilePath: null`), which is what the TypeScript `string | null` mirror in 4.3 asserts.
- The field sits after `project_path_resolution`, outside the `#[serde(flatten)]` member, so it serializes as a sibling of the flattened `AppSettings` keys under `rename_all = "camelCase"` -> `settingsFilePath`.

### 4.2 Backend: populating it in the single shared builder

In the same file, replace the doc comment and body of `settings_snapshot_from` (`:387-403`) with:

```rust
/// Snapshot builder: clone `settings`, clear the root token, attach the
/// resolution report, and record the instance settings-file path (#1347).
/// Shared by the Tauri and WebSocket transports so both clients receive the
/// identical report and `rootToken` is absent from each. The only non-argument
/// input is the process-wide, cached instance location behind `config_dir()`.
pub(crate) fn settings_snapshot_from(
    settings: &AppSettings,
    reconciliation_error: Option<ProjectPathReconciliationError>,
) -> SettingsSnapshot {
    let resolution =
        build_project_path_resolution(&settings.project_path_state, reconciliation_error);
    let mut cleaned = settings.clone();
    // Clear so the existing skip_serializing_if omits rootToken (absent, not null).
    cleaned.root_token = None;
    SettingsSnapshot {
        settings: cleaned,
        project_path_resolution: resolution,
        // #1347: derived from the process instance location, deliberately NOT
        // from `settings_snapshot_helper`'s injectable `settings_path`, which is
        // a test-only reconciliation write target and not a client-facing
        // location. Same expression already used at the reconciliation site.
        settings_file_path: crate::config::config_dir()
            .map(|d| d.join("settings.json").to_string_lossy().into_owned()),
    }
}
```

Fixed details and rationale:

- This is Option A of the Step 1 investigation report, approved by the coordinator. Option B (a new `get_settings_file_path` command) is rejected: more LOC, an extra IPC round trip, and a second source of truth for a value the UI already receives in one fetch.
- `settings_snapshot_from` is the only construction site (evidence item 2), so this single edit covers the Tauri command and the WebSocket route with no duplication.
- `to_string_lossy().into_owned()` matches the report's recommendation and is total: a non-UTF-8 Windows path yields the lossy rendering rather than dropping the notice's location.
- The doc comment must lose the word "Pure": after this change the function reads the cached process instance location. Reviewers rely on that comment.
- No other Rust file changes. `web/commands.rs` is untouched; it already routes through `settings_snapshot_helper` (`web/commands.rs:382`).

### 4.3 Frontend: the mirrored type

In `src/shared/types.ts`, replace `:870-874` with:

```ts
/** The flattened `get_settings` response: the runtime-selected AppSettings plus
 *  the structured resolution report. */
export interface SettingsSnapshot extends AppSettings {
  projectPathResolution: ProjectPathResolution;
  /** #1347 - absolute path of the instance settings.json this snapshot came
   *  from, or null when the backend could not resolve its config dir. Read-only
   *  metadata: it is never edited, never part of a save payload. */
  settingsFilePath: string | null;
}
```

Required (not optional) with `| null`, mirroring the always-present nullable key of 4.1 and the `reconciliationError: ... | null` convention of the sibling interface (evidence item 9).

### 4.4 Frontend: the notice component

In `src/sidebar/components/SettingsModal.tsx`, at module scope, immediately after `cloneSettings` (that is, after `:199`), add:

```tsx
/** #1347 - the notice shown under every settings input whose value is persisted
 *  unencrypted in the instance settings.json. `path` is the backend-resolved
 *  absolute file path (SettingsSnapshot.settingsFilePath); a null path degrades
 *  to the same warning without the location, because the storage fact holds
 *  whether or not config_dir() resolved. `overflow-wrap` is inline so a long
 *  path with no spaces wraps inside the modal instead of overflowing; the shared
 *  .settings-hint rules stay untouched. */
const PlaintextSecretHint: Component<{ path: string | null; testId: string }> = (props) => (
  <div
    class="settings-hint settings-hint-error"
    style={{ "overflow-wrap": "break-word" }}
    data-ac-testid={props.testId}
  >
    Stored unencrypted in {props.path ?? "this instance's settings.json file"}.
    Anyone who can read that file can read this value.
  </div>
);
```

Fixed details and rationale:

- `Component` is already imported and used in this file (`:642`); no new import is added.
- `<div>`, `class="settings-hint settings-hint-error"`, `data-ac-testid`, no `role`/`aria-live`: byte-for-byte the shape of the #1313 precedent (evidence item 17).
- Inline `overflow-wrap: break-word` is the layout guard. The path is a single token with no spaces (for example `D:\0_repos\AgentsCommander_iac\.agentscommander_ac2\settings.json`), and neither `.settings-hint` nor `.settings-hint-error` sets any wrapping property (evidence item 16). `break-word` breaks a long word only when it cannot fit, and unlike `anywhere` it does not affect intrinsic min-content sizing, so no surrounding layout can shift. Kebab-case inline style is this codebase's convention (evidence item 18). This keeps `sidebar.css` out of the diff, as required.
- One component, one copy string, two call sites: the Gemini notice and the bot notice are never allowed to drift.
- The `?? ` fallback yields exactly one of these two sentences pairs:
  - path known: `Stored unencrypted in D:\...\settings.json. Anyone who can read that file can read this value.`
  - path null: `Stored unencrypted in this instance's settings.json file. Anyone who can read that file can read this value.`
  The JSX line break between the two sentences collapses to a single space; JSX text nodes do not process backslash escapes, and the path is an interpolated runtime string, so no escaping applies to it either.

### 4.5 Frontend: carrying the path into the component

Inside `SettingsModal`, immediately after the `saveError` signal (`:685`), add:

```tsx
  // #1347 - snapshot-only metadata, kept out of `settings.data` on purpose: it
  // is not a setting, must never enter the draft, the dirty check, or the save
  // payload, and the draft store is typed AppSettings and cannot carry it.
  const [settingsFilePath, setSettingsFilePath] = createSignal<string | null>(null);
```

In the existing `onMount` (`:1001-1016`), insert one line immediately after `setApiServerRunning(apiRunning);` (`:1009`), before the `if (!draftDirty())` block:

```tsx
    // `?? null` tolerates a mixed-version backend that predates #1347.
    setSettingsFilePath(loaded.settingsFilePath ?? null);
```

Fixed details and rationale:

- `loaded` is typed `SettingsSnapshot` (evidence item 13), so this is type-safe with no cast and no import change.
- Placed **outside** the `if (!draftDirty())` guard: the path is authoritative metadata, never draft state, and must be set even when a dirty draft is preserved.
- Placed **before** `setSettings("data", ...)` so the path is already known on the first render in which the tab body can appear (the body is gated on `settings.data`, evidence item 15).
- Set once. The path is derived from `current_exe()` and cached process-wide by `instance_location()`; it cannot change while the app runs. Nothing is added to the conflict-reload path at `:1707-1717`.
- `createSignal` is already imported and used throughout the component.

### 4.6 Frontend: the two render sites

**Gemini API Key.** In `renderIntegrationsTab`, immediately after the field's closing `</label>` at `:3707` and before the `Gemini Model` label at `:3708`:

```tsx
          </label>
          <PlaintextSecretHint
            path={settingsFilePath()}
            testId="settings.integrations.geminiApiKey.plaintextWarning"
          />
          <label class="settings-field">
            <span class="settings-label">Gemini Model</span>
```

**Bot Token.** Inside the `<For>` body, immediately after the token field's closing `</label>` at `:3797` and before the `Chat ID` `<Show>` at `:3798`:

```tsx
            </label>
            <PlaintextSecretHint
              path={settingsFilePath()}
              testId={`settings.integrations.telegramBots.${i()}.plaintextWarning`}
            />
            <Show when={bot.chatId}>
```

Fixed details and rationale:

- **Per bot, not once per section.** Decided. The notice must sit where the user pastes the secret; a bot card is tall (label, token, chat id, color, and more), so a single section-level line is easily scrolled past. Field adjacency also makes the Gemini and Telegram treatments identical, which is why one component serves both. With zero bots configured no notice renders, which is correct: no bot token is stored. The realistic bot count is one to three and the notice is 11px text, so the repetition cost is bounded.
- Both notices render unconditionally within their already-conditional containers. No new `<Show>`: the Gemini field only exists under `<Show when={settings.data!.voiceToTextEnabled}>` (`:3695`), and the bot notice only exists per iterated bot. Therefore the notice is visible exactly when its input is visible.
- The bot testid is index-based (`i()` from the `<For>` callback), which needs no assumption about a bot id field and is stable for the tests, which render a fixed bot list.
- Passing `settingsFilePath()` inside JSX props is compiled by Solid into a getter, so the prop stays reactive and both notices pick up the path when `onMount` resolves.

## 5. Affected surfaces (exact files and symbols)

| File | Owner | Change |
| --- | --- | --- |
| `src-tauri/src/commands/config.rs` | dev-rust | Add `pub settings_file_path: Option<String>` to `SettingsSnapshot` (4.1). Rewrite the `settings_snapshot_from` doc comment and add the one field initializer (4.2). Add the Rust unit test (Section 9). No other change. |
| `src/shared/types.ts` | dev-webpage-ui | Add `settingsFilePath: string \| null` to the `SettingsSnapshot` interface (4.3). No other change. |
| `src/sidebar/components/SettingsModal.tsx` | dev-webpage-ui | Add module-scope `PlaintextSecretHint` after `cloneSettings` (4.4). Add the `settingsFilePath` signal after `:685` and one `setSettingsFilePath` line in `onMount` after `:1009` (4.5). Insert the two `<PlaintextSecretHint />` elements after `:3707` and after `:3797` (4.6). No other change. |
| `src/shared/testing/ui-harness.tsx` | dev-webpage-ui | In `settingsSnapshot` (`:206-209`), add `settingsFilePath: null,` after the `projectPathResolution` line. Required because the interface field is required (evidence item 20). `baseSettings` is NOT touched. |
| `src/sidebar/components/SettingsModal.automation.test.ts` | dev-webpage-ui | In the local `settings()` factory, add `settingsFilePath: null,` after the `projectPathResolution` block that ends at `:171`. Same reason. No test body changes. |
| `src/sidebar/components/SettingsModal.plaintext-secrets.test.tsx` | dev-webpage-ui | NEW test file (Section 9). |
| Everything else | - | Untouched. Explicitly: `src/sidebar/styles/sidebar.css`, `src/shared/ipc.ts`, `src/shared/stores/settings.ts`, `src/sidebar/App.tsx`, `src/sidebar/components/settings-save.ts`, `src-tauri/src/web/commands.rs`, `src-tauri/src/config/settings.rs`, `src-tauri/src/config/mod.rs`, `src-tauri/src/telegram/*`, `Cargo.toml`, `package.json`. |

No new Tauri command, no new event, no persisted-schema change, no migration, no new dependency, no module restructure. `git diff` after implementation must touch exactly those six files (five modified, one added).

## 6. Required behavior, edge cases, failure behavior

Required behavior:

- B1. With Voice to Text enabled and the backend resolving its config dir, Settings > Integrations shows a red notice directly under the `Gemini API Key` input reading `Stored unencrypted in <absolute path to settings.json>. Anyone who can read that file can read this value.`
- B2. Each configured Telegram bot shows the identical notice directly under its `Bot Token` input, above its `Chat ID` row.
- B3. The notice never blocks anything: the Save button is unaffected, `currentValidationError()` is untouched, and no stored value changes.
- B4. `get_settings` (Tauri and WebSocket alike) carries `settingsFilePath` as a top-level key of the snapshot.

Edge cases, all decided:

- E1. **`settings_file_path` is `None`** (`config_dir()` unresolvable). Decided: **render the notice in degraded form, without the path** (`this instance's settings.json file`). Rationale: the security fact the issue is about is that these values are stored unencrypted, and that is true whether or not the process can name the file. Suppressing the notice would hide the important half in exactly the degraded mode where the user has the least information, and would make the warning's presence depend on an unrelated failure. The alternative (hide the notice) is explicitly rejected.
- E2. **Mixed-version backend** that predates 4.1 and omits `settingsFilePath` entirely. Behaves as E1 via the `?? null` at 4.5. Mirrors the #1077 absent-report legacy fallback convention.
- E3. **Voice to Text disabled.** The `Gemini API Key` input does not render, so neither does its notice. Correct: there is no visible secret input to annotate. The Telegram notices are unaffected.
- E4. **Zero Telegram bots.** No bot card, so no bot notice. Correct: no token is stored.
- E5. **Multiple bots.** One notice per bot, each naming the same path. Decided in 4.6.
- E6. **Long path with no spaces.** Wraps inside the modal because of the inline `overflow-wrap: break-word` (4.4). No horizontal overflow, no layout shift.
- E7. **First paint before `onMount` resolves.** When the modal is opened in the running sidebar, `settingsStore.current` is already populated, so the tab body can paint from the seed while `SettingsAPI.get()` is still in flight; the notice then shows the E1 degraded copy for that interval and gains the path when `onMount` resolves. Accepted, not a defect: the degraded copy is a complete and correct warning on its own (which is exactly why E1 degrades instead of hiding), so the transition adds information rather than correcting a falsehood. No extra loading state, no gating, and no widening of the `settingsStore` signal type is introduced to avoid a sub-second copy change on one tab.
- E8. **Save round-trip.** `settingsFilePath` may ride along in the object passed to `saveDraft` (via the `{ ...draft }` spread at `settings-save.ts:68-69`), exactly as `projectPathResolution` does today. `AppSettings` has no `deny_unknown_fields` (evidence item 7), so serde ignores it and nothing is persisted. The field is never bound to `updateField`, never editable, and never part of the dirty check.
- E9. **Windows non-UTF-8 path.** `to_string_lossy()` yields a replacement-character rendering rather than `None`, so the notice still names a recognizable location.

Failure behavior: there is no new failure mode. The backend expression is total (`Option::map`); the component is pure JSX with a total `??` fallback and cannot throw; no new IPC call, no new async path, no new error surface. If `get_settings` itself fails, the modal's pre-existing `onMount` behavior is unchanged and the signal simply stays `null` (E1).

## 7. Compatibility and security impact

- **Wire compatibility.** `SettingsSnapshot` is `Serialize`-only and gains one additive key. An older frontend against a newer backend ignores it (TypeScript structural typing tolerates extra keys at runtime). A newer frontend against an older backend gets E2. No persisted format changes; `settings.json` written before and after this change is byte-compatible. No migration.
- **Save path.** Unchanged. `AppSettings` accepts and ignores unknown keys (evidence item 7), which is already exercised by `projectPathResolution`.
- **Cross-platform.** The path comes from `config_dir()`, which is already the platform-correct resolver with a documented home-dir fallback. The notice text is platform-independent.
- **New information disclosure, stated explicitly.** Because `settings_snapshot_from` is shared with the WebSocket route (`web/commands.rs:382`), the absolute config-dir path is now also sent to web-server clients. Decided: **accept**. The value is a local filesystem path, not a credential; the same response already carries `geminiApiKey` and `telegramBots[].token` in the clear (the separately tracked, out-of-scope leak identified in the Step 1 report), so this adds no meaningful capability to any party who can reach that endpoint, and the coordinator approved Option A knowing the helper is shared. Recorded here so the follow-up that filters those two secrets can decide about this key at the same time.
- **No new attack surface otherwise.** No filesystem write, no command construction, no new endpoint, no new dependency. The secrets themselves are neither moved nor re-encoded.
- **The notice is honest about its limits.** It states where the value is stored and who can read it. It makes no claim about protection, because none is added by this change.

## 8. Implementation order

The frontend depends on the backend field, so the order across the two owners is fixed:

1. **dev-rust** applies 4.1 and 4.2 in `src-tauri/src/commands/config.rs`, adds the Rust test of Section 9, and runs A1 and A2. Hands off when green.
2. **dev-webpage-ui** applies 4.3 (`src/shared/types.ts`), then the two factory lines of Section 5 (`ui-harness.tsx`, `SettingsModal.automation.test.ts`), then 4.4, 4.5 and 4.6 in `SettingsModal.tsx`, in that order (component before signal before render sites, so the file typechecks at each step).
3. **dev-webpage-ui** adds `src/sidebar/components/SettingsModal.plaintext-secrets.test.tsx` (Section 9) and runs A3, A4 and A5.
4. Either owner runs A6 and A7 on the final branch head.

Step 2 must not start before step 1 is committed: `loaded.settingsFilePath` at 4.5 does not typecheck until 4.3 lands, and 4.3 is only truthful once 4.1 emits the key.

## 9. Tests and objective acceptance criteria

### Rust (dev-rust)

One test, in the existing `#1077 SettingsSnapshot / resolution report` submodule of `src-tauri/src/commands/config.rs` (opened at `:2336-2338`, which already imports `settings_snapshot_from`), added after the last existing test in that submodule:

```rust
    #[test]
    fn snapshot_carries_the_instance_settings_file_path() {
        // #1347: the UI names the file that holds the plaintext secrets. Assert
        // agreement with config_dir() rather than a literal path, so the test is
        // independent of where the test binary happens to live.
        let snap = settings_snapshot_from(&AppSettings::default(), None);
        match (crate::config::config_dir(), snap.settings_file_path) {
            (Some(dir), Some(path)) => assert_eq!(path, dir.join("settings.json").to_string_lossy()),
            (None, None) => {}
            (dir, path) => panic!("settings_file_path disagrees with config_dir: {dir:?} vs {path:?}"),
        }
    }
```

`AppSettings::default()` is already used in this test module (`config.rs:2541`). No existing Rust test changes: every one of them builds the snapshot through `settings_snapshot_from` / `settings_snapshot_helper` (evidence item 2), and none asserts an exact serialized key set.

### TypeScript (dev-webpage-ui)

One new file, `src/sidebar/components/SettingsModal.plaintext-secrets.test.tsx`, following `SettingsModal.default-shell.test.tsx` exactly for harness setup (evidence item 22): `// @vitest-environment jsdom` first line, the same imports, the same `beforeEach`/`afterEach`, the same local `byTestId` helper, `r.cleanup()` in a `finally`.

Render helper: build the `get_settings` payload as

```tsx
{ ...settingsSnapshot({ voiceToTextEnabled: true, telegramBots: [ /* two bots */ ] }), settingsFilePath: <value> }
```

(`settingsSnapshot` from `../../shared/testing/ui-harness`; the spread-and-override is needed because `settingsFilePath` is not part of `Partial<AppSettings>`, and `FakeTransport.resolve` is untyped so the shape is accepted, evidence item 21). Resolve the same four commands as the #1313 test, and render `<SettingsModal section="integrations" onClose={() => {}} />`. Use two bots so the per-bot decision of 4.6 is actually covered.

Test 1, path known (`settingsFilePath: "D:\\ac\\.agentscommander_ac2\\settings.json"`):

- `waitFor` `settings.integrations.geminiApiKey.plaintextWarning` to exist.
- Its `textContent` contains `Stored unencrypted in` and contains the exact path string.
- `settings.integrations.telegramBots.0.plaintextWarning` and `settings.integrations.telegramBots.1.plaintextWarning` both exist and both contain the same exact path string.
- The Gemini notice's `className` contains both `settings-hint` and `settings-hint-error`.
- The Save button (`settings.save`) is not disabled.

Test 2, degraded (`settingsFilePath: null`):

- `waitFor` the Gemini notice to exist.
- Its `textContent` contains `Stored unencrypted in` and `settings.json`, and does **not** contain the string `null`.
- Both per-bot notices exist with the same text.

Acceptance criteria for the implementers (all must pass):

- A1. `cargo test -p agentscommander snapshot_carries_the_instance_settings_file_path` green (run from `src-tauri`; use the crate name as declared in `src-tauri/Cargo.toml`).
- A2. `cargo test` for the `commands::config` and `web::commands` test modules green, in particular `get_settings_route_returns_snapshot_and_omits_root_token` (`web/commands.rs:993-1014`), which must still pass unchanged: the new key is not `rootToken` and carries no secret.
- A3. `npx vitest run src/sidebar/components/SettingsModal.plaintext-secrets.test.tsx` green.
- A4. `npx vitest run src/sidebar/components/SettingsModal.automation.test.ts src/sidebar/components/SettingsModal.default-shell.test.tsx src/sidebar/components/SettingsModal.test.ts src/sidebar/components/settings-save.test.ts` green, plus the remaining `SettingsModal.*.test.*` files.
- A5. `npm run typecheck` green.
- A6. `git diff --stat f9b8cbd5..HEAD` lists exactly the six files of Section 5. `git diff f9b8cbd5..HEAD -- src/sidebar/styles/sidebar.css` is empty. `git diff f9b8cbd5..HEAD -- src-tauri` touches only `src-tauri/src/commands/config.rs`. `git diff f9b8cbd5..HEAD -- src-tauri/Cargo.toml package.json` is empty.
- A7. Manual spot check (visual gate), Settings > Integrations: with Voice to Text enabled, the red notice appears under `Gemini API Key` naming the real `settings.json` path of this instance; with at least two bots configured, the same notice appears under each `Bot Token`; the full path wraps inside the modal with no horizontal scrollbar and no widened dialog, in both dark and light theme; the Save button stays enabled.

## 10. Dependency-cycle gate (mandatory, verify-no-dependency-cycles skill)

Enumerated module arcs added or removed by this plan, from the actual files:

- **Rust.** ZERO new arcs. The only Rust edit is inside `crate::commands::config`. The one non-local reference it adds is `crate::config::config_dir`, and the arc `commands::config -> config` already exists at `commands/config.rs:424` (`crate::config::config_dir().map(|d| d.join("settings.json"))`, the identical expression) and at `:431` (`crate::config::settings::refresh_and_decode_project_paths_from_path`). No `use` statement is added, no module is created, moved, or removed, and no other crate module is touched. The new test lives inside the same module's existing `#[cfg(test)]` tree and adds no arc that production code does not already have.
- **TypeScript, production.** ZERO new arcs. `src/shared/types.ts` gains one field on an existing interface and no import. `SettingsModal.tsx` gains a module-scope component, a signal, one `onMount` line and two JSX elements, all using symbols it already imports (`Component`, `createSignal`); it does **not** import `SettingsSnapshot` (the type flows in by inference from `SettingsAPI.get()`, which it already calls at `:1004`), so the `SettingsModal -> shared/types` import set is unchanged.
- **TypeScript, test-only.** `ui-harness.tsx` gains one property and no import (it already imports `SettingsSnapshot` at `:11`). `SettingsModal.automation.test.ts` gains one property and no import. The new `SettingsModal.plaintext-secrets.test.tsx` imports `./SettingsModal`, `../../shared/testing/fake-transport` and `../../shared/testing/ui-harness`, which are exactly the edges every existing `SettingsModal.*.test.*` file already has. Vitest entry points are not part of the app module graph and cannot join an SCC.

Per-arc verdicts: every arc this plan touches already exists; no arc crosses a previously-clean SCC boundary because no arc is new. `cyclicSccs` before and after: unchanged. SCC member sets: identical. Arc record: byte-identical (`src-tauri/module-arcs.txt` is not modified, by construction, since no Rust module-to-module reference changes).

Role and layering hygiene: no lower-layer module gains a `AppHandle` / `tauri` UI-transport dependency. `commands::config` is already the transport-facing layer and is where the transport-shaped `SettingsSnapshot` lives; the new field is co-located there rather than pushed down into `config::settings`. `config_dir()` remains a pure accessor in the lower layer and is only read from above. No transport-taking function moves downward, and no predicate moves upward.

**Gate result: PASS.**

Step-N acceptance criterion for the implementation reviewer (the levelization instrument is Rust-only; the per-arc analysis above is the documented manual fallback for the TypeScript half): on the final branch head, `git diff f9b8cbd5..HEAD -- src-tauri` must touch only `src-tauri/src/commands/config.rs`, `git status --porcelain src-tauri/module-arcs.txt` must be empty, and `git diff f9b8cbd5..HEAD -- src-tauri/src/commands/config.rs` must contain no added or removed `use ` line and no added `mod ` line. If any of those three fails, the Rust levelization run must be executed and its `cyclicSccs` and SCC member sets compared against the base before merge. The structural layering guards over spellings stay green because no cross-module reference, in either language, is added or removed.

## 11. Certification

Certification performed at the Lite gate (architect authors and certifies in one pass; no dev or grinch enrichment per the coordinator's workflow) on the exact plan bytes recorded in this file, against planning base `f9b8cbd5` on branch `feature/1347-plaintext-secrets-warning`, with a clean tree.

- The plan satisfies the Plan Contract: objective (1), fixed evidence (2), scope and non-goals (3), decided solution (4), affected surfaces (5), behavior, edges and failure (6), compatibility and security (7), implementation order across the two owners (8), tests and objective acceptance criteria (9), dependency-cycle gate (10). No TBD, no open alternative, no decision delegated to the implementer.
- Every file, line number, symbol, and convention cited in Section 2 was read directly at `f9b8cbd5`, not inherited from the Step 1 report. Two facts the report did not cover were established here and change the shape of the frontend work: the modal draft store is typed `AppSettings`, not `SettingsSnapshot` (item 13), so the path must be read at the `onMount` boundary rather than from `settings.data`; and two `SettingsSnapshot`-typed test factories enumerate every required field (item 20), so they must gain the new key in the same commit.
- Delivery path Lite is correct. The change is one additive serialize-only field on a struct with a single construction site, plus a localized notice in one component. No architectural decision, nothing cross-cutting, no non-obvious approach: no Full criterion is triggered.
- The four decisions the coordinator delegated are closed, with rationale in the body: (a) exact copy, in English, verified as the modal's language at item 19 and fixed in 4.4; (b) a `None` path renders the notice in degraded form without the location rather than hiding it (E1); (c) the scope stays at the two named inputs, with the coding-agent env rows recorded in Section 3 as a separable follow-up that reuses this component unchanged; (d) the notice renders once per bot, under each `Bot Token`, not once per section (4.6).
- Two facts are recorded that the coordinator should carry into the follow-ups rather than treat as defects here: the shared builder means the config-dir path also reaches web-server clients (Section 7, accepted with rationale), and the first paint of the tab can briefly show the degraded copy before `onMount` resolves (E7, accepted with rationale).
- Dependency-cycle gate: PASS, zero new module arcs in either language; see Section 10. Role and layering hygiene: no inversion.
- The final certified `Plan-SHA256` is reported in the architect's reply message; the digest is computed over the exact bytes of this file and cannot be embedded in it.

Final verdict: READY_FOR_IMPLEMENTATION. Blockers: none.
Status: READY_FOR_IMPLEMENTATION
