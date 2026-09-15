# Plan #2016: npm launcher resolves the macOS `.app` bundle executable

Status: READY_FOR_IMPLEMENTATION (round 5: npm-cli resolution for Homebrew and Debian/Ubuntu hosts, on top of the implemented fix)

- Issue: https://github.com/mblua/AgentsCommander/issues/2016 (OPEN; technical score 23 → band 1-25, Lite; the PR #2025 re-score comment is the round-4 input)
- Repository: `repo-AgentsCommander`
- Branch: `fix/2016-npm-macos-launcher-path`
- Base: originally `main` `f9ee9f654802d823def68cc15b291f040c5c65f9` (the round-1..3 merge-base). Current first-parent history (verified with `git log --oneline --first-parent` and `git rev-parse`): `7838aea0` plan round 1, `bb630a53` plan round 2, `3c5e8fc7` plan round 3 (last approved revision), `2b78f5c9` implementation of §5 (`Grinch` PASS), `5b41d9be` merge of `origin/main` `36709240`, `55b6101c` plan round 4 (SonarCloud remediation; `Grinch` verified §9.4.1 and the harness mechanics OK but raised one blocker: the §9.4.2 `resolveNpmCli` candidate list misses on macOS Homebrew and Debian/Ubuntu). Round 5 (this revision) fixes that blocker in the plan; the code edits of §9.4 are still pending implementation.
- Author: `ac-dev-rust-v4`. Reviewer: `ac-dev-rust-grinch-v4`.
- Vetoes in force: Verification ≥ 8 and Environment ≥ 8. Both are addressed in §7 (proof package and the mandatory in-writing environment statement).
- Task class: routine application fix in the npm wrapper plus a static-analysis cleanup of the PR diff (§9); no Rust, no frontend, no IPC, no release, no version bump.

## 1. Objective

`npm install -g @mblua/agentscommander` on macOS completes, but the installed `agentscommander` command exits 1 with
`Cannot find AgentsCommander executable at <prefix>/.../bin/agentscommander`, because the postinstall extracts the app bundle and the launcher looks for a renamed binary that was never created. Fix the launcher so the published npm package starts the app on macOS.

Rounds 4-5 (this revision) keep that fix and clear the 8 SonarCloud findings the PR #2025 analysis raised (§9): same behavior, same files, no version bump.

Requirements from the user (mapped to design/test in §7.5):

| # | Requirement | Design element | Tests |
|---|---|---|---|
| R1 | `run.js` resolves per platform; on darwin targets the executable inside the extracted `.app`, resilient to a bundle rename; direct `spawn` with `stdio:'inherit'`; no `open -a`; no copy out of the bundle | `npm/resolve-bin.js` (bundle glob + `Info.plist`), unchanged spawn block | R3, H1, H2, A1, macOS handoff |
| R2 | `install.js` darwin branch validates the expected executable exists at the end and fails clearly | `assertExecutable('darwin', binDir)` after extraction | V1-V3, I2 |
| R3 | `run.js` error shows the concrete platform-specific path searched | resolver throws messages with absolute paths; `run.js` prints them; the `existsSync` message receives the resolved path | H2, H5, H6, H7, R5-R11, I2 |
| R4 | Linux and Windows paths unchanged | non-darwin branch is the exact current expression | R1, R2 |
| R5 | No version bump in this change | only `npm/package.json#files` changes | P1, P2 |
| R6 | Do not replicate the user's shell-script workaround | no copy, no shell wrapper, no `open -a` | P3 |
| R7 | An install whose postinstall never ran (`--ignore-scripts`) tells the user how to repair it | one `IGNORE_SCRIPTS_HINT` line in `run.js`, on both not-found paths | H2, H7, macOS handoff step 3 |

## 2. Cause with evidence

The postinstall and the launcher disagree about the on-disk contract for macOS:

| Fact | Evidence |
|---|---|
| `install.js` (darwin) extracts every top-level tarball entry into `bin/`, so the app stays a bundle: `bin/Agents Commander.app/...`; only the non-darwin `else` renames the downloaded file to `bin/agentscommander[.exe]` | `npm/install.js:118-134` (darwin branch) and `:135-140` (rename) |
| `run.js` always resolves `bin/agentscommander` (`.exe` on win32) and exits 1 when missing | `npm/run.js:7-14` |
| The published v0.32.0 tarball really contains only the bundle: `Agents Commander.app/` and the executable is `Agents Commander.app/Contents/MacOS/agentscommander` | `tar -tzf agentscommander-mac-x86_64.app.tar.gz` (release asset, 111 entries, no non-bundle top-level entry) |
| The bundle's own statement of the executable name is `CFBundleExecutable = agentscommander` | `Agents Commander.app/Contents/Info.plist` extracted from the same release asset |
| Bundle name and executable name are two independent settings (`productName` → bundle, `mainBinaryName` → executable) | `src-tauri/tauri.conf.json:3` and `:6`; the shipped plist above confirms the executable is `mainBinaryName`, not `productName` |
| The version the installer downloads must match the package version | `npm/install.js:8` `VERSION = "0.32.0"`, `npm/package.json:3` `0.32.0`, enforced by `scripts/check-version-sync.mjs` |

Root cause: two files each hardcoded their own path assumption; the darwin extraction contract was never represented in one shared place. The fix therefore puts resolution in one shared function used by both files.

## 3. In scope / out of scope

In scope:

- New `npm/resolve-bin.js`: the single, shared, platform-aware executable locator used by `run.js` and `install.js`.
- `npm/run.js`: use the shared locator; keep spawn/exit/signal behavior byte-identical.
- `npm/install.js`: validate the installed darwin executable after extraction.
- `npm/package.json`: add `resolve-bin.js` to `files` so `npm pack` ships it.
- New `scripts/check-npm-launcher.mjs`: the executable proof harness (§7.1).
- This plan.
- Rounds 4-5 only (§9): the 8 SonarCloud findings on the PR diff are cleared inside `npm/resolve-bin.js` (`node:` specifiers) and `scripts/check-npm-launcher.mjs` (optional chaining, `String.raw`, fixed absolute `tar` / `npm-cli.js` paths, with the npm-cli layout candidate list of §9.6 and the `--npm-cli` override). No other file changes.

Out of scope (decided, not open):

- `npm/README.md` support matrix. The package page currently says macOS is "Not supported … do not install through npm" (`npm/README.md`), while the issue asks to fix the launcher. Changing the declared support tier is a product decision tied to `docs/install-with-agent.md` platform gates, not part of this launcher fix. The macOS validation in §7.3 is therefore a tester/contributor validation, not a support declaration. Flagged to the tech lead in the reply.
- Version bump / release / CHANGELOG (`R5`; the release is separate, and the published v0.32.0 assets are already valid input for a local `npm pack` handoff).
- CI/release workflows: no new jobs, no path filters. In particular the root `package.json` is deliberately NOT touched, so `bundle-validation.yml` (path-filtered on `package.json`) is not triggered by this PR; the harness is run directly with `node`.
- Permissions (`chmod +x` of the extracted bundle executable): `tar` restores the archive mode; if it were ever wrong, `run.js` already reports `Failed to start AgentsCommander: spawn EACCES`. Adding a chmod is unrequested behavior.
- `open -a`, copying the executable out of the bundle, or any shell workaround (R6).
- Windows/Linux behavior changes of any kind.
- Symlinked `.app` directories, nested bundles, renaming the executable at runtime.

## 4. Decided solution (single)

`npm/resolve-bin.js` resolves the executable path as follows:

- `win32` → `path.join(binDir, 'agentscommander.exe')`; every other platform except darwin → `path.join(binDir, 'agentscommander')` — the exact current expression, so Linux/Windows resolve to the identical strings.
- `darwin` → find the **single top-level `*.app` directory** in `bin/`, then read that bundle's own `Contents/Info.plist` and take `CFBundleExecutable`; the executable is `<bundle>/Contents/MacOS/<CFBundleExecutable>`.

Why the bundle glob plus `Info.plist` (and why the rejected alternatives are rejected):

- The bundle directory name comes from `productName` ("Agents Commander", with a space) and the executable name from `mainBinaryName` ("agentscommander"). Hardcoding either half would re-create the same class of bug the next time the product or binary is renamed. The plist is the bundle's canonical, self-describing statement and travels inside the same artifact.
- Glob `Contents/MacOS/*` and pick the first entry: rejected. Directory order is not a contract, the directory may hold helper or non-executable entries, and "first match" silently selects a wrong target instead of failing.
- Hardcode `Contents/MacOS/agentscommander` while globbing only the bundle: rejected, see above — it fixes one rename surface out of two for the cost of one shared function anyway.
- No third-party plist parser: a Tauri `Info.plist` states `CFBundleExecutable` as a single `<key>`/`<string>` pair; a bounded regex is enough and keeps the npm package dependency-free (`npm/run.js` runs on the user's `node` with no install of extras).

Failure policy: when the executable cannot be determined (no bundle, more than one bundle, unreadable plist, no/empty/unsafe `CFBundleExecutable`), the resolver throws an `Error` whose message contains the concrete absolute path that was searched, and the caller prints it. It never guesses a fallback: a wrong guess would launch the wrong file instead of telling the user what is missing.

## 5. Exact changes (files and symbols)

### 5.1 NEW `npm/resolve-bin.js` (CommonJS, no dependencies beyond `node:fs`/`node:path`)

```js
const fs = require('node:fs');
const path = require('node:path');

const CFBUNDLE_EXECUTABLE_RE = /<key>\s*CFBundleExecutable\s*<\/key>\s*<string>([^<]*)<\/string>/;

function resolveBinPath(platform, binDir) { /* see rules below */ }

function assertExecutable(platform, binDir) { /* resolve + stat check, returns the path */ }

module.exports = { resolveBinPath, assertExecutable };
```

`resolveBinPath(platform, binDir) -> string`

1. `platform !== 'darwin'` → `path.join(binDir, platform === 'win32' ? 'agentscommander.exe' : 'agentscommander')` (identical to `npm/run.js:7-8` today).
2. darwin:
   - `fs.readdirSync(binDir, { withFileTypes: true })`; on throw: `Error: macOS app bundle not found: cannot read ${binDir} (${err.code})`.
   - candidates = entries with `entry.isDirectory() && entry.name.endsWith('.app')` (a plain file named `*.app` does not count; `readdir` is non-recursive).
   - exactly one required, else throw `macOS app bundle not found in ${binDir}: expected exactly one *.app directory, found none` / `found 2: A.app, B.app`.
   - `plistPath = path.join(appDir, 'Contents', 'Info.plist')`; read as UTF-8, on throw: `macOS app bundle is incomplete: cannot read ${plistPath} (${err.code})`.
   - `CFBUNDLE_EXECUTABLE_RE.exec(plist)` → capture, `trim()`. Reject when empty, or equal to `.`/`..`, or containing `/` or `\` (no path escape out of `Contents/MacOS`), else throw `macOS app bundle is incomplete: valid CFBundleExecutable not found in ${plistPath}`.
   - return `path.join(appDir, 'Contents', 'MacOS', executable)`.

`assertExecutable(platform, binDir) -> string`

- `const binPath = resolveBinPath(platform, binDir)`, then `fs.statSync(binPath)`; on throw: `macOS app bundle is incomplete: executable missing at ${binPath} (${err.code})`; if the stat is not a regular file: `macOS app bundle is incomplete: not a regular file at ${binPath}`; otherwise return `binPath`.

Every thrown message contains at least one absolute path (requirement R3).

### 5.2 MODIFIED `npm/run.js`

- Add `const { resolveBinPath } = require('./resolve-bin');` after the existing requires (`:2-5`).
- Add the shared hint constant and the resolver call, replacing `:7-8`:

```js
const binDir = path.join(__dirname, 'bin');

const IGNORE_SCRIPTS_HINT =
  'Hint: if npm install ran with --ignore-scripts, reinstall without that flag, or run: npm rebuild -g @mblua/agentscommander';

let binPath;
try {
  binPath = resolveBinPath(os.platform(), binDir);
} catch (err) {
  console.error(`Error: ${err.message}`);
  console.error('Please ensure the package was installed correctly.');
  console.error(IGNORE_SCRIPTS_HINT);
  process.exit(1);
}
```

- `:10-14` (`existsSync` guard + `Error: Cannot find AgentsCommander executable at ${binPath}` + `Please ensure the package was installed correctly.` + exit 1) keeps its current text; add `console.error(IGNORE_SCRIPTS_HINT);` after the `Please ensure…` line. It now receives the platform-specific resolved path, which satisfies R3 for the missing-file case.
- The hint is printed on exactly the two not-found paths: the resolver throw (`bin/` missing/unreadable, no or ambiguous bundle, unusable plist) and the `existsSync` miss on the resolved path. Both are the visible symptom of an install whose postinstall never produced `bin/`, most commonly `npm install --ignore-scripts`, and the hint names the two repairs (R7). It is deliberately NOT printed for `Failed to start AgentsCommander` spawn errors: the executable exists there, so a reinstall hint would be wrong.
- `:16-45` (spawn options `{ stdio: 'inherit', windowsHide: true }`, `error` handler, exit/signal mapping, `SIGINT`/`SIGTERM`/`SIGQUIT` forwarding) is untouched.

### 5.3 MODIFIED `npm/install.js`

- Add `const { assertExecutable } = require('./resolve-bin');` next to the other requires (`:1-6`).
- In the darwin branch, immediately after `fs.unlinkSync(tmpPath);` (`:134`), add `assertExecutable('darwin', binDir);`.
- Effect: a postinstall that produced no bundle, an ambiguous bundle, a bundle without a usable `Info.plist`, or a missing executable fails inside the existing `catch` (`:145-150`) as `Installation failed: <clear message with the absolute path>` and `process.exit(1)`. Downloads, checksum verification, `VERSION`, asset names, extraction/move code and the non-darwin branch are untouched.
- Partial-state decision: no new cleanup code. A validation failure can leave the extracted `.app` in `bin/` (the catch removes only the two temporary files, as today). That leftover is package-owned scratch and inert: the same defect makes `run.js` fail loudly, and a retry re-extracts over it because the extraction loop removes each top-level destination before renaming. No failure path can launch a wrong file, so tracking of the entries already moved is not added. I2 asserts this behavior.

### 5.4 MODIFIED `npm/package.json`

- `files` (`:12-15`) becomes `["run.js", "install.js", "resolve-bin.js"]`. Version stays `0.32.0`. No other field changes.

### 5.5 NEW `scripts/check-npm-launcher.mjs`

Fixture harness, plain Node ESM, zero dependencies (§7.1). Not shipped (it lives outside `npm/` and outside `files`). It prepares its own work dir (a `{"type":"commonjs"}` marker `package.json` so the copied CommonJS launcher files load as CJS inside this repo, a `stub.cjs` preload, fixture bundles and fixture tarballs) and never writes into the repository tree.

## 6. Behavior, edge cases and failure behavior

| Case | Behavior |
|---|---|
| macOS, healthy install | resolves `<bin>/<X>.app/Contents/MacOS/<CFBundleExecutable>`; spawn direct, stdio inherited, exit code/signals propagated as today |
| Bundle directory renamed (future `productName` change) | still found: the bundle is discovered by the `*.app` suffix, not by name |
| Executable renamed (future `mainBinaryName` change) | still found: the name comes from the bundle's own plist |
| Extra files in `bin/` (`.DS_Store`, `SHASUMS256.txt`, a regular file named `Fake.app`) | ignored |
| Zero `*.app` directories (or `bin/` missing) | clear error naming `bin/`; `install.js` fails the install; `run.js` exits 1 |
| More than one `*.app` directory | clear error listing the found names; no arbitrary pick |
| Missing/unreadable `Info.plist`, missing/empty/unsafe `CFBundleExecutable` | clear error naming the plist path; no fallback guess |
| Extraction produced an unusable bundle (install-time validation failure) | `install.js` exits 1 with the absolute path; downloaded temp files and `tmp-extract` are removed; already-extracted bundle entries stay in `bin/` (package-owned scratch): they are inert because the same defect makes `run.js` fail loudly, and a retry re-extracts over them |
| Resolved executable missing at run time | `Error: Cannot find AgentsCommander executable at <resolved absolute path>` + the existing `Please ensure the package was installed correctly.` line + `IGNORE_SCRIPTS_HINT`, exit 1 |
| Install whose postinstall never ran (`--ignore-scripts`): `bin/` missing, or the executable missing | `run.js` exits 1 with the resolver/`Cannot find` message naming the absolute path, plus the one-line hint: reinstall without `--ignore-scripts`, or run `npm rebuild -g @mblua/agentscommander` |
| Executable present but not executable / quarantine | unchanged behavior: `spawn` emits `error` → `Failed to start AgentsCommander: <message>`, exit 1 |
| Windows | `bin/agentscommander.exe`, exactly as today |
| Linux | `bin/agentscommander`, exactly as today |
| Signals | unchanged: `SIGINT`/`SIGTERM`/`SIGQUIT` forwarded; on child signal, the launcher re-raises the same signal on itself |

## 7. Verification

### 7.1 Proof harness: `scripts/check-npm-launcher.mjs`

Usage: `node scripts/check-npm-launcher.mjs [--work-dir <dir>] [--asset <agentscommander-mac-*.app.tar.gz>] [--npm-cli <path-to-npm-cli.js>] [--keep]`.
Defaults: `--work-dir` = fresh `fs.mkdtempSync(path.join(os.tmpdir(), 'ac-npm-launcher-'))`; a caller-provided work dir must be absent or empty and is removed at the end unless `--keep` (removal runs in a `finally`, so a failing run still cleans up). `--npm-cli` pins the npm CLI that P1 runs; without it the harness auto-detects it from `process.execPath` (§9.4.2/§9.6). Exit 0 = no `FAIL`; 1 = any `FAIL`; a `SKIP` is reported with its reason and does not fail the run. No network; `npm pack` runs with `npm_config_cache` redirected into the work dir.

Harness mechanics (each item below was executed and verified on this Windows host while writing this revision):

- The work dir gets a marker `package.json` containing `{"type": "commonjs"}`. This is load-bearing: the work dir under `target/` sits inside this repository, whose root `package.json` is `"type": "module"`, so without the marker every copied `.js` file — not only the preload stub — loads as ESM and dies with `ReferenceError: require is not defined` (verified with an unmodified copy of `run.js`).
- The harness copies `npm/run.js`, `npm/install.js` and `npm/resolve-bin.js` byte-for-byte into the work dir (asserted before running) so the repository tree is never mutated (`npm/bin/` is gitignored anyway; the copy also proves the require graph).
- The preload is `<work>/stub.cjs` — the `.cjs` extension pins CommonJS regardless of any enclosing `package.json` (verified: `node -r ./stub.cjs` loads and its `os.platform` override reaches the loaded script). It overrides `os.platform` and `os.arch` (fixed `x64`, so the asset name is deterministic) and replaces `https.get` with an offline transport used by I1-I2: `SHASUMS256.txt` is answered with the SHA-256 of the selected fixture tarball, the asset URL with that tarball's bytes (fixture selected by the `I2016_FIXTURE` env var, default `mac-ok.tar.gz`).
- Host-pinned fixture executable: `FIXTURE_EXE = process.platform === 'win32' ? 'renamed-exe.exe' : 'renamed-exe'`. Windows `CreateProcess` appends `.exe` to an extensionless application path, so a fixture named `renamed-exe` cannot be spawned on Windows (verified: extensionless copy of `node.exe` alone → `spawn ENOENT`; with `renamed-exe.exe` beside it, spawning `.../renamed-exe` runs `renamed-exe.exe`). Every synthetic fixture plist, fixture file and expected resolved path uses `FIXTURE_EXE` (including the I1-I2 fixtures); the real published asset in A1 keeps its real `agentscommander` name. The harness prints which name it pinned.
- Each H/I scenario starts from a freshly recreated `<work>/bin`, so no scenario can inherit another's state.
- The I scenarios spawn `node -r ./stub.cjs ./install.js` with `cwd` = work dir. On win32 that child's env puts `%SystemRoot%\System32` first on `PATH`, so `install.js`'s `execSync('tar ...')` resolves to the Windows-native bsdtar (macOS also ships bsdtar) instead of Git Bash's GNU tar, which misreads a `D:\...` argument as the remote-host form `host:path` (verified: GNU tar fails with `tar (child): Cannot connect to D: resolve failed`; bsdtar extracts the same archive cleanly). If no usable `tar` is found, I1-I2 report `SKIP` with the reason (does not happen on this host).
- Fixture tarballs and A1's real asset are always handled with relative paths (`tar -czf mac-ok.tar.gz -C fixture-ok "Agents Commander.app"`; the asset is copied into the work dir and extracted by relative name), so GNU tar never sees a drive-letter path.
- Round 4-5 (§9.4): every program the harness itself starts is addressed by a fixed absolute path — `%SystemRoot%\System32\tar.exe` (win32) or `/usr/bin/tar` / `/bin/tar` (darwin/linux) for tar, and `node <npm-cli.js>` for `npm pack`. `npm-cli.js` is located by the fixed candidate list of §9.4.2/§9.6 (`resolveNpmCli`, derived only from `process.execPath`, one candidate per documented layout: Windows installer, unix prefix, Homebrew keg, Homebrew post_install copy, Debian/Ubuntu `share/nodejs`); `--npm-cli <path>` pins it explicitly if a host uses yet another layout. The `PATH` prepend above remains only for the child `install.js` process, whose shipped `execSync('tar ...')` is out of scope; its directory is fixed and OS-owned.

| Id | Scenario | Expectation |
|---|---|---|
| R1 | `resolveBinPath('linux', bin)` | `bin/agentscommander` |
| R2 | `resolveBinPath('win32', bin)` | `bin/agentscommander.exe` |
| R3 | darwin fixture `Renamed Bundle.app` with `CFBundleExecutable=<FIXTURE_EXE>` and that file in `Contents/MacOS` | `<bin>/Renamed Bundle.app/Contents/MacOS/<FIXTURE_EXE>` |
| R4 | same plus a regular file `Decoy.app`, `notes.txt`, and a nested `Inner.app` inside the bundle | still exactly R3 (files and nested bundles ignored) |
| R5 | `bin/` does not exist | throws, message contains `bin`'s absolute path |
| R6 | empty `bin/` | throws, message contains `expected exactly one` and the path |
| R7 | two `.app` directories | throws, message contains both names |
| R8 | `Info.plist` removed | throws, message contains the plist absolute path |
| R9 | plist without `CFBundleExecutable` | throws, message contains the plist absolute path |
| R10 | plist with empty `<string>` | throws |
| R11 | plist value `../evil` | throws (no path escape) |
| R12 | plist value split over newlines/tabs | resolves (whitespace tolerated) |
| V1 | `assertExecutable('darwin', validFixture)` | returns R3's path, which ends in `<FIXTURE_EXE>` |
| V2 | same fixture, `<FIXTURE_EXE>` deleted | throws, message contains the full `.app/Contents/MacOS/<FIXTURE_EXE>` path |
| V3 | resolved path replaced by a directory | throws, `not a regular file` + path |
| P1 | `node <npm-cli.js> pack --dry-run --json` from `npm/` (npm CLI located as in P4; `--npm-cli <path>` overrides) | file set exactly `README.md, install.js, package.json, resolve-bin.js, run.js`; version equals root `package.json` version (`0.32.0`); on a resolution miss, `FAIL` lists every candidate tried and names the `--npm-cli` remedy |
| P2 | `node scripts/check-version-sync.mjs` | exit 0, every location at `0.32.0` |
| P3 | static guard on `npm/run.js` | no `open -a`, no `copyFile`/`cp`, executable not copied out of the bundle |
| P4 | synthetic node/npm trees, one per documented layout: Windows installer, unix prefix, Homebrew keg, Homebrew post_install copy, Debian/Ubuntu `share/nodejs` | each resolves to that layout's `npm-cli.js`; an unknown layout resolves to `null` and `npmCliCandidates` lists one candidate per layout |
| A1 | only with `--asset`: copy the asset into the work dir, extract it there with a relative-path `tar` call, then resolve | `<work>/real-bin/Agents Commander.app/Contents/MacOS/agentscommander`, existing regular file |

End-to-end launcher scenarios (the fixture "executable" is a copy of `process.execPath`, so it runs on the host; the plist name is honored, proving the plist is really read):

| Id | Scenario | Expectation |
|---|---|---|
| H1 | host platform success — win32/linux: `bin/agentscommander[.exe]` copy of `process.execPath`; darwin: `Renamed Bundle.app` with `<FIXTURE_EXE>`. Child args `-e "console.log('AC_MARKER:' + process.argv[1])" 'hello-arg'` | exit 0, stdout `AC_MARKER:hello-arg` (resolution + argv pass-through + stdio inherit) |
| H2 | host platform, executable missing | exit 1, stderr contains the concrete resolved path (`.../bin/agentscommander[.exe]`, or the `.app/Contents/MacOS/<FIXTURE_EXE>` path on darwin) and `IGNORE_SCRIPTS_HINT` |
| H3 | darwin forced on any host via `node -r ./stub.cjs` (overrides `os.platform`); bundle fixture with `<FIXTURE_EXE>` whose bytes are a copy of `process.execPath` | exit 0, `AC_MARKER:hello-arg` |
| H4 | darwin forced, child `-e "process.exit(7)"` | exit 7 (exit-code propagation) |
| H5 | darwin forced, bundle exists, `<FIXTURE_EXE>` deleted | exit 1, stderr contains the full `.app/Contents/MacOS/<FIXTURE_EXE>` path |
| H6 | darwin forced, no bundle | exit 1, stderr contains the fixture `bin/` absolute path and `expected exactly one` |
| H7 | darwin forced, `bin/` does not exist (the exact `--ignore-scripts` symptom: no postinstall ran) | exit 1, stderr contains `cannot read`, the fixture `bin/` absolute path, and `IGNORE_SCRIPTS_HINT` |
| E7 | `node --check` on `npm/run.js`, `npm/install.js`, `npm/resolve-bin.js` | exit 0 each |

Darwin install-branch scenarios (offline; they execute the real `install.js` darwin branch from §5.3, including the extraction move loop, the `assertExecutable` call and the catch):

| Id | Scenario | Expectation |
|---|---|---|
| I1 | fixture tarball `mac-ok.tar.gz` contains `Agents Commander.app` with a plist naming `<FIXTURE_EXE>` and a regular `Contents/MacOS/<FIXTURE_EXE>`; `node -r ./stub.cjs ./install.js` | exit 0; stdout `Installation completed successfully.`; `bin/Agents Commander.app/Contents/MacOS/<FIXTURE_EXE>` exists; no `*.tmp`, no `SHASUMS256.txt*`, no `tmp-extract` |
| I2 | same with `mac-incomplete.tar.gz`, whose bundle misses `Contents/MacOS/<FIXTURE_EXE>` | exit 1; stderr `Installation failed:` and `executable missing at` and the full absolute `.../Agents Commander.app/Contents/MacOS/<FIXTURE_EXE>` path; the incomplete `bin/Agents Commander.app` is still present (decided partial state below); no `*.tmp`, no `SHASUMS256.txt*`, no `tmp-extract` |

Partial-state decision (installs, §5.3): a validation failure does not clean up the extracted bundle. `bin/` is package-owned scratch, the leftover is inert (the same defect makes `run.js` fail loudly), and a retry re-extracts over it. I2 asserts exactly this, so the decision is evidence, not prose.

### 7.2 Implementation order with red/green control (owner: ac-dev-rust-v4)

All commands from `repo-AgentsCommander` with Git Bash; logs under `target/` (gitignored). The base-fix order below was executed as written in `2b78f5c9` (its step 1 names the branch tip at that time, `3c5e8fc7`); it stays as the record of that round. §7.2.1 is the round-4-5 order.

1. Preconditions: `git merge-base HEAD origin/main` = the base above (the branch tip is the docs-only plan commit, not the base); `git status --porcelain` empty; `test -f plans/2016-npm-macos-launcher-path.md`.
2. Add `npm/resolve-bin.js` and `scripts/check-npm-launcher.mjs` only.
3. Red control against the unmodified launcher (`npm/run.js`, `npm/install.js` and `npm/package.json` are still the base revisions; only `npm/resolve-bin.js` and the harness exist):
   `mkdir -p target && set -o pipefail && node scripts/check-npm-launcher.mjs --work-dir target/i2016-work 2>&1 | tee target/i2016-red.log; echo "red_exit=$?"`
   Expected on this win32 host: `PASS` = R1-R12, V1-V3, P2, P3, E7, H1, I1 (plus P4 once §9.4 adds it — it checks harness-internal layout arithmetic, not launcher behavior); `FAIL` = P1, H2, H3, H4, H5, H6, H7, I2; `red_exit` non-zero. Rationale: H1 passes because win32 is deliberately unchanged (R4); H2 and H7 fail because the base launcher prints no hint line (H7 also because the base launcher never reads the bundle and so never prints `cannot read`); H3-H6 fail because the unmodified launcher resolves `bin/agentscommander` for darwin and never reads the bundle (this is the bug proof); P1 fails because `files` does not list `resolve-bin.js` yet; I2 fails because no install-time validation exists yet, while I1 passes because extraction is unchanged. If H3-H6 (or I2) pass here, the harness does not prove the fix — stop and fix the harness.
4. Apply §5.2, §5.3, §5.4.
5. Green control:
   `node scripts/check-npm-launcher.mjs --work-dir target/i2016-work 2>&1 | tee target/i2016-green.log; echo "green_exit=$?"` → all PASS — including H2 and H7 (the new hint line), H3-H6 and I1-I2 on this host — `green_exit=0`.
6. Real-artifact replay: download the published tarball
   `gh release download v0.32.0 --repo mblua/AgentsCommander --pattern "agentscommander-mac-x86_64.app.tar.gz" --dir target` (and `aarch64` if cheap) then
   `node scripts/check-npm-launcher.mjs --work-dir target/i2016-work --asset target/agentscommander-mac-x86_64.app.tar.gz` → A1 passes against the real bundle layout.
7. Scope check: `git diff --name-only $(git merge-base HEAD origin/main)` lists exactly the five code files plus this plan; nothing else.
8. Commit `fix(npm): resolve the macOS .app bundle executable in the npm launcher (#2016)`, push `git push origin HEAD`. No merge, no release.

### 7.2.1 Round 4-5 order (SonarCloud remediation) — red/green re-validated in this revision

Run from the head of this plan revision (implementation of §5 already committed). Each run needs its own work dir: the harness accepts only an absent or empty `--work-dir` (it removes the dir it prepared), so the two controls use distinct names instead of one shared `target/i2016-work`.

1. Apply only §9.4.1 and §9.4.2 (round-5 revision).
2. Green: `node scripts/check-npm-launcher.mjs --work-dir target/i2016-work-green 2>&1 | tee target/i2016-green.log; echo "green_exit=$?"` → `29 passed, 0 failed, 1 skipped` (A1 skipped without `--asset`; P4 is the round-5 layout check), `green_exit=0`.
3. Red (harness validity): restore the pre-fix launcher revisions — `git checkout 3c5e8fc7 -- npm/run.js npm/install.js npm/package.json` — then run `node scripts/check-npm-launcher.mjs --work-dir target/i2016-work-red 2>&1 | tee target/i2016-red.log; echo "red_exit=$?"`, expect exactly `FAIL P1, H2, H3, H4, H5, H6, H7, I2` with `21 passed, 8 failed, 1 skipped`, `red_exit=1`; then `git checkout HEAD -- npm/run.js npm/install.js npm/package.json`. An identical red set (same 8 ids) to the base-fix red set in §7.2 step 3 means the remediation did not weaken the harness; P4 passes in both controls because it tests harness-internal layout arithmetic, not launcher behavior.
4. After step 3, `git status --porcelain` lists only `npm/resolve-bin.js` and `scripts/check-npm-launcher.mjs` as modified.
5. Commit `fix(npm): clear SonarCloud findings on the #2016 launcher changes`, `git push origin HEAD`, then verify §7.5 criteria 10-11. The A1 real-asset replay (step 6 above) is unaffected by rounds 4-5 and is run the same way.

Executed while writing this revision from `55b6101c` (the §9.4 edits were applied in the working tree, exercised, then reverted so the round-5 commit carries only this plan): step 2 gave `29 passed, 0 failed, 1 skipped` (log `target/i2016-green-round5.log`); step 3 gave `21 passed, 8 failed, 1 skipped` with the 8 failures exactly P1/H2/H3/H4/H5/H6/H7/I2 (log `target/i2016-red-round5.log`); P1 in the green control resolved `C:\Program Files\nodejs\node_modules\npm\bin\npm-cli.js` (candidate 1) and P4 passed all five synthetic layouts plus the unknown-layout miss; the regenerated `stub.cjs` is byte-identical to the pre-round-4 one (1129 bytes, LF — verified both by evaluating both `STUB_SOURCE` templates and by `cmp` against the `stub.cjs` the harness itself wrote in a `--keep` run, log `target/i2016-stub-run.log`); with the published asset the same command (work dir `target/i2016-work-asset`, log `target/i2016-green-round5-asset.log`) gave `30 passed, 0 failed, 0 skipped` (A1 resolved the real bundle); `--npm-cli <path>` was exercised positively (`29 passed, 0 failed, 1 skipped`, log `target/i2016-npmcli-override.log`); the auto-detection miss was exercised by running the harness under a node copied to `target/i2016-fake-node/bin/node.exe`, which failed only P1 (`28 passed, 1 failed, 1 skipped`, log `target/i2016-npmcli-missing.log`) and printed all five candidate paths plus the `--npm-cli` remedy.

### 7.3 macOS handoff (cannot be executed in this room; owner: user, prepared by ac-dev-rust-v4)

On the branch: `cd npm && npm pack` → `mblua-agentscommander-0.32.0.tgz` (the postinstall downloads the already-published v0.32.0 assets, so no release is needed to test the launcher fix).

On one x86_64 Mac and one arm64 Mac:

1. `npm install -g ./mblua-agentscommander-0.32.0.tgz` (exit 0; if the bundle validation fails, the exact path is in the error).
2. `agentscommander --version; echo exit=$?` → prints `0.32.0`, `exit=0`; the process exits immediately and no window opens.
3. `--ignore-scripts` round trip (the hint's own scenario, plus its remedy): `npm uninstall -g @mblua/agentscommander`, then `npm install -g --ignore-scripts ./mblua-agentscommander-0.32.0.tgz`, then `agentscommander --version; echo exit=$?` → `exit=1` and the `IGNORE_SCRIPTS_HINT` line; then `npm rebuild -g @mblua/agentscommander` and again `agentscommander --version; echo exit=$?` → `0.32.0`, `exit=0`.
4. Start the app normally, confirm the window opens and the process command is `.../Agents Commander.app/Contents/MacOS/agentscommander`, quit; `echo exit=$?` → 0.
5. Terminal interaction (observational; record the observed behavior; not a pass/fail gate): with the app running from a terminal, press Ctrl+C and record whether the app quits; start it again, close the terminal window and record whether the app quits or keeps running. Direct `spawn` with `stdio: 'inherit'` ties the app to its terminal (no `open -a`, no detach), so this behavior is pre-existing and unchanged by this fix; the step documents what actually happens.
6. Exit propagation and error path: `cd "$(npm root -g)/@mblua/agentscommander" && mv bin bin.off && node run.js --version; echo exit=$?` → `Error: macOS app bundle not found: cannot read .../bin (ENOENT)` plus `IGNORE_SCRIPTS_HINT`, `exit=1`; `mv bin.off bin`. If the global prefix is root-owned (e.g. `/usr/local`, the default with some Node installers), `mv` fails with `Permission denied`: prefix that command and its restore with `sudo`.
7. Run the shipped validation directly against the real installed bundle: `cd "$(npm root -g)/@mblua/agentscommander" && node -e "const path = require('path'); console.log(require('./resolve-bin').assertExecutable('darwin', path.join(process.cwd(), 'bin')))"` → prints `.../Agents Commander.app/Contents/MacOS/agentscommander`, exit 0 (the exact function step 1's postinstall calls).
8. Run `node scripts/check-npm-launcher.mjs` on macOS (H1/H2/H7 then exercise the real darwin branch, including H4's exit-7 propagation). The harness's own tar calls now use the fixed `/usr/bin/tar` (§9.4.2); `install.js` still resolves the same bsdtar through its PATH. P1 must resolve npm on Homebrew: candidate 3 (keg `libexec/lib/node_modules/npm`) or candidate 4 (post_install copy at `$(brew --prefix)/lib/node_modules/npm`) hits; if it ever reports `npm-cli.js not found`, the failure lists every candidate tried and the run can be pinned with `--npm-cli "$(brew --prefix)/lib/node_modules/npm/bin/npm-cli.js"`.
9. Record `uname -m`, `node -v`, `node -p process.arch`, `npm list -g @mblua/agentscommander`, and the outputs above. An x64 Node on an arm64 Mac reports `x64`, so `install.js` downloads the `x86_64` bundle (pre-existing `os.arch()` mapping, unchanged by this fix); to validate the `aarch64` asset, run the handoff with an arm64-native Node and record both values.

### 7.4 Environment risk (mandatory in-writing statement)

Host is Windows; this room has no macOS machine, and a Mach-O binary cannot execute here. Therefore:

- Testable on this host and actually executed (the harness mechanics in §7.1 were exercised while writing this revision): the resolution contract against the **real published v0.32.0 tarball layout** extracted on Windows (A1: bundle name, plist read, resolved path is a real regular file); the full launcher behavior — resolve, direct `spawn`, stdio inherit, argv pass-through, exit-code propagation, and every failure message — using a copy of `node` as a stand-in executable inside fixture bundles, with darwin forced through `stub.cjs` and the fixture executable pinned per host (H1-H7); the darwin branch of `install.js` end-to-end — download plumbing, checksum, extraction, move loop, `assertExecutable`, failure exit and partial state — driven offline by a fixture HTTPS transport (I1-I2); the validation function the postinstall calls, invoked directly (V1-V3); the shipped file set (`npm pack --dry-run`); version sync.
- NOT testable here and delegated to the macOS handoff (§7.3): the real HTTPS download (the harness substitutes the transport), real `npm install -g` postinstall on macOS, tar preserving the executable bit on APFS, launching the real (unsigned) Mach-O bundle, Gatekeeper/quarantine behaviour on the npm-installed tree, and real GUI startup, on both architectures; also the `--ignore-scripts` install and the `npm rebuild -g` remedy (handoff step 3), the terminal-tie behavior — Ctrl+C and closing the terminal while the app runs (handoff step 5) — and which architecture a Rosetta (x64) Node selects on an arm64 Mac (handoff step 9).
- Residual risk of the fixture method: a stand-in executable cannot expose macOS-only spawn failures (for example a lost `+x` bit or quarantine), the fixture transport skips TLS/redirect/HTTP-error paths of `install.js`, and the offline I scenarios run the host's bsdtar rather than macOS's. The tests state their own limits; the handoff is the acceptance gate for that residue. The issue report already states the downloaded tree carries no quarantine attribute and runs, but it must be re-verified by the user after a real install.
- Verification veto (≥ 8): the proof package is §7.1 plus the executed evidence of §7.2 and §7.2.1; the reviewer checks the red/green logs, the real-asset replay, and that the handoff steps have concrete commands and expected outputs (they do, §7.3). The implementation report must not claim macOS launch as verified from this host.

### 7.5 Acceptance criteria

| # | Criterion | Verified by |
|---|---|---|
| 1 | On macOS the launcher starts the executable inside the extracted bundle with a direct spawn | H1/H3/H4 + A1; final proof: macOS handoff step 4 |
| 2 | Bundle rename does not break resolution | R3, R4, H3 |
| 3 | `install.js` fails clearly when the expected executable is absent after extraction | V1-V3, R5-R11 (direct calls to the same exported functions `install.js` uses), I1-I2 (the darwin branch of `install.js` executed offline), macOS handoff steps 1 and 7 |
| 4 | Error messages show the concrete platform-specific path searched | H2, H5, H6, H7, R5-R11, I2 |
| 5 | Linux/Windows resolution is unchanged | R1, R2, H1/H2 on Windows, diff shows the identical expression |
| 6 | No version change (package 0.32.0, install.js VERSION 0.32.0) | P1, P2 |
| 7 | No `open -a`, no copy out of the bundle, no shell workaround | P3 |
| 8 | The published package contains every file needed at run time | P1 |
| 9 | An install that skipped the postinstall (`--ignore-scripts`) fails with a one-line reinstall/rebuild hint | H2 and H7 (harness, both launcher error paths); macOS handoff step 3 (real `--ignore-scripts` install plus the `npm rebuild -g` remedy) |
| 10 | SonarCloud Quality Gate on PR #2025 is passed (`new_security_rating` A = 0 new vulnerabilities) | `gh pr checks 2025 --repo mblua/AgentsCommander` shows `SonarCloud Code Analysis pass`; `api/qualitygates/project_status?projectKey=mblua_AgentsCommander&pullRequest=2025` → `"status":"OK"` |
| 11 | 0 unresolved Sonar findings on the PR (both `S4036` vulnerabilities and all 6 smells) | `api/issues/search?componentKeys=mblua_AgentsCommander&pullRequest=2025&resolved=false` → `"total":0` |
| 12 | The remediation changes no launcher behavior | Red/green sets of the harness are identical to the base fix (§7.2.1): red is exactly `FAIL P1, H2-H7, I2` (21/8/1), green all-pass (29/0/1; 30/0/0 with `--asset`), and round-5's P4 passes in both controls; the rounds-4-5 diff touches only `npm/resolve-bin.js` and `scripts/check-npm-launcher.mjs`; no version bump |

## 8. Inventory and dependency impact

| Type | Path |
|---|---|
| Added | `npm/resolve-bin.js` |
| Added | `scripts/check-npm-launcher.mjs` |
| Modified | `npm/run.js` |
| Modified | `npm/install.js` |
| Modified | `npm/package.json` (`files` only) |
| Modified | `plans/2016-npm-macos-launcher-path.md` (this plan; tracked since the round-1 plan commit) |
| Removed | none |
| Modified (round 4) | `npm/resolve-bin.js` (two `require` specifiers → `node:fs` / `node:path`) |
| Modified (round 4) | `scripts/check-npm-launcher.mjs` (S7780 `String.raw`, S6582 optional chaining, S4036 fixed absolute paths) |
| Modified (round 4) | `plans/2016-npm-macos-launcher-path.md` (round-4 revision, `55b6101c`) |
| Modified (round 5) | `scripts/check-npm-launcher.mjs` (`resolveNpmCli` candidate list and `--npm-cli`, the P1 failure message, the new P4 layout test — same two files, no shipped change) |
| Modified (round 5) | `plans/2016-npm-macos-launcher-path.md` (this round-5 revision) |

Dependency impact: two new intra-package require edges (`run.js → resolve-bin.js`, `install.js → resolve-bin.js`) and a new standalone script; no Rust, frontend, IPC, event, schema or lockfile changes; no new npm dependencies. `scripts/check-npm-launcher.mjs` is not shipped and is not wired into CI or into root `package.json` (avoids triggering `bundle-validation.yml`), so it is run directly as documented in §7.1-§7.2. Rounds 4-5 add no dependency: `resolveNpmCli()` reads only `process.execPath` and fixed candidate paths derived from it, and the npm CLI ships with node.

## 9. Rounds 4-5: SonarCloud remediation on PR #2025 (this revision)

### 9.1 Input: the 8 findings

PR #2025 ran SonarCloud (`projectKey=mblua_AgentsCommander`, `pullRequest=2025`). The Quality Gate is `ERROR` on exactly one condition — `new_security_rating` (actual 2, threshold 1); `new_reliability_rating`, `new_maintainability_rating`, `new_duplicated_lines_density` and `new_security_hotspots_reviewed` are OK. The PR carries 8 unresolved issues (checked while writing this revision; analysis of `5b41d9be`):

| # | Rule | Severity | Location (analysis) | Message |
|---|---|---|---|---|
| 1 | `javascript:S7772` | MINOR | `npm/resolve-bin.js:4` | Prefer `node:fs` over `fs` |
| 2 | `javascript:S7772` | MINOR | `npm/resolve-bin.js:5` | Prefer `node:path` over `path` |
| 3 | `javascript:S7780` | MINOR | `scripts/check-npm-launcher.mjs:60-91` (`STUB_SOURCE`) | `String.raw` should be used to avoid escaping `\` |
| 4 | `javascript:S7780` | MINOR | `scripts/check-npm-launcher.mjs:220` (`resolveTar`) | `String.raw` should be used to avoid escaping `\` |
| 5 | `javascript:S6582` | MINOR | `scripts/check-npm-launcher.mjs:133` (`record` catch) | Prefer using an optional chain expression |
| 6 | `javascript:S6582` | MINOR | `scripts/check-npm-launcher.mjs:157` (`assertThrows` catch) | Prefer using an optional chain expression |
| 7 | `javascript:S4036` | MINOR | `scripts/check-npm-launcher.mjs:225` (`execFileSync('tar', ...)`) | Make sure the "PATH" variable only contains fixed, unwriteable directories |
| 8 | `javascript:S4036` | MINOR | `scripts/check-npm-launcher.mjs:384` (`spawnSync('npm pack ...', { shell: true })`) | Make sure the "PATH" variable only contains fixed, unwriteable directories |

Findings 1-2 are in the shipped `npm/resolve-bin.js`; findings 3-8 are in the non-shipped harness. `npm/run.js`, `npm/install.js` and `npm/package.json` have no findings and are not touched by rounds 4-5.

### 9.2 Per-finding resolution (one edit each)

| # | Finding | Resolution |
|---|---|---|
| 1-2 | `S7772` ×2 | `require('node:fs')` / `require('node:path')` in `npm/resolve-bin.js` (§9.4.1); the package's `engines.node` is already `>=18.0.0`, which supports the `node:` scheme in CommonJS (compatibility note in §9.4.1) |
| 3 | `S7780` `STUB_SOURCE` | Tag the template literal with `String.raw` and write the generated newline as `'\n'`; the emitted `stub.cjs` stays byte-identical (verified: 1129 bytes before and after) |
| 4 | `S7780` `'C:\\Windows'` | Replace the escaped literal with ``String.raw`C:\Windows` `` |
| 5-6 | `S6582` ×2 | Translate the flagged `err && err.message` to `err?.message`; the ternary result is unchanged for every input (falsy or absent message → `String(err)`) |
| 7 | `S4036` `tar` | `resolveTar()` returns fixed OS-owned absolute paths (`%SystemRoot%\System32\tar.exe`, or the first runnable of `/usr/bin/tar`, `/bin/tar`) and keeps the `--version` availability probe at the absolute path; the harness no longer resolves `tar` through PATH |
| 8 | `S4036` `npm pack` | Replace the shell string with `spawnSync(process.execPath, [npmCli, 'pack', '--dry-run', '--json'])`, where `resolveNpmCli()` locates the npm CLI from `process.execPath` through the five fixed layout candidates of §9.6 (Windows installer, unix prefix, Homebrew keg, Homebrew post_install copy, Debian/Ubuntu `share/nodejs`); no shell, no PATH; `--npm-cli <path>` pins it if a host uses another layout |

### 9.3 The one approach for PATH/tar resolution in the harness (decision)

**Chosen: every program the harness itself starts is addressed by a fixed absolute path; exactly one PATH prepend remains, and only for the child `install.js` process.**

1. tar: `%SystemRoot%\System32\tar.exe` on win32; `/usr/bin/tar` (probed with `--version`) then `/bin/tar` on darwin/linux. Both are fixed, OS-owned locations.
2. npm: `process.execPath` plus the npm CLI found through the fixed candidate list of §9.4.2/§9.6 (one candidate per documented node/npm layout: Windows installer, unix prefix, Homebrew keg, Homebrew post_install copy, Debian/Ubuntu `share/nodejs`). No shell, no PATH. `--npm-cli <path-to-npm-cli.js>` pins the CLI when a host uses another layout; when nothing resolves, P1 fails with the full candidate list and the remedy (§9.6, failure behavior).
3. The child `install.js` still gets `%SystemRoot%\System32` prepended on win32, because the shipped `install.js` resolves `tar` through the child's PATH inside its own `execSync('tar ...')`; that shipped call is out of scope and was never a Sonar finding. The prepended directory is fixed and OS-owned.

Verified facts preserved: Git Bash's GNU tar still misreads a `D:\...` argument as `host:path` and is still avoided for `install.js` (the System32 prepend is unchanged), while the harness's own tar calls no longer depend on which tar a caller's PATH would select; darwin/linux behavior is unchanged (system tar, relative-path fixtures). Round 5 only broadens npm-cli discovery inside the same rule: the candidate list is absolute and derived from `process.execPath`, and the `--npm-cli` override is a user-supplied path; no lookup goes through PATH.

Rejected alternatives: (a) `// NOSONAR` suppressions — hide the finding instead of fixing the resolution; (b) mutating the harness's own `PATH` — the lookup remains, only its source changes; (c) replacing the child's PATH entirely — a larger deviation from the verified environment with no finding to fix; (d) passing an absolute tar path into `install.js` — requires editing shipped `install.js`, outside the approved scope.

### 9.4 Exact edits

Two diffs: the shipped `npm/resolve-bin.js` (S7772) and the non-shipped harness (S6582, S7780, S4036). Both were applied, exercised and reverted while writing this revision; results in §7.2.1.

#### 9.4.1 `npm/resolve-bin.js` — S7772 ×2

```diff
diff --git a/npm/resolve-bin.js b/npm/resolve-bin.js
index e9a5c962..5c132352 100644
--- a/npm/resolve-bin.js
+++ b/npm/resolve-bin.js
@@ -1,8 +1,8 @@
 // Shared, platform-aware locator for the AgentsCommander executable inside the
 // installed package. run.js (launch) and install.js (post-extraction validation)
 // both use it so the two files agree on one on-disk contract. Issue #2016.
-const fs = require('fs');
-const path = require('path');
+const fs = require('node:fs');
+const path = require('node:path');
 
 // A Tauri Info.plist states CFBundleExecutable as a single <key>/<string> pair.
 // A bounded regex reads it without adding a plist parser to the published package.
```

Engines compatibility: the `node:` scheme is accepted by `require()` since Node 14.18 / 16.0.0 (the `node:`-scheme support for `require`), and `npm/package.json#engines` declares `>=18.0.0`, so every runtime the package supports accepts both lines. There is no bundler or transpiler in this package, and the ESM harness already imports `node:`-prefixed specifiers. `npm/run.js` and `npm/install.js` keep their existing `require('fs')`/`require('path')` lines: those are pre-existing, unflagged lines, and touching shipped files for no finding is out of scope.

#### 9.4.2 `scripts/check-npm-launcher.mjs` — S6582 ×2, S7780 ×2, S4036 ×2

Full diff (applied and red/green-verified while writing this revision; run results in §7.2.1):

```diff
diff --git a/scripts/check-npm-launcher.mjs b/scripts/check-npm-launcher.mjs
index 230322b2..b80cff15 100644
--- a/scripts/check-npm-launcher.mjs
+++ b/scripts/check-npm-launcher.mjs
@@ -4,7 +4,9 @@
 // outside npm/package.json#files); run it directly with node.
 //
 // Usage:
-//   node scripts/check-npm-launcher.mjs [--work-dir <dir>] [--asset <agentscommander-mac-*.app.tar.gz>] [--keep]
+//   node scripts/check-npm-launcher.mjs [--work-dir <dir>] [--asset <agentscommander-mac-*.app.tar.gz>] [--npm-cli <path-to-npm-cli.js>] [--keep]
+//
+// --npm-cli pins the npm CLI used by P1 when auto-detection misses the host layout.
 //
 // Exit codes: 0 = no FAIL; 1 = at least one FAIL. A SKIP is reported with its
 // reason and never fails the run.
@@ -42,6 +44,7 @@ function argValue(name) {
 const keep = argv.includes('--keep');
 const workDirArg = argValue('--work-dir');
 const assetArg = argValue('--asset');
+const npmCliArg = argValue('--npm-cli');
 
 const workDir = workDirArg
   ? path.resolve(workDirArg)
@@ -57,7 +60,7 @@ const NODE_BYTES = fs.readFileSync(process.execPath);
 const CHILD_SCRIPT = "console.log('AC_MARKER:' + process.argv[1])";
 const COPIED_FILES = ['run.js', 'install.js', 'resolve-bin.js'];
 
-const STUB_SOURCE = `// Generated by scripts/check-npm-launcher.mjs for issue #2016. Do not edit.
+const STUB_SOURCE = String.raw`// Generated by scripts/check-npm-launcher.mjs for issue #2016. Do not edit.
 const fs = require('fs');
 const path = require('path');
 const crypto = require('crypto');
@@ -78,7 +81,7 @@ https.get = function (url, options, callback) {
   let payload;
   if (String(url).indexOf('SHASUMS256.txt') !== -1) {
     const digest = crypto.createHash('sha256').update(fs.readFileSync(fixturePath)).digest('hex');
-    payload = digest + '  ' + ASSET_NAME + '\\n';
+    payload = digest + '  ' + ASSET_NAME + '\n';
   } else {
     payload = fs.readFileSync(fixturePath);
   }
@@ -130,7 +133,7 @@ function test(id, fn) {
     const detail = fn();
     record(id, 'PASS', detail || '');
   } catch (err) {
-    record(id, 'FAIL', err && err.message ? err.message : String(err));
+    record(id, 'FAIL', err?.message ? err.message : String(err));
   }
 }
 function skip(id, reason) {
@@ -154,7 +157,7 @@ function assertThrows(fn, needles, label) {
   try {
     fn();
   } catch (err) {
-    message = err && err.message ? err.message : String(err);
+    message = err?.message ? err.message : String(err);
   }
   assert(message !== null, `${label}: expected a throw, but the call succeeded`);
   for (const needle of needles) assertIncludes(message, needle, `${label} thrown message`);
@@ -215,18 +218,56 @@ function freshBin() {
 
 // ---------- tar ----------
 
+// Fixed, OS-owned tar locations only: the harness never resolves tar through
+// PATH (javascript:S4036). Windows uses the native bsdtar in System32 because
+// Git Bash's GNU tar misreads a D:\... argument as the remote-host form
+// host:path; macOS (/usr/bin/tar is bsdtar) and Linux keep their system tar.
+const POSIX_TAR_CANDIDATES = ['/usr/bin/tar', '/bin/tar'];
+
 function resolveTar() {
   if (process.platform === 'win32') {
-    const dir = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32');
+    const dir = path.join(process.env.SystemRoot || String.raw`C:\Windows`, 'System32');
     const exe = path.join(dir, 'tar.exe');
     return fs.existsSync(exe) ? { cmd: exe, dir } : null;
   }
-  try {
-    execFileSync('tar', ['--version'], { stdio: 'ignore' });
-    return { cmd: 'tar', dir: null };
-  } catch {
-    return null;
+  for (const cmd of POSIX_TAR_CANDIDATES) {
+    try {
+      execFileSync(cmd, ['--version'], { stdio: 'ignore' });
+      return { cmd, dir: null };
+    } catch {
+      // try the next fixed location
+    }
   }
+  return null;
+}
+
+// ---------- npm CLI ----------
+
+// Fixed absolute locations derived only from the running node, never from PATH
+// (javascript:S4036). One per documented packaging layout:
+//   <nodeDir>/node_modules/npm          Windows installer, nvm-windows, scoop
+//   <prefix>/lib/node_modules/npm       unix prefix: official tarball, nvm,
+//                                       fnm, Volta, asdf, Nix, snap, Fedora,
+//                                       Arch, Alpine, /usr -> /usr/lib
+//   <keg>/libexec/lib/node_modules/npm  Homebrew keg (node is built
+//                                       --without-npm; the formula installs
+//                                       npm under libexec)
+//   <prefix>/lib/node_modules/npm       Homebrew post_install copy, prefix
+//                                       derived from <prefix>/Cellar/node/<v>/bin
+//   <prefix>/share/nodejs/npm           Debian and Ubuntu distro npm
+function npmCliCandidates(execPath = process.execPath) {
+  const nodeDir = path.dirname(execPath);
+  return [
+    path.join(nodeDir, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
+    path.join(nodeDir, '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
+    path.join(nodeDir, '..', 'libexec', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
+    path.join(nodeDir, '..', '..', '..', '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
+    path.join(nodeDir, '..', 'share', 'nodejs', 'npm', 'bin', 'npm-cli.js'),
+  ];
+}
+
+function resolveNpmCli(execPath = process.execPath) {
+  return npmCliCandidates(execPath).find((candidate) => fs.existsSync(candidate)) || null;
 }
 
 function prependPath(env, dir) {
@@ -249,6 +290,9 @@ function runLauncher(launcherArgs, options = {}) {
 
 function runInstall(fixture, tarInfo) {
   const env = { ...process.env, I2016_PLATFORM: 'darwin', I2016_FIXTURE: fixture };
+  // Shipped install.js resolves `tar` through the child's PATH (its own
+  // execSync('tar ...') is out of scope); this is the only PATH use left in the
+  // harness and the prepended directory is fixed and OS-owned.
   if (tarInfo.dir) prependPath(env, tarInfo.dir);
   return spawnSync(
     process.execPath,
@@ -380,11 +424,21 @@ function runAllChecks() {
 
   // --- package plumbing ---
   test('P1', () => {
+    const npmCli = npmCliArg ? path.resolve(npmCliArg) : resolveNpmCli();
+    assert(
+      npmCli !== null,
+      `P1: npm-cli.js not found for ${process.execPath}; tried:\n` +
+        npmCliCandidates().map((candidate) => `  ${candidate}`).join('\n') +
+        '\nPass --npm-cli <path-to-npm-cli.js> to pin it.',
+    );
+    assert(
+      fs.existsSync(npmCli),
+      `P1: npm-cli.js not found at ${npmCli}${npmCliArg ? ' (--npm-cli)' : ''}`,
+    );
     const env = { ...process.env, npm_config_cache: path.join(workDir, 'npm-cache') };
-    const r = spawnSync('npm pack --dry-run --json', {
+    const r = spawnSync(process.execPath, [npmCli, 'pack', '--dry-run', '--json'], {
       cwd: NPM_DIR,
       env,
-      shell: true,
       encoding: 'utf8',
     });
     expectExit(r, 0, 'P1 npm pack');
@@ -418,6 +472,54 @@ function runAllChecks() {
     );
   });
 
+  test('P4', () => {
+    // Synthetic trees, one per documented npm layout: checks the candidate
+    // arithmetic (notably the Homebrew prefix derived from the keg path). Only a
+    // real host can prove that a vendor lays its files out this way.
+    const layouts = [
+      {
+        name: 'windows-installer',
+        node: ['node.exe'],
+        npm: ['node_modules', 'npm', 'bin', 'npm-cli.js'],
+      },
+      {
+        name: 'unix-prefix',
+        node: ['bin', 'node'],
+        npm: ['lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'],
+      },
+      {
+        name: 'homebrew-keg',
+        node: ['Cellar', 'node', '26.8.2', 'bin', 'node'],
+        npm: ['Cellar', 'node', '26.8.2', 'libexec', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'],
+      },
+      {
+        name: 'homebrew-prefix',
+        node: ['Cellar', 'node', '26.8.2', 'bin', 'node'],
+        npm: ['lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'],
+      },
+      {
+        name: 'debian-share',
+        node: ['bin', 'node'],
+        npm: ['share', 'nodejs', 'npm', 'bin', 'npm-cli.js'],
+      },
+    ];
+    for (const layout of layouts) {
+      const root = path.join(workDir, 'npm-cli-layouts', layout.name);
+      const nodePath = path.join(root, ...layout.node);
+      const expected = path.join(root, ...layout.npm);
+      fs.mkdirSync(path.dirname(expected), { recursive: true });
+      fs.writeFileSync(expected, '// layout fixture\n');
+      expectEqual(resolveNpmCli(nodePath), expected, `P4 ${layout.name}`);
+    }
+    const unknown = path.join(workDir, 'npm-cli-layouts', 'unknown', 'deep', 'bin', 'node');
+    expectEqual(resolveNpmCli(unknown), null, 'P4 unknown layout');
+    expectEqual(
+      npmCliCandidates(unknown).length,
+      layouts.length,
+      'P4: one candidate per documented layout',
+    );
+  });
+
   // --- real published artifact (opt-in) ---
   const tarInfo = resolveTar();
   if (!assetArg) {
```

Semantics checks executed while writing this revision: the generated `stub.cjs` is byte-identical to the pre-remediation one (1129 bytes, LF; verified by evaluating both `STUB_SOURCE` templates, which the JS parser LF-normalizes, and by `cmp` against the `stub.cjs` the harness itself wrote in a `--keep` run, log `target/i2016-stub-run.log`); the optional-chain lines keep the same fallback for non-object errors and falsy messages; `resolveTar()` still returns `null` (→ I1/I2/A1 `SKIP`) when no usable tar exists; `resolveNpmCli()` resolves `C:\Program Files\nodejs\node_modules\npm\bin\npm-cli.js` on this host via candidate 1 and P1's `PASS` is unchanged; P4 resolves the four synthetic non-host layouts and the unknown-layout miss; run under a node copied to `target/i2016-fake-node/bin/node.exe` (the auto-detection miss), P1 fails and prints all five candidate paths plus the `--npm-cli` remedy (log `target/i2016-npmcli-missing.log`); `--npm-cli <path>` passes P1 with the CLI pinned (log `target/i2016-npmcli-override.log`).

### 9.5 Scope guard for rounds 4-5

- No change to launcher behavior: `npm/run.js` and `npm/install.js` are untouched by rounds 4-5; `npm/package.json` (version, `files`, `engines`) is untouched; no version bump.
- No new dependency; no new shipped file; `npm pack` file set is unchanged.
- No CI workflow edit: the SonarCloud analysis is the SonarCloud GitHub App, re-run automatically when a commit is pushed to PR #2025; the gate and issue checks are API queries against sonarcloud.io (§7.5 criteria 10-11).
- Round 5 changes only npm-cli discovery inside the non-shipped harness (`resolveNpmCli` candidates, `--npm-cli`, the P1 failure message, the new P4 test); §9.4.1 and every shipped-file decision are unchanged.

### 9.6 Round 5: npm-cli resolution on Homebrew and Debian/Ubuntu (decision)

The round-4 review found that §9.4.2's two npm-cli candidates miss on macOS: libuv realpaths the executable, so on a Homebrew Mac `process.execPath` is `/opt/homebrew/Cellar/node/<v>/bin/node`, while npm lives in the keg at `<keg>/libexec/lib/node_modules/npm` and is copied by the formula's post-install step to `{{HOMEBREW_PREFIX}}/lib/node_modules/npm`. Candidate 1 (`<nodeDir>/node_modules/npm`) and candidate 2 (`<nodeDir>/../lib/node_modules/npm`, i.e. `<keg>/lib/node_modules/npm`) both miss; `assert(npmCli !== null)` then failed P1 on the very host §7.3 step 8 asks the user to run. Debian/Ubuntu distro npm (`/usr/share/nodejs/npm`) was missed as well.

Vendor evidence (fetched from the vendor sources while writing this revision):

| Layout | Evidence |
|---|---|
| Homebrew keg: `<keg>/libexec/lib/node_modules/npm/bin/npm-cli.js` | `homebrew-core` `Formula/n/node.rb` (master): node is configured `--without-npm`, then `system "node", bootstrap/"bin/npm-cli.js", "install", ..., "--global", "--prefix=#{libexec}", ...` installs npm under `libexec`; the formula symlinks `bin/npm` to `libexec/lib/node_modules/npm/bin/npm-cli.js` |
| Homebrew prefix copy: `{{HOMEBREW_PREFIX}}/lib/node_modules/npm/bin/npm-cli.js` | same formula, `post_install_steps`: `copy "{{libexec}}/lib/node_modules/npm", "{{HOMEBREW_PREFIX}}/lib/node_modules", recursive: true` and `symlink "{{HOMEBREW_PREFIX}}/lib/node_modules/npm/bin/npm-cli.js", "{{bin}}/npm"`; the prefix is derivable from the keg path `<prefix>/Cellar/node/<v>/bin` (four levels up) |
| Debian bookworm and Ubuntu noble: `/usr/share/nodejs/npm/bin/npm-cli.js` | `packages.debian.org/bookworm/amd64/npm/filelist` and `packages.ubuntu.com/noble/amd64/npm/filelist` list exactly that path; Ubuntu noble `nodejs` installs `/usr/bin/node`, so `<nodeDir>/../share/nodejs/npm` reaches it |

Decision (one resolution; the alternatives and rejections follow):

1. Candidate list (`npmCliCandidates`), all fixed absolute paths derived only from `process.execPath`, in this order: `<nodeDir>/node_modules/npm/bin/npm-cli.js` (Windows installer, nvm-windows, scoop), `<nodeDir>/../lib/node_modules/npm/bin/npm-cli.js` (official tarball, nvm, fnm, Volta, asdf, Nix, snap, Fedora, Arch, Alpine, `/usr/bin/node` → `/usr/lib`), `<nodeDir>/../libexec/lib/node_modules/npm/bin/npm-cli.js` (Homebrew keg), `<nodeDir>/../../../../lib/node_modules/npm/bin/npm-cli.js` (Homebrew post_install copy, prefix derived from `<prefix>/Cellar/node/<v>/bin`), `<nodeDir>/../share/nodejs/npm/bin/npm-cli.js` (Debian/Ubuntu). First existing path wins; `resolveNpmCli(execPath)` takes the node path as a parameter so P4 can exercise every layout.
2. Failure behavior: no SKIP variant. When nothing resolves (and no `--npm-cli` was given), P1 fails with a message that names `process.execPath`, lists every candidate tried, and names the remedy `--npm-cli <path-to-npm-cli.js>`. Rationale: P1 is the only evidence for §7.5 criteria 6 and 8; a silent SKIP would delete acceptance evidence on exactly the hosts this round targets, while the explicit remedy turns an exotic layout into a one-flag run. The failure was exercised under a node copy with no npm beside it (log `target/i2016-npmcli-missing.log`).
3. `--npm-cli <path-to-npm-cli.js>`: parsed from argv, used instead of auto-detection (resolved against the CWD), with its own clear failure if the path does not exist. Verified positive (`target/i2016-npmcli-override.log`).
4. New P4 test: synthetic trees, one per documented layout (Windows installer, unix prefix, Homebrew keg, Homebrew post_install copy, Debian/Ubuntu share), each must resolve to that layout's npm-cli.js; an unknown layout must resolve to `null` and `npmCliCandidates` must return one candidate per documented layout. This runs on the Windows host and covers the Homebrew prefix arithmetic (deriving `<prefix>` from `<prefix>/Cellar/node/<v>/bin`). Stated limit: it proves the candidate arithmetic against the documented layouts, not that a vendor host really lays its files out that way; the macOS handoff (§7.3 step 8) is the real-host gate.

Rejected alternatives: (a) resolve `npm`/`which npm` through PATH or a shell — that is the S4036 finding; (b) follow `<keg>/bin/npm` as a symlink — its target differs before and after the Homebrew post-install step, so the explicit keg and prefix paths are simpler and both verified; (c) hardcode only the default prefixes `/opt/homebrew` and `/usr/local` — breaks custom Homebrew prefixes, while the keg-derived candidate is relative arithmetic that works for any prefix; (d) keep P1 failing with no remedy — rejected as in item 2; (e) `// NOSONAR` or a shell — rejected in §9.3.
