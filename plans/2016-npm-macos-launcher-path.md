# Plan #2016: npm launcher resolves the macOS `.app` bundle executable

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2016 (OPEN; technical score 23 → band 1-25, Lite)
- Repository: `repo-AgentsCommander`
- Branch: `fix/2016-npm-macos-launcher-path`
- Base: `main` `f9ee9f654802d823def68cc15b291f040c5c65f9` = local `HEAD` = `origin/fix/2016-npm-macos-launcher-path` (verified with `git rev-parse HEAD` and `git ls-remote`)
- Author: `ac-dev-rust-v4`. Reviewer: `ac-dev-rust-grinch-v4`.
- Vetoes in force: Verification ≥ 8 and Environment ≥ 8. Both are addressed in §7 (proof package and the mandatory in-writing environment statement).
- Task class: routine application fix in the npm wrapper; no Rust, no frontend, no IPC, no release, no version bump.

## 1. Objective

`npm install -g @mblua/agentscommander` on macOS completes, but the installed `agentscommander` command exits 1 with
`Cannot find AgentsCommander executable at <prefix>/.../bin/agentscommander`, because the postinstall extracts the app bundle and the launcher looks for a renamed binary that was never created. Fix the launcher so the published npm package starts the app on macOS.

Requirements from the user (mapped to design/test in §7.5):

| # | Requirement | Design element | Tests |
|---|---|---|---|
| R1 | `run.js` resolves per platform; on darwin targets the executable inside the extracted `.app`, resilient to a bundle rename; direct `spawn` with `stdio:'inherit'`; no `open -a`; no copy out of the bundle | `npm/resolve-bin.js` (bundle glob + `Info.plist`), unchanged spawn block | R3, H1, H2, A1, macOS handoff |
| R2 | `install.js` darwin branch validates the expected executable exists at the end and fails clearly | `assertExecutable('darwin', binDir)` after extraction | V1-V3, E5-H2 |
| R3 | `run.js` error shows the concrete platform-specific path searched | resolver throws messages with absolute paths; `run.js` prints them; existing `existsSync` message now receives the resolved path | H2, E4, E5, E6 |
| R4 | Linux and Windows paths unchanged | non-darwin branch is the exact current expression | R1, R2 |
| R5 | No version bump in this change | only `npm/package.json#files` changes | P1, P2 |
| R6 | Do not replicate the user's shell-script workaround | no copy, no shell wrapper, no `open -a` | P3 |

## 2. Cause with evidence

The postinstall and the launcher disagree about the on-disk contract for macOS:

| Fact | Evidence |
|---|---|
| `install.js` (darwin) extracts every top-level tarball entry into `bin/`, so the app stays a bundle: `bin/Agents Commander.app/...`; only the non-darwin `else` renames the downloaded file to `bin/agentscommander[.exe]` | `npm/install.js:118-134` (darwin branch) and `:135-140` (rename) |
| `run.js` always resolves `bin/agentscommander` (`.exe` on win32) and exits 1 when missing | `npm/run.js:7-14` |
| The published v0.32.0 tarball really contains only the bundle: `Agents Commander.app/` and the executable is `Agents Commander.app/Contents/MacOS/agentscommander` | `tar -tzf agentscommander-mac-x86_64.app.tar.gz` (release asset, 111 entries, no non-bundle top-level entry) |
| The bundle's own statement of the executable name is `CFBundleExecutable = agentscommander` | `Agents Commander.app/Contents/Info.plist` extracted from the same release asset |
| Bundle name and executable name are two independent settings (`productName` → bundle, `mainBinaryName` → executable) | `src-tauri/tauri.conf.json:3` and `:6`; the shipped plist above confirms the executable is `mainBinaryName`, not `productName` |
| The version the installer downloads must match the package version | `npm/install.js:9` `VERSION = "0.32.0"`, `npm/package.json:3` `0.32.0`, enforced by `scripts/check-version-sync.mjs` |

Root cause: two files each hardcoded their own path assumption; the darwin extraction contract was never represented in one shared place. The fix therefore puts resolution in one shared function used by both files.

## 3. In scope / out of scope

In scope:

- New `npm/resolve-bin.js`: the single, shared, platform-aware executable locator used by `run.js` and `install.js`.
- `npm/run.js`: use the shared locator; keep spawn/exit/signal behavior byte-identical.
- `npm/install.js`: validate the installed darwin executable after extraction.
- `npm/package.json`: add `resolve-bin.js` to `files` so `npm pack` ships it.
- New `scripts/check-npm-launcher.mjs`: the executable proof harness (§7.1).
- This plan.

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

### 5.1 NEW `npm/resolve-bin.js` (CommonJS, no dependencies beyond `fs`/`path`)

```js
const fs = require('fs');
const path = require('path');

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
- Replace `:7-8` with:

```js
const binDir = path.join(__dirname, 'bin');

let binPath;
try {
  binPath = resolveBinPath(os.platform(), binDir);
} catch (err) {
  console.error(`Error: ${err.message}`);
  console.error('Please ensure the package was installed correctly.');
  process.exit(1);
}
```

- `:10-14` (`existsSync` + `Cannot find AgentsCommander executable at ${binPath}` + hint + exit 1) stays byte-identical; it now receives the platform-specific resolved path, which satisfies R3 for the missing-file case.
- `:16-45` (spawn options `{ stdio: 'inherit', windowsHide: true }`, `error` handler, exit/signal mapping, `SIGINT`/`SIGTERM`/`SIGQUIT` forwarding) is untouched.

### 5.3 MODIFIED `npm/install.js`

- Add `const { assertExecutable } = require('./resolve-bin');` next to the other requires (`:1-6`).
- In the darwin branch, immediately after `fs.unlinkSync(tmpPath);` (`:134`), add `assertExecutable('darwin', binDir);`.
- Effect: a postinstall that produced no bundle, an ambiguous bundle, a bundle without a usable `Info.plist`, or a missing executable fails inside the existing `catch` (`:143-149`) as `Installation failed: <clear message with the absolute path>` and `process.exit(1)`. Downloads, checksum verification, `VERSION`, asset names, extraction/move code and the non-darwin branch are untouched.

### 5.4 MODIFIED `npm/package.json`

- `files` (`:12-15`) becomes `["run.js", "install.js", "resolve-bin.js"]`. Version stays `0.32.0`. No other field changes.

### 5.5 NEW `scripts/check-npm-launcher.mjs`

Fixture harness, plain Node ESM, zero dependencies (§7.1). Not shipped (it lives outside `npm/` and outside `files`).

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
| Resolved executable missing at run time | `Error: Cannot find AgentsCommander executable at <resolved absolute path>` + existing hint, exit 1 |
| Executable present but not executable / quarantine | unchanged behavior: `spawn` emits `error` → `Failed to start AgentsCommander: <message>`, exit 1 |
| Windows | `bin/agentscommander.exe`, exactly as today |
| Linux | `bin/agentscommander`, exactly as today |
| Signals | unchanged: `SIGINT`/`SIGTERM`/`SIGQUIT` forwarded; on child signal, the launcher re-raises the same signal on itself |

## 7. Verification

### 7.1 Proof harness: `scripts/check-npm-launcher.mjs`

Usage: `node scripts/check-npm-launcher.mjs [--work-dir <dir>] [--asset <agentscommander-mac-*.app.tar.gz>] [--keep]`.
Defaults: `--work-dir` = fresh `fs.mkdtempSync(path.join(os.tmpdir(), 'ac-npm-launcher-'))`; a caller-provided work dir must be absent or empty and is removed at the end unless `--keep`. Exit 0 = all scenarios pass, 1 = any failure; every scenario prints `PASS`/`FAIL` with an id. No network; `npm pack` runs with `npm_config_cache` redirected into the work dir.

The harness copies `npm/run.js` and `npm/resolve-bin.js` byte-for-byte into the work dir (asserted before running) so the repository tree is never mutated (`npm/bin/` is gitignored anyway; the copy also proves the require graph).

| Id | Scenario | Expectation |
|---|---|---|
| R1 | `resolveBinPath('linux', bin)` | `bin/agentscommander` |
| R2 | `resolveBinPath('win32', bin)` | `bin/agentscommander.exe` |
| R3 | darwin fixture `Renamed Bundle.app` with `CFBundleExecutable=renamed-exe` | `<bin>/Renamed Bundle.app/Contents/MacOS/renamed-exe` |
| R4 | same plus a regular file `Decoy.app`, `notes.txt`, and a nested `Inner.app` inside the bundle | still exactly R3 (files and nested bundles ignored) |
| R5 | `bin/` does not exist | throws, message contains `bin`'s absolute path |
| R6 | empty `bin/` | throws, message contains `expected exactly one` and the path |
| R7 | two `.app` directories | throws, message contains both names |
| R8 | `Info.plist` removed | throws, message contains the plist absolute path |
| R9 | plist without `CFBundleExecutable` | throws, message contains the plist absolute path |
| R10 | plist with empty `<string>` | throws |
| R11 | plist value `../evil` | throws (no path escape) |
| R12 | plist value split over newlines/tabs | resolves (whitespace tolerated) |
| V1 | `assertExecutable('darwin', validFixture)` | returns R3's path |
| V2 | same fixture, executable file deleted | throws, message contains the full `.app/Contents/MacOS/...` path |
| V3 | resolved path replaced by a directory | throws, `not a regular file` + path |
| P1 | `npm pack --dry-run --json` from `npm/` | file set exactly `README.md, install.js, package.json, resolve-bin.js, run.js`; version equals root `package.json` version (`0.32.0`) |
| P2 | `node scripts/check-version-sync.mjs` | exit 0, every location at `0.32.0` |
| P3 | static guard on `npm/run.js` | no `open -a`, no `copyFile`/`cp`, executable not copied out of the bundle |
| A1 | only with `--asset`: extract the real `agentscommander-mac-*.app.tar.gz` and resolve | `<work>/real-bin/Agents Commander.app/Contents/MacOS/agentscommander`, existing regular file |

End-to-end launcher scenarios (the fixture "executable" is a copy of `process.execPath`, so it runs on the host; the plist name is honored, proving the plist is really read):

| Id | Scenario | Expectation |
|---|---|---|
| H1 | host platform success — win32/linux: `bin/agentscommander[.exe]` copy; darwin: bundle fixture. Child args `-e "console.log('AC_MARKER:' + process.argv[1])" 'hello-arg'` | exit 0, stdout `AC_MARKER:hello-arg` (resolution + argv pass-through + stdio inherit) |
| H2 | host platform, executable missing | exit 1, stderr contains the concrete resolved path (`.../bin/agentscommander[.exe]`, or the `.app/Contents/MacOS/...` path on darwin) |
| H3 | darwin forced on any host via `node -r <preload stub>` that overrides `os.platform` (stub written into the work dir) | exit 0, `AC_MARKER:hello-arg` |
| H4 | darwin forced, child `-e "process.exit(7)"` | exit 7 (exit-code propagation) |
| H5 | darwin forced, bundle exists, executable file deleted | exit 1, stderr contains the full `.app/Contents/MacOS/<name>` path |
| H6 | darwin forced, no bundle | exit 1, stderr contains the fixture `bin/` absolute path and `expected exactly one` |
| E7 | `node --check` on `npm/run.js`, `npm/install.js`, `npm/resolve-bin.js` | exit 0 each |

### 7.2 Implementation order with red/green control (owner: ac-dev-rust-v4)

All commands from `repo-AgentsCommander` with Git Bash; logs under `target/` (gitignored).

1. Preconditions: `git rev-parse HEAD` = base above; `git status --porcelain` empty; `test -f plans/2016-npm-macos-launcher-path.md`.
2. Add `npm/resolve-bin.js` and `scripts/check-npm-launcher.mjs` only.
3. Red control against the unmodified launcher:
   `mkdir -p target && set -o pipefail && node scripts/check-npm-launcher.mjs --work-dir target/i2016-work 2>&1 | tee target/i2016-red.log; echo "red_exit=$?"`
   Expected: resolver/packaging scenarios pass, H1-H6 FAIL with the old `bin/agentscommander` path; `red_exit` non-zero. If H1-H6 pass here, the harness does not prove the bug — stop.
4. Apply §5.2, §5.3, §5.4.
5. Green control:
   `node scripts/check-npm-launcher.mjs --work-dir target/i2016-work 2>&1 | tee target/i2016-green.log; echo "green_exit=$?"` → all PASS, `green_exit=0`.
6. Real-artifact replay: download the published tarball
   `gh release download v0.32.0 --repo mblua/AgentsCommander --pattern "agentscommander-mac-x86_64.app.tar.gz" --dir target` (and `aarch64` if cheap) then
   `node scripts/check-npm-launcher.mjs --work-dir target/i2016-work --asset target/agentscommander-mac-x86_64.app.tar.gz` → A1 passes against the real bundle layout.
7. Scope check: `git diff --name-only f9ee9f65` lists exactly the five code files + (`git add -f`) the plan; nothing else.
8. Commit `fix(npm): resolve the macOS .app bundle executable in the npm launcher (#2016)`, push `git push origin HEAD`. No merge, no release.

### 7.3 macOS handoff (cannot be executed in this room; owner: user, prepared by ac-dev-rust-v4)

On the branch: `cd npm && npm pack` → `mblua-agentscommander-0.32.0.tgz` (the postinstall downloads the already-published v0.32.0 assets, so no release is needed to test the launcher fix).

On one x86_64 Mac and one arm64 Mac:

1. `npm install -g ./mblua-agentscommander-0.32.0.tgz` (exit 0; if the bundle validation fails, the exact path is in the error).
2. `agentscommander --version; echo exit=$?` → prints `0.32.0`, `exit=0`.
3. Start the app normally, confirm the window opens and the process command is `.../Agents Commander.app/Contents/MacOS/agentscommander`, quit; `echo exit=$?` → 0.
4. Exit propagation and error path: `cd "$(npm root -g)/@mblua/agentscommander" && mv bin bin.off && node run.js --version; echo exit=$?` → error naming the `.app`/`bin` path, `exit=1`; `mv bin.off bin`.
5. Run `node scripts/check-npm-launcher.mjs` on macOS (H1/H2 then exercise the real darwin branch, including H4's exit-7 propagation).
6. Record `uname -m`, `node -v`, `npm list -g @mblua/agentscommander`, and the outputs above.

### 7.4 Environment risk (mandatory in-writing statement)

Host is Windows; this room has no macOS machine, and a Mach-O binary cannot execute here. Therefore:

- Testable on this host and actually executed: the resolution contract against the **real published v0.32.0 tarball layout** extracted on Windows (A1: bundle name, plist read, resolved path is a real regular file); the full launcher behavior — resolve, direct `spawn`, stdio inherit, argv pass-through, exit-code propagation, and every failure message — using a copy of `node` as a stand-in executable inside fixture bundles, with darwin forced through a preload stub (H1-H6); the install-time validation through the exact exported function the postinstall calls (V1-V3); the shipped file set (`npm pack --dry-run`); version sync.
- NOT testable here and delegated to the macOS handoff (§7.3): real `npm install -g` postinstall on macOS, tar preserving the executable bit on APFS, launching the real (unsigned) Mach-O bundle, Gatekeeper/quarantine behaviour on the npm-installed tree, and real GUI startup, on both architectures.
- Residual risk of the fixture method: a stand-in executable cannot expose macOS-only spawn failures (for example a lost `+x` bit or quarantine). The tests state their own limit; the handoff is the acceptance gate for that residue. The issue report already states the downloaded tree carries no quarantine attribute and runs, but it must be re-verified by the user after a real install.
- Verification veto (≥ 8): the proof package is §7.1 plus the executed evidence of §7.2; the reviewer checks the red/green logs, the real-asset replay, and that the handoff steps have concrete commands and expected outputs (they do, §7.3). The implementation report must not claim macOS launch as verified from this host.

### 7.5 Acceptance criteria

| # | Criterion | Verified by |
|---|---|---|
| 1 | On macOS the launcher starts the executable inside the extracted bundle with a direct spawn | H1/H3/H4 + A1; final proof: macOS handoff step 3 |
| 2 | Bundle rename does not break resolution | R3, R4, H3 |
| 3 | `install.js` fails clearly when the expected executable is absent after extraction | V1-V3, R5-R11 (thrown from the postinstall path), macOS handoff step 1 |
| 4 | Error messages show the concrete platform-specific path searched | H2, H5, H6, R5-R11 |
| 5 | Linux/Windows resolution is unchanged | R1, R2, H1/H2 on Windows, diff shows the identical expression |
| 6 | No version change (package 0.32.0, install.js VERSION 0.32.0) | P1, P2 |
| 7 | No `open -a`, no copy out of the bundle, no shell workaround | P3 |
| 8 | The published package contains every file needed at run time | P1 |

## 8. Inventory and dependency impact

| Type | Path |
|---|---|
| Added | `plans/2016-npm-macos-launcher-path.md` (this plan, `git add -f`) |
| Added | `npm/resolve-bin.js` |
| Added | `scripts/check-npm-launcher.mjs` |
| Modified | `npm/run.js` |
| Modified | `npm/install.js` |
| Modified | `npm/package.json` (`files` only) |
| Removed | none |

Dependency impact: two new intra-package require edges (`run.js → resolve-bin.js`, `install.js → resolve-bin.js`) and a new standalone script; no Rust, frontend, IPC, event, schema or lockfile changes; no new npm dependencies. `scripts/check-npm-launcher.mjs` is not shipped and is not wired into CI or into root `package.json` (avoids triggering `bundle-validation.yml`), so it is run directly as documented in §7.1-§7.2.
