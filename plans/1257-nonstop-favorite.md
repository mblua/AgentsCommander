# Implementation Plan: #1257 Make the Non-stop pseudo-group ("Alert me!") favoritable

Status: READY_FOR_IMPLEMENTATION

Full path. Certified by the architect in the Step 7 consensus pass, round 1, after enrichment by `dev-rust` (Section 12), `dev-webpage-ui` (Section 13) and adversarial review by `dev-rust-grinch` (Section 14). No implementation decision is left open (Section 11).

**How to read this file.** Sections 1 through 11 are the plan and are what the implementer executes. They now incorporate every correction the three enrichment passes produced, so **Sections 12, 13 and 14 are the audit trail, not a second source of instructions**: where an enrichment section proposed a change, Sections 1 through 11 already carry it, and where the two ever read differently, Sections 1 through 11 win. Each correction is cross-referenced to the finding that produced it, and Section 11 tabulates all fourteen with their resolutions.

Sections 12 through 14 are preserved **byte-for-byte as their authors wrote them**, including statements this pass has since superseded (for example, 12.6 still says criterion 4 stays at 20 to 22 because R3 had not been adopted yet; it now is, and criterion 4 says 23). An audit trail that gets edited after the fact stops being one.

**None of D1 through D12 was challenged by any of the three passes.** D3 and D11, the two the adversarial pass was pointed at, were attacked deliberately and survived with executed evidence. Everything applied below is a correction to a supporting statement, a defective test artifact, or an owner assignment.

## 1. Issue, baseline and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1257 (`Allow the Non-stop pseudo-group (Alert me!) to be favorited from the groups rail`).
- Branch: `feature/1257-nonstop-favorite`, created from `main` at `f15f59a4b451cc6773be8604af4ccc9b0908f0a8`.
- **Baseline for every coordinate and every command in this plan: `f15f59a4`.** The branch carries no commits yet, so every `file:line` below is valid at branch HEAD. Working tree was clean when the coordinates were taken.
- Delivery classification: **FULL**. It crosses the IPC/persistence boundary (a new field on a serialized config struct in `src-tauri/` mirrored in `src/shared/types.ts`), changes on-disk `project-settings.json` shape, adds a store command, changes a context menu's contract, and rewrites a test that deliberately locks the current behaviour. None of that is LITE.

**Objective.** The Non-stop pseudo-entry in the workgroup groups rail, displayed under its user-editable name (`"Alert me!"` by default), can be added to and removed from the rail's cross-project `Favorites` section through its context menu, exactly like a user group, and the choice survives restart.

**Non-objective.** `All` and `Ungrouped` stay non-favoritable (Section 3). No change to what the Non-stop watchdog detects, alerts on, or matches. No change to the rail's reorder gesture, to the Favorites collapse bit, or to the groups modal.

## 2. Verified current state

Every claim below was re-verified against `f15f59a4` by reading the files, not inferred from the issue.

### 2.1 The UI gate

`src/sidebar/components/WorkgroupGroupRail.tsx:798`:

```tsx
<Show when={contextTarget()?.kind === "group"}>
  <button ... data-ac-testid="workgroupGroups.contextMenu.favorite">
    {favoriteTargetIsFavorited() ? "Unfavorite" : "Favorite"}
  </button>
</Show>
```

`Edit` (`:790-797`) always renders; `Favorite`/`Unfavorite` only for a `"group"` target. That is the single-item menu the issue reports.

### 2.2 Why the Non-stop entry is never a `"group"` target

Three links, all verified:

1. `RailContextTarget` has two variants only (`:45-47`): `{kind:"project"; projectPath}` and `{kind:"group"; projectPath; groupId}`.
2. The rail's `onContextMenu` picks the variant from `button.groupId` (`:602-609`): truthy means `"group"`, otherwise `"project"`.
3. The Non-stop button is built with `groupId: null` (`:337-352`, field at `:350`), because it is not a `groups[]` record. Compare `groupButtonFor` (`:131-148`), which sets `groupId: group.id` at `:145`.

Consequence: `groupId: null` resolves to `"project"`, the `Show` at `:798` is false, only `Edit` renders.

### 2.3 The structural blocker behind the gate

The favorite flag is persisted **inside the group record**, not in an external list of ids:

- `src-tauri/src/config/project_settings.rs:18-29` (`WorkgroupGroup`), field `favorite` at `:27-28` with `#[serde(default)]` and the #965 comment explaining the design: the flag survives rename, travels with reorder, and dies with the group.
- The Non-stop entry is `non_stop: Option<NonStopGroupConfig>` (`:41-42`), a sibling of `groups`, with no `id` and no `favorite` field (`NonStopGroupConfig` at `:86-101`).
- TS mirror: `src/shared/types.ts:1026-1031` has `WorkgroupGroup.favorite?: boolean` (`:1030`); `NonStopGroupConfig` (`:1043-1050`) has no such field.

So removing the `Show` alone leaves nowhere to write the flag. Both halves have to change.

### 2.4 The test that locks current behaviour

`src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx:387-405` asserts that `all`, `ungrouped` **and `nonstop`** expose `Edit` and no Favorite item. The current behaviour is deliberate, not an accident, and this plan rewrites that test on purpose (Section 5.5).

### 2.5 The silent-loss trap

`normalizeNonStop` (`src/sidebar/stores/workgroup-groups.ts:89-123`) rebuilds the object **field by field** and returns a fresh literal (`:104-122`). Any field not listed there is discarded. `setConfig` (`:384-398`) applies it at `:391`, and `setConfig` is the single write path for **both** load (`ensureLoaded`, `:423` and `:426`) and save (`save`, `:535`) and external updates (`applyExternalUpdate`, `:509`).

This is the single most important line of the change. **What exactly breaks without it** was corrected during enrichment (13.2) and then verified by execution against `f15f59a4` (14.5: feeding `favorite: true` through `get_project_groups` today yields `favorite present after load = undefined`). The failure is earlier and louder than "it reverts later":

1. **Load.** A `favorite: true` already on disk is stripped before it reaches the store, so the entry never renders at startup.
2. **The favoriting click itself.** `save()` sends the config **outbound** as `cloneConfig(config)` (`:529`), which is spread-safe, so the flag reaches disk correctly and the backend echoes it back. `setConfig` then strips it from the **response** (`:535`). The Favorites entry **never appears at all**, on the very first click.
3. **Only afterwards** does the on-disk value decay, because the next `save()` reads the now-flagless store config and `#[serde(default)]` writes `false`.

Point 2 is why acceptance criterion 9a, not 9b, is the manual detector for a missing D2, and why S1's very first assertion is the automated one. Criterion 9b covers point 3, which is a distinct and later failure.

`cloneConfig` (`:125-134`) is **not** a trap: it spreads (`{ ...config.nonStop, telegram: {...}, sound: {...} }`), so it carries any new scalar field for free. Do not "fix" it. It is also the **only** producer of a `nonStop` object in `src/` that could have been one: the full inventory (13.2) is the store's `cloneConfig` (`:130-131`), `addWorkgroupToNonStop` (`:655`, `:666`), `removeWorkgroupFromNonStop` (`:686`), the modal's `cloneConfig` (`WorkgroupGroupsModal.tsx:32-34`) and the modal's three patchers (`:139`, `:144`, `:149`), and every one of them spreads. `normalizeNonStop` is the single field-by-field rebuild in the whole product, on either side of the IPC boundary (14.2 checked the Rust side: `save_workgroup_groups` serializes the whole struct at `project_settings.rs:257-263` and `load_workgroup_groups` deserializes the whole struct at `:217`, so there is no Rust twin of this trap).

### 2.6 What renders Favorites today

`FavoritesRailSection` (`:240-291`). Entries (`:246-253`):

```ts
props.projects.flatMap((project) =>
  workgroupGroupsStore.config(project.path)
    .groups.filter((group) => group.favorite)
    .map((group) => ({ project, group, button: groupButtonFor(project, group, null, false) }))
)
```

Order is therefore project order times `config.groups` order, and the Non-stop entry has no index in `groups`, so a position has to be decided (Section 4, D4). Test ids come from `favoriteRailTestIds(folderName, groupId)` (`:122-129`), keyed on the group id. The section renders at all only when `entries().length > 0` (`:256`); its collapse bit lives in global settings (`rail_favorites_collapsed`, `src-tauri/src/config/settings.rs:483`) and is untouched here.

### 2.7 Facts that constrain the edge cases

- The rail draws the Non-stop button only when `nonStop?.show` (`:338`).
- `normalizeSelection` (`src/sidebar/stores/workgroup-groups.ts:191-193`) bounces a `{kind:"nonstop"}` selection to All (or Ungrouped) when `!config.nonStop?.show`.
- `isSelected` (`:150-155`) already handles a `"nonstop"` selection correctly with no change: it compares `current.kind !== button.selection.kind` first and only demands an id match for `"group"`.
- `nonStopMatchesWorkgroup(config, wg)` (`src/sidebar/stores/workgroup-groups.ts:307-312`) is the membership test for the Non-stop entry. `groupButtonFor` cannot be reused for it: it takes a `WorkgroupGroup` and compiles `group.regex` from a `groups[]` record.
- The automation bridge resolves a `data-ac-testid` by exact match and fails the request with `duplicate_selector` when more than one element matches (`src/shared/automation-bridge.ts:132-141`). Any new test id must be unique in the rendered DOM. It consumes ids **entirely from the live DOM** and keeps no registry, allowlist or generated catalog (`queryAutomationTargets`, `:335-338`; `availableTargets`, `:383-388`), so a new id needs nothing registered anywhere. It also queries across open shadow roots (`queryAcrossOpenRoots`, `:390-392`), so "unique in the rendered DOM" means the whole document tree, not just the rail (13.6).
- `createGroupId` (`:373-382`) falls back to `group-N` when `crypto.randomUUID` is unavailable, and `project-settings.json` is hand-editable, so the string `"nonstop"` is **not** reserved as a group id.
- **The project section already emits a duplicate test id in that configuration, today, independently of this issue.** `groupButtonFor` sets `key: group.id` (`:139`) and the Non-stop button sets `key: "nonstop"` (`:343`); both feed `projectRailTestIds(button.key)` (`:592`), which emits `workgroupGroups.button.${key}`. A hand-edited group whose id is `nonstop` (or `all`, or `ungrouped`) therefore collides with the corresponding pseudo-button. Executed on unmodified `f15f59a4` (14.3, F1): `duplicate count for workgroupGroups.button.nonstop = 2`, and the bridge answers `{"ok":false,"error":"duplicate_selector"}`. This constrains what Section 6 edge case 12 may promise, and it is recorded as adjacent finding 5 in Section 10. D5 is unaffected: it prevents this change from adding a **second** collision, inside Favorites.
- Rust builds `NonStopGroupConfig` by literal in exactly two places: `impl Default` (`project_settings.rs:102-113`) and the test helper `populated_non_stop()` (`:338-353`). Verified with `grep -rn "NonStopGroupConfig" src-tauri/ crates/ --include=*.rs`, and independently re-verified in 12.3 by a call-graph trace of `save_workgroup_groups`, which returns only two production callers and otherwise reaches tests. Compile blast radius is those two sites. (Two further Rust literals build the **parent** `WorkgroupGroupsConfig` with `non_stop: None` and keep compiling untouched: `web/commands.rs:1220-1230`, which is inside `#[cfg(test)] mod tests`, and `commands/project_settings.rs:64-76` (`sample_config()`). Neither is a `NonStopGroupConfig` literal.)
- The watchdog does not read the flag, and more strongly than "does not read it" (12.3): `src-tauri/src/loops/non_stop_watchdog.rs` never sees `NonStopGroupConfig` at all. It works off `NonStopReport`, a separate DTO the frontend pushes in through `non_stop_report` (`src-tauri/src/commands/non_stop.rs:12-14`). On the frontend, `src/sidebar/watchdog/non-stop-watchdog-client.ts` consumes `show`, `regex`, `toleranceSeconds`, `telegram` and `sound` only.
- Every TS fixture that builds a `NonStopGroupConfig` does so as `{ ...defaultNonStop(), ... }`, with one exception: `workgroup-groups.test.ts:341-348` passes a complete literal to `normalizeNonStop`. That exception is what makes the TS field optional rather than required (Section 4, D1). The fixture surface is **larger than an earlier draft of this list counted** (13, correction): besides `workgroup-groups.test.ts:210`, `:333`, `:336`, `:375`, `:388`, `:394`, `WorkgroupGroupRail.favorites.test.tsx:388` and `rail-watchdog-parity.test.tsx:54-58`, there are literals in `WorkgroupGroupRail.raise-hand.test.tsx:341-345`, `WorkgroupGroupRail.autofocus.test.tsx:70-74`, `WorkgroupGroupRail.test.tsx:531` and `:552`, and `WorkgroupGroupsModal.nonstop.test.tsx:89`. All of them spread `defaultNonStop()`, so none breaks, and the larger count **strengthens** D1: a required field would have cost more than the shorter list suggested. (`workgroup-groups.test.ts:226` and `:410` look like exceptions but are `toMatchObject` partial matchers, not typed values.)
- `FavoritesRailSection` never calls `ensureLoaded`; only `ProjectRailSection` does (`:298-300`). A favorited Non-stop entry therefore appears only once that project's rail section has loaded its config, exactly like a favorited group. Pre-existing and unchanged, recorded because D4 could be read as promising a self-sufficient Favorites section (13.5).

### 2.8 Architecture profile check

The `apply-typescript-best-practices` bundle defines an **optional** profile (`features` split into `domain`/`application`/`ports`/`adapters`/`ui`) that binds only when an ADR or an applicable `AGENTS.md` adopts it explicitly. This repo does not adopt it: there is no `AGENTS.md`, no `eslint.config.mjs`, no `dependency-cruiser.config.mjs`, and `src/` is split by window (`sidebar`, `terminal`, `watchers`, ...) plus `shared`, not by feature. Rules 1 to 15 of that profile therefore do not bind this change, and no conflict with a higher-priority instruction arises. The universal obligations it cites are respected anyway and are what shape D2 and D9: the store stays the single owner of the writable config, `normalizeNonStop` stays the single normalization boundary for untrusted JSON on the frontend, and the Rust load path stays the single validation boundary on disk.

## 3. Scope

### In scope

| Side | File | Nature |
| --- | --- | --- |
| Backend | `src-tauri/src/config/project_settings.rs` | one struct field, one `Default`, one test helper, three new tests (R1, R2, R3) |
| Frontend | `src/shared/types.ts` | one optional field |
| Frontend | `src/sidebar/stores/workgroup-groups.ts` | `defaultNonStop`, `normalizeNonStop`, new `setNonStopFavorite` |
| Frontend | `src/sidebar/components/WorkgroupGroupRail.tsx` | context target variant, routing, favorites entry, menu gate, staleness |
| Tests | `src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx` | rewrite of `:387-405`, plus eight new cases (C2 to C9) |
| Tests | `src/sidebar/stores/workgroup-groups.test.ts` | two new store cases (S1, S2) |

Seven files in total, which is what acceptance criterion 8 pins. Three further test files are **in scope as verification and out of scope as edits**: see Section 5.7.

### Out of scope

- **`All` and `Ungrouped`.** They have no persisted record at all: they are the `show_all` / `show_ungrouped` booleans (`project_settings.rs:36-39`). Making them favoritable would require inventing a new config slot and a synthetic identity, which is more surface for less value. If it is ever wanted it is a separate issue.
- `src/sidebar/components/WorkgroupGroupsModal.tsx`. Verified safe as-is: its local `cloneConfig` (`:24-36`) and its `setNonStop`/`setNonStopTelegram`/`setNonStopSound` patchers (`:138-151`) all spread, so the new field survives an edit session untouched. Section 6 edge case 8 records the one pre-existing behaviour this exposes.
- `src/sidebar/stores/workgroup-groups.ts` `cloneConfig` (`:125-134`). Already correct (Section 2.5).
- The watchdog, on both sides. The flag is presentation state.
- The rail reorder gesture. The Non-stop entry stays `reorderable: false` in both sections.
- The Favorites collapse bit and `rail_favorites_collapsed`.
- `ProjectPanel.tsx`, `src/shared/ipc.ts`. `ProjectAPI.getGroups`/`updateGroups` (`ipc.ts:824-827`) pass `WorkgroupGroupsConfig` whole; a new field needs no wrapper change.
- Any data migration. `#[serde(default)]` covers every existing `project-settings.json` (Section 7).

## 4. The decided solution

Replicate the #965 pattern on the Non-stop record: the flag lives on `NonStopGroupConfig`, so it survives rename (the display name is free text), needs no id, and dies with the record (saving `non_stop: None` removes the whole `nonStop` key from disk, `project_settings.rs:257-267`, so an orphan favorite is impossible).

Every decision is fixed here. None is left to the implementer.

| # | Decision | Taken | Why |
| --- | --- | --- | --- |
| D1 | Where the flag lives, and its type | Rust: `#[serde(default)] pub favorite: bool` on `NonStopGroupConfig`, **no** `skip_serializing_if`. TS: `favorite?: boolean`, optional, exactly like `WorkgroupGroup.favorite` (`types.ts:1030`) | `#[serde(default)]` is the #965 migration precedent (`legacy_group_json_defaults_favorite_false`, `:308-312`). No `skip_serializing_if` because `favorite_flag_round_trips_through_save_load` (`:315-335`) enshrines the convention that a non-favorite still emits explicit `false` on disk, so the frontend never receives `undefined` where it expects a boolean. TS optional rather than required because `normalizeNonStop` materializes a concrete boolean in the store anyway, and requiring it would break the complete literal at `workgroup-groups.test.ts:341-348` and every future hand-written fixture for no gain. |
| D2 | `normalizeNonStop` propagation | Add `favorite: !!nonStop.favorite` to the returned literal (`workgroup-groups.ts:104-122`) | Section 2.5, which now states the executed failure mode: without it the entry never appears at all, on the first click and at every load, and the on-disk value decays afterwards. `!!` and not `?? false`, matching the `!!nonStop.show` line right above it and coercing a hand-edited non-boolean JSON value at the same boundary. D2 and D11 are independent filters in series: neither rescues the other (12.2). |
| D3 | Behaviour when `nonStop.show === false` | The favorited entry is **hidden** from Favorites. The flag is kept, not cleared. Turning Non-stop back on restores the entry | Confirmed technically sound, not assumed, and then confirmed by execution (14.2). The rail only draws the button when `show` (`:338`) and `normalizeSelection` bounces a `{kind:"nonstop"}` selection when `!show` (`workgroup-groups.ts:191-193`), so a visible favorite would be a ghost button that jumps to All on click. Forcing `show: true` on favorite is rejected: it would let a presentation action silently re-arm a watchdog that sends Telegram messages and plays sounds, and `show` is the single switch for both concerns (`WorkgroupGroupsModal.tsx:198-201`). **The "flag is kept" half is now a measured fact, not a hope**: unchecking the modal's show toggle and saving was executed and sends the whole `nonStop` object with every sibling field intact (`setNonStop` spreads `ns()`, `:138-141`), and on the Rust side `normalize_groups_config` touches neither `show` nor a bool (`project_settings.rs:134-154`), so `show:false + favorite:true` is not repaired away behind the frontend's back. Manual steps 9e and 9f are therefore reachable. **Declared consequence, accepted:** while `show` is off the flag is unreachable, so a user cannot *un*favorite a hidden Non-stop, and re-enabling it months later resurfaces an entry they may not remember pinning (edge case 3). |
| D4 | Position inside Favorites | **First within its own project's block**, before that project's favorited groups | Makes the order total and deterministic, and mirrors the project section's own order (All, Ungrouped, Non-stop, then groups, `:311-355`). Appending last would put it after groups, contradicting the section it duplicates. |
| D5 | Test ids for the Favorites Non-stop entry | A new `nonStopFavoriteRailTestIds(folderName)` with a **disjoint prefix**: `workgroupGroups.favoriteNonStopButton\|favoriteNonStopRaiseHand\|favoriteNonStopDot.${folderName}`. Not `favoriteRailTestIds(folderName, "nonstop")` | `favoriteRailTestIds` keys on the group id, and `"nonstop"` is not a reserved id (Section 2.7): a hand-edited config with a group whose id is `nonstop` would emit two identical `data-ac-testid`s and make the automation bridge fail `duplicate_selector`. A disjoint prefix cannot collide, and it also keeps every existing assertion on `^workgroupGroups.favoriteButton.` exact. Verified sufficient by construction: the bridge is DOM-driven with no registry to update (Section 2.7), and an inventory of all eleven `data-ac-testid^=` prefixes in `src/` confirms none of them starts counting the new ids (14.5). What D5 buys is **the absence of a second collision, inside Favorites**; it does not and cannot fix the pre-existing project-section collision described in Section 2.7 and recorded as adjacent finding 5. |
| D6 | Context-menu staleness | New `nonstop` branch in the effect at `:704-718`: stale when `!config(target.projectPath).nonStop?.show` | Not `!config.nonStop`; optional chaining makes the single condition cover both a switched-off and a deleted Non-stop. `show === false` is when the entry stops rendering in **both** places (the rail button `:338` and, per D3, the Favorites entry), which is what makes it the right trigger. Covers a concurrent modal save and an external `project_groups_updated` event. **Two limits, both deliberate.** (a) It closes *this* staleness case, not the whole staleness surface: a menu opened on a **Favorites** entry whose record still exists but whose `favorite` flag is cleared externally stays open over a removed button. Executed against today's group behaviour (14.4, F5): `favorites entry still in DOM = false | context menu still open = true`. `nonStopGone` inherits that hole exactly as `groupGone` has since #965, and widening it is a larger change than #1257 should make. (b) It re-runs on a `show` flip only because `workgroupGroupsStore.config()` returns `cloneConfig(...)` (`:440-442`) and `cloneConfig` **spreads** the store's `nonStop` proxy (`:130-131`), so the effect tracks every own property. **Treat that as an invariant**: if `config()` is ever changed to return the raw object, to memoize, or to shallow-copy lazily, D6 degrades to a no-op and only C5 fails (13.7). |
| D7 | How the Non-stop button routes to its target | A `railContextTargetFor(projectPath, button)` helper keyed on `button.selection.kind === "nonstop"` first, then `button.groupId`, then project. Replaces the inline ternary at `:602-609` | The selection is the button's real identity; `groupId` is a `groups[]` implementation detail that is `null` for all three pseudo-entries. Keying on the selection makes the three cases explicit in one place instead of being inferred from a nullable field. |
| D8 | The menu gate at `:798` | `contextTarget()?.kind === "group" \|\| contextTarget()?.kind === "nonstop"` | An explicit positive list. `kind !== "project"` would silently grant the Favorite item to any variant added later. |
| D9 | Store command | New `setNonStopFavorite(projectPath, favorite)` next to `setGroupFavorite` (`:558-569`). Throws `"Alert me! is no longer configured."` when `config.nonStop` is null; returns early when the flag already equals the requested value; otherwise `save()` the whole config with the patched `nonStop` | Structural mirror of `setGroupFavorite`, which throws `"Group no longer exists."` in the same situation and is likewise called with `.catch(() => {})`. It inherits the same `saveVersions` (`:534`) and `applyExternalUpdate` (`:494-515`) handling, so it introduces no new concurrency shape. The message uses the literal `"Alert me!"` like the store's other Non-stop errors (`:661`, `:681`). It does **not** check `show`: persistence is not the place to enforce a display rule, and the UI already gates it (Section 6, edge case 3). |
| D10 | Duplication between the two render sites | Extract `nonStopButtonFor(project, nonStop)` at module level, next to `groupButtonFor`; `ProjectRailSection` (`:337-352`) and `FavoritesRailSection` both call it | The two renderings must agree on display name, `working/total` counter, running dot and raise-hand badge. Two inline copies would drift. This is the only refactor in the plan and it exists because the change itself creates the second call site. **It must be a verbatim move**, and its regression net is three existing test files named in Section 5.6 that must stay green **without being edited**. |
| D11 | Landing order across the two repo halves | **Rust first, as its own commit. Frontend second.** They may be authored in parallel; they must not land in the other order | Serde ignores unknown fields by default (no `deny_unknown_fields` on either struct, verified in 12.2 and independently in 14.2 against both entry points, the Tauri command and the browser dispatch at `web/commands.rs:760-766`). The consequence is worse than "the toggle reverts": **reading breaks before writing does.** Against a backend without the field, `get_project_groups` (`commands/project_settings.rs:19-26`) can never emit `favorite`, because the struct has nowhere to hold it, so the feature is dead from the first load. The write-side symptom (`update_project_groups` drops it on the way in, `save()` rehydrates from the response, `workgroup-groups.ts:529-540`) is second-order, and the cross-window event is a third failure point, since `project_groups_updated_payload` serializes the same struct (`commands/project_settings.rs:12-17`) and feeds both the Tauri event and the WebSocket broadcast (`:43-49`), which is what edge case 15 depends on. **The reverse direction is safe, which is what makes the ordering usable**: with commit 1 landed and the old frontend still running, `#[serde(default)]` supplies `false`, no parse error, nothing to roll back (12.2). Section 8 makes this a hard gate and states plainly that the gate is enforced by review, not by any acceptance criterion. |
| D12 | The locked test at `favorites.test.tsx:387-405` | Narrowed to `["all", "ungrouped"]` plus the project header, renamed, and annotated with why `nonstop` left the list. New positive cases added for the Non-stop entry | The assertion is still valuable for All and Ungrouped, which stay out of scope permanently. Deleting it outright would lose that guard. |
| D13 | The Favorites Non-stop entry renders in **bold** | Accepted as-is. Not changed, and pinned by an assertion in C2 | `RailButton` applies `workgroup-group-rail-title-system` from `props.button.selection.kind !== "group"` (`:222`), and that class is `font-weight: 700` (`src/sidebar/styles/sidebar.css:3424-3426`). `nonStopButtonFor` must keep `selection: {kind:"nonstop"}` for `isSelected` (`:150-155`) and for edge case 5, so the bold weight follows. Executed check (14.5): the only bold titles today are `All`, `Ungrouped` and `Alert me!`, all in the project section, and **Favorites has none**, so this entry would be the first bold row that section has ever had. It matches how the same entry already renders in its project section and how #775/#777 deliberately distinguish built-ins from user groups. The product owner was asked and did not ask for a change, so the default stands. Recorded and pinned so a later reviewer reads it as a decision rather than filing it as a defect (13.4). |

## 5. Affected surfaces: exact files and symbols

### 5.1 `src-tauri/src/config/project_settings.rs`

**5.1.1 The field.** Append to `NonStopGroupConfig` (`:86-101`), after `sound`:

```rust
    /// (#1257) Pinned into the rail's cross-project `Favorites` section, mirroring
    /// the group flag added by #965. Absent on legacy configs => false. Lives on
    /// the record, so it dies with it: `save_workgroup_groups` removes the whole
    /// `nonStop` key when `non_stop` is `None`, which makes an orphan favorite
    /// impossible. No `skip_serializing_if`: a non-favorite must still emit an
    /// explicit `false`, same convention as `WorkgroupGroup::favorite`.
    #[serde(default)]
    pub favorite: bool,
```

**5.1.2 `impl Default for NonStopGroupConfig`** (`:102-113`): add `favorite: false,`.

**5.1.3 `populated_non_stop()`** (`:338-353`): add `favorite: true,`. Deliberately `true`, not `false`: `false` there would be indistinguishable from the serde default, so the existing round trips would not actually exercise the new field.

It has **three** existing consumers, not two (12.5), and none of them breaks:

| Consumer | Why it is unaffected |
| --- | --- |
| `non_stop_round_trips` (`:720-734`) | `assert_eq!(reloaded, config)` at `:733` is a whole-struct comparison. **This is the one that gains real coverage**, which is the point of choosing `true`. |
| `save_persists_non_stop_and_preserves_unknown_keys` (`:655-684`) | Compares struct to struct at `:674` (`Some(populated_non_stop())` on both sides) and otherwise asserts on unrelated JSON keys. |
| `save_none_removes_stale_non_stop_key` (`:686-718`, call at `:694`) | Only asserts presence and then absence of the `nonStop` key (`:700`, `:712-715`). |

**5.1.4 `normalize_groups_config`** (`:134-154`): **no change**. A bool has no range to clamp and no repair to perform. Confirmed in 12.4 that this is the only repair pass, that it runs on load only (`:225`) and not on save (`:234` validates without normalizing), and that `validate_groups_config_structure` (`:156-202`) never inspects `nonStop` at all. **This is also the load-bearing half of D3**: because nothing repairs the field, the backend persists and reloads `show: false` together with `favorite: true` instead of resolving the combination behind the frontend's back.

**5.1.5 New tests** (Section 9.1).

Nothing else in `src-tauri/` changes, and 12.3 established the boundary twice (text search plus a call-graph trace of `save_workgroup_groups`, which agree). Three points of precision, none of which changes the work:

- **The production browser dispatch does not build the struct by literal at all.** It is at `src-tauri/src/web/commands.rs:760-766` and deserializes into `WorkgroupGroupsConfig` via `require_json`, so it carries the new field for free. It is the browser-side twin of the Tauri command and the second entry point D11 has to hold for.
- `web/commands.rs:1220-1230` is a `WorkgroupGroupsConfig` literal with `non_stop: None` **inside `#[cfg(test)] mod tests`** (which opens at `:939`), not production code. There is a second such test-only literal at `commands/project_settings.rs:64-76` (`sample_config()`). Both build the parent struct, not `NonStopGroupConfig`, so both keep compiling untouched. Recorded so "exactly two literal sites" is not misread as "exactly two sites mentioning the config".
- **There is no JSON fixture or golden file to regenerate.** A search for `nonStop` across `*.json` in the repo returns nothing; every Rust assertion about the on-disk shape builds its JSON inline (12.3).

### 5.2 `src/shared/types.ts`

`NonStopGroupConfig` (`:1043-1050`) gains one line, mirroring `WorkgroupGroup.favorite` at `:1030`:

```ts
export interface NonStopGroupConfig {
  show: boolean;
  name: string;
  regex: string;
  toleranceSeconds: number;
  telegram: NonStopTelegramConfig;
  sound: NonStopSoundConfig;
  favorite?: boolean;
}
```

### 5.3 `src/sidebar/stores/workgroup-groups.ts`

**5.3.1 `defaultNonStop()`** (`:70-79`): add `favorite: false,`.

**5.3.2 `normalizeNonStop()`** (`:104-122`): add one line to the returned literal, next to `show`:

```ts
  return {
    show: !!nonStop.show,
    // (#1257) DO NOT DROP THIS LINE. This function rebuilds the object field by
    // field and `setConfig` (:391) runs it on EVERY load and EVERY save, so a field
    // omitted here is dropped silently on the first unrelated save and the favorite
    // appears to revert on its own.
    favorite: !!nonStop.favorite,
    name,
    ...
  };
```

**5.3.3 New `setNonStopFavorite`**, immediately after `setGroupFavorite` (`:558-569`):

```ts
  async setNonStopFavorite(projectPath: string, favorite: boolean): Promise<void> {
    const config = this.config(projectPath);
    const current = config.nonStop;
    if (!current) throw new Error("Alert me! is no longer configured.");
    if (!!current.favorite === favorite) return;
    await this.save(projectPath, { ...config, nonStop: { ...current, favorite } });
  },
```

Note `this.config()` returns a clone (`:440-442`), so the spread cannot alias store state.

### 5.4 `src/sidebar/components/WorkgroupGroupRail.tsx`

**5.4.1 Import** (`:3`): add `NonStopGroupConfig` to the type import from `../../shared/types`.

**5.4.2 `RailContextTarget`** (`:45-47`):

```ts
type RailContextTarget =
  | { kind: "project"; projectPath: string }
  | { kind: "group"; projectPath: string; groupId: string }
  | { kind: "nonstop"; projectPath: string };
```

**5.4.3 New test-id helper**, after `favoriteRailTestIds` (`:122-129`):

```ts
// (#1257) A DISJOINT prefix on purpose, not `favoriteRailTestIds(folderName, "nonstop")`.
// That helper keys on the group id, and nothing reserves the string "nonstop":
// `createGroupId` falls back to `group-N` (:373-382) and project-settings.json is
// hand-editable. A real group with id "nonstop" would then emit a duplicate
// data-ac-testid, which the automation bridge rejects as `duplicate_selector`.
function nonStopFavoriteRailTestIds(folderName: string): RailButtonTestIds {
  return {
    button: `workgroupGroups.favoriteNonStopButton.${folderName}`,
    raiseHand: `workgroupGroups.favoriteNonStopRaiseHand.${folderName}`,
    dot: `workgroupGroups.favoriteNonStopDot.${folderName}`,
  };
}
```

**5.4.4 New `nonStopButtonFor`**, after `groupButtonFor` (`:131-148`). This is the body currently inlined at `:337-352`, moved verbatim:

```ts
// (#1257) Built here, not inline in ProjectRailSection, because the Favorites
// section renders the SAME entry. Two copies would drift on display name,
// counter, running dot, raise-hand or tooltip.
function nonStopButtonFor(project: ProjectState, nonStop: NonStopGroupConfig): GroupButton {
  const workgroups = project.workgroups.filter((wg) => nonStopMatchesWorkgroup(nonStop, wg));
  return {
    key: "nonstop",
    ...buttonContent(nonStopDisplayName(nonStop.name), workgroups),
    selection: { kind: "nonstop" },
    workgroups,
    title: tooltipFor(project.folderName, workgroups),
    reorderable: false,
    groupId: null,
    groupIndex: null,
  };
}
```

The `raiseHand` value keeps coming from `buttonContent` and is **not** overridden to `false`. That matches the current Non-stop button and differs from `All` (`:315`), which is intentional and preserved.

**5.4.5 New `railContextTargetFor`**, after `nonStopButtonFor`:

```ts
// (#1257) Keyed on the selection, not on `groupId`. The Non-stop button carries
// `groupId: null` because it is not a `groups[]` record, so the previous
// `button.groupId ? group : project` ternary routed it to the project target and
// the Favorite item never rendered.
function railContextTargetFor(projectPath: string, button: GroupButton): RailContextTarget {
  if (button.selection.kind === "nonstop") return { kind: "nonstop", projectPath };
  if (button.groupId) return { kind: "group", projectPath, groupId: button.groupId };
  return { kind: "project", projectPath };
}
```

**5.4.6 `FavoritesRailSection`** (`:240-291`). New entry type above the component:

```ts
type FavoriteEntry =
  | { kind: "group"; project: ProjectState; group: WorkgroupGroup; button: GroupButton }
  | { kind: "nonstop"; project: ProjectState; button: GroupButton };
```

A discriminated union, not `{ project; group?; button }`: the optional-field shape would force non-null assertions at both use sites and would not make the two cases exhaustive for the compiler.

The memo (`:246-253`) becomes:

```ts
  const entries = createMemo<FavoriteEntry[]>(() =>
    props.projects.flatMap((project) => {
      const config = workgroupGroupsStore.config(project.path);
      const result: FavoriteEntry[] = [];
      const nonStop = config.nonStop;
      // (#1257 D3) `show` is part of the condition on purpose. The rail only draws
      // the Non-stop button when `show` (:338) and `normalizeSelection` bounces a
      // {kind:"nonstop"} selection to All/Ungrouped when it is off
      // (workgroup-groups.ts:191-193), so a favorite rendered while Non-stop is off
      // would be a ghost button that jumps elsewhere on click. The flag is kept,
      // not cleared: turning Non-stop back on brings the entry back.
      // (#1257 D4) Pushed BEFORE the groups so the block mirrors the project
      // section's order and the list is deterministic.
      if (nonStop?.show && nonStop.favorite) {
        result.push({ kind: "nonstop", project, button: nonStopButtonFor(project, nonStop) });
      }
      for (const group of config.groups) {
        if (group.favorite) {
          result.push({
            kind: "group",
            project,
            group,
            button: groupButtonFor(project, group, null, false),
          });
        }
      }
      return result;
    })
  );
```

The `<For>` body (`:269-285`) becomes:

```tsx
              {(entry) => (
                <RailButton
                  button={entry.button}
                  testIds={
                    entry.kind === "nonstop"
                      ? nonStopFavoriteRailTestIds(entry.project.folderName)
                      : favoriteRailTestIds(entry.project.folderName, entry.group.id)
                  }
                  selected={isSelected(entry.project.path, entry.button)}
                  onContextMenu={(event) =>
                    props.onOpenContextMenu(
                      event,
                      entry.kind === "nonstop"
                        ? { kind: "nonstop", projectPath: entry.project.path }
                        : {
                            kind: "group",
                            projectPath: entry.project.path,
                            groupId: entry.group.id,
                          }
                    )
                  }
                  onClick={() => selectFromRail(entry.project, entry.button.selection)}
                />
              )}
```

No pointer handlers are bound here, so the Non-stop favorite entry cannot arm a drag, same as every other Favorites entry.

**5.4.7 `ProjectRailSection.buttons()`** (`:337-352`) collapses to:

```tsx
    const nonStop = config().nonStop;
    if (nonStop?.show) {
      result.push(nonStopButtonFor(props.project, nonStop));
    }
```

**5.4.8 The project-section `onContextMenu`** (`:602-609`):

```tsx
              onContextMenu={(event) =>
                openContextMenu(event, railContextTargetFor(props.project.path, button))
              }
```

**5.4.9 The staleness effect** (`:704-718`): add the third condition.

```tsx
      const nonStopGone =
        target.kind === "nonstop" &&
        !workgroupGroupsStore.config(target.projectPath).nonStop?.show;
      if (projectGone || groupGone || nonStopGone) closeContextMenu();
```

**5.4.10 `favoriteTargetIsFavorited`** (`:722-728`):

```ts
  const favoriteTargetIsFavorited = () => {
    const target = contextTarget();
    if (target?.kind === "nonstop") {
      return !!workgroupGroupsStore.config(target.projectPath).nonStop?.favorite;
    }
    if (target?.kind !== "group") return false;
    return !!workgroupGroupsStore
      .config(target.projectPath)
      .groups.find((group) => group.id === target.groupId)?.favorite;
  };
```

**5.4.11 `toggleFavoriteFromContextMenu`** (`:737-746`):

```ts
  const toggleFavoriteFromContextMenu = () => {
    const target = contextTarget();
    if (!target || target.kind === "project") return;
    const next = !favoriteTargetIsFavorited();
    closeContextMenu();
    const write =
      target.kind === "nonstop"
        ? workgroupGroupsStore.setNonStopFavorite(target.projectPath, next)
        : workgroupGroupsStore.setGroupFavorite(target.projectPath, target.groupId, next);
    void write.catch(() => {
    });
  };
```

Excluding `"project"` narrows `target` to the two writable variants, and the ternary narrows further on the discriminant. If a fourth variant is ever added it lands in the `setGroupFavorite` branch and fails to compile for want of `groupId`, which is the intended fail-loud behaviour.

**5.4.12 The menu gate** (`:798`):

```tsx
            <Show
              when={
                contextTarget()?.kind === "group" || contextTarget()?.kind === "nonstop"
              }
            >
```

### 5.5 Test files

`src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx` and `src/sidebar/stores/workgroup-groups.test.ts`. Specified in full in Section 9.

### 5.6 Files deliberately not touched

`src/sidebar/components/WorkgroupGroupsModal.tsx`, `src/sidebar/components/ProjectPanel.tsx`, `src/shared/ipc.ts`, `src/sidebar/stores/rail-collapse.ts`, `src/sidebar/watchdog/non-stop-watchdog-client.ts`, `src-tauri/src/loops/non_stop_watchdog.rs`, `src-tauri/src/commands/non_stop.rs`, `src-tauri/src/commands/project_settings.rs`, `src-tauri/src/web/commands.rs`, `src-tauri/module-arcs.txt`. If the implementation needs any of them, the change was misunderstood; stop and say so.

On `module-arcs.txt` specifically: it records module-to-module edges only, never symbols or fields (12.3 checked the four `project_settings` rows, `:383-384`, `:553-554`, `:957`, `:964`). Adding a field creates no edge, so there is nothing to regenerate.

On `WorkgroupGroupsModal.tsx`: it is safe **with respect to this change's new field**, verified rather than assumed, because every path that produces a `nonStop` object spreads (Section 2.5's inventory). That is the whole of the claim. It is **not** a statement that the modal has no other problems: its draft is a construction-time snapshot with no resynchronization (`createSignal(cloneConfig(...))` at `:46-48`), which is a pre-existing defect recorded as adjacent finding 4 in Section 10 and deliberately out of scope here (14.4, F6).

### 5.7 The D10 regression net: three files that must stay green WITHOUT being edited

D10 is a verbatim move, and these three existing test files are the evidence that it was one. None of them appears in acceptance criterion 8's seven-file diff, and none may be edited (13.3):

| File | What it pins |
| --- | --- |
| `src/sidebar/components/WorkgroupGroupRail.test.tsx:541-544`, `:558` | rail order `["all","ungrouped","nonstop","ui","rust"]`, the `1/1` counter, the running dot, and disappearance at `show: false` |
| `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx:356-367` | the `workgroup-group-rail-title-system` class on the Non-stop title and `railRaiseHands()` containing `nonstop`, which is the #775 "Non-stop keeps the hand" decision |
| `src/sidebar/watchdog/rail-watchdog-parity.test.tsx:84-85`, `:157-158` | parity between the rail's Non-stop `working/total` counter and the watchdog report |

**If the implementer finds themselves editing any of these three, the extraction was not verbatim. That is the signal to stop, not to fix the test.** Without this instruction a well-meaning implementer could "fix" a failure in `rail-watchdog-parity.test.tsx` and quietly break the counter contract instead. All three were executed green at `f15f59a4` (14.1: 74 tests, 0 failures across the five relevant files).

## 6. Required behaviour, edge cases and behaviour on failure

| # | Situation | Required behaviour |
| --- | --- | --- |
| 1 | Right-click the Non-stop button in a project section, `show: true` | Menu shows `Edit` and `Favorite`. Clicking `Favorite` closes the menu, persists `nonStop.favorite = true`, and the entry appears in Favorites while **staying** in its project section. The duplication is the point, identical to a favorited group. |
| 2 | Right-click the same entry once favorited, from either place | Menu shows `Edit` and `Unfavorite`. Clicking it clears the flag and removes the Favorites entry. If it was the last favorite in the whole rail, the Favorites section disappears (`Show when={entries().length > 0}`, `:256`). |
| 3 | `nonStop.show === false`, or `nonStop === null` | No Non-stop button in the project section (unchanged, `:338`) and no Favorites entry (D3). There is therefore **no reachable way to open the Non-stop context menu**, so the store command cannot be invoked in this state through the UI. Turning `show` back on in the modal restores the entry with its flag intact. |
| 4 | The user turns Non-stop off while its context menu is open | The staleness effect (5.4.9) closes the menu. Covers both a concurrent modal save in this window and an external `project_groups_updated` event. |
| 5 | Click the Non-stop entry in Favorites | Same as clicking it in the project section: `selectFromRail` sets `{kind:"nonstop"}`, the project becomes active, `ProjectPanel` auto-focus fires, and the rail section stays folded if it was folded (Option B, RC-2). `isSelected` marks both renderings pressed, because both carry the same `selection`. |
| 6 | A project has a favorited Non-stop **and** favorited groups | Within that project's block, Non-stop renders first, then the groups in `config.groups` order (D4). Across projects, project order is unchanged. |
| 7 | Several projects each have a favorited Non-stop | One entry each, at most one per project (`non_stop` is an `Option`). Same display name is possible; the tooltip's first line is the owning project's `folderName` (`tooltipFor`, `:82-97`), which is exactly how favorited groups already disambiguate. Test ids are per `folderName`, so they stay unique. |
| 8 | The groups modal is open with a stale draft while the user toggles the favorite from the rail | The modal's Save writes its whole draft and reverts the flag. **Pre-existing and symmetric with groups**: the modal's `cloneConfig` (`:24-36`) copies `groups` with `{ ...group }`, so a group favorite has always had the same exposure. Not fixed here: fixing it means changing how the modal reconciles its draft, which is out of scope and a much larger blast radius. Recorded so it is a known limitation rather than a surprise. |
| 9 | The Non-stop regex matches nothing | The entry renders `0/0` with no dot, in Favorites exactly as in the project section. Same as an empty favorited group. Not an error. |
| 10 | Legacy `project-settings.json` with a `nonStop` object and no `favorite` key | Loads as `favorite: false`. No migration, no rewrite of the file on load. |
| 11 | Hand-edited `project-settings.json` with `"favorite": "yes"` under `nonStop` | Rust rejects the parse with the existing `Failed to parse project groups from ...` error (serde cannot read a string into `bool`), which is the pre-existing behaviour for every typed field on this struct. No new failure mode. |
| 12 | Hand-edited config with a real group whose `id` is `"nonstop"`, and a favorited Non-stop | Both render, and **the two Favorites test ids are disjoint** (D5), so this change adds no collision there. It does **not** promise the DOM is collision-free: the **project section** already emits `workgroupGroups.button.nonstop` twice in exactly this configuration, today, and the automation bridge already answers `duplicate_selector` for it. Executed on unmodified `f15f59a4` (14.3, F1). Pre-existing, out of scope, recorded as adjacent finding 5. C6 pins the Favorites half and carries a comment naming the known project-section duplicate so the next reader does not file it as new. |
| 13 | `setNonStopFavorite` called when `config.nonStop` is null | Throws `"Alert me! is no longer configured."` before touching the transport. The caller swallows it (`.catch(() => {})`), the menu is already closed, and no store error is set, exactly like `setGroupFavorite`'s "Group no longer exists." path. Unreachable from the UI (edge case 3); it exists so a race cannot corrupt state. Note that `!current` catches both `null` and `undefined`, which matters because the store normalizes a backend `null` to `undefined` (edge case 17). |
| 14 | The save fails: disk error or backend rejection | `save()` restores the previous entry, sets `saving: false` and writes the message into the project's `error` (`:541-546`), which the rail already surfaces as the `!` badge (`:626-632`). The favorite reverts to its persisted value because the store was never optimistically mutated. Unchanged mechanics. |
| 15 | **The save fails locally, before the transport, because an unrelated group has a syntactically invalid regex** | The favorite silently does nothing and the rail shows an `!` badge about a *different* group. This is the most likely failure of the three, not the rarest. It is reachable because the three validation boundaries disagree: Rust checks regex **length only** (`project_settings.rs:194-198`), `ensureLoaded` validates with `validateRegexSyntax: false` (`workgroup-groups.ts:414`), and `save()` validates with `true` (`:520`). So a hand-edited or legacy config with one broken group regex loads clean, renders, and then makes **every** subsequent save throw before reaching the backend. Executed (14.4, F4): `setGroupFavorite(...) threw = "Group 1: regex is invalid."`, `update_project_groups calls = 0`, and the badge is sticky until the next successful save because `save()` has already written the message into the entry (`:523`). `setNonStopFavorite` inherits this verbatim (D9). **Pre-existing, symmetric with groups, not #1257's to fix**, and not a reason to change D9. Listed so the plan stops implying the only way a favorite save fails is the backend. |
| 16 | Two windows, one favorites the Non-stop | The other receives `project_groups_updated`, `applyExternalUpdate` runs `setConfig` and therefore `normalizeNonStop`, so the flag arrives intact (D2) and the second window's Favorites section updates. This path depends on the Rust field existing, because `project_groups_updated_payload` serializes the same struct (D11). |
| 17 | A backend `nonStop: null` reaches the store | It is stored as `undefined`, not `null`: `cloneConfig` returns `config.nonStop ?? undefined` on the falsy arm (`workgroup-groups.ts:130-132`), executed and confirmed (14.4, F7). Behaviour is unchanged everywhere, because every guard in this plan uses `!current` or `?.`. **Recorded so nobody writes `expect(config.nonStop).toBeNull()` in the new tests and then debugs it.** |
| 18 | The Favorites Non-stop entry's visual weight | It renders **bold**, next to favorited groups at normal weight, and that is the accepted behaviour (D13). Pinned by an assertion in C2. |

## 7. Compatibility and security

- **On-disk format.** `project-settings.json` gains `nonStop.favorite`. Forward compatible via `#[serde(default)]`; the precedent is #965's `legacy_group_json_defaults_favorite_false` (`:308-312`). No migration code, no file rewrite on load.
- **Unknown keys inside `nonStop` do not survive a save, by any build.** The `Some` arm of `save_workgroup_groups` does `obj.insert("nonStop", ...)` (`:257-263`), which replaces the entire value with one rebuilt from the struct. So *any* key inside `nonStop` that the running binary does not model is dropped on the next save. Stating it this way makes the guarantee checkable rather than version-dependent (12.7). **Consequence for downgrade:** an older build reading a file that already has `nonStop.favorite` ignores the unknown field (no `deny_unknown_fields`) and **erases** it on its next save. One presentation bit, recoverable by re-favoriting, identical in kind to what #965 already accepted for groups. Acceptable; recorded so it is a decision rather than a surprise.
- **Root-level unknown keys are unaffected**, and structurally cannot be. `save_persists_non_stop_and_preserves_unknown_keys` (`:655-684`) guards `agents` and `tooling` at the **root**, which survive because `update_config_json_object` reads the existing root object, hands only that map to the mutator and reserializes the whole root (`src-tauri/src/config/local_config_io.rs:23-44`). That path never inspects the shape of `NonStopGroupConfig`. Caveat for whoever reads that test next, flagged by 12.7 and not changed: its name reads like a stronger guarantee than its body provides, since it does not demonstrate preservation of unknown keys *inside* `nonStop`, and per the previous bullet no such preservation exists.
- **IPC.** No new command, no new event, no changed signature. `get_project_groups` / `update_project_groups` carry one more field inside an existing payload. The Rust field is snake_case with `#[serde(rename_all = "camelCase")]` on the struct, so it serializes as `favorite` and matches the TS name; the field is single-word, so there is no camelCase trap here.
- **Cross-side type parity.** Rust `bool` (always present on the wire, no `skip_serializing_if`) against TS `favorite?: boolean`. The optionality is one-directional slack for hand-written TS values; the backend always sends a concrete boolean.
- **Security.** No new input surface. The flag is a boolean parsed by serde at the same boundary as every other field on the struct, never interpolated into a regex, a path, a command line or markup. It does not gate any privileged operation, and D3 explicitly prevents a presentation action from re-arming the watchdog's Telegram and sound alerts.
- **Accessibility.** The Favorites Non-stop entry reuses `RailButton`, so it inherits `aria-pressed`, `title` and the existing raise-hand `aria-label` with no new pattern.
- **Rollback.** Reverting both commits leaves `nonStop.favorite` keys on disk in the wild; they are ignored on read and dropped on the next save. Nothing else has to be undone.

## 8. Implementation order

Four commits.

0. **Commit this plan file first, on its own.** `plans/` is in `.gitignore` (`.gitignore:11`), so it needs `git add -f plans/1257-nonstop-favorite.md`; a plain `git add` silently adds nothing and the plan never reaches the branch. Precedent: #1193's plan, and #1177's before it.
1. **Backend, owned by `dev-rust`.** Section 5.1 plus the three Rust tests of 9.1. Verify with `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins --tests`. This commit is self-contained: the field round-trips and nothing reads it yet, and it is benign against the still-old frontend, which simply sends no `favorite` and gets `false` from `#[serde(default)]` (12.2). `dev-rust-grinch` is the adversarial reviewer and **must not** implement this step: an earlier draft of this plan named it here, which would have had the reviewer implement what it later reviews (14.3, F3).
2. **Frontend types and store, owned by `dev-webpage-ui`.** Sections 5.2 and 5.3 plus the store tests of 9.2.
3. **Frontend rail and component tests, owned by `dev-webpage-ui`.** Sections 5.4 and 5.5 plus the component tests of 9.3.

**Hard gate between 1 and 3, and the reason the order is not negotiable:** the Rust field must be on the branch before the frontend behaviour is verified end to end. See D11 for the full chain; the short form is that reading breaks first, so a frontend-first landing yields a feature that is dead from the first load rather than one that merely reverts.

Two things about this gate that the implementer has to know, because both are counter-intuitive:

- **The component tests of 9.3 run against `FakeTransport`, which echoes the config back** (`fake.onInvoke("update_project_groups", (args) => args.config)`), so **they would pass even with the Rust half missing**. Green frontend tests are not evidence that step 1 landed.
- **No acceptance criterion detects a violation of this order.** Criteria 5, 6 and 8 all inspect the *final* state, which is identical under either order (14.2). **The gate is enforced by review, not by a check.** That is tolerable because a wrong order produces a transient, self-healing defect that disappears the moment the Rust commit lands; it is stated here so nobody reads a green suite as proof the order was respected.

**The forbidden split is step 2 without step 3**, because a store command with no caller is dead code. The symmetric error, step 3 without step 2, is worse in kind but harmless in practice: `WorkgroupGroupRail.tsx` would call a `setNonStopFavorite` that does not exist and `tsc` fails (14.2). It is the one ordering error that cannot ship, which is exactly why the dangerous direction is the **silent** one. Steps 2 and 3 may be one commit. Steps 1 and 2/3 may be **authored** concurrently.

Final verification before handing back, from the repo root: `npm run typecheck`, `npm test`, `npm run test:debt`, and the Rust trio from step 1. **Plus one explicit check: the three regression files of Section 5.7 pass unmodified.** `git diff --name-only f15f59a4..HEAD` must not list any of them.

## 9. Tests and acceptance criteria

### 9.1 Rust, in `src-tauri/src/config/project_settings.rs`

**R1: `legacy_non_stop_json_defaults_favorite_false`.** Mirrors `legacy_group_json_defaults_favorite_false` (`:308-312`), which is the migration precedent this change leans on.

```rust
    #[test]
    fn legacy_non_stop_json_defaults_favorite_false() {
        let legacy = r#"{"groups":[],"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3}}}"#;
        let config: WorkgroupGroupsConfig = serde_json::from_str(legacy).expect("parse legacy");
        assert!(!config.non_stop.expect("nonStop present").favorite);
    }
```

**R2: `non_stop_favorite_round_trips_and_emits_false_when_unset`.** The Non-stop counterpart of `favorite_flag_round_trips_through_save_load` (`:315-335`), including the guard against a `skip_serializing_if` creeping in later.

```rust
    #[test]
    fn non_stop_favorite_round_trips_and_emits_false_when_unset() {
        let project = project_with_workspace();
        let config = WorkgroupGroupsConfig {
            groups: Vec::new(),
            show_all: true,
            show_ungrouped: true,
            non_stop: Some(populated_non_stop()), // favorite: true
        };

        save_workgroup_groups(project.path(), config.clone()).expect("save");
        let reloaded = load_workgroup_groups(project.path()).expect("reload");
        assert_eq!(reloaded, config);
        let persisted: Value = serde_json::from_str(
            &std::fs::read_to_string(settings_path(project.path())).expect("read"),
        )
        .expect("parse");
        assert_eq!(persisted["nonStop"]["favorite"], true);

        // (#965 convention, #1257) A NON-favorited Non-stop must still emit an
        // explicit `false` on disk, or a later `skip_serializing_if` would hand the
        // frontend `undefined` where it expects a concrete boolean.
        let mut off = config;
        off.non_stop.as_mut().expect("nonStop").favorite = false;
        save_workgroup_groups(project.path(), off).expect("save non-favorite");
        let persisted: Value = serde_json::from_str(
            &std::fs::read_to_string(settings_path(project.path())).expect("read"),
        )
        .expect("parse");
        assert_eq!(persisted["nonStop"]["favorite"], false);
    }
```

**R3: `non_stop_favorite_survives_deserialization_from_frontend_json`.** Adopted (proposed in 12.6). R1 proves that an absent key defaults to `false`; R2 proves that a Rust-constructed `true` survives a disk round trip. **Neither proves the direction D11 is actually about**, which is that a `favorite: true` arriving in the frontend's JSON is deserialized rather than ignored. That is the precise behaviour whose absence D11 warns about, and it costs one cheap test. It lives inside `project_settings.rs`, so acceptance criterion 8's seven-file diff is unaffected.

```rust
    #[test]
    fn non_stop_favorite_survives_deserialization_from_frontend_json() {
        let project = project_with_workspace();
        // The shape `update_project_groups` receives from the store, favorite included.
        let incoming = r#"{"groups":[],"showAll":true,"showUngrouped":true,"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3},"favorite":true}}"#;
        let config: WorkgroupGroupsConfig = serde_json::from_str(incoming).expect("parse incoming");
        assert!(config.non_stop.as_ref().expect("nonStop present").favorite);

        save_workgroup_groups(project.path(), config).expect("save");
        let reloaded = load_workgroup_groups(project.path()).expect("reload");
        assert!(reloaded.non_stop.expect("nonStop present").favorite);
    }
```

**A test at the `update_project_groups` layer is deliberately NOT added** (reasoned in 12.6, accepted). `update_project_groups_inner` is a one-line delegation to `save_workgroup_groups` (`commands/project_settings.rs:28-33`) with no logic of its own, so a test there would assert serde's behaviour a third time through one extra stack frame, and it would have to touch `src-tauri/src/commands/project_settings.rs`, which is in Section 5.6's not-touched list and would break criterion 8's exact seven-file diff. R3 covers the same risk inside a file already in scope. Recorded so the omission reads as a decision.

**R1, R2 and R3 compile as written**, checked symbol by symbol in 12.6 rather than assumed: `project_with_workspace` (`:279`), `settings_path` (`:285`) and `populated_non_stop` (`:338`) all exist in the module; `Value` is in scope through `use super::*` (`:276`) reaching `use serde_json::Value` (`:2`); `WorkgroupGroupsConfig` derives `Clone` (`:31`); R2's `let mut off = config;` move is legal because the preceding `assert_eq!` only borrows; and `assert_eq!(persisted[...], true)` compares `Value` against `bool`, which `:331` already does today, so clippy's `bool_assert_comparison` does not fire.

### 9.2 Store, in `src/sidebar/stores/workgroup-groups.test.ts`

**S1: "normalizeNonStop carries `favorite` through load AND save (#1257)".** The regression test for the trap of Section 2.5, and **the automated detector for a missing D2**: its very first assertion, right after `ensureLoaded`, is the load-side loss. Its second half covers a save that is **not** a favorite save, which is the later decay.

```ts
  it("keeps the nonStop favorite flag through load and through an unrelated save (#1257)", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve(
      "get_project_groups",
      config({ nonStop: { ...defaultNonStop(), show: true, favorite: true } })
    );
    fake.onInvoke("update_project_groups", (args) => args.config);
    await workgroupGroupsStore.ensureLoaded(projectPath);
    expect(workgroupGroupsStore.config(projectPath).nonStop?.favorite).toBe(true);

    // `setConfig` runs normalizeNonStop on the save path too, so a field it forgets
    // to copy is lost on the first save that has nothing to do with favorites.
    await workgroupGroupsStore.addWorkgroupToNonStop(projectPath, "wg-1-dev-team");
    expect(
      (fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig).nonStop
        ?.favorite
    ).toBe(true);
    expect(workgroupGroupsStore.config(projectPath).nonStop?.favorite).toBe(true);

    expect(normalizeNonStop({ ...defaultNonStop(), favorite: true })?.favorite).toBe(true);
    expect(normalizeNonStop({ ...defaultNonStop() })?.favorite).toBe(false);
  });
```

**S2: "setNonStopFavorite writes, no-ops and refuses an absent nonStop".**

```ts
  it("setNonStopFavorite writes the flag, no-ops when unchanged, and refuses an absent nonStop", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve("get_project_groups", config({ nonStop: { ...defaultNonStop(), show: true } }));
    fake.onInvoke("update_project_groups", (args) => args.config);
    await workgroupGroupsStore.ensureLoaded(projectPath);

    await workgroupGroupsStore.setNonStopFavorite(projectPath, true);
    expect(
      (fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig).nonStop
        ?.favorite
    ).toBe(true);

    // Already true: no second write.
    await workgroupGroupsStore.setNonStopFavorite(projectPath, true);
    expect(fake.callsFor("update_project_groups")).toHaveLength(1);

    workgroupGroupsStore.resetForTests();
    fake.resolve("get_project_groups", config({ nonStop: null }));
    await workgroupGroupsStore.ensureLoaded(projectPath);
    await expect(workgroupGroupsStore.setNonStopFavorite(projectPath, true)).rejects.toThrow(
      "Alert me!"
    );
  });
```

### 9.3 Component, in `src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx`

New local helper, next to `favoriteButtonKeys()` (`:100-104`):

```ts
function nonStopFavoriteKeys(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>(
      '[data-ac-testid^="workgroupGroups.favoriteNonStopButton."]'
    )
  ).map((button) =>
    button.dataset.acTestid!.replace("workgroupGroups.favoriteNonStopButton.", "")
  );
}
```

**C1: rewrite of `:387-405`.** Narrowed, renamed, annotated. Nothing else in the test changes.

```tsx
    it("exposes no favorite option on All / Ungrouped or the project header", async () => {
      const fake = railFake(groupsConfig({ nonStop: { ...defaultNonStop(), show: true } }));
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(target("workgroupGroups.button.nonstop")).toBeTruthy());

        // (#1257) `nonstop` left this list deliberately: it IS favoritable now, and
        // C2/C3 pin that. All and Ungrouped stay out permanently because they have
        // no persisted record to hold the flag (they are the showAll/showUngrouped
        // booleans), so this assertion keeps its value for them.
        for (const key of ["all", "ungrouped"]) {
          await openMenuOn(target(`workgroupGroups.button.${key}`));
          expect(target("workgroupGroups.contextMenu.edit")).toBeTruthy();
          expect(maybeTarget("workgroupGroups.contextMenu.favorite")).toBeNull();
        }

        await openMenuOn(target("workgroupGroups.projectLabel.Project"));
        expect(maybeTarget("workgroupGroups.contextMenu.favorite")).toBeNull();
      } finally {
        rendered.cleanup();
      }
    });
```

**C2: "offers Favorite on the Non-stop entry, renders it in Favorites, and KEEPS it in its project section".** Mirrors the group test at `:314-336`. Setup: `railFake(groupsConfig({ nonStop: { ...defaultNonStop(), show: true } }))`. Open the menu on `workgroupGroups.button.nonstop`, assert the item reads `Favorite`, click it, then assert:
- `nonStopFavoriteKeys()` equals `["Project"]`;
- `target("workgroupGroups.button.nonstop")` still exists;
- the persisted config from `fake.lastCall("update_project_groups")` has `nonStop.favorite === true` and its `groups` are untouched;
- **the Favorites entry's title carries `workgroup-group-rail-title-system`** (D13). One line, and it turns the bold rendering into a pinned decision instead of an unrecorded side effect:
  ```ts
  expect(
    target<HTMLElement>("workgroupGroups.favoriteNonStopButton.Project")
      .querySelector(".workgroup-group-rail-title")
      ?.classList.contains("workgroup-group-rail-title-system")
  ).toBe(true);
  ```

**C3: "offers Unfavorite from the Favorites entry itself and removes it".** Mirrors `:364-385`. Setup with `nonStop: { ...defaultNonStop(), show: true, favorite: true }`. Wait for `nonStopFavoriteKeys()` to equal `["Project"]`, open the menu on `workgroupGroups.favoriteNonStopButton.Project`, assert the item reads `Unfavorite`, click it, assert the Favorites section disappears and the saved config has `nonStop.favorite === false`.

**C4: "hides a favorited Non-stop from Favorites when it is switched off, without clearing the flag" (D3).** Setup with `nonStop: { ...defaultNonStop(), show: false, favorite: true }`. Assert `maybeTarget("workgroupGroups.favorites")` is null, `maybeTarget("workgroupGroups.button.nonstop")` is null, and no `update_project_groups` call was made (the flag was not cleared, only hidden). This is the test that pins D3 as a decision instead of an accident.

**C5: "auto-closes the Non-stop context menu when Non-stop is switched off underneath it" (D6).** Setup with `show: true`. Open the menu on the Non-stop button, assert `workgroupGroups.contextMenu` exists, then push an external update through `workgroupGroupsStore.applyExternalUpdate(projectPath, { ...config, nonStop: { ...nonStop, show: false } })`, and assert the menu is gone.

**C6: "keeps the FAVORITES test-id namespaces disjoint when a group is literally named `nonstop`" (D5).** Setup with `groups: [{ id: "nonstop", name: "Nonstop", regex: ..., favorite: true }]` and `nonStop: { ...defaultNonStop(), show: true, favorite: true }`. Assert `favoriteButtonKeys()` equals `["Project.nonstop"]`, `nonStopFavoriteKeys()` equals `["Project"]`, and that both `document.querySelectorAll('[data-ac-testid="workgroupGroups.favoriteButton.Project.nonstop"]')` and `document.querySelectorAll('[data-ac-testid="workgroupGroups.favoriteNonStopButton.Project"]')` have length exactly 1.

This test's scope is **Favorites only**, and it must carry a comment saying so, because the DOM it builds contains a live pre-existing duplicate that is not this change's to fix (Section 6, edge case 12, and adjacent finding 5):

```ts
      // (#1257) SCOPE: this pins the FAVORITES ids only. The project section in this
      // same DOM emits `workgroupGroups.button.nonstop` TWICE - once for the group
      // whose id is literally "nonstop" (`key: group.id`, :139) and once for the
      // pseudo-button (`key: "nonstop"`, :343), both through
      // `projectRailTestIds(button.key)` (:592). That collision predates #1257, is
      // reproducible on `main` with no favorites involved, and is deliberately NOT
      // asserted here. Do not "fix" it as part of this issue.
```

**C7: "orders the Non-stop favorite before the group favorites of its own project" (D4).** Setup with `nonStop` favorited and shown plus two favorited groups. Assert the rendered order is Non-stop, then the two groups in `config.groups` order.

**The selector matters, and the obvious one is wrong.** `[data-ac-testid^="workgroupGroups.favorite"]` also matches the section's own header button, `data-ac-testid="workgroupGroups.favorites.header"` (`:263`), because `"workgroupGroups.favorites.header"` starts with `"workgroupGroups.favorite"`. Scoping to the `workgroupGroups.favorites` container excludes the container but **not** the header, which is a child of it. Executed (14.3, F2), the header comes out first and the order assertion fails against a correct implementation. Independently: an inventory of all eleven `data-ac-testid^=` prefixes in `src/` shows the bare `workgroupGroups.favorite` prefix exists nowhere in the codebase today, so this defect would have been introduced by the plan rather than inherited. Use the prefix-exact union of the two **button** prefixes, which mirrors how `favoriteButtonKeys()` (`:100-104`) and `nonStopFavoriteKeys()` already select (13.8, Option B):

```ts
function favoriteRowKeys(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>(
      '[data-ac-testid^="workgroupGroups.favoriteButton."], [data-ac-testid^="workgroupGroups.favoriteNonStopButton."]'
    )
  ).map((button) => button.dataset.acTestid!);
}
// expect(favoriteRowKeys()).toEqual([
//   "workgroupGroups.favoriteNonStopButton.Project",
//   "workgroupGroups.favoriteButton.Project.ui",
//   "workgroupGroups.favoriteButton.Project.rust",
// ]);
```

Note the trailing dots: they are what makes the match exact. `querySelectorAll` returns document order, which is the render order, so no sort is needed.

**C8: "marks BOTH renderings pressed when the Non-stop entry is selected" (edge case 5, and the property D10 could regress).** Section 6 edge case 5 asserts in prose that `isSelected` marks both renderings pressed because both carry the same `selection`, and nothing pins it. That property depends on `nonStopButtonFor` returning the same `selection` shape at both call sites, which is exactly what a non-verbatim extraction would break. Setup with `nonStop: { ...defaultNonStop(), show: true, favorite: true }`; click `workgroupGroups.favoriteNonStopButton.Project`; assert `aria-pressed === "true"` on both that button and `workgroupGroups.button.nonstop`.

**C9: "restores the hidden favorite when Non-stop is switched back on, with no save in between" (the return trip of D3).** C4 pins only the render side of D3. Its other half, *"the flag is kept, not cleared"*, is unpinned, and **an implementation that clears the flag on hide would pass C4 and still violate D3**. Setup with `show: false, favorite: true`; assert the Favorites section is absent; then flip `show` back to `true` through `workgroupGroupsStore.applyExternalUpdate`; assert `nonStopFavoriteKeys()` equals `["Project"]` and that `fake.callsFor("update_project_groups")` is still empty throughout.

**Every symbol these tests depend on exists and compiles**, checked individually in 13.8 rather than assumed: `railFake` (`:153-162`, which already keys `get_project_groups` on `args.path`, as C2 through C9 need), `openMenuOn` (`:144-147`), `target` / `maybeTarget` (`:78-86`), `favoriteButtonKeys` (`:100-104`), `groupsConfig` (`:66-76`), `project()` (`:44-54`), `defaultNonStop` imported at `:19`; `FakeTransport.lastCall` (`fake-transport.ts:117`) and `.callsFor` (`:113`); `applyExternalUpdate` for C5 and C9 (`workgroup-groups.ts:494-515`); and `normalizeNonStop` is **already imported** in `workgroup-groups.test.ts:11`, so S1's direct calls need no new import.

### 9.4 Acceptance criteria

Objective and individually checkable, all from the repo root at branch HEAD:

**Every baseline below was executed, not inferred** (14.1). The replica now has `node_modules` (`npm ci --prefer-offline`, exit 0, nothing versioned touched) and a Cargo target, so the numbers are measured at `f15f59a4`: favorites suite 15/15 pass, store suite 19/19 pass, `project_settings` 20 pass / 0 fail, `npm run typecheck` exit 0, `npm run test:debt` exit 0, `npm run validate-branch-name` OK, and `git check-ignore -v plans/1257-nonstop-favorite.md` confirms `.gitignore:11:plans/` so the `-f` of step 0 is mandatory.

1. `npx tsc --noEmit` exits 0 with no diagnostics.
2. `npm test` is green. `npx vitest run src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx` goes from **15** tests to **23** (C2 through C9 added, C1 rewritten in place, none deleted). `npx vitest run src/sidebar/stores/workgroup-groups.test.ts` goes from **19** to **21** (S1, S2).
3. `npm run test:debt` exits 0.
4. `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings` and `cargo test --lib --bins --tests` all exit 0. The `project_settings` test module goes from **20** `#[test]` functions to **23** (R1, R2, R3) and loses none.
5. `grep -c 'favorite' src-tauri/src/config/project_settings.rs` counts at least one occurrence inside the `NonStopGroupConfig` struct body (`:86` to its closing brace), and `grep -n 'skip_serializing_if' src-tauri/src/config/project_settings.rs` returns nothing.
6. `grep -n 'favorite' src/sidebar/stores/workgroup-groups.ts` shows the field inside the `normalizeNonStop` return literal (between `:104` and `:122` at baseline coordinates). **If it is absent, the change is broken even though every UI test passes against `FakeTransport`.** Section 8 explains why.
7. `grep -rn 'favoriteRailTestIds(.*nonstop' src/` returns nothing: the reserved-key approach was not used (D5).
8. `git diff --stat f15f59a4..HEAD` touches exactly these seven files and no others: `plans/1257-nonstop-favorite.md` (force-added, Section 8 step 0), `src-tauri/src/config/project_settings.rs`, `src/shared/types.ts`, `src/sidebar/stores/workgroup-groups.ts`, `src/sidebar/stores/workgroup-groups.test.ts`, `src/sidebar/components/WorkgroupGroupRail.tsx`, `src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx`. Any other file means the scope was exceeded; a missing plan file means the force-add was skipped. **In particular, none of the three regression files of Section 5.7 may appear**, because their staying untouched is the evidence D10 was a verbatim move.
9. Manual check, once, in a real build. It is the only check that exercises the real backend rather than `FakeTransport`, and the order matters:
   a. Enable Non-stop for a project, right-click it in the rail, click `Favorite`. The entry appears under `Favorites`. **This is the manual detector for a missing D2** (Section 2.5): without D2 the entry never appears at all, on this very first click, because `setConfig` strips the flag from the backend's response. The automated equivalent is S1's first assertion.
   b. **Edit the Non-stop regex in the groups modal and save.** The Favorites entry survives. This covers the *decay* case (Section 2.5, point 3): the flag surviving a save that has nothing to do with favorites. It is not the D2 detector, which is 9a.
   c. Restart the app. The entry is still there.
   d. Open `<project>/.ac/project-settings.json` and confirm `nonStop.favorite` is `true`.
   e. Uncheck `Alert me! (watchdog)` in the modal and save. The Favorites entry disappears and `nonStop.favorite` stays `true` on disk. Reachability of this step was verified by execution (14.2): the modal's show toggle sends the whole `nonStop` object with every sibling intact, and no Rust normalization repairs `show:false + favorite:true`.
   f. Re-check it. The entry comes back.

**What no criterion checks.** The landing order of Section 8 (D11). Criteria 5, 6 and 8 all inspect the final state, which is identical under either order. It is a review gate, not a check.

## 10. Adjacent findings, reported and not changed

None of these is #1257's to fix. Each is recorded so it reads as a known pre-existing property rather than as something this change introduced or overlooked.

1. **The groups modal's stale draft** can revert a favorite toggled from the rail while it is open. Pre-existing for groups since #965 and not introduced here (Section 6, edge case 8).
2. **The modal's draft is a construction-time snapshot with no resynchronization**, which is a strictly worse instance of finding 1. `createSignal(cloneConfig(workgroupGroupsStore.config(props.projectPath)))` (`WorkgroupGroupsModal.tsx:46-48`) runs once, with no effect re-seeding it once the store finishes loading. Hit by accident during the adversarial pass (14.4, F6): a modal constructed before `ensureLoaded` resolved, and Save then wrote `{"show":false,"name":"Alert me!","regex":"(?!)",...}` over a config whose real values were `"Watchers"` and `"^(wg-1-dev-team)$"`. In the real app the rail loads the config before the modal can be opened, so it needs a narrow race, but it is a fourth path that can revert `favorite` and it destroys far more than one presentation bit. **This belongs in its own issue**, titled around the modal draft not resynchronizing with the store, and it is deliberately not fixed here: the fix changes how the modal reconciles its draft, which is a much larger blast radius than #1257.
3. **`folderName`-keyed test ids** (`workgroupGroups.rail.${folderName}`, `workgroupGroups.favoriteButton.${folderName}...`, and the new `workgroupGroups.favoriteNonStopButton.${folderName}`) collide across two registered projects whose leaf folder names are equal. Pre-existing pattern, followed here for consistency rather than diverging in one place. Recorded because the automation bridge would report `duplicate_selector` in that configuration.
4. **`nonStopMatchesWorkgroup`** (`workgroup-groups.ts:307-312`) builds a throwaway `{ id: "nonstop", name: "nonstop", regex }` object just to reuse `compileGroupRegex`. Harmless, and it is the reason the string `"nonstop"` reads as if it were reserved when it is not (D5). Not touched.
5. **The project section already emits duplicate test ids for a group whose id equals a pseudo-button key.** `groupButtonFor` sets `key: group.id` (`:139`), the pseudo-buttons set `key: "all"` / `"ungrouped"` / `"nonstop"` (`:313`, `:327`, `:343`), and all of them feed `projectRailTestIds(button.key)` (`:592`). Executed on unmodified `f15f59a4` (14.3, F1): `duplicate count for workgroupGroups.button.nonstop = 2` and the bridge answers `duplicate_selector`. Predates #1257 (it dates from #777 for `nonstop`, and earlier for the other two), is reachable only through a hand-edited `project-settings.json`, and is the same shape as finding 3. Fixing it means namespacing the pseudo-button keys, which would churn test ids across the whole rail suite. Recorded, and named in C6's comment so the next reader does not file it as new.
6. **Downgrade data loss** of the flag when an older binary saves a newer file, which is one instance of the more general "no unknown key inside `nonStop` survives a save" property (Section 7). Same shape as #965 accepted; recorded, not mitigated.
7. **The `nonStop` module comment promises more than the code delivers.** `project_settings.rs:56-59` says a bad hand-edited `nonStop` value is "repaired (clamped) on load, never fatally rejected", but that only ever covered the numeric fields: a `"favorite": "yes"` fails the whole load, exactly as a bad `show` already does (Section 6, edge case 11). This change adds one more key the comment's promise does not cover. Making `favorite` behave differently from `WorkgroupGroup::favorite` to satisfy the comment would be worse than the inconsistency (12.8). Recorded only.
8. **`save_workgroup_groups` validates but never normalizes** (`:234`, against the load path's `:225-226`), so a config written through the IPC command reaches disk unclamped and is repaired only on the next load (12.8). Irrelevant to a bool, and the reason 5.1.4's "no change" is safe rather than merely convenient.

## 11. Open decisions

**None.** D1 through D13 in Section 4 fix the storage location and type, the normalization propagation, the `show: false` behaviour, the ordering, the test-id namespace, the staleness condition, the routing, the menu gate, the store command's contract and error, the shared-button extraction, the landing order, the fate of the locked test, and the bold rendering in Favorites. Sections 5 and 9 give the exact code and the exact tests.

**Resolved in the Step 7 consensus pass, round 1.** All fourteen items raised by the three enrichment passes are settled and applied above:

| # | Raised as | Resolution |
| --- | --- | --- |
| 1 | F3, step 1 assigned to the reviewer | **Fixed.** Section 8 step 1 is owned by `dev-rust`. This was an authoring error, and it was the only thing making the plan not executable cold. |
| 2 | F2 / 13.8, C7's selector matches the section header | **Fixed.** C7 now uses the prefix-exact union of the two button prefixes (Option B) and states why the obvious selector is wrong. |
| 3 | F1, edge case 12 promises no `duplicate_selector` | **Fixed.** Edge case 12 now promises disjointness within Favorites only; the pre-existing project-section collision is recorded in Section 2.7 and as adjacent finding 5, and named in C6's comment. |
| 4 | R3 proposed by `dev-rust` (12.6) | **Adopted.** It is the only test covering the direction D11 is about, it lives in a file already in scope, and it costs one test. Criterion 4 is now 20 to 23. The reasoned refusal of a command-layer test is adopted with it. |
| 5 | C8 and C9 proposed by `dev-webpage-ui` (13.8) | **Both adopted.** C9 in particular closes a real hole: an implementation that *clears* the flag on hide passes C4 and violates D3. Criterion 2 is now 15 to 23. |
| 6 | Bold rendering in Favorites (13.4, 14.5) | **Accepted and recorded as D13**, with edge case 18 and a pinning assertion in C2. The product owner was asked and did not request a change. |
| 7 | F5, D6's rationale over-promises | **Fixed.** D6 keeps its condition and now states both limits explicitly, including the Favorites-entry staleness case it does not close. No widening: that hole is the #965 status quo. |
| 8 | F4, edge case 14 incomplete | **Fixed.** Split into edge cases 14 and 15, with the invalid-regex local failure and its executed evidence. D9 unchanged. |
| 9 | F6, Section 5.6 oversells the modal | **Fixed.** 5.6 now scopes its claim to the new field, and the draft-snapshot defect is adjacent finding 2 **with a recommendation that it become its own issue**. Not fixed here. |
| 10 | F7, `nonStop: null` is stored as `undefined` | **Recorded** as edge case 17, so no new test writes `toBeNull()`. |
| 11 | D11 is a process gate no criterion detects | **Fixed.** Section 8 says so plainly, and Section 9.4 ends with "what no criterion checks". The step-3-without-step-2 order is named as compiler-enforced, to show the dangerous direction is the silent one. |
| 12 | `dev-rust`'s corrections to the 5.1 inventory | **Applied.** Production browser dispatch at `web/commands.rs:760-766` deserializes rather than building a literal; `web/commands.rs:1220-1230` is test-only; `sample_config()` added; `populated_non_stop()` has three consumers, tabulated in 5.1.3. |
| 13 | `dev-webpage-ui`'s correction to the 2.7 fixture inventory | **Applied.** Five more fixture files listed; all spread, so none breaks, and the larger count strengthens D1. |
| 14 | D10's unnamed regression net | **Fixed.** New Section 5.7 names the three files, forbids editing them, and Sections 8 and 9.4 criterion 8 enforce it. |

Two corrections that no reviewer asked for but that follow from #2 and #10 above are also applied: Section 2.5 now states the executed failure mode of a missing D2 (the entry never appears, rather than reverting later), and acceptance criterion 9a rather than 9b is identified as the manual detector.

## 12. Enrichment pass: dev-rust (backend half)

Written by `dev-rust`. Scope of this section: `src-tauri/` only. Nothing above this line was edited.

**Verification method.** Every coordinate below was re-read at branch HEAD `f15f59a4` with a clean working tree (`git status --porcelain` empty), which is the plan's own baseline, so the plan's coordinates and mine are directly comparable. The blast radius was established twice by independent means: a text search (`NonStopGroupConfig` across `*.rs`) and a call-graph trace of `save_workgroup_groups` against a full index of the same `f15f59a4` tree (22317 nodes, 119657 edges). The two agree.

### 12.1 Verdicts at a glance

| Asked | Verdict |
| --- | --- |
| D1, the field and its attributes | **Confirmed.** No other serialization or migration site needs work (12.4). |
| D11, Rust lands first | **Confirmed, and the case is stronger than the plan states** (12.2). |
| Blast radius is exactly two literal sites | **Confirmed exactly**, with two bookkeeping corrections (12.3). |
| `populated_non_stop()` gets `favorite: true` | **Agreed.** The plan under-counts its consumers; none of them breaks (12.5). |
| R1 and R2 sufficient? `update_project_groups` covered? | R1 and R2 are correct and compile as written. One genuine gap, one proposed R3, and a reasoned "no" on a command-layer test (12.6). |
| Downgrade acceptable? | **No major problem.** The mechanism is confirmed and is broader than "downgrade" (12.7). |
| Unknown-key preservation still holds? | **Confirmed**, and the test's name oversells what it guards (12.7). |
| Is the plan enough to implement the Rust half cold? | **Yes** (12.9). |

### 12.2 D11: confirmed, and it rests on three failure points rather than one

The chain the architect asserts is real, and I traced it end to end:

1. `save()` awaits the backend and rehydrates from the **response**, not from the local config: `ProjectAPI.updateGroups(...)` at `src/sidebar/stores/workgroup-groups.ts:529`, then `setConfig(projectPath, saved, ...)` at `:535`.
2. `update_project_groups` takes `config: WorkgroupGroupsConfig` (`src-tauri/src/commands/project_settings.rs:36-41`). `WorkgroupGroupsConfig` and `NonStopGroupConfig` carry no `deny_unknown_fields` (`project_settings.rs:31-32`, `:86-88`; a search for `deny_unknown_fields` across `src-tauri/` finds it only in `api/schema.rs`, `phone/`, `seed_manifest.rs` and three unrelated payload structs at `web/commands.rs:44,50,54`), so an unmodelled `favorite` is dropped at parse time.
3. `save_workgroup_groups` returns the struct it was handed (`project_settings.rs:271`), which `update_project_groups` forwards as `Ok(result)` (`commands/project_settings.rs:50`).

So the toggle would visibly revert. **D11 stands.**

Three refinements, none of which weakens it:

- **The plan names the weakest of the three failure points.** Writing is not the first thing to break; reading is. Against a backend without the field, `get_project_groups` (`commands/project_settings.rs:19-26`) can never *emit* `favorite`, because the struct has nowhere to hold it. A frontend-first landing therefore does not produce "a toggle that reverts on save"; it produces a feature that is dead from the first load, before any save happens. The write-side argument in D11 is the second-order symptom.
- **Third failure point: the cross-window event.** `project_groups_updated_payload` serializes the same struct (`commands/project_settings.rs:12-17`) and feeds both the Tauri event and the WebSocket broadcast (`:43-49`). Without the Rust field, Section 6 edge case 15 (two windows) cannot work either.
- **D11 and D2 are independent filters in series, and the plan reads as though D11 were "the" gate.** Even with the Rust field landed, `setConfig` runs `normalizeNonStop` on the save path (`workgroup-groups.ts:391`) and that function rebuilds the object field by field (`:104-122`, verified: `favorite` is absent today). Getting the landing order right does not rescue a missing D2, and getting D2 right does not rescue a wrong landing order. Section 8's hard gate is necessary but not sufficient, and acceptance criterion 6 is the check that covers the other half.

**The reverse direction is safe, which is what makes the ordering usable.** With commit 1 landed and the old frontend still running, the frontend sends `nonStop` without `favorite`, `#[serde(default)]` supplies `false` (`project_settings.rs:89` is the same pattern already proven by `show`), and `favorite: false` is written to disk. No parse error, no data loss, nothing to roll back. Commit 1 is not merely self-contained at compile time; it is benign at runtime.

### 12.3 Blast radius: confirmed exactly, plus two bookkeeping corrections

**Confirmed.** `NonStopGroupConfig` appears in exactly six places, all inside `src-tauri/src/config/project_settings.rs`: the field on the parent struct (`:42`), a doc comment (`:56`), the struct itself (`:88`), `impl Default` (`:102`), and the test helper's signature and literal (`:338-339`). Nothing in `crates/`, nothing elsewhere in `src-tauri/`. Only two of those are struct literals, exactly as Section 2.7 claims: `impl Default` (`:102-113`) and `populated_non_stop()` (`:338-353`). The call-graph trace of `save_workgroup_groups` independently returns only two production callers, `update_project_groups_inner` and, at the second hop, `dispatch_browser_project_command` plus `update_project_groups`; everything else it reaches is a test.

Two corrections to the surrounding bookkeeping, neither of which changes the work:

1. **Section 5.1's parting note mischaracterizes its example.** `src-tauri/src/web/commands.rs:1220-1230` is inside `#[cfg(test)] mod tests`, which opens at `:939`. The production dispatch is at `:760-766` and it does not build the struct by literal at all; it deserializes into it via `require_json`, so it carries the new field for free. Worth stating plainly because that dispatch is the browser-side twin of the Tauri command and is the second entry point D11 has to hold for.
2. **There is a second test-only literal the plan does not mention**: `sample_config()` at `src-tauri/src/commands/project_settings.rs:64-76`, also with `non_stop: None`. Like the one at `web/commands.rs`, it builds `WorkgroupGroupsConfig`, not `NonStopGroupConfig`, so it keeps compiling untouched. Recorded so the "exactly two sites" claim is not read as "exactly two sites mentioning the config at all".

**Two further claims checked and confirmed**, because both are load-bearing for Section 5.6:

- `src-tauri/module-arcs.txt` records module-to-module edges only (`:383-384`, `:553-554`, `:957`, `:964` are the `project_settings` rows), never symbols or fields. Adding a field creates no edge. No update needed, as Section 5.6 says.
- There is no JSON fixture or golden file anywhere in the repo that pins the on-disk shape: a search for `nonStop` across `*.json` returns nothing. Every Rust assertion about the file builds its JSON inline. So there is no snapshot to regenerate.

**The backend watchdog is confirmed uninvolved, and more strongly than Section 2.7 puts it.** `src-tauri/src/loops/non_stop_watchdog.rs` never sees `NonStopGroupConfig`; it works off `NonStopReport`, a separate DTO with its own `tolerance_seconds` (`:55`) that the frontend pushes in through `non_stop_report` (`src-tauri/src/commands/non_stop.rs:12-14`). The backend loop does not read the config struct at all, so it cannot be affected by a new field on it.

### 12.4 D1 confirmed: the full serialization inventory, and why nothing else moves

`#[serde(default)] pub favorite: bool` with no `skip_serializing_if` is right, for the reason D1 gives: `favorite_flag_round_trips_through_save_load` (`:315-335`, specifically the `assert_eq!(persisted["groups"][1]["favorite"], false)` at `:335`) enshrines the convention. Confirmed that no `skip_serializing_if` exists anywhere in the file today, so acceptance criterion 5 passes at baseline and stays meaningful.

Every place `NonStopGroupConfig` crosses a serialization boundary, and what each needs:

| # | Site | Needs work? |
| --- | --- | --- |
| 1 | `save_workgroup_groups`, `serde_json::to_value(ns)` (`project_settings.rs:259-262`) | No. Serializes the whole struct. |
| 2 | `get_project_groups` return value (`commands/project_settings.rs:19-26`) | No. Tauri serializes the struct. |
| 3 | `update_project_groups` argument and return (`commands/project_settings.rs:36-51`) | No. |
| 4 | `project_groups_updated_payload` (`commands/project_settings.rs:12-17`), feeding the Tauri event and the WS broadcast | No. |
| 5 | Browser dispatch `UpdateProjectGroups` (`web/commands.rs:760-766`), via `require_json` | No. |

**Migrations: none required, and none exist to extend.** `normalize_groups_config` (`:134-154`) is the only repair pass, it runs on load only (`:225`) and not on save (`:234` validates but does not normalize), and it clamps numbers and repairs the legacy name. A bool has no range, which is what 5.1.4 says. Confirmed correct.

One consequence worth stating explicitly, because it is the load-bearing half of D3: since `normalize_groups_config` is not asked to touch `favorite`, the backend will happily persist and reload `show: false` together with `favorite: true`. That combination is not repaired away behind the frontend's back, which is exactly what D3 and Section 6 edge case 3 require. The behaviour D3 promises is reachable.

### 12.5 `populated_non_stop()` with `favorite: true`: agreed, with a corrected consumer list

Agreed, and for the stated reason: `false` there would be indistinguishable from the serde default and the existing round trips would not actually exercise the field.

**Correction to 5.1.3: the helper has three existing consumers, not two.** The plan names `non_stop_round_trips` (`:720-734`) and `save_persists_non_stop_and_preserves_unknown_keys` (`:655-684`). It misses `save_none_removes_stale_non_stop_key` (`:686-718`, calls it at `:694`). I checked all three against `favorite: true`:

- `save_persists_non_stop_and_preserves_unknown_keys`: compares struct to struct at `:674` (`Some(populated_non_stop())` on both sides) and asserts on unrelated JSON keys at `:678-683`. Unaffected.
- `save_none_removes_stale_non_stop_key`: only asserts presence and then absence of the `nonStop` key (`:700`, `:712-715`). Unaffected.
- `non_stop_round_trips`: `assert_eq!(reloaded, config)` at `:733`, a whole-struct comparison. This is the one that gains real coverage from the change, which is the point of choosing `true`.

No existing test breaks.

### 12.6 Tests: R1 and R2 assessed, one real gap, and a reasoned "no" on the command layer

**R1 and R2 compile as written.** I checked the symbols rather than assuming: `project_with_workspace` (`:279`), `settings_path` (`:285`) and `populated_non_stop` (`:338`) all exist in the module; `Value` is in scope through `use super::*` (`:276`) reaching `use serde_json::Value` (`:2`); `WorkgroupGroupsConfig` derives `Clone` (`:31`) so `config.clone()` is fine, and R2's later `let mut off = config;` move is legal because the preceding `assert_eq!` only borrows. `assert_eq!(persisted["nonStop"]["favorite"], true)` compares `Value` against `bool`, which serde_json supports and which `:331` already does today, so clippy's `bool_assert_comparison` does not fire (it targets `bool` operands, not `Value`).

**Baseline count confirmed.** The `project_settings` test module holds exactly 20 `#[test]` functions today (`:307` through `:736`, counted individually). R1 and R2 take it to 22, matching acceptance criterion 4.

**The real gap.** Neither R1 nor R2 covers the direction D11 is actually about: **inbound JSON that carries `favorite: true`**. R1 proves absence defaults to `false`. R2 proves a Rust-constructed `true` survives a disk round trip. Nothing proves that a `favorite: true` arriving from the frontend is deserialized rather than ignored, which is the precise behaviour whose absence D11 warns about. That path is worth one cheap test, and it fits inside `project_settings.rs`, so it does not touch acceptance criterion 8's seven-file list:

```rust
    #[test]
    fn non_stop_favorite_survives_deserialization_from_frontend_json() {
        let project = project_with_workspace();
        // The shape `update_project_groups` receives from the store, favorite included.
        let incoming = r#"{"groups":[],"showAll":true,"showUngrouped":true,"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3},"favorite":true}}"#;
        let config: WorkgroupGroupsConfig = serde_json::from_str(incoming).expect("parse incoming");
        assert!(config.non_stop.as_ref().expect("nonStop present").favorite);

        save_workgroup_groups(project.path(), config).expect("save");
        let reloaded = load_workgroup_groups(project.path()).expect("reload");
        assert!(reloaded.non_stop.expect("nonStop present").favorite);
    }
```

**Cost of accepting R3:** acceptance criterion 4 becomes 20 to 23 rather than 20 to 22. No other criterion moves. Architect's call; I have not changed criterion 4.

**On a test at the `update_project_groups` layer: not worth it, and it would cost more than it gives.** `update_project_groups_inner` is a one-line delegation to `save_workgroup_groups` (`commands/project_settings.rs:28-33`) with no logic of its own, so a test there would assert serde's behaviour a third time through one extra stack frame. It would also have to touch `src-tauri/src/commands/project_settings.rs`, which appears in neither Section 3's in-scope table nor Section 5.6's not-touched list, and would break acceptance criterion 8's exact seven-file diff. R3 above covers the same risk inside a file already in scope. Recorded so the omission is a decision.

One observation, not a request: `sample_config()` (`commands/project_settings.rs:64-76`) uses `non_stop: None`, so the command layer has never exercised `nonStop` at all. That predates this issue and is not #1257's to fix.

### 12.7 Downgrade and unknown-key preservation: both confirmed, both described imprecisely

**Downgrade (Section 7): no major problem, and I agree it is acceptable.** The mechanism is confirmed at `project_settings.rs:257-267`: the `Some` arm does `obj.insert("nonStop", ...)`, which replaces the entire value. An older binary parses the file, ignores the unknown `favorite`, and on its next save writes back a `nonStop` object rebuilt from its own struct, so the key is gone. One presentation bit, recoverable by re-favoriting, identical in kind to what #965 accepted.

**Precision worth adding:** this is not a downgrade-specific behaviour. Every save by every build replaces the whole `nonStop` object, so *any* unknown key inside `nonStop` is dropped on every save. Downgrade is just the case where `favorite` happens to be one of them. Stating it that way makes the guarantee checkable instead of version-dependent.

**Unknown-key preservation (`save_persists_non_stop_and_preserves_unknown_keys`, `:655-684`): confirmed unaffected, and structurally incapable of being affected.** What that test guards is **root-level** keys, `agents` and `tooling` (`:678-679`), and they survive because `update_config_json_object` reads the existing root object, hands only that map to the mutator, and reserializes the whole root (`src-tauri/src/config/local_config_io.rs:23-44`). That path never inspects the shape of `NonStopGroupConfig`, so adding a field to the struct cannot influence it. The test's own `nonStop` assertions (`:680-683`) read individual fields by name and are indifferent to a new sibling.

Caveat on that test's name, for whoever reads it next: it does **not** demonstrate preservation of unknown keys *inside* `nonStop`, and per the paragraph above no such preservation exists. The name reads like a stronger guarantee than the body provides. Pre-existing since #777, flagged rather than changed.

### 12.8 Adjacent findings, reported and not changed

1. **Section 6 edge case 11 is correct but sits in tension with the file's stated philosophy.** A hand-edited `"favorite": "yes"` does fail the whole load with `Failed to parse project groups from ...` (`:217-224`), and that is indeed how every typed field on the struct already behaves, `show` included. But the module comment at `:56-59` promises that a bad hand-edited `nonStop` value is "repaired (clamped) on load, never fatally rejected, so a bad/hand-edited value can never nuke the user's real groups", and that promise only ever covered the numeric fields. The new field adds one more key to which the comment's promise does not apply. Pre-existing and consistent with `WorkgroupGroup::favorite`; changing it now would make the two favorite fields behave differently, which would be worse. Recorded only.
2. **`save_workgroup_groups` validates but never normalizes** (`:234` versus the load path's `:225-226`). A config written through the IPC command therefore reaches disk unclamped and is repaired only on the next load. Irrelevant to a bool, and unchanged by this issue. Noted because it is the reason 5.1.4's "no change" is safe rather than merely convenient.

### 12.9 Readiness

**The plan is sufficient to implement the Rust half cold.** Section 5.1 is literal, its three edit sites are verified to exist at the quoted coordinates, and Section 9.1 supplies compilable test bodies. I need no further input from the architect to execute step 1 of Section 8. The only item awaiting a decision is whether R3 (12.6) is adopted, and its absence does not block: R1 and R2 alone still leave step 1 self-contained and verifiable by `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings` and `cargo test --lib --bins --tests`.

Nothing in D1 through D12 is challenged. The corrections above are to supporting statements, not to decisions.

## 13. Enrichment pass: dev-webpage-ui (frontend half)

Written by `dev-webpage-ui`. Scope of this section: `src/` only. Nothing above this line was edited, including Section 12.

**Verification method.** Every coordinate below was re-read at branch HEAD (`feature/1257-nonstop-favorite`, zero commits, `git status --porcelain` empty, so the tree is byte-identical to the plan's `f15f59a4` baseline). The plan file's SHA-256 matched the one I was handed before I started. This replica has **no `node_modules`**, so I could not execute `npx tsc --noEmit` or `vitest`: every claim below comes from reading the code, and the test counts are static (`^\s+it\(`), same method the architect used.

### 13.1 Verdicts at a glance

| Asked | Verdict |
| --- | --- |
| D2 closes every path through `setConfig`? Any second field-by-field rebuild? | **Confirmed, and `normalizeNonStop` is the only one** — inventory in 13.2. But the plan **misdescribes the failure mode**, which makes acceptance criterion 9b claim a job it does not do (13.2). |
| D10 extraction correct, nothing broken? | **Confirmed verbatim.** Two additions: three test files outside the seven-file diff are its regression net and are unnamed (13.3), and it carries an unstated visual consequence (13.4). |
| D4 holds against the current `flatMap`? | **Confirmed** (13.5). |
| D5 covers everything `automation-bridge.ts` consumes? | **Confirmed, and by construction**: the bridge is entirely DOM-driven, there is no registry to update (13.6). |
| D6 is the right condition? | **Confirmed**, including `nonStop === null`. It depends on a reactivity property the plan never states (13.7). |
| C2..C7 and the C1 rewrite sufficient? | **One defect: C7 as specified fails.** Two cases missing (13.8). |
| Anything unimplementable as written in `src/`? | **No** (13.9). |
| Enough to implement the frontend cold? | **Yes**, with C7 corrected (13.9). |

Nothing in D1 through D12 is challenged.

### 13.2 D2: confirmed, the inventory is complete, and the failure mode is misdescribed

**The propagation fix is right and there is no second trap.** I enumerated every site in `src/` that produces a `nonStop` object. All of them spread, so all of them carry a new scalar field for free:

| Site | Form |
| --- | --- |
| store `cloneConfig` (`workgroup-groups.ts:130-131`) | `{ ...config.nonStop, telegram: {...}, sound: {...} }` |
| `addWorkgroupToNonStop` (`:655`, `:666`) | `{ ...defaultNonStop(), ... }` / `{ ...current, regex }` |
| `removeWorkgroupFromNonStop` (`:686`) | `{ ...current, regex }` |
| modal `cloneConfig` (`WorkgroupGroupsModal.tsx:32-34`) | spread |
| modal `setNonStop` / `setNonStopTelegram` / `setNonStopSound` (`:139`, `:144`, `:149`) | spread |

`normalizeNonStop` (`:104-122`) is the **only** field-by-field rebuild in the frontend. Section 2.5 is correct that it is the single most important line of the change, and `cloneConfig` is correctly identified as a non-trap.

**Correction, and it is material because it retargets a manual check.** Section 2.5 and acceptance criterion 9b both describe the missing-D2 symptom as *"favoriting appears to work and then silently reverts"* on a later, unrelated save. That is not what happens. `save()` sends the config **outbound** as `cloneConfig(config)` (`:529`), which is spread-safe, and only then rehydrates the store from the **response** through `setConfig` (`:535`) → `normalizeNonStop`. And `ensureLoaded` rehydrates through the same `setConfig` (`:423`). So with D2 missing:

1. **Load:** a `favorite: true` already on disk is dropped before it ever reaches the store. The entry never renders at startup.
2. **The favoriting click itself:** the flag reaches disk correctly, the backend echoes it back, and `setConfig` strips it from the response. The Favorites entry **never appears at all**, on the very first click.
3. Only *afterwards* does the on-disk value decay, because the next `save()` reads the now-flagless store config and `#[serde(default)]` writes `false`.

The practical consequence: **acceptance criterion 9b is not the step that catches a missing D2 — 9a is**, and 9a is the first step of the manual check, so the failure is caught one step earlier and far more cheaply than the plan promises. 9b should keep its place for a different and still-good reason: it is the only manual check that the flag survives a save that has nothing to do with favorites, which is the *decay* in point 3. I suggest rewording 9b's purpose rather than moving it. On the automated side nothing changes: **S1 already contains the real detector** — its first assertion, right after `ensureLoaded`, is exactly case 1 above.

**One check on 5.3.3 that the plan does not make explicit.** `setNonStopFavorite` reads `this.config()`, which is post-normalization. With D2 present that is a concrete boolean, so `!!current.favorite === favorite` is exact. With D2 absent it is `undefined`, and `!!undefined === false` still makes the guard behave correctly rather than silently no-opping. The command has no hidden dependency on D2 landing first; it just cannot be *observed* to work without it.

### 13.3 D10: verbatim, and its regression net is three files the plan never names

I diffed the proposed `nonStopButtonFor` field by field against the inlined block at `:337-352`. It is a verbatim move: same `key`, same `buttonContent` spread **after** `key` (so `name`/`counter`/`working`/`raiseHand` are not shadowed), same `selection`, `workgroups`, `title`, `reorderable: false`, `groupId: null`, `groupIndex: null`. 5.4.4's note that `raiseHand` is deliberately not overridden to `false` (unlike `All` at `:315`) is correct and load-bearing.

**What the plan is missing: three existing test files pin that behaviour, none of them is in the seven-file diff of acceptance criterion 8, and none is in Section 5.6's not-touched list.** They are the evidence the move was verbatim, and they must stay green **without being edited**:

| File | What it pins |
| --- | --- |
| `src/sidebar/components/WorkgroupGroupRail.test.tsx:541-544`, `:558` | rail order `["all","ungrouped","nonstop","ui","rust"]`, the `1/1` counter, the running dot, and disappearance at `show: false` |
| `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx:356-367` | the `workgroup-group-rail-title-system` class on the Non-stop title and `railRaiseHands()` containing `nonstop` — the #775 "Non-stop keeps the hand" decision |
| `src/sidebar/watchdog/rail-watchdog-parity.test.tsx:84-85`, `:157-158` | parity between the rail's Non-stop `working/total` counter and the watchdog report |

Request: name these three in Section 5.6 and add "these three pass unmodified" to Section 8's final verification. **If the implementer finds themselves editing any of them, the extraction was not verbatim and that is the signal to stop.** Right now a well-meaning implementer could "fix" a failure in `rail-watchdog-parity.test.tsx` and quietly break the counter contract instead.

### 13.4 D10's unstated visual consequence: the Favorites Non-stop entry renders bold

`RailButton` applies `workgroup-group-rail-title-system` from `props.button.selection.kind !== "group"` (`WorkgroupGroupRail.tsx:222`), and that class is `font-weight: 700` (`src/sidebar/styles/sidebar.css:3424-3426`).

Because `nonStopButtonFor` keeps `selection: { kind: "nonstop" }` — which it must, for `isSelected` (`:150-155`) and for edge case 5 — the Non-stop entry in **Favorites** renders **bold**, sitting next to favorited groups that render at normal weight. Today the Favorites section is visually uniform; after this change it is not.

I think the behaviour is right: it matches how the same entry already renders in its project section, and #775/#777 deliberately bold the built-ins to distinguish them from user groups. But it is a visible design change inside the section this issue touches, it is nowhere in Sections 4, 6 or 7, and it is not pinned by any test the plan specifies. Recommend: record it as a line in Section 6 and pin it with one assertion inside C2, so a reviewer reads it as a decision instead of filing it as a defect.

### 13.5 D4: holds

The rewrite from `.filter().map()` to a per-project imperative push is the correct shape. The `flatMap` still fixes project order, and the push order fixes the intra-project order, which together make the list total and deterministic as D4 claims. No reactivity regression: `<For>` is reference-keyed, and the current `.map()` already mints fresh objects on every recompute, so the re-render behaviour is unchanged by moving to a loop.

One non-obvious property worth recording, unchanged by this plan: `FavoritesRailSection` never calls `ensureLoaded` — only `ProjectRailSection` does (`:298-300`). So a favorited Non-stop entry appears only once that project's rail section has loaded its config, exactly like a favorited group. Pre-existing, but someone reading D4 could reasonably expect Favorites to be self-sufficient.

### 13.6 D5: confirmed, and the reason is stronger than "a disjoint prefix cannot collide"

`automation-bridge.ts` consumes test ids **entirely from the live DOM**. There is no registry, allowlist, catalog or generated list of ids anywhere to keep in sync:

- `queryAutomationTargets` (`:335-338`) builds `[data-ac-testid="${cssEscape(testId)}"]` and runs it — exact match, no prefix logic.
- `availableTargets` (`:383-388`) enumerates `[data-ac-testid]` from the DOM, snapshots, sorts and truncates.

So D5 is sufficient by construction: nothing else needs to learn about the new ids. Two footnotes, neither of which changes the decision:

1. `availableTargets()` sorts by `testId` and truncates at `MAX_AVAILABLE_TARGETS`, so the new ids will appear adjacent to `workgroupGroups.favoriteButton.*` in `missing_selector` diagnostics. Cosmetic.
2. The bridge queries **across open shadow roots** (`queryAcrossOpenRoots`, `:390-392`), so "unique in the rendered DOM" means the whole document tree, not just the rail. The disjoint prefix satisfies that too, but it is the reason the reserved-key alternative would have been worse than D5 already argues: a duplicate could arrive from outside the rail entirely.

### 13.7 D6: correct condition, resting on a reactivity property the plan does not state

`!workgroupGroupsStore.config(target.projectPath).nonStop?.show` is right, and it handles `nonStop === null` for free: optional chaining yields `undefined` and `!undefined` is `true`, so a deleted Non-stop closes the menu as well as a switched-off one. The `&&` short-circuit on `target.kind === "nonstop"` keeps the config read out of the `project`-target path, matching how `groupGone` is already written.

**What the rationale should say and does not:** the effect only re-runs on a `show` flip because `workgroupGroupsStore.config()` returns `cloneConfig(...)` (`:440-442`), and `cloneConfig` **spreads** the store's `nonStop` proxy (`:130-131`). A spread reads every own property, so `show` is tracked by the effect. If `config()` were ever changed to return the raw object, to memoize, or to shallow-copy lazily, D6 would degrade into a no-op with no test failure anywhere except C5. One sentence in D6 turns that from an accident into an invariant.

### 13.8 Tests: one defect in C7, two missing cases, everything else verified to compile

**C7 as specified fails.** It selects `[data-ac-testid^="workgroupGroups.favorite"]` inside `workgroupGroups.favorites`. That prefix also matches the section's own header button, `data-ac-testid="workgroupGroups.favorites.header"` (`:263`) — `"workgroupGroups.favorites.header"` starts with `"workgroupGroups.favorite"`. Scoping to the container excludes the container but **not** the header, which is a child. The header would come out as the first element and the order assertion would fail on an otherwise-correct implementation.

Two ways to fix it; I recommend the second, because it is prefix-exact (note the trailing dots) and mirrors how `favoriteButtonKeys()` (`:100-104`) and the new `nonStopFavoriteKeys()` already select:

```ts
// Option A: scope to the scroll container, which excludes the header (:268).
document.querySelectorAll('.workgroup-group-rail-favorites-scroll [data-ac-testid^="workgroupGroups.favorite"]')

// Option B, preferred: select the union of the two BUTTON prefixes explicitly.
document.querySelectorAll(
  '[data-ac-testid^="workgroupGroups.favoriteButton."], [data-ac-testid^="workgroupGroups.favoriteNonStopButton."]'
)
```

**Missing C8: both renderings must show pressed when the entry is selected.** Section 6 edge case 5 asserts in prose that "`isSelected` marks both renderings pressed, because both carry the same `selection`", and nothing pins it. That is precisely the property D10's extraction could regress, because it depends on `nonStopButtonFor` returning the same `selection` shape at both call sites. One cheap case: click `workgroupGroups.favoriteNonStopButton.Project`, then assert `aria-pressed === "true"` on both it and `workgroupGroups.button.nonstop`.

**Missing C9: the return trip of D3.** C4 pins the render side (`show: false, favorite: true` → hidden, no `update_project_groups` call). What is unpinned is D3's other half, "the flag is kept, not cleared": flip `show` back to `true` through `applyExternalUpdate` and assert the Favorites entry reappears with no save in between. Without it, an implementation that clears the flag on hide would pass C4 and still violate D3.

**Everything the plan's tests depend on exists and compiles.** I checked each symbol rather than assuming: `railFake` (`favorites.test.tsx:153-162`, and note it already keys `get_project_groups` on `args.path`, which C2..C7 need), `openMenuOn` (`:144-147`), `target` / `maybeTarget` (`:78-86`), `favoriteButtonKeys` (`:100-104`), `groupsConfig` (`:66-76`), `project()` (`:44-54`), `defaultNonStop` imported at `:19`; `FakeTransport.lastCall` (`fake-transport.ts:117`) and `.callsFor` (`:113`); and **`normalizeNonStop` is already imported** in `workgroup-groups.test.ts:11`, so S1's direct calls need no new import. `applyExternalUpdate` for C5 is at `workgroup-groups.ts:494-515`.

**Baseline counts independently verified, both correct.** `^\s+it\(` at branch HEAD gives **15** in `WorkgroupGroupRail.favorites.test.tsx` and **19** in `workgroup-groups.test.ts`, matching acceptance criterion 2. If C8 and C9 are adopted, the favorites target becomes **15 → 23** instead of 15 → 21; `workgroup-groups.test.ts` stays 19 → 21. Architect's call — I have not edited criterion 2.

**One correction to Section 2.7's fixture inventory, which does not change D1.** It names `workgroup-groups.test.ts:341-348` as the single non-spread exception. There are more files building `nonStop` literals than the list covers — `WorkgroupGroupRail.raise-hand.test.tsx:341-345`, `WorkgroupGroupRail.autofocus.test.tsx:70-74`, `WorkgroupGroupRail.test.tsx:531`, `:552`, `WorkgroupGroupsModal.nonstop.test.tsx:89` — though I checked each and every one spreads `defaultNonStop()`, so none breaks. (`workgroup-groups.test.ts:226` and `:410` look like exceptions but are `toMatchObject` partial matchers, not typed values.) The inventory is incomplete rather than wrong, and the incompleteness **strengthens** D1: there is more hand-written fixture surface than the plan counted, so making the TS field required would cost more than it appeared to.

### 13.9 Readiness

**The plan is sufficient to implement the frontend half cold, with C7 corrected.** Nothing in it is unimplementable in `src/`. I confirmed each edit site exists at the quoted coordinates: the type import (`:3`, which already pulls `AcWorkgroup` and `WorkgroupGroup` from `../../shared/types`, so 5.4.1 is a one-token change), `RailContextTarget` (`:45-47`), `favoriteRailTestIds` (`:122-129`), `groupButtonFor` (`:131-148`), `FavoritesRailSection` (`:240-291`), the `buttons()` memo's Non-stop block (`:337-352`), the project-section `onContextMenu` (`:602-609`), the staleness effect (`:704-718`), `favoriteTargetIsFavorited` (`:722-728`), `toggleFavoriteFromContextMenu` (`:737-746`) and the menu gate (`:798`).

The TypeScript narrowing the plan leans on works as written: `if (nonStop?.show)` narrows `NonStopGroupConfig | null | undefined` to `NonStopGroupConfig` at both call sites of `nonStopButtonFor`; the `FavoriteEntry` discriminated union narrows `entry.group.id` correctly in the `<For>` body; and 5.4.11's `if (!target || target.kind === "project") return;` narrows the remainder to the two writable variants, so the fail-loud property it claims for a future fourth variant is real.

**Open items for the architect, in priority order:**

1. **C7's selector is wrong** (13.8) — must be fixed or the test fails against a correct implementation.
2. **Acceptance criterion 9b's stated purpose is wrong** (13.2) — 9a is the D2 detector. Reword, do not move.
3. Name the three D10 regression files in Sections 5.6 and 8 (13.3).
4. Record the bold-in-Favorites consequence in Section 6 and pin it in C2 (13.4).
5. Adopt C8 and C9? (13.8) — if yes, criterion 2's favorites count becomes 15 → 23.
6. Add the `cloneConfig`-spread reactivity sentence to D6's rationale (13.7).

None of these blocks step 2 or step 3 of Section 8. Items 1 and 2 are the only ones that would cost the implementer time if left as they are.

## 14. Adversarial review: dev-rust-grinch

Written by `dev-rust-grinch`. Nothing above this line was edited, including Sections 12 and 13.

**Verification method — and the one thing that changed since the other two passes: this replica now has `node_modules` and a Cargo target, so everything below that says "verified" was RUN, not read.** `npm ci --prefer-offline` completed (exit 0) inside the replica repo. It touches nothing versioned (`git status --porcelain` empty before and after; `node_modules/` and `target/` are gitignored). `dev-rust` and `dev-webpage-ui` can now execute too. I also wrote a throwaway probe file, ran it, and deleted it; the tree is clean at `f15f59a4`.

### 14.1 Executed baseline: every count in Sections 9.4 and 12/13 is now measured, not inferred

| Claim | Command | Result |
| --- | --- | --- |
| Criterion 2, favorites suite = 15 | `npx vitest run src/sidebar/components/WorkgroupGroupRail.favorites.test.tsx` | **15 tests, 15 pass** |
| Criterion 2, store suite = 19 | `npx vitest run src/sidebar/stores/workgroup-groups.test.ts` | **19 tests, 19 pass** |
| Criterion 4, `project_settings` = 20 `#[test]` | `cargo test --lib config::project_settings` | **20 pass, 0 fail** |
| Criterion 1 | `npm run typecheck` | **exit 0, no diagnostics** |
| Criterion 3 | `npm run test:debt` | **exit 0** |
| 13.3's three D10 regression files | `npx vitest run` over all five files | **74 tests, 0 fail** |
| Section 8 step 0's force-add | `git check-ignore -v plans/1257-nonstop-favorite.md` | `.gitignore:11:plans/` — the `-f` is mandatory, confirmed |
| Branch-name gate | `npm run validate-branch-name` | `OK: feature/1257-nonstop-favorite` |

Nothing in the plan's arithmetic is wrong. The static counts both earlier passes produced match the real runs exactly.

### 14.2 Verdicts on the two decisions I was asked to break

**D11 (Rust first): CONFIRMED. I attacked it three ways and it did not give.**

1. *Is there a second `deny_unknown_fields` that would make the reverse order fail loudly instead of silently?* No. 12.2 calls `web/commands.rs:44,50,54` unrelated; I read them: `ThemeChangedPayload`, `ResourceMonitorAttachPayload`, `OpenSettingsPayload`. The browser dispatch for groups is `require_json` into `WorkgroupGroupsConfig` with no deny (`web/commands.rs:760-766`). The silent-drop premise holds on both entry points.
2. *Is there a Rust twin of the Section 2.5 field-by-field trap?* This was the most dangerous thing I could have found, and it does not exist. `save_workgroup_groups` writes `serde_json::to_value(ns)` for the whole struct (`project_settings.rs:257-263`) and `load_workgroup_groups` reads `serde_json::from_value` into the whole struct (`:217`). Neither side rebuilds `NonStopGroupConfig` key by key. The frontend's `normalizeNonStop` is the only such rebuild in the product, exactly as 13.2 says.
3. *Is there an order worse than the forbidden one?* Yes, and the plan does not name it. Section 8 forbids "step 2 without step 3". The symmetric case — **step 3 without step 2** — is worse in kind but harmless in practice: `WorkgroupGroupRail.tsx` would call `workgroupGroupsStore.setNonStopFavorite`, which would not exist, and `tsc` fails. It is the one ordering error that cannot ship. Worth a clause in Section 8 so the reader sees that the dangerous direction is the *silent* one and the other is compiler-enforced.

**One new observation that does not weaken D11 but should change how Section 8 words it.** The landing order is a **process** gate, and **no acceptance criterion detects a violation of it**. Criteria 5, 6 and 8 all inspect the *final* state, which is identical under either order. That is fine — a wrong order produces a transient, self-healing defect that disappears the moment the Rust commit lands — but Section 8 currently reads as though the criteria backed the gate up. They do not. Say "enforced by review, not by a check", so nobody assumes a green suite proves the order was respected.

**D3 (hide the favorite when `show === false`, keep the flag): CONFIRMED, and I can now show the load-bearing half instead of asserting it.**

The half of D3 nobody had verified is the promise in edge case 3 and manual steps 9e/9f: *"Turning `show` back on in the modal restores the entry with its flag intact."* That is only true if the modal's `show` toggle preserves the rest of the `nonStop` object instead of nulling it out. **I ran it.** Rendering `WorkgroupGroupsModal` against a loaded store holding `nonStop: {show:true, name:"Watchers", regex:"^(wg-1-dev-team)$"}`, unchecking `workgroupGroups.nonstop.show` and clicking Save sends:

```
{"show":false,"name":"Watchers","regex":"^(wg-1-dev-team)$","toleranceSeconds":30,"telegram":{...},"sound":{...}}
```

The object survives with every sibling field intact, because `setNonStop` spreads `ns()` (`WorkgroupGroupsModal.tsx:138-141`). On the Rust side the `Some(ns)` arm rewrites the whole `nonStop` key (`project_settings.rs:257-263`) and `normalize_groups_config` touches neither `show` nor a bool (`:134-154`), so `show:false + favorite:true` is not repaired away behind the frontend's back. **9e and 9f are reachable, and D3's "the flag is kept" is a fact rather than a hope.**

The rejected alternative also holds up: forcing `show: true` really would re-arm the watchdog, because `show` is the single switch for both concerns — the modal says so at `WorkgroupGroupsModal.tsx:198-201` ("single show toggle = rail visibility AND watchdog on/off").

**What breaks with the chosen option:** the flag becomes unreachable while `show` is off (edge case 3 admits this), so a user who hides Non-stop can never *un*favorite it, and re-enabling it months later resurfaces an entry they no longer remember pinning. That is a declared consequence of D3, not a defect, and the alternative is worse. I am not asking for a change.

### 14.3 Findings that break something as written

**F1 — [BREAKS] Edge case 12 is false, and test C6 builds the exact DOM that disproves it. Severity: medium.**

Edge case 12 states: *"Hand-edited config with a real group whose `id` is `"nonstop"`, and a favorited Non-stop → Both render, with disjoint test ids (D5). **No `duplicate_selector`**."* The second half is wrong. D5 makes the *Favorites* ids disjoint, but the **project section** has collided since #777, independently of this issue: `groupButtonFor` sets `key: group.id` (`WorkgroupGroupRail.tsx:139`), the Non-stop button sets `key: "nonstop"` (`:343`), and both feed `projectRailTestIds(button.key)` (`:592`), which emits `workgroupGroups.button.${key}`.

Executed, on unmodified `f15f59a4`:

```
duplicate count for workgroupGroups.button.nonstop = 2
all rail button ids = ["...button.all","...button.ungrouped","...button.nonstop","...button.nonstop"]
automation bridge response = {"ok":false,...,"error":"duplicate_selector",
  "message":"Multiple automation targets matched data-ac-testid=\"workgroupGroups.button.nonstop\"..."}
```

So in the very configuration edge case 12 describes, the bridge **does** fail `duplicate_selector` — on a different selector than the one D5 protects. C6's setup (`groups: [{ id: "nonstop", ... }]` plus a shown Non-stop) renders that DOM, so the plan would ship a test that stands in front of a live collision and asserts around it.

Nothing about D5 changes: D5 is right, and it correctly prevents this issue from adding a *second* collision. What must change is the claim. Fix: reword edge case 12 to promise disjointness **within Favorites** only, and record the project-section collision in Section 10 (it is the same shape as adjacent finding 2, and the same shape a group with `id: "all"` or `"ungrouped"` already produces today). Optionally one line in C6 documenting the known duplicate, so the next reader does not file it as new.

**F2 — [BREAKS] C7's selector, independently confirmed by running it. Severity: medium. Same defect 13.8 found.**

Executed inside `workgroupGroups.favorites`:

```
matches of ^workgroupGroups.favorite = ["workgroupGroups.favorites.header",
  "workgroupGroups.favoriteButton.Project.ui","workgroupGroups.favoriteButton.Project.rust"]
```

The header comes out first, so C7's order assertion fails against a correct implementation. One piece of evidence 13.8 does not have: I inventoried **every** `data-ac-testid^=` prefix in `src/` — there are eleven (`archivedProjects.row.`, `autoUnarchive.row.`, `session.${sessionId}.menu.repo.`, `settings.agent.reseedDefault.`, `settings.agentPreset.`, `settings.agentRow.`, `workgroupGroups.button.`, `workgroupGroups.dot.`, `workgroupGroups.favoriteButton.`, `workgroupGroups.raiseHand.`, plus C7's) — and **the bare `workgroupGroups.favorite` prefix exists nowhere in the codebase today**. The defect is introduced by the plan, not inherited. 13.8's Option B is the right fix.

**F3 — [BREAKS execution, not design] Section 8 step 1 assigns the backend commit to `dev-rust-grinch`. Severity: medium.**

Step 1 reads "**Backend (dev-rust-grinch)**". `dev-rust-grinch` is the adversarial reviewer, whose role forbids implementing ("Never implement fixes; report them to the developer", "Never merge, push, or modify branches"). Meanwhile Section 12 was written by `dev-rust`, which states in 12.9 that it needs no further input "to execute step 1 of Section 8". As written, the plan either has the reviewer implement the code it must later review — destroying the review's independence — or leaves step 1 unowned. Fix: step 1 belongs to `dev-rust`. This is the only thing in the plan that makes it not executable cold.

### 14.4 Findings that are real but do not break a decision

**F4 — [An edge case declared covered that is not] An unrelated group with an invalid regex makes every favorite save fail locally, before the transport. Severity: medium-low, pre-existing, symmetric with `setGroupFavorite`.**

Edge case 14 says a save can fail through "disk error, backend rejection". There is a third failure, more likely than either, and it never reaches the backend. Executed:

```
error after load = null
groups reached the store = ["bad","ui"]        // bad.regex === "^(["
setGroupFavorite(...,"ui",true) threw = "Group 1: regex is invalid."
store error now = "Group 1: regex is invalid."
update_project_groups calls = 0
```

Reachable because the three validation boundaries disagree: Rust checks regex **length only** (`project_settings.rs:194-198`), `ensureLoaded` validates with `validateRegexSyntax: false` (`workgroup-groups.ts:414`), and `save()` validates with `true` (`:520`). So a hand-edited or legacy config carrying one syntactically broken group regex loads clean, renders, and then makes **every** subsequent save fail — including `setNonStopFavorite`, which inherits the behaviour verbatim (D9). The user right-clicks Non-stop, clicks Favorite, nothing happens, and the rail shows an `!` badge about a *different* group. `.catch(() => {})` hides the throw, but `save()` has already written the message into the entry (`:523`), so the badge is sticky until the next successful save.

Not #1257's to fix, and not a reason to change D9. Ask: give it its own row in Section 6, so the plan stops implying that the only way a favorite save fails is the backend.

**F5 — [D6 is the correct condition, but its rationale is incomplete] The open menu has a second staleness case that D6 does not close. Severity: low, symmetric with groups.**

D6 says `!nonStop?.show` is *"the condition under which the open menu points at something the user can no longer see"*. There is a second one. Executed against today's group behaviour — the exact shape the Non-stop entry will inherit: open the context menu on a **Favorites** entry, then push an external update clearing `favorite` while the record still exists —

```
favorites entry still in DOM = false | context menu still open = true
```

The entry is gone; the menu stays. `groupGone` only asks whether the group still exists, and `nonStopGone` will only ask whether `show` is on. For a Non-stop favorite the consequence is identical: menu open, anchored to a button that no longer exists, while the entry remains visible in the project section (so it is relocated rather than invisible). I am **not** asking to widen D6 — closing on "the target's Favorites entry disappeared" is a bigger change, and the group path has lived with this since #965. I am asking that D6's rationale stop claiming to cover the whole staleness surface, because a reader will otherwise assume it does.

**F6 — [Section 5.6 overstates the modal's safety] The modal's draft is a construction-time snapshot with no resynchronization. Severity: low, pre-existing, out of scope.**

Section 5.6 calls `WorkgroupGroupsModal.tsx` "Verified safe as-is". It is safe with respect to the new field — every path spreads, 13.2's inventory is right and I re-checked it. But the draft is seeded exactly once, at component construction: `createSignal(cloneConfig(workgroupGroupsStore.config(props.projectPath)))` (`:46-48`), with no effect re-seeding it once the store finishes loading. I hit this by accident: my first modal probe constructed the modal before `ensureLoaded` resolved, and Save then wrote `{"show":false,"name":"Alert me!","regex":"(?!)",...}` over a config whose real values were `"Watchers"` and `"^(wg-1-dev-team)$"`. In the real app the rail loads the config before the modal can be opened, so this needs a very narrow race — but it is a fourth path that can revert `favorite`, alongside edge case 8, and it destroys far more than one presentation bit. Record it in Section 10; do not fix it here.

**F7 — [Type note] `nonStop: null` becomes `undefined` inside the store. Severity: informational.**

`cloneConfig` returns `config.nonStop ?? undefined` on the falsy arm (`workgroup-groups.ts:130-132`), so a backend `null` is stored as `undefined`. Executed: `typeof cfg.nonStop === "undefined"`, own property present. S2 is unaffected (`if (!current)` catches both) and `setNonStopFavorite` as written in 5.3.3 is correct. Flagged only so nobody writes `expect(config.nonStop).toBeNull()` in the new tests and then debugs it.

### 14.5 Things I attacked that did not give

Listed because a PASS with no attack surface named is worthless.

- **D2 closes the leak, and the leak is at load.** I confirmed 13.2's correction by execution rather than by reading. Feeding `favorite: true` through `get_project_groups` today: `favorite present after load = undefined`, and `nonStop keys after load = ["show","name","regex","toleranceSeconds","telegram","sound"]`. The outbound payload of the next unrelated save also carries no `favorite`. So without D2 the entry never appears **at all**, and Section 2.5's "appears to work and then silently reverts" is the wrong symptom. 13.2's item 2 is right, and so is its consequence for criterion 9b.
- **D5 is stronger than "a disjoint prefix cannot collide".** Beyond 13.6's argument that the bridge is DOM-driven, I checked the actual inventory: none of the eleven existing prefix selectors captures `workgroupGroups.favoriteNonStopButton.` / `...RaiseHand.` / `...Dot.`. No existing assertion silently starts counting the new entry.
- **D4 mirrors reality.** Executed rail order today is `["all","ungrouped","nonstop","ui","rust"]`, and Favorites renders in `config.groups` order. "Non-stop first inside its own project's block" is a faithful mirror, and the `flatMap` → per-project push rewrite preserves both orders.
- **D10's bold consequence is real.** Confirming 13.4 by execution: the only titles carrying `workgroup-group-rail-title-system` today are `All`, `Ungrouped` and `Alert me!`, all in the project section — **Favorites currently has none**. After this change the Non-stop entry is the first bold row that section has ever had. 13.4's recommendation stands.
- **No Rust migration or fixture is hiding anywhere.** 12.3's claims re-checked at source: `normalize_groups_config` clamps numbers only (`:134-154`), `validate_groups_config_structure` never inspects `nonStop` (`:156-202`), and the load path validates *after* normalizing (`:225-226`).
- **The web/browser half carries the field for free**, confirmed by reading the dispatch rather than trusting a summary (`web/commands.rs:760-766`).

### 14.6 Readiness

**Implementable cold: yes for the Rust half, yes for the frontend half, once three things are settled.** In priority order:

1. **F3** — reassign Section 8 step 1 to `dev-rust`. Until this is fixed the plan cannot be executed as written without violating the reviewer's role.
2. **F2** — fix C7's selector (13.8, Option B). Otherwise a correct implementation fails its own test.
3. **F1** — reword edge case 12 and record the pre-existing project-section collision.

F4, F5, F6 and F7 are documentation-level: they change what the plan *claims*, not what the implementer *writes*. None of them blocks any step of Section 8.

**No decision D1 through D12 is challenged.** D11 and D3, the two I was pointed at, both survived a deliberate attempt to break them, and I now have executed evidence for the parts of each that were previously only argued. The plan's coordinates, counts and baselines are accurate — I checked them by running the suites, not by counting `it(` lines.
