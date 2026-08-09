# Isolated validation package

The isolated validation package is a purpose-built packaged candidate for the combined #1286 and #1271 gates. It starts with a single explicit isolated application-state root, a package-owned fixed identity, and no access to the WG-12 tester or matrix state.

This is not a general profile feature. A normal build and a normal launch retain their existing behavior. Do not use a project root, an environment variable, an executable location, a session, or a shared application-state directory to select isolated behavior.

## Use the package

Use the supplied launcher for validation. It is the supported way to establish the fixture root, verify the handoff, record the receipt, and start the package.

~~~powershell
powershell.exe -NoProfile -NonInteractive -File packaging/isolated-validation/launch-isolated.ps1 -FixtureRoot <absolute-fixture-root> -ExpectedManifestSha256 <trusted-handoff-hash>
pwsh -NoProfile -NonInteractive -File packaging/isolated-validation/launch-isolated.ps1 -FixtureRoot <absolute-fixture-root> -ExpectedManifestSha256 <trusted-handoff-hash>
~~~

The launcher requires an existing absolute <code>-FixtureRoot</code>. It derives exactly these paths:

| Purpose | Derived path |
| --- | --- |
| Isolated application-state root | <code>&lt;FixtureRoot&gt;/app-state</code> |
| Launch receipt | <code>&lt;FixtureRoot&gt;/launch-receipt.json</code> |

It accepts no caller-selected receipt path. It validates a complete pre-existing receipt before starting a child, runs the status command, verifies every payload and status value, starts the package with <code>--app --isolated-state-root &lt;FixtureRoot&gt;/app-state</code>, then atomically publishes a new receipt. A valid re-launch retains the original receipt bytes.

## Launch modes and option grammar

The package adds two global CLI options:

| Option | Grammar and behavior |
| --- | --- |
| <code>--isolated-state-root &lt;absolute-directory&gt;</code> | Opts into isolated mode only in the isolated validation package. It is global, so it can occur before or after a GUI subcommand. A normal build rejects this option before GUI initialization. |
| <code>--isolation-status</code> | Requires <code>--isolated-state-root</code>. It is mutually exclusive with <code>--app</code> and every subcommand. |

The status form is exact:

~~~text
agentscommander --isolated-state-root <root> --isolation-status
~~~

The GUI launch used by the launcher is:

~~~text
agentscommander --app --isolated-state-root <root>
~~~

<code>--isolation-status</code> performs the same root validation and atomic bootstrap as a GUI launch, emits one JSON object to standard output, releases its short-lived bootstrap lock, and exits. It does not initialize logging, create a WebView, create a terminal or child process, start a server, or acquire the final GUI singleton mutex.

The status JSON contains only the canonical effective root, package ID, profile hash, workspace, matrix, replica agent, header identity, bundle identifier, and mutex hash. It never exposes a token, session credential, environment value, or free-form caller input.

## Fixed package identity

The package identity is compiled into the isolated validation build. It is not selected at runtime.

| Field | Fixed value |
| --- | --- |
| Package ID | <code>agentscommander-1271-isolated-gates</code> |
| Product label | <code>Agents Commander Isolated Gates</code> |
| Bundle identifier | <code>dev.agentscommander.isolatedgates</code> |
| Workspace | <code>AgentsCommander_1271_isolated</code> |
| Matrix | <code>WG-1271-ISOLATED-GATES</code> |
| Replica agent | <code>gate-tester</code> |
| Required header identity | <code>WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated</code> |

Before any terminal or shell process exists, the titlebar reads a narrow, typed, read-only bridge. In isolated mode it displays the fixed workgroup and <code>agent@workspace</code> values above. It does not consult an active terminal, a session name, a root path, a marker, or caller input. If that bridge fails, the titlebar shows <code>ISOLATION IDENTITY UNAVAILABLE</code> and marks the state with <code>data-isolation-titlebar-state="error"</code>; it never falls back to <code>Terminal</code>.

Normal mode receives only <code>{ mode: "normal" }</code> from the bridge and retains the existing terminal-derived titlebar behavior. The normal bridge result has no filesystem side effect.

## Root contract

An isolated root must be a local absolute Windows path with no lexical <code>.</code> or <code>..</code> component. The root's parent must already exist, be accessible, and remain verified while the package creates at most the final root directory.

The package rejects all of the following before it creates isolated state:

- Empty or relative paths.
- A drive root, UNC path, or <code>\\?\</code> extended-namespace path.
- A non-directory target, a missing or inaccessible parent, or a read-only target.
- A root that equals, is under, or contains a normal-root candidate.
- A symlink, junction, or other reparse point in the existing parent or component chain.
- A path alias that resolves to the same object as a rejected path, including a case variant or 8.3 alias.
- A root or parent that is replaced after validation.
- A long path whose retained verified handle cannot be opened.

Validation compares retained filesystem object identities rather than strings. The package rechecks those identities before each child creation and each atomic marker or profile write. It never recursively creates an arbitrary caller-supplied path.

The first launch takes a short-lived bootstrap lock. Its bounded name is derived from the fixed package ID, retained parent object identity, and final child name, not from a raw path. This permits two distinct roots to bootstrap independently while allowing exactly one complete bootstrap for a shared root.

The root marker, <code>isolation-root.toml</code>, atomically binds the package ID, compiled-profile hash, and root object identity. A missing profile, malformed marker, profile mismatch, root replacement, or any validation failure fails closed. Isolated mode never reads, repairs from, copies, migrates, or falls back to normal application state.

The fixed isolated layout is:

~~~text
<fixture-root>/app-state/
  isolation-root.toml
  settings.json
  profile-project/AgentsCommander_1271_isolated/.ac/
    _agent_gate-tester/
    wg-1271-isolated-gates/__agent_gate-tester/config.json
  instances/
  agent-templates/
  context-cache/
  webview-data/
  app.log and existing config-owned files
~~~

Bootstrap creates a real profile-owned project and ordinary replica structure under this root. It registers exactly that verified project in isolated settings and writes the normal relative <code>config.json</code> identity. It does not introduce a separate isolation-specific identity or session-context format.

## Normal and isolated behavior

Normal behavior is intentionally unchanged. Without <code>--isolated-state-root</code>, the package does not load a profile, marker, bootstrap lock, isolated WebView directory, or isolated titlebar identity. Its existing application behavior, static identity, mutex bytes, CLI behavior, and terminal-derived titlebar remain as they were.

With <code>--isolated-state-root</code>, root selection occurs before logging, CLI dispatch, window placement, or final GUI singleton acquisition:

~~~text
raw argv
  -> global CLI parse
  -> resolve isolated startup state
  -> validate or bootstrap the verified root
  -> install the resolved root once
  -> status: print JSON, release bootstrap lock, exit
  -> GUI: acquire root mutex and disable profile web/API servers
  -> logging and application startup
  -> every native WebView uses root-scoped webview-data
  -> typed titlebar bridge resolves before terminal-derived identity
~~~

In isolated mode, normal-root resolution is not invoked. In particular, inherited <code>AGENTSCOMMANDER_*</code> values are not root or identity selectors and are ignored. The child process environment receives no such selector values.

The final isolated GUI mutex is <code>AC-ISO-&lt;SHA-256(package-id || root-object-identity)&gt;</code>. The main acquisition, GUI-running probe, and testability probe use the same root-scoped mutex. A normal process and processes using distinct isolated roots can coexist; a second GUI process for the same isolated root follows the existing single-instance behavior.

## State-routing inventory

Every isolated state-bearing path must remain under the declared isolated root unless an operation has an explicit user destination. The table is the required routing and proof inventory.

| State or input | Normal disposition | Isolated disposition | Required proof |
| --- | --- | --- | --- |
| CLI root and status fields | Existing parser behavior | Typed global CLI values only | Grammar and duplicate-flag tests |
| Normal-root resolver and debug override | Existing behavior | Not called; debug override ignored | Call recorder and inherited-environment test |
| Settings and project registry | Existing configuration directory | <code>&lt;root&gt;/settings.json</code> and one verified profile project | Sentinel and project round-trip |
| Coding-agent catalog and templates | Existing configuration directory | Root-scoped catalog and template paths | Write trace |
| Instance and session metadata | Existing <code>instances</code> layout | <code>&lt;root&gt;/instances</code> | Restart and trace |
| Context cache and replica identity | Existing project data | Profile-project <code>.ac</code> tree only | External-path rejection |
| Tokens, activity log, and <code>app.log</code> | Existing configuration directory | Root only | No-normal-log/token assertion |
| Agent and OpenCode directories | Existing explicit command semantics | Unchanged explicit semantics | Literal and placeholder regression |
| WebView profile and cache | Tauri default | <code>&lt;root&gt;/webview-data</code> at every native builder | Builder coverage and restart |
| Screenshot output | Existing explicit user destination | Existing explicit user destination; overlay WebView remains root-scoped | No implicit normal-root write |
| Package profile resource | Normal package resources | Installed read-only resource only | Installed-byte hash check |
| Child-process environment | Existing behavior | No root or identity selector exported; inherited <code>AGENTSCOMMANDER_*</code> ignored | Child-environment capture |
| Web/API endpoints and singleton mechanisms | Existing profile behavior | Servers disabled; final mutex root-scoped; no plugin singleton or shared endpoint | Two-root process proof |

## Failure policy

The isolated option is fail-closed:

- A normal build that receives <code>--isolated-state-root</code> fails before GUI initialization.
- An invalid root, marker, profile, lock handoff, or root identity produces a deterministic isolated error and never changes to normal mode.
- Status failures produce no standard output, one sanitized <code>E_ISOLATION_&lt;CODE&gt;</code> line on standard error, and exit with code <code>2</code>.
- Successful status exits with code <code>0</code>.
- No isolated failure reads normal content, starts a GUI or server, creates a WebView, creates a terminal or child process, writes a log, or acquires the final GUI mutex.

Do not retry a failed isolated launch by pointing the package at a normal root or by substituting a shared state directory. Correct the reported root, package, marker, or provenance error and relaunch through the launcher.

## Package and launcher trust model

Build the validation package only with <code>scripts/build-isolated-validation-package.ps1</code> from a clean detached worktree. The build requires these full SHA parameters:

~~~text
-Frozen1271Commit d68495086e168e5258500832b2ef45b4337ed21a
-IsolatedStateRootCommit <40-hex-SHA>
~~~

The build verifies that the normal configuration is unmodified, verifies that both commits descend from the fixed merge base, checks out the specified combined head, and invokes:

~~~text
node_modules/.bin/tauri.cmd build --features isolated-validation-package --config src-tauri/tauri.conf.isolated-validation.json
~~~

The isolated overlay preserves normal capabilities, signing, and base settings while applying the fixed product label, bundle identifier, and read-only bundled profile resource. The profile is compiled and bundled read-only. It is not a runtime root or identity input.

The build handoff manifest records provenance plus a hash for exactly four staged payloads:

- Base SHA, frozen #1271 SHA, isolated-state-root SHA, combined source SHA, and combined tree SHA.
- Clean-worktree result, executable SHA-256, compiled-profile SHA-256, and installed-profile SHA-256.
- UTC timestamp, mode, target, product label, bundle identifier, fixed header identity, and exact launcher command.
- <code>Agents Commander Isolated Gates.exe</code>, <code>launch-isolated.ps1</code>, <code>native-process.psm1</code>, and <code>resources/package-profile.toml</code>.

The manifest does not hash itself. The tech lead supplies the final manifest SHA-256 out of band with the artifact. Before parsing or trusting it, the launcher verifies that final hash, then verifies all four payload hashes, including <code>native-process.psm1</code> before importing the module. This detects manifest-only and individual staged-payload substitution.

The launcher serializes each child argument with its Windows-native argument serializer into <code>System.Diagnostics.ProcessStartInfo.Arguments</code> and sets <code>UseShellExecute = $false</code>. It does not build a shell command from a path, profile, label, identity, or manifest value. It removes inherited <code>AGENTSCOMMANDER_*</code> values from <code>ProcessStartInfo.EnvironmentVariables</code> only, as defense in depth.

## Immutable preflight for gates 38–48

Run gates 38–48 only after this preflight succeeds on one immutable combined artifact:

1. Build a clean detached combined artifact from frozen #1271 commit <code>d68495086e168e5258500832b2ef45b4337ed21a</code> and the completed #1286 commit. Hand off all four staged payloads, the manifest, and the trusted out-of-band final manifest SHA-256.
2. Create a new fixture under the tester replica. Record read-only before snapshots of known WG-12 tester and matrix state.
3. Run the launcher with an absolute fixture root and the trusted manifest hash. Confirm it verifies the final manifest before parsing, validates every staged payload before module import, captures status JSON, and writes its receipt only after validation.
4. Before any fixture agent, project, terminal, or shell exists, capture the earliest window. The status, receipt, and header must all show <code>WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated</code>. Reject <code>Terminal</code>, a shared identity, or shared state.
5. Create exactly one fixture coding agent and one fixture project. Confirm that CLI and sidebar discovery show only fixture objects and that state writes remain under the declared root.
6. Capture the initial receipt bytes, then close and relaunch the same staged artifact with the same root and trusted manifest hash. Reconfirm the header, status, receipt provenance, root-contained data, unchanged shared-state snapshots, and byte-for-byte unchanged receipt.
7. Run gates 38–48 only when every screenshot, gate result, and handoff record cites the same combined source, executable, profile, and manifest hashes.

Do not substitute a rebuilt artifact, source-only run, separate #1271 build, shared root, shared session, or a different executable/profile/manifest/receipt hash set. Any mismatch invalidates the gate run.
