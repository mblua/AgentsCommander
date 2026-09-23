# Changelog

All notable user-facing changes are tracked here and in [GitHub Releases](https://github.com/mblua/AgentsCommander/releases).

This file follows a lightweight [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape: one section per release in reverse-chronological order. Each entry groups changes under **Added / Changed / Removed / Fixed / Security** where useful.

## Unreleased

### Added

- **The sidebar can collapse to a compact rail.** A `>>` control on the groups rail collapses the sidebar to the rail, theme, settings and `<<` controls, and the terminal takes the released space. Selecting any group expands it again and restores the width it had before collapsing. A configurable shortcut toggles it (default `Ctrl+Shift+E`, set in Settings); it works with terminal focus and the matched key never reaches the terminal. ([#2236](https://github.com/mblua/AgentsCommander/issues/2236), [#2283](https://github.com/mblua/AgentsCommander/issues/2283), [#2284](https://github.com/mblua/AgentsCommander/issues/2284), [#2285](https://github.com/mblua/AgentsCommander/issues/2285))
- **Coding agents can be reordered.** Move controls on the Settings rows and the profile-assignment Step 1 cards change the order, and both lists now show the saved order instead of an alphabetical one. A failed move shows an inline error and never applies a guessed order. ([#2314](https://github.com/mblua/AgentsCommander/issues/2314))
- **`room activity --project <PROJECT>` lists every room of a project** with its working state, CI state (`running`, `idle` or `unknown`) and task title. It is read-only. ([#2374](https://github.com/mblua/AgentsCommander/issues/2374))
- **Loop modals say when saving restarts the schedule.** Editing a Loop shows a notice only when the save would move the next run; creating a Loop always says the schedule starts at creation. ([#2288](https://github.com/mblua/AgentsCommander/issues/2288))

### Removed

- **The Guide window (Hints and Tutorial) and its ActionBar button are gone.** Home is unchanged, and existing settings files keep loading. ([#2412](https://github.com/mblua/AgentsCommander/issues/2412))

### Fixed

- **A window maximized on a first run is now remembered.** With no placement saved yet, AC samples the window's rectangle at startup, so launch, maximize and close stores that rectangle with `"maximized"` and the next start comes back maximized. A saved rectangle still wins over the startup sample, and a window that is already maximized, fullscreen or minimized when AC starts still stores no rectangle. ([#2393](https://github.com/mblua/AgentsCommander/issues/2393))
- **The Resource Monitor no longer runs out of memory on a process-tree cycle.** Windows PID reuse could leave a parent-PID cycle that made the tree walk loop until the app aborted; each process is now visited once. ([#2443](https://github.com/mblua/AgentsCommander/issues/2443))
- **Codex turns are closed reliably.** The `turn_complete` closure is accepted as well as `task_complete`, and a late answer for a superseded turn is no longer emitted. ([#2356](https://github.com/mblua/AgentsCommander/issues/2356))
- **Config files publish on deep Windows paths.** Paths longer than `MAX_PATH` no longer fail with os error 3. ([#2378](https://github.com/mblua/AgentsCommander/issues/2378))
- **An auto-closed orchestrator keeps its context.** A CI or other internal wake resumes its previous session, and a failed reopen keeps the close markers so a retry still resumes. ([#2411](https://github.com/mblua/AgentsCommander/issues/2411), [#2413](https://github.com/mblua/AgentsCommander/issues/2413))
- **Process liveness is real on Linux and macOS.** Session and `daemon.pid` checks no longer treat every process as alive. ([#2382](https://github.com/mblua/AgentsCommander/issues/2382), [#2394](https://github.com/mblua/AgentsCommander/issues/2394))
- **The Telegram icon in the sidebar menu renders on Linux and macOS.** It was drawn at 0x0 in WebKit. ([#2399](https://github.com/mblua/AgentsCommander/issues/2399))
- **Typing-hold padlock corrections.** The held count has no `#` prefix, the padlock is open and dimmed while inactive and closed and colored while holding, and a manual close releases itself after twice the `typingHoldSeconds` window. ([#2379](https://github.com/mblua/AgentsCommander/issues/2379))

## 0.38.0

### Added

- **Peer wakes are held while you are typing.** A message injected into a session by another agent now waits instead of landing in the middle of your keystrokes. The hold releases after a quiet window, or immediately when you open the padlock added to the terminal status bar, which also shows how many messages are being held. The window is the new `typingHoldSeconds` setting in Settings > General (default 30 seconds, accepted 1-3600; an invalid draft is rejected, never coerced). Your own input is never blocked. ([#2336](https://github.com/mblua/AgentsCommander/issues/2336), [#2337](https://github.com/mblua/AgentsCommander/issues/2337), [#2335](https://github.com/mblua/AgentsCommander/issues/2335))
- **The Resource Monitor has an integral view**, with validated PID filtering, text search, sorting, several pinned expansions at once, a process summary and hierarchy, explicit feedback when observation is partial, accessible controls and a responsive layout. ([#2245](https://github.com/mblua/AgentsCommander/issues/2245), [#2244](https://github.com/mblua/AgentsCommander/issues/2244))

### Changed

- **Terminal snapshots are now enabled by default.** A fresh install and an existing settings file without the key both start with snapshots on; an explicit `false` is preserved and a malformed value still fails closed. The Settings checkbox copy now reads "Enabled by default." ([#2317](https://github.com/mblua/AgentsCommander/issues/2317), [#2318](https://github.com/mblua/AgentsCommander/issues/2318), [#2319](https://github.com/mblua/AgentsCommander/issues/2319), [#2309](https://github.com/mblua/AgentsCommander/issues/2309))
- **The main window remembers its position, size and maximized state.** The placement pair is persisted through one narrow writer, fullscreen and minimized stay transient, and unmaximizing restores the last usable normal rectangle. ([#2348](https://github.com/mblua/AgentsCommander/issues/2348), [#2349](https://github.com/mblua/AgentsCommander/issues/2349), [#2350](https://github.com/mblua/AgentsCommander/issues/2350), [#2347](https://github.com/mblua/AgentsCommander/issues/2347))

### Fixed

- **CI activity on a repository's default branch no longer rings the chip or notifies the orchestrator.** A resolved default branch reports idle whatever GitHub answers, so it adds no room CI activity and sends no `ci-started` or `ci-finished` notice; an unresolved default branch is unchanged. The chip tooltip no longer claims `no CI activity for this commit`, because an absent CI suffix cannot tell idle from unknown. Rate-limit backoff, failure warnings and branch staleness are untouched. ([#2326](https://github.com/mblua/AgentsCommander/issues/2326), [#2329](https://github.com/mblua/AgentsCommander/issues/2329))
- **A coding agent's context file no longer repeats the `Role.md` YAML frontmatter.** That block is display metadata for the agent listing, not role instructions, so it is stripped once at the single join point that writes every generated `AGENTS.md` and `CLAUDE.md`. The canonical `Role.md` on disk keeps its frontmatter. ([#2364](https://github.com/mblua/AgentsCommander/issues/2364))
- **Deferred menu injections no longer surface as an Application Error.** A recoverable deferral from either mailbox injection path is logged at debug instead; real PTY failures still raise Application Error unchanged. ([#1883](https://github.com/mblua/AgentsCommander/issues/1883))
- **Canonical `room-<n>-<team>/<agent>` identifiers are accepted where only the legacy `wg-` form was.** Terminal-snapshot targets and the api-helper session bridge now share one room-aware validator, with every length, delimiter and character restriction unchanged. ([#2316](https://github.com/mblua/AgentsCommander/issues/2316), [#2361](https://github.com/mblua/AgentsCommander/issues/2361))
- **Internal work with no user-visible change**: the cognitive-complexity CI gate and its baseline, capture pipeline and three-platform merge; groundwork for the compact sidebar (core signal, main and browser hosts, terminal pulse, and the `sidebarCompactHotkey` setting) and for reordering registered coding agents (persisted order and its narrow IPC command); added test coverage; and documentation updates. ([#2234](https://github.com/mblua/AgentsCommander/issues/2234), [#2236](https://github.com/mblua/AgentsCommander/issues/2236), [#2306](https://github.com/mblua/AgentsCommander/issues/2306), [#2289](https://github.com/mblua/AgentsCommander/issues/2289), [#2321](https://github.com/mblua/AgentsCommander/issues/2321), [#2323](https://github.com/mblua/AgentsCommander/issues/2323), [#2246](https://github.com/mblua/AgentsCommander/issues/2246), [#2327](https://github.com/mblua/AgentsCommander/issues/2327))
## 0.37.0

### Fixed

- **`list-peers-lean --snapshot-targets` no longer aborts on a neighbouring directory it cannot verify.** A linked or junctioned room, a stale `__agent_*` replica, or a sibling project directory that fails identity verification is now skipped and counted instead of failing the whole command with `Error: unsafe_path` and exit 1. The counts are reported on stderr as integers only (`snapshot_targets_note skipped_project_children=… skipped_rooms=… skipped_replicas=…`), a `[]` result now says why on stderr, and a genuinely rejected `--root` names the rule that rejected it. Every emitted target still passes the same unchanged identity verification, and stdout JSON is unchanged. ([#2228](https://github.com/mblua/AgentsCommander/issues/2228), [#2223](https://github.com/mblua/AgentsCommander/issues/2223))
- **`terminal-snapshot` now says which argument it rejected and why.** Every CLI-originated rejection adds `field=` (when exactly one argument is at fault, from `token`, `root`, `to`, `format`, `output`, `timeout`) and `reason=` to the `terminal_snapshot_error` line, so the six previously identical `invalid_request` failures are distinguishable. `code=` values, exit codes, stdout and the HTTP API are unchanged, and the new tokens are fixed literals that never carry a path or a token. ([#2227](https://github.com/mblua/AgentsCommander/issues/2227), [#2223](https://github.com/mblua/AgentsCommander/issues/2223))

## 0.36.0

### Added

- **The "Update available" toast has a Copy button** that copies the upgrade command to the clipboard. The toast stays open so the command remains readable, and repeated copies reuse one confirmation toast instead of stacking. ([#2135](https://github.com/mblua/AgentsCommander/issues/2135))

### Fixed

- **The CI ring no longer blanks for minutes under GitHub's secondary rate limit.** The remote sweeper reserves every `gh` call against a 30-calls-per-minute budget, prioritizes repositories with confirmed running CI, and backs off a secondary 403 from 60 s instead of jumping to the 900 s cap. ([#2152](https://github.com/mblua/AgentsCommander/issues/2152))
- **Blind-gap notices no longer fire on healthy polling rounds.** The gap threshold rises to 600 s and the default branch-staleness interval drops to 260 s, so the notice means two consecutive lost checks. Installs that already persist `branchStalenessIntervalSecs: 300` keep that value. ([#2149](https://github.com/mblua/AgentsCommander/issues/2149))
- **CI activity counts only runs on the repository's current branch**, matched by exact name, so a branch sitting on the default branch's tip reports no CI and sends no notice. ([#2126](https://github.com/mblua/AgentsCommander/issues/2126))
- **No branch-stale notice on the default branch itself.** The repo chip still shows the orange bar with its behind-count, and an orchestrator row takes the working wash while its repo chip shows CI running. ([#2131](https://github.com/mblua/AgentsCommander/issues/2131))
- **The CI working tint also reaches the Orchestrators strip**, so one orchestrator can no longer appear tinted in the room tree and untinted in the strip. ([#2151](https://github.com/mblua/AgentsCommander/issues/2151))
- **Short status-line bursts after a long silence no longer keep idle sessions awake.** Output following at least 60 s of silence is held as a pending burst and counts as activity only once it reaches 1024 bytes or lasts 3 s, so auto-close and busy/idle edges ignore a coding agent's periodic update check. The per-agent `idleBurst` settings are overridable in `agents.local.json`. ([#2124](https://github.com/mblua/AgentsCommander/issues/2124))
- **Internal system notices survive case-skewed project paths.** A project path spelled with different letter case than the folder on disk no longer drops context alerts and remote-activity notices; filesystem identity still decides the match. ([#2113](https://github.com/mblua/AgentsCommander/issues/2113))
- **The project `.ac/.gitignore` ignores `seed-manifest.toml`**, migrating the previous un-ignore block while preserving user bytes. ([#2090](https://github.com/mblua/AgentsCommander/issues/2090))
- **Documentation corrections** for the seed manifest, portable readme storage after the configuration-directory change, macOS screenshot support, and the remote-activity settings, chip signals and notices. ([#2112](https://github.com/mblua/AgentsCommander/issues/2112), [#2080](https://github.com/mblua/AgentsCommander/issues/2080), [#2087](https://github.com/mblua/AgentsCommander/issues/2087), [#2085](https://github.com/mblua/AgentsCommander/issues/2085))
- **Frontend static-analysis findings were cleared and two test suites made deterministic**, with no behavior change. ([#2108](https://github.com/mblua/AgentsCommander/issues/2108), [#2011](https://github.com/mblua/AgentsCommander/issues/2011), [#1582](https://github.com/mblua/AgentsCommander/issues/1582))
## 0.35.0

### Added

- **Room repositories show remote CI and branch staleness.** When the GitHub CLI (`gh`) is available, a background sweeper checks whether CI is running on each room repository's exact HEAD and whether its default branch has commits the branch lacks. The repo chip shows a yellow ring while CI runs and an orange left bar when the branch is stale; an unknown state changes nothing, and without `gh` the feature stays inert. ([#2064](https://github.com/mblua/AgentsCommander/issues/2064), [#2082](https://github.com/mblua/AgentsCommander/issues/2082), [#2084](https://github.com/mblua/AgentsCommander/issues/2084))
- **The room orchestrator receives remote-activity notices** when CI starts, when CI finishes and when the branch goes stale, with editable message texts. ([#2083](https://github.com/mblua/AgentsCommander/issues/2083))
- **Screenshot capture works on macOS**, with a Screen Recording permission check at capture time and Retina-aware overlay sizing. Real-Mac acceptance is still pending. ([#2086](https://github.com/mblua/AgentsCommander/issues/2086))
- **A global npm install on Linux creates a desktop menu entry.** `agentscommander.desktop` is written under `$XDG_DATA_HOME/applications` (or `~/.local/share/applications`) with the packaged icon. It is skipped for local installs and with `AGENTSCOMMANDER_NO_SHORTCUT=1`, never fails the install, and must be removed by hand. ([#2066](https://github.com/mblua/AgentsCommander/issues/2066))

### Removed

- **The `portable.txt` marker no longer affects configuration selection.** Unsuffixed executables use `$HOME/.agentscommander`; suffixed executables use their adjacent `.agentscommander_<suffix>` directory and refuse to start when it is not writable. `AGENTSCOMMANDER_CONFIG_DIR` still wins. ([#1932](https://github.com/mblua/AgentsCommander/issues/1932))

### Fixed

- **TASK.md lock acquisition retries transient Windows access-denied and sharing-violation errors** under its existing deadline instead of failing. ([#1579](https://github.com/mblua/AgentsCommander/issues/1579))
- **Container shutdown workers are reclaimed after a shutdown overruns its deadline** instead of leaking. ([#1581](https://github.com/mblua/AgentsCommander/issues/1581))
- **CI runs every cargo invocation with `--locked`**, and several static-analysis findings were cleared without behavior changes. ([#2075](https://github.com/mblua/AgentsCommander/issues/2075), [#2088](https://github.com/mblua/AgentsCommander/issues/2088), [#2095](https://github.com/mblua/AgentsCommander/issues/2095), [#2102](https://github.com/mblua/AgentsCommander/issues/2102), [#2107](https://github.com/mblua/AgentsCommander/issues/2107), [#2109](https://github.com/mblua/AgentsCommander/issues/2109))
- A regression test now guards Delete Profile against deleting a held slot. ([#2061](https://github.com/mblua/AgentsCommander/issues/2061))
## 0.34.0

### Added

- **`settings.json` saves keep a bounded backup history.** Every successful save archives the bytes it replaced into five rotating slots, `settings.backup.1.json` through `settings.backup.5.json`, beside `settings.json` in the same configuration directory. Slot 1 holds the version the most recent save replaced; slot 5 is the oldest kept. ([#2058](https://github.com/mblua/AgentsCommander/issues/2058))
- **A global npm install on Windows creates a per-user Start Menu shortcut.** `AgentsCommander.lnk` targets the installed executable and uses its icon. It is skipped for local installs and when `AGENTSCOMMANDER_NO_SHORTCUT=1` is set, never fails the install, and must be removed by hand because npm runs no uninstall scripts. ([#2053](https://github.com/mblua/AgentsCommander/issues/2053))
- **A global npm install on macOS makes the app reachable from Launchpad and Spotlight** through a `~/Applications/AgentsCommander.app` alias to the extracted bundle. It needs no elevation, is skipped for local installs and with `AGENTSCOMMANDER_NO_SHORTCUT=1`, leaves an unrelated application of the same name untouched, and never fails the install. ([#2065](https://github.com/mblua/AgentsCommander/issues/2065))
- **The project global context template seeds an Answering section.** The previous template is recognized and replaced with a backup on scan, so generated standalone templates stay retirable. ([#2031](https://github.com/mblua/AgentsCommander/issues/2031))

### Changed

- **Left click on an offline Agent Matrix row no longer opens the session launch flow.** It shows a notice explaining that replicas, not the matrix, are what gets launched, and that the matrix folder is available from the row's right-click menu. ([#2046](https://github.com/mblua/AgentsCommander/issues/2046))
- **The sidebar lock chip shows only the lock icon**, without the KEEP label. ([#2030](https://github.com/mblua/AgentsCommander/issues/2030))
- **The Coding Agent profile modal scrolls its Profile and Same Profile columns independently.** ([#2038](https://github.com/mblua/AgentsCommander/issues/2038))
- **Documentation updates**: the configuration-directory guides, the macOS and Linux support tiers, the deprecation of the legacy `codingAgentProfiles` settings, the OCA-008 tombstones for enabled Grok and disabled Muse, and Team wording in the use-case recipes. ([#1936](https://github.com/mblua/AgentsCommander/issues/1936), [#2017](https://github.com/mblua/AgentsCommander/issues/2017), [#2019](https://github.com/mblua/AgentsCommander/issues/2019), [#2009](https://github.com/mblua/AgentsCommander/issues/2009), [#2032](https://github.com/mblua/AgentsCommander/issues/2032))

### Fixed

- **A distinct submission is no longer silently deduplicated** when a critical-admission key outlives the completion the caller observes. ([#1580](https://github.com/mblua/AgentsCommander/issues/1580))
- **Assign-and-lock on a replica no longer fails with `stalePreview`.** ([#2051](https://github.com/mblua/AgentsCommander/issues/2051))
- **Delete Profile no longer removes configured profiles from other coding agents.** ([#2057](https://github.com/mblua/AgentsCommander/issues/2057))
- **The sidebar Ungrouped counter agrees with the Ungrouped panel list.** ([#2036](https://github.com/mblua/AgentsCommander/issues/2036))
- **npm publication no longer fails right after a successful publish** when the registry's `latest` dist-tag lags. ([#2042](https://github.com/mblua/AgentsCommander/issues/2042))
- **Reseed tests are deterministic under concurrent same-binary runs.** ([#2001](https://github.com/mblua/AgentsCommander/issues/2001))
- **CI uploads artifacts with a Node 24 release of `actions/upload-artifact`.** ([#2040](https://github.com/mblua/AgentsCommander/issues/2040))
## 0.33.0

### Added

- **Coding-agent selections for replicas can be locked.** Assignments offer ordinary and assign-and-lock scopes with reviewed conflict decisions, independent lock removal, KEEP badges and an explicit Save default for future replicas. Replica creation preserves existing selections and applies Matrix defaults only when a replica config is first created. Selection ownership is retained through restart completion, and re-applying the same locked pair is an unchanged write. ([#1939](https://github.com/mblua/AgentsCommander/issues/1939), [#1940](https://github.com/mblua/AgentsCommander/issues/1940), [#1941](https://github.com/mblua/AgentsCommander/issues/1941), [#1942](https://github.com/mblua/AgentsCommander/issues/1942), [#1943](https://github.com/mblua/AgentsCommander/issues/1943), [#2010](https://github.com/mblua/AgentsCommander/issues/2010))
- **The managed coding-agent catalog is persisted with local overrides.** User edits in `agents.local.json` win and survive restart; the catalog is seeded on first run even when no project is registered, and legacy, edited, foreign or corrupt files are never rewritten. ([#1968](https://github.com/mblua/AgentsCommander/issues/1968), [#1969](https://github.com/mblua/AgentsCommander/issues/1969), [#2021](https://github.com/mblua/AgentsCommander/issues/2021))
- **Grok Build (`grok`) is available in the coding-agent catalog.** ([#1999](https://github.com/mblua/AgentsCommander/issues/1999))

### Changed

- **The Coding Agent assignment modal has a new layout**: three columns, a selection lock bar, Matrix default and Apply to controls, and profile comparison rows that show the launch line. ([#2014](https://github.com/mblua/AgentsCommander/issues/2014))
- **Suffixed executables refuse to start when their adjacent `.agentscommander_<suffix>` configuration directory is not writable**, before any durable write and without a HOME fallback. Unsuffixed executables keep `HOME/.agentscommander`, and explicit overrides are retained. ([#1935](https://github.com/mblua/AgentsCommander/issues/1935))
- **Muse is disabled in the coding-agent catalog.** ([#1999](https://github.com/mblua/AgentsCommander/issues/1999))
- **The npm package README recommends global installation.** ([#1955](https://github.com/mblua/AgentsCommander/issues/1955))

### Removed

- **The legacy pre-v2 `codingAgentProfiles` settings migration was removed.** It ran on every load and dropped profile labels. ([#2018](https://github.com/mblua/AgentsCommander/issues/2018))

### Fixed

- **The npm launcher starts the app on macOS** by resolving the executable inside the installed `.app` bundle. ([#2016](https://github.com/mblua/AgentsCommander/issues/2016))
- **Profile labels survive restart.** ([#2018](https://github.com/mblua/AgentsCommander/issues/2018))
- **Current Codex final answers are forwarded to Telegram.** ([#1997](https://github.com/mblua/AgentsCommander/issues/1997))
- **Injected messages are submitted in Hermes, OpenCode and Grok.** ([#1999](https://github.com/mblua/AgentsCommander/issues/1999))
- **npm release verification installs the Linux runtime before running the CLI.** ([#1988](https://github.com/mblua/AgentsCommander/issues/1988))
- Several intermittent test fixtures are now deterministic. ([#1466](https://github.com/mblua/AgentsCommander/issues/1466), [#1984](https://github.com/mblua/AgentsCommander/issues/1984), [#1996](https://github.com/mblua/AgentsCommander/issues/1996), [#1998](https://github.com/mblua/AgentsCommander/issues/1998))
## 0.32.0

### Added

- **Linux/X11 screenshot capture is available**, with documented capture commands and native build dependencies. ([#1915](https://github.com/mblua/AgentsCommander/issues/1915), [#1916](https://github.com/mblua/AgentsCommander/issues/1916))
- **Muse sessions can resume the latest workspace session automatically** on supported local macOS/Linux launches. Explicit fresh starts and configured arguments retain their own behavior. ([#1873](https://github.com/mblua/AgentsCommander/issues/1873))
- **Blocking-menu patterns now have shipped, local and remote files.** Existing settings arrays are exported once into the local overlay without overwriting operator entries. An enabled-by-default Settings checkbox controls the background startup download, throttled to once per 24 hours. Downloaded files are validated before replacing the cache; failures retain the prior copy, and new patterns apply on the next start. Local overrides take precedence. ([#1905](https://github.com/mblua/AgentsCommander/issues/1905), [#1925](https://github.com/mblua/AgentsCommander/issues/1925))
- **Coding-agent catalog availability and diagnostics are visible in Settings and Quick Configuration**, with a Reload catalog action and guards against stale selections when the primary project changes. Catalog consumers and the startup updater use persisted catalog data without a bundled fallback. ([#1963](https://github.com/mblua/AgentsCommander/issues/1963), [#1964](https://github.com/mblua/AgentsCommander/issues/1964), [#1965](https://github.com/mblua/AgentsCommander/issues/1965), [#1966](https://github.com/mblua/AgentsCommander/issues/1966), [#1967](https://github.com/mblua/AgentsCommander/issues/1967))

### Changed

- **Normal executables now store configuration in the user's home directory, under `.agentscommander`.** An explicit `AGENTSCOMMANDER_CONFIG_DIR` still wins. Unsuffixed executables no longer select an adjacent portable directory, and no old configuration is discovered, copied or migrated automatically. Preserve the active configuration before updating and select its location explicitly when needed. Suffixed instances retain their existing location rules. ([#1868](https://github.com/mblua/AgentsCommander/issues/1868))
- **The Root Agent banner uses its right-click menu for actions**, removing the five hover buttons while retaining the Telegram indicator. ([#1896](https://github.com/mblua/AgentsCommander/issues/1896))
- **Documentation now covers native Linux package installation, platform-specific shell guidance, the Orchestrator alias, and outbound network calls.** The obsolete codebase-memory cache-directory recommendation was removed. ([#1840](https://github.com/mblua/AgentsCommander/issues/1840), [#1951](https://github.com/mblua/AgentsCommander/issues/1951), [#1900](https://github.com/mblua/AgentsCommander/issues/1900), [#1924](https://github.com/mblua/AgentsCommander/issues/1924), [#1897](https://github.com/mblua/AgentsCommander/issues/1897))
- **The Cargo package is named `agentscommander`.** Native regression CI separates focused release builds from test execution, aligns cache targets, and checks the served-path inventory. ([#1934](https://github.com/mblua/AgentsCommander/issues/1934), [#1929](https://github.com/mblua/AgentsCommander/issues/1929), [#1974](https://github.com/mblua/AgentsCommander/issues/1974), [#1946](https://github.com/mblua/AgentsCommander/issues/1946))

### Fixed

- **Concurrent local configuration writes are serialized across processes** to protect persisted settings. ([#1938](https://github.com/mblua/AgentsCommander/issues/1938))
- **Integration tests isolate unsuffixed binaries from the user's configuration.** The built-in coding-agent support table consistently controls catalog filtering and seeding. ([#1867](https://github.com/mblua/AgentsCommander/issues/1867), [#1912](https://github.com/mblua/AgentsCommander/issues/1912))

### Security

- Updated the container base images to Node `22.23.2-trixie-slim` and Debian `13.6-slim`. ([#1931](https://github.com/mblua/AgentsCommander/pull/1931), [#1933](https://github.com/mblua/AgentsCommander/pull/1933))
## 0.31.0

### Added

- **Agents resume automatically after an app restart.** Settings > General gains an **On app restart** section with two checkboxes and two editable restart prompts. When enabled, AgentsCommander restores the saved sessions, wakes the replicas that were working when the app closed, waits until each one is ready for input, and types the configured prompt into it once, so a team picks up where it left off without anyone retyping into every terminal. The existing `Orchestrator wake state` control moves into that section. ([#1793](https://github.com/mblua/AgentsCommander/issues/1793), [#1801](https://github.com/mblua/AgentsCommander/issues/1801), [#1802](https://github.com/mblua/AgentsCommander/issues/1802), [#1803](https://github.com/mblua/AgentsCommander/issues/1803), [#1804](https://github.com/mblua/AgentsCommander/issues/1804))
- **The bar that marks the selected sidebar row now has user-settable width and colour.** Two new fields in Settings set the rail width (up to 14px) and its colour; both are persisted, published to the DOM and validated in the modal. The factory colour is now `#630707`. ([#1796](https://github.com/mblua/AgentsCommander/issues/1796), [#1828](https://github.com/mblua/AgentsCommander/issues/1828), [#1829](https://github.com/mblua/AgentsCommander/issues/1829), [#1830](https://github.com/mblua/AgentsCommander/issues/1830), [#1831](https://github.com/mblua/AgentsCommander/issues/1831), [#1844](https://github.com/mblua/AgentsCommander/issues/1844))
- **Muse Code joins the embedded coding-agent catalog** as a beta preset: command `muse`, macOS and Linux hosts only, no config seed and no update commands. ([#1860](https://github.com/mblua/AgentsCommander/issues/1860))
- **`agentscommander --version` (and `-V`) prints the CLI version.** The npm clean-install check in the release pipeline relies on it, and that pipeline's post-publish verification now tolerates registry propagation delays and compares the provenance registry URL canonically. ([#1826](https://github.com/mblua/AgentsCommander/issues/1826))
- **The Golden Rule block seeded to every room replica now lists six shared filesystem locations:** the project-level `plans`, `tools`, `errors` and `project-shared` directories under the project `.ac` root (read and write), plus the room's `TASK.md` (read-only) and `room-shared/` (read and write). AgentsCommander creates the directories; an already-materialized context picks them up with no migration. ([#1795](https://github.com/mblua/AgentsCommander/issues/1795))
- **New documentation page for running Claude Code with the third-party `codebase-memory-mcp` server**, directly or through an AgentsCommander profile cell. The earlier "why not MCP" stance is removed from the README and the FAQ. ([#1882](https://github.com/mblua/AgentsCommander/issues/1882))

### Changed

- **The Root Agent's right-click menu reaches parity with the replica rows.** Every session-row context menu is now rendered by one catalogue component, so the same items, separators and dismissal behaviour apply everywhere. ([#1871](https://github.com/mblua/AgentsCommander/issues/1871))

### Fixed

- **Blocked-menu notices no longer get lost.** Each session's communication state is reconciled from the 5-second listing poll, so a raised hand and a blocked menu no longer overwrite each other and a dropped event, a reconnect or a window reload heals within seconds ([#1856](https://github.com/mblua/AgentsCommander/issues/1856)); blocked menus surface as one pinned, aggregated toast carrying the latest notice text and requesting taskbar attention ([#1857](https://github.com/mblua/AgentsCommander/issues/1857)); the blocked-menu chip has its own glyph and colour, distinct from a raised hand, and the toast action button is styled ([#1858](https://github.com/mblua/AgentsCommander/issues/1858)); and collapsed projects, teams and orchestrator groups, as well as the sidebar filter, roll the blocked state up so it stays visible ([#1859](https://github.com/mblua/AgentsCommander/issues/1859)).
- **`test-reset --confirm-testeable` now names the process holding the single-instance mutex** (PID, image name, path and handle) instead of only reporting `testable_gui_active`, and the panic paths that leaked a testable process are closed. Windows only. ([#1773](https://github.com/mblua/AgentsCommander/issues/1773))
- **`npm test` can no longer exit non-zero while every test passes.** A debounced settings-preview call fired against an incomplete mock after the test finished. ([#1797](https://github.com/mblua/AgentsCommander/issues/1797))
## 0.30.5

### Changed

- **Releases publish to npm again: the release pipeline was repaired and hardened end to end during its first full execution.** The guard no longer probes a repository setting its workflow token can never read ([#1811](https://github.com/mblua/AgentsCommander/issues/1811)), every build step now runs on the macOS runners' bash 3.2 ([#1813](https://github.com/mblua/AgentsCommander/issues/1813)), the windows runner's own `NPM_CONFIG_PREFIX` no longer aborts the npm-registry guard and bundle assets are matched by their GitHub-sanitized upload names with explicit fail-closed errors ([#1815](https://github.com/mblua/AgentsCommander/issues/1815)), the single bundler updater archive satisfies every ledger `.app.tar.gz` alias and the draft coordinator absorbs the releases-list read-after-write race ([#1817](https://github.com/mblua/AgentsCommander/issues/1817)), and a fresh run purges stale draft assets before its byte-exact uploads ([#1819](https://github.com/mblua/AgentsCommander/issues/1819)). Version 0.30.4 exists only as a GitHub Release: its npm publication was blocked by a toolchain-contract mismatch in release verification, so this version supersedes it on npm and the registry goes 0.30.3 → 0.30.5.

### Fixed

- **A session no longer keeps the amber pending-review dot forever while the backend considers it active.** The sidebar's waiting mirror is reconciled from the backend session list on every poll, so a latched `pendingReview` state the backend no longer reports now clears instead of surviving polling and application restarts. ([#1779](https://github.com/mblua/AgentsCommander/issues/1779))

## 0.30.4

### Added

- **New `self-handoff-and-restart` CLI subcommand.** An agent can hand off through `SELF-HANDOFF.md` and come back on a genuinely new process running the **same configured coding agent** with the **same profile letter**, instead of an in-process `/clear` or a borrowed `self-handoff-and-switch`. Available to Room replicas, origin Agent Matrix agents, and the Root Agent; it does not change the replica's Selection-UI coding-agent or profile assignment, and adds no message-format field. ([#1632](https://github.com/mblua/AgentsCommander/issues/1632))

### Changed

- **Workgroups are now Rooms.** AgentsCommander creates `room-<N>-<team>` directories instead of `wg-<N>-<team>`, and every surface a person or an agent reads calls the concept a Room. **Nothing on disk is renamed, moved, converted or deleted:** every existing `wg-*` directory is still discovered, listed, addressed, operated and deleted exactly as before, and its inter-agent message filenames keep their `wg<N>` short token. Room and legacy Workgroup slot numbers are independent, so a project that already holds `wg-1-<team>` gets `room-1-<team>` next. The CLI gains the canonical names `room`, `purge-room` and `--room`; `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases that parse to the identical value and produce identical side effects, exit codes and output, and a later release will remove them. Persisted config keys, event names, IPC command names, refresh reason codes, the outbox `action` value and the `%WORKGROUP%` injected-message token are unchanged, so nothing in flight breaks. ([#1614](https://github.com/mblua/AgentsCommander/issues/1614))
- **Startup coding-agent updates are cancellable, and the update overlay is all English.** Every unfinished row of the startup update timeline carries its own `Cancel` control, and a `Cancel all` control stops the whole pass; a row that is still `Verifying...` counts as unfinished and keeps both. Cancelling stops the step that is running, terminates its process tree, waits for those processes to be gone and prevents the steps after it; it leaves rows that already finished exactly as they are, and it does not reverse an updater command that had already completed. The prompt, the timeline and the cancel controls now share the card, so you can cancel an update while a first-time question is still open: Enter on a focused cancel control cancels without answering, while every other Enter and every Escape still answers `No`. Finished rows now state what actually happened instead of only success or failure: `Ready - <old> -> <new>`, `<version> (Nothing to update)`, `Update completed - Version could not be verified`, `Failed - <reason>` or `Cancelled`, and a cancelled row counts as completed rather than failed and raises no failure notification. The Settings > Coding Agents **Status** column is unchanged: it still reports `Updating...`, `Updated`, `Update failed`, or `-` when this AC start recorded no result for that command. ([#1672](https://github.com/mblua/AgentsCommander/issues/1672))
## 0.30.3

### Changed

- **Releases now publish to npm via OIDC Trusted Publishing.** `publish-npm` pins npm to 11.6.2, because OIDC trusted publishing requires npm >= 11.5.1 and the Node 22 runner ships 10.9.8. The `NPM_AGENTSCOMMANDER` token is retained as a fallback for this release only; npm prefers OIDC and falls back to a token, so the migration carries no risk to the release itself. No application change: this version is functionally identical to 0.30.2. ([#1563](https://github.com/mblua/AgentsCommander/issues/1563))

## 0.30.2

### Added

- **Web server: the bind address is now editable from the titlebar popover.** The PORT row grew into a BIND section (ADDR + PORT). The address chooser offers `Localhost only (127.0.0.1)`, `All interfaces (0.0.0.0)` (which discloses that any device on your network can reach the server), every detected IPv4 with its adapter name (virtual and tunnel adapters grouped and collapsed), and a validated manual entry. A stored address that is no longer on the machine stays visible as a disabled `Unavailable` row. Applying an address restarts a running server, starts a stopped-but-enabled one, and only saves otherwise. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))

### Changed

- **The context-alert message injected into a coordinator's terminal is now operator-editable**, and its visible prefix changes from `[AgentsCommander context alert]` to `[AC context alert]`. The wording lives in `injected-messages.toml`, next to the executable in the config directory, alongside a read-only `injected-messages.default.toml` reference. Markdown is preserved byte for byte, the placeholders are `%MEMBER%`, `%WORKGROUP%`, `%THRESHOLDS%` and the optional `%OBSERVED%`, and an entry you have edited is never overwritten by an upgrade. `injected-messages reseed --id <id>` (or `--all`) restores a shipped default, taking a timestamped backup first. ([#1157](https://github.com/mblua/AgentsCommander/issues/1157))

### Fixed

- **`close-session` no longer reports a false timeout on graceful closes.** The CLI's delivery-confirmation wait was hardcoded to 30s while a graceful close takes ~30s per session, so 97.9% of graceful closes exited 1 as "delivery confirmation timeout" despite succeeding moments later. The CLI now runs a single wait for the daemon's response (still fast-failing on rejection), budgets `--timeout` + 60 seconds (default 90s, matching `send --confirm-timeout`), and when the wait expires it exits 2 ("outcome unknown": the close keeps running server-side) instead of a fabricated exit 1. Timeout messages now print both UUIDs with correct labels (`request` vs `message`). ([#1440](https://github.com/mblua/AgentsCommander/issues/1440))
- **Web server bind failures are no longer invisible.** The status payload now carries the failed address, port and verbatim OS error; the popover explains the failure in plain language (`Stopped · bind failed`, amber dot) instead of a bare `Stopped`. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))
- **The titlebar web server toggle now follows runtime state.** A failed bind no longer shows a contradictory `Stop Server`, and starting the server only persists the "enable web server" setting once the server has actually started, so a failed attempt no longer turns it on for every future launch. The `Enable web server` checkbox in Settings remains the way to change that setting directly. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))
- **Settings no longer reports a web server that failed to start as `Running`.** The Start button in Settings now reflects the actual result of the start attempt. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))

## 0.30.1

### Changed

- Advanced all desktop, Cargo, root/npm wrapper, lockfile, Tauri, and installer Release references to 0.30.1.
- Routed v0.30.1 through the repository's enabled immutable-Release path.
- Prepared `@mblua/agentscommander@0.30.1` for a separate protected OIDC publication after immutable Release verification.

## 0.30.0

### Added

- Activated v1 seed-manifest emission for project, team, and workgroup flows ([#1109](https://github.com/mblua/AgentsCommander/pull/1109)).
- Added per-agent activity intervals and application lifecycle records in `activity.jsonl` ([#1152](https://github.com/mblua/AgentsCommander/pull/1152)).
- Seeded a per-instance `.gitignore` for generated instance files ([#1165](https://github.com/mblua/AgentsCommander/pull/1165)).
- Added configurable regex watchers over PTY output with an activity window ([#1174](https://github.com/mblua/AgentsCommander/pull/1174)).
- Added rotation of an origin Agent Matrix `memory/` directory when a fresh session spawns ([#1181](https://github.com/mblua/AgentsCommander/pull/1181)).
- Added a user-editable registry for PTY-injected message templates ([#1203](https://github.com/mblua/AgentsCommander/pull/1203)).
- Added a cross-platform checker for `SKILL.md` structure ([#1222](https://github.com/mblua/AgentsCommander/pull/1222)).
- Added authorized terminal snapshots in JSON and PNG through the API and CLI ([#1238](https://github.com/mblua/AgentsCommander/pull/1238)).
- Made the Non-stop pseudo-group favoritable from the groups rail ([#1260](https://github.com/mblua/AgentsCommander/pull/1260)).
- Displayed the active screenshot shortcut in the sidebar ([#1275](https://github.com/mblua/AgentsCommander/pull/1275)).
- Added a setting to disable `activity.jsonl` generation, with generation off by default ([#1300](https://github.com/mblua/AgentsCommander/pull/1300)).
- Added a warning when Default Shell is not configured as a complete executable path ([#1314](https://github.com/mblua/AgentsCommander/pull/1314)).
- Added an authenticated API endpoint for native-window screenshots ([#1316](https://github.com/mblua/AgentsCommander/pull/1316)).
- Added ordered per-agent update commands and the startup auto-update flow ([#1324](https://github.com/mblua/AgentsCommander/pull/1324), [#1326](https://github.com/mblua/AgentsCommander/pull/1326), [#1328](https://github.com/mblua/AgentsCommander/pull/1328)).
- Added `window-list` and `window-screenshot` CLI verbs for native window capture ([#1333](https://github.com/mblua/AgentsCommander/pull/1333)).
- Added a per-agent auto-update dropdown to Coding Agent profiles ([#1345](https://github.com/mblua/AgentsCommander/pull/1345)).
- Added a dedicated collapsible Coordinator Quick-Access section ([#1354](https://github.com/mblua/AgentsCommander/pull/1354)).
- Added shell-specific RTK hooks, including PowerShell tool coverage ([#1426](https://github.com/mblua/AgentsCommander/pull/1426)).
- Added a statusline branch to Claude context suggestions ([#1435](https://github.com/mblua/AgentsCommander/pull/1435)).
- Added native-tool usage records to the RTK savings database ([#1465](https://github.com/mblua/AgentsCommander/pull/1465)).
- Added an artifact registry as the source for per-instance `.gitignore` generation ([#1470](https://github.com/mblua/AgentsCommander/pull/1470)).
- Exposed the web server bind address and startup failures in the UI ([#1475](https://github.com/mblua/AgentsCommander/pull/1475)).

### Changed

- Unified coding-agent badge styling across sidebar rows ([#1168](https://github.com/mblua/AgentsCommander/pull/1168)).
- Removed the unused phone feature and unreachable `sync_workgroup_repos` command ([#1201](https://github.com/mblua/AgentsCommander/pull/1201)).
- Renamed watcher toolbar controls to match the product vocabulary ([#1207](https://github.com/mblua/AgentsCommander/pull/1207)).
- Moved the activity log checkbox below the Log level hint in settings ([#1308](https://github.com/mblua/AgentsCommander/pull/1308)).
- Bounded terminal output admission to prevent renderer saturation ([#1312](https://github.com/mblua/AgentsCommander/pull/1312)).
- Moved the coding-agent catalog into project `.ac` data and recorded per-agent auto-update metadata ([#1322](https://github.com/mblua/AgentsCommander/pull/1322)).
- Added the `{{AGENT_REPOS}}` and `# Agent Repos` context vocabulary while retaining the frozen alias ([#1430](https://github.com/mblua/AgentsCommander/pull/1430)).

### Fixed

- Made Coding Agents row clicks honor 1-rail mode ([#1099](https://github.com/mblua/AgentsCommander/pull/1099)).
- Prevented phantom resource-monitor cap exhaustion on non-Windows platforms ([#1145](https://github.com/mblua/AgentsCommander/pull/1145)).
- Gated the file-in-use classifier by platform ([#1150](https://github.com/mblua/AgentsCommander/pull/1150)).
- Recovered orphaned quarantined Windows resource groups ([#1159](https://github.com/mblua/AgentsCommander/pull/1159)).
- Kept the watcher activity polling chain running after refreshes ([#1212](https://github.com/mblua/AgentsCommander/pull/1212)).
- Bounded the watcher mount chain and armed polling unconditionally ([#1226](https://github.com/mblua/AgentsCommander/pull/1226)).
- Accepted the coordinator entry in `team_members` instead of rejecting the team configuration ([#1247](https://github.com/mblua/AgentsCommander/pull/1247)).
- Excluded Alert me sessions from the Ungrouped sidebar group ([#1278](https://github.com/mblua/AgentsCommander/pull/1278)).
- Matched the Codex profile to the context-first Coding Agent row layout ([#1288](https://github.com/mblua/AgentsCommander/pull/1288)).
- Stopped the orphan-session warning loop in session persistence ([#1296](https://github.com/mblua/AgentsCommander/pull/1296)).
- Moved Git status polling off the async runtime and deduplicated requests ([#1303](https://github.com/mblua/AgentsCommander/pull/1303)).
- Launched Windows agent commands through the configured Default Shell ([#1311](https://github.com/mblua/AgentsCommander/pull/1311)).
- Limited automatic updates to registered coding agents ([#1336](https://github.com/mblua/AgentsCommander/pull/1336)).
- Moved startup restore work off the main thread so the auto-update prompt can render ([#1344](https://github.com/mblua/AgentsCommander/pull/1344)).
- Replayed bounded PTY output history when terminal views are rebuilt ([#1376](https://github.com/mblua/AgentsCommander/pull/1376)).
- Required rendered content before a cold-spawn wake is considered settled ([#1390](https://github.com/mblua/AgentsCommander/pull/1390)).
- Registered the screenshot global hotkey at the start of setup to avoid the startup race ([#1402](https://github.com/mblua/AgentsCommander/pull/1402)).
- Retained diagnostics when saving settings fails ([#1400](https://github.com/mblua/AgentsCommander/pull/1400)).
- Restored PTY broadcast push delivery ([#1432](https://github.com/mblua/AgentsCommander/pull/1432)).
- Made `close-session` wait once for the daemon response ([#1447](https://github.com/mblua/AgentsCommander/pull/1447)).
- Reconciled the screen-parser grid at attach and healed the embedded viewport ([#1450](https://github.com/mblua/AgentsCommander/pull/1450)).
- Made kill verification corpse-aware so terminated processes reach the Terminated state ([#1449](https://github.com/mblua/AgentsCommander/pull/1449)).
- Guaranteed that the PTY attach seed starts on a line boundary ([#1464](https://github.com/mblua/AgentsCommander/pull/1464)).
- Ignored RTK runtime artifacts under Agent Matrix directories ([#1473](https://github.com/mblua/AgentsCommander/pull/1473)).
- Sequenced local TASK writes against session snapshots by owning workgroup ([#1477](https://github.com/mblua/AgentsCommander/pull/1477)).
- Used loopback URLs when opening a browser for wildcard web-server binds ([#1485](https://github.com/mblua/AgentsCommander/pull/1485)).
- Logged a warning when seed rendering panics during terminal-output activation ([#1460](https://github.com/mblua/AgentsCommander/pull/1460)).
- Stopped replica config seeding from persisting `replica_config_file` rows in `seed-manifest.toml`, while retaining support for reading and later pruning legacy rows ([#1487](https://github.com/mblua/AgentsCommander/pull/1487)).

### Security

- Warned in settings that API keys and bot tokens are stored in plaintext ([#1353](https://github.com/mblua/AgentsCommander/pull/1353)).

## 0.20.0 – 2026-07-23

Large release covering everything merged since `0.10.0` (616 commits across ~110 PRs). Headlines: **containerized coding agents** (Docker / "Camino 2" backend), an **in-daemon Control Plane API**, a **coding-agent catalog overhaul** (Hermes / Cursor CLI / Pi), **live context-usage visibility** (CTX badges + alerts), **workgroup UI Groups** (Telegram-style sidebar rail), a **single self-contained web-server executable**, plus a deep PTY/terminal reliability and settings-persistence hardening pass.

### Added

- **Containerized coding agents (Docker / "Camino 2" backend)**: run coding agents inside Docker containers. New PTY session-backend refactor, async spawn routing, container transport backend + session-transport endpoint, a `session-bridge` crate + Docker runtime, and a DB-backed `MessageStore` + dispatcher. Adds an `ac-claude-ready` prebuilt image, a UI+CLI container runtime selector per agent, read-write mounting of enabled repos into container sessions, and container auth via copied host credentials. ([#819](https://github.com/mblua/AgentsCommander/issues/819): #823, #826, #829, #832, #834; #865, #868, #930, #935)
- **In-daemon Control Plane API server**: an HTTP control-plane API hosted in the daemon (starting with `send` + `list-peers-lean`), an enable/disable toggle, a bind/port editor, an in-app API-client mint command with Settings UI, and backend hardening. ([#791](https://github.com/mblua/AgentsCommander/issues/791), #838, #846, #853, #872)
- **Coding-agent catalog overhaul**: removed the Gemini CLI preset; added **Hermes**, **Cursor CLI**, and **Pi**; externalized the catalog to a seeded, editable backend JSON (2 phases); and added scriptable coding-agent config management via CLI. ([#766](https://github.com/mblua/AgentsCommander/issues/766), #769, #786)
- **Pi Coding Agent support**: auto-resume, use as a self-handoff-and-switch source, logical-clear → `/new` mapping, and a suggested context-badge pattern. (#1069, #1081, #1059, #1054)
- **Live context-usage visibility (CTX)**: a per-session context-usage scrape off the vt100 mirror, a sidebar **CTX badge** with a configurable Settings pattern, team context-usage alerts, and per-peer CTX percent exposed in `list-peers` / `list-peers-lean`. (#1032, #1033, #1056, #1088)
- **Workgroup UI Groups**: a Telegram-style sidebar rail that filters projects into groups, with reorder, auto-focus, edit-in-context-menu, live web↔desktop sync, and a raise-hand indicator on the group tab. ([#737](https://github.com/mblua/AgentsCommander/issues/737), #808, #810, #851, #822, #763)
- **Project seed manifest & config seeding**: a staged project seed-manifest system (core, plumbing, lifecycle-removal outcomes, conformance/scale harness), context-publication outcomes, `%USER_HOME%` expansion in seeded content, and plaintext env values (masking only `PASSWORD*` keys). (#1038, #1060, #1062, #1063, #1064, #1061, #924, #1052)
- **Single self-contained web-server executable**: the frontend `dist` is embedded into the binary. (#796)
- **Sidebar & workflow tools**: sidebar rail favorites + collapsible categories, archive/unarchive projects, a coordinator repo "Browse Main / Browse Branch" submenu, a delete-agent context-menu action, a reusable CodingAgentQuickConfiguration modal, a titlebar zoom (%) stepper, a browser web-server titlebar menu, and a Sidebar left/right option. (#965, #881, #943, #843, #975, #863, #835, #840)
- **Active-agent screenshot capture.** (#714)
- **"Non-stop" / "Alert me!" watchdog group** with Telegram / sound alerts on a working-vs-total disparity. (#777, #799)
- **CLI**: `purge-wg` one-shot workgroup purge; coordinator-scoped exact PTY text injection (including Root→coordinator); `send --confirm-timeout` with the default raised to 90s. (#885, #1057, #782)
- **Persisted raise-hand indicator** across app restarts, plus raise-hand group rules. (#747, #775)

### Changed

- **Agent template / context-lifecycle minimization** ([#1005](https://github.com/mblua/AgentsCommander/issues/1005) S1–S6): trimmed the messaging, GOLDEN RULE / write-restriction, skills-intro, and root/coordinator templates; added coordinator self-clear, durable fresh-conversation intent, and a settled live-wake path before injection.
- **Agent boundary hardening**: restrict agent reads to allowed zones (GOLDEN RULE), move the cross-workgroup boundary into the coordinator context, stop the Root Agent from consuming the global context template, and protect user-set task titles. (#923, #1030, #979, #738)
- **Coding-agent UX**: rail selection by row click, a 1/2-rail toggle for the Coding Agents screen, profile cards expanded by default, last Coding Agent + Profile shown on powered-off tiles, and onboarding that fits all options without scrolling. (#895, #1095, #790, #733, #768)
- **Removed the RTK (Rust Token Killer) integration.** (#928)
- **Docs**: document the factory-default seed tier, expose code-signing and privacy links on releases, and correct the Windows signing status. (#876, #754, #719)
- **Repo hygiene**: stop tracking `_logbooks/`, gitignore/untrack `plans/` and remove `_prototypes/`, purge explanatory comments from 56 TS/TSX files, and add a workgroup build-artifact reclaim script. (#990, #1048, #1046, #932)

### Fixed

- **Terminal reliability (the black/blank terminal)**: never gate live PTY output behind the snapshot round-trip; spawn the PTY at real size and never resize an unrendered child; PTY spawn diagnostics; cancel-safe local PTY spawn; and PTY spawn offloaded off the async runtime. (#955, #973, #942, #847, #839)
- **Settings persistence**: disk-authoritative project paths (stop the GUI clobbering CLI writes), a unique per-writer temp filename, keeping project lists disk-authoritative to stop silent session deletion, and atomic + retried git-guard writes. (#778, #774, #888, #836)
- **Container transport**: surface real startup failures, require an explicit image, redact secrets from diagnostics, translate host↔container env paths, guard the bind mount, strip the Windows verbatim prefix from mount sources, stop the stray Docker console window on Windows startup, Camino 2 hardening, and WSS TLS. (#892, #894, #993, #992, #831, #1017)
- **Sidebar / UI**: fixed first-click loss from sidebar DOM recreation, keep sidebar modals open across refresh, reset scroll to the selected project, refresh the agent list on partial-delete failure, align context-menu glyphs, close the replica context menu on pointer leave, highlight only the active project's group, align project-header search controls, keep the group rail drag alive after pointer-capture loss, and correct APPLY-TO scope counters. (#748, #710, #941, #856, #987, #977, #860, #816, #815, #800)
- **Titlebar zoom**: handle fire-and-forget zoom-apply rejections and harden against a leaked `initZoom` document listener. (#1083, #1093)
- **API server**: fix the `0.0.0.0`-bind status false-negative + readiness await, and use distinct newtypes for Web/Api handles (startup panic). (#878, #794)
- **Coordinator / CLI robustness**: prevent auto-close terminal hijack, preserve Restart-Session fresh intent against non-substantive PTY writes, ignore the team-config coordination lock, compact Git scope warnings, persist absolute + instance-relative project paths, hide the TASK section for the Root Agent, read the on-disk `--get-output` response before checking timeout, normalize Windows verbatim cwd, add a Telegram auto-close exemption, and skip the create-gate for the restart replacement create. (#1027, #871, #1070, #1072, #1077, #771, #729, #730, #817, #1101)
- **Web/desktop parity**: route web coding-agent profile commands and sync project group changes live between web and desktop. (#859, #822)
- **Repaired the agency-agents-roles skill** the indexer was silently dropping. (#909)
- **Hardened the send/messaging slice** and extracted a shared WG-replica walk-up helper. (#724, #726)
- **CI**: always report the lockfile-drift check and make test-debt comment-masking string-aware. (#1022, #801)

### Security

- **Restrict agent reads to allowed zones (GOLDEN RULE enforcement).** (#923)
- **Redact secrets from container transport diagnostics.** (#904)
- **Decouple domain logic from UI presentation values** to reduce accidental exposure surface. (#882)

## 0.9.0 – 2026-06-15

Feature release: **Project Loops** (scheduled, recurring agent runs), a public marketing-copy overhaul around the "compound your coding agents" message, OpenCode documented as usable (provider-agnostic), and a large test/CI regression-hardening pass.

### Added

- **Project Loops**: scheduled, recurring agent runs. Define a cron-style schedule that re-triggers a coordinator, workgroup, or agent; ships the scheduler backend, the sidebar UI, and scheduler safety guards. ([#354](https://github.com/mblua/AgentsCommander/issues/354))
- **Agency template install in the New Agent modal**: browse and install [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents) role templates directly from the create-agent flow, wired over the embedded WebSocket. ([#465](https://github.com/mblua/AgentsCommander/issues/465))
- **Semantic GUI automation bridge**: a CLI and UI context-click automation surface for deterministic, scriptable GUI interactions. ([#499](https://github.com/mblua/AgentsCommander/issues/499))
- **External link confirmation for terminal links**: clicking an external link in an xterm session now asks for confirmation before opening, in both desktop and browser modes. ([#474](https://github.com/mblua/AgentsCommander/issues/474))
- **Deterministic GUI test mode**: a fake-transport mode that makes browser and GUI flows reproducible in CI.

### Changed

- **Public marketing copy overhaul**: README and docs now lead with the "compound your coding agents" message: bring any coding agent at full power, put a Fusion team on a Loop, and let scheduled runs compound toward the best answer. Positioned around "only adds, never subtracts." ([#520](https://github.com/mblua/AgentsCommander/issues/520))
- **OpenCode documented as supported (provider-agnostic)**: OpenCode can be run today via the custom coding-agent path and pointed at any provider or model. A first-class tuned profile (its own `CodingAgentKind`, resume tokens, idle tuning) remains planned. ([#315](https://github.com/mblua/AgentsCommander/issues/315))
- **Profile modal layout**: enlarged desktop layout and fixed intermediate stacking. ([#471](https://github.com/mblua/AgentsCommander/issues/471))
- **Test and CI regression-hardening pass**: PR regression gates, GUI regression suites, PTY lifecycle coverage, mailbox wake-routing tests, close-session integration fixtures, CLI behavior contract tests, and a Windows release CLI smoke test. ([#475](https://github.com/mblua/AgentsCommander/issues/475), [#479](https://github.com/mblua/AgentsCommander/issues/479), [#485](https://github.com/mblua/AgentsCommander/issues/485))

### Fixed

- **Coordinator repo badges for discovered workgroups**: workgroups discovered on disk now show their repo badge correctly. ([#500](https://github.com/mblua/AgentsCommander/issues/500))
- **Outbound network port exhaustion**: hardened outbound network resource handling and fixed bridge shutdown and poll backoff so repeated outbound requests no longer exhaust ports. ([#501](https://github.com/mblua/AgentsCommander/issues/501), [#502](https://github.com/mblua/AgentsCommander/issues/502))
- **Frontend websocket rejections**: fixed spurious rejection of valid frontend WebSocket connections. ([#480](https://github.com/mblua/AgentsCommander/issues/480), [#491](https://github.com/mblua/AgentsCommander/issues/491))
- **Project Loops scheduler races**: scheduler safety hardening, stale-prompt pre-write race, stale-delivery revalidation, enable and disable UI refresh, and delete-modal concurrency.

### Security

- **Hardened destructive filesystem delete paths**: tightened guards around destructive delete operations and closed review gaps found during the hardening pass. ([#512](https://github.com/mblua/AgentsCommander/issues/512))

## 0.8.43 — 2026-05-27

Public-push release: repo cleanup, documentation rewrite scaffolding, and factual corrections to public copy. See umbrella issue [#313](https://github.com/mblua/AgentsCommander/issues/313).

### Added

- `ROADMAP.md` at repo root — Shipped / Planned / Considering tracked publicly, with links to GitHub issues.
- `SECURITY.md` at repo root — vulnerability reporting policy + supported versions + 90-day coordinated disclosure.
- `CHANGELOG.md` at repo root — this file.
- `.github/ISSUE_TEMPLATE/` — `bug_report.yml`, `feature_request.yml`, and `config.yml` (directs Q&A to GitHub Discussions).
- `.github/PULL_REQUEST_TEMPLATE.md` — short checklist for contributors.

### Changed

- Documentation: `ROLE_AC_BUILDER.md` moved to `docs/agent-matrix-conventions.md`.
- `.gitignore`: added `_logbooks/`; replaced hand-listed workgroup entries with a single workspace workgroup glob.
- README factual fixes:
  - Supported coding agents corrected to **Claude Code · Codex · Gemini** (was: Claude Code · Codex · OpenCode — OpenCode is not yet supported; tracked in [#315](https://github.com/mblua/AgentsCommander/issues/315)).
  - Window-model description updated to reflect the unified main window (the old "Sidebar and Terminal are independent windows" description was pre-unification).
  - Settings tab name "Dark Factory" referenced as **"Teams"** in public copy. Internal code rename is tracked in [#314](https://github.com/mblua/AgentsCommander/issues/314).
  - Release-tag example updated from the stale `v0.4.9` to the current `0.8.x` line.

### Removed

- Obsolete artifact files at repo root: `DIAG-telegram-emission.md`, `FIXES_CODEX.md`, `PLAN-telegram-bridge.md`, `agentscommander-prompt.md`, and the `_test_dark_factory/` stub directory.
- Spanish-language and obsolete plan files from `docs/`: `Descripcion.md`, `home-es.md`, `PLAN_dark_factory.md`, `PLAN_OrganigramaDF.md`, `PROMPT_Etapa2_OrganigramaDF.md`.

### Notes

- Full README rewrite, new docs prose (quickstart, concepts, comparison, troubleshooting, faq, glossary, style-guide, use-cases, integrations, agents, features, reference), Acknowledgments section, and visual assets ship in follow-up commits on the same `chore/313-public-push` branch.

## Earlier releases

For all releases before `0.8.43`, see the auto-generated changelog on the [GitHub Releases](https://github.com/mblua/AgentsCommander/releases) page.
