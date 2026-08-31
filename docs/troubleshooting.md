# Troubleshooting

For developers hitting an error. Skim the headings for the symptom that matches yours.

## Installation

Start with the [installation support gates](install-with-agent.md#support-gates). Windows 10 1809+ and Windows 11 on x86_64/AMD64 are fully supported. Linux x86_64/AMD64 is partial and requires an explicit warning and confirmation. macOS is not a normal install target yet.

### Windows: SmartScreen blocks the installer

Windows code signing is planned through SignPath, but current release artifacts may be unsigned until [epic #717](https://github.com/mblua/AgentsCommander/issues/717) is complete. SmartScreen can also warn on newly signed apps before they build reputation. Do not bypass the warning silently. Before running a downloaded installer, [verify its exact filename and checksum](install-with-agent.md#verify-a-downloaded-asset-manually) against the same release's `SHASUMS256.txt`, then inspect Authenticode status separately:

```powershell
Import-Module (Join-Path $PSHOME "Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1") -ErrorAction Stop
Get-AuthenticodeSignature -LiteralPath ".\Agents.Commander_<version>_x64-setup.exe"
```

Until Windows signing is active, `Status` may read `NotSigned`. Ask separately before running unsigned software. Once SignPath signing is active, `Status` should read `Valid`. A matching checksum is not publisher-compromise protection and does not make an unsigned artifact signed. See [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md).

### Linux: `.AppImage` will not execute

Linux x86_64/AMD64 support is partial and in progress. Continue only after acknowledging that tier and verifying `Agents.Commander_<version>_amd64.AppImage` against the same release's `SHASUMS256.txt`.

```bash
asset='Agents.Commander_<version>_amd64.AppImage'
chmod +x "$asset"
"./$asset"
```

`chmod` exits 0 without output; the next command launches the verified AppImage. If it fails with `error while loading shared libraries: libwebkit2gtk-4.1.so.0`, stop and identify the WebKitGTK 4.1 runtime package for that exact distribution and version. Linux package names vary. A Coding Agent must report the package source and exact command, then ask separately before elevation or a system package change.

### macOS: installation stops

macOS is not supported yet because maintainer and test capacity is insufficient. A `.dmg` or other artifact on a release does not make macOS supported. Do not bypass Gatekeeper as part of a normal installation.

If you deliberately choose the tester/contributor path, use the [reproducible platform report template](install-with-agent.md#help-extend-linux-and-macos-support) and follow [`CONTRIBUTING.md`](../CONTRIBUTING.md). That choice is separate from normal install approval.

## Coding-agent detection

### "No coding agents detected"

AC launches the commands configured under `settings.json → agents[]`. Verify that the CLI you use is on the `PATH` inherited by AC:

```powershell
where.exe claude
where.exe codex
where.exe agy
where.exe pi
```

On Linux or macOS:

```bash
command -v claude
command -v codex
command -v agy
command -v pi
```

Each successful command prints an executable path. If the binary is installed but no path appears, add it to `PATH` or point the matching `agents[]` entry's `command` at the full path. See [Installing the coding-agent CLIs](integrations/coding-agents.md#installing-the-clis) and the [Settings reference](reference/settings.md).

### A coding-agent wrapper is not detected

Claude and Codex use the legacy **executable basename prefix** detector. Wrappers such as `claude-mb` or `codex-foo` retain tuned behavior because the prefix matches. Antigravity matches the exact executable stems `agy` and `antigravity` (prefix wrappers are not inferred). An unrelated name such as `my-llm` is treated as a plain shell.

Pi is intentionally stricter. Use an exact executable leaf named `pi`, `pi.exe`, or `pi.cmd`, directly or as the first command under `cmd.exe /C` or `/K`. An alias such as `my-pi`, a wrapper such as `npx pi`, `/S /C pi`, grouped Pi, or Pi after a compound separator does not receive Pi resume behavior. Unsupported Pi-shaped positions fail closed rather than being reclassified from a later Claude or Codex option value. See [How AC identifies a tuned integration](integrations/coding-agents.md#how-ac-identifies-a-tuned-integration).

### Pi starts but does not continue a conversation

First determine whether the launch should be fresh. AC adds `--continue` only when its final lifecycle decision requests known state, such as an eligible restore or reopen. A fresh create, fresh restart, or final orchestrator fresh override deliberately launches without an AC-authored selector.

For an expected known-state launch, set `logLevel` to `info` and look for:

```text
Auto-injected Pi `--continue` for trusted agent '<agent-id>'
```

If the line is absent, check these conditions:

- Launch Pi from a configured Coding Agents entry. Heuristic session metadata does not authorize injection.
- Use an [exact supported Pi command position](integrations/coding-agents.md#how-ac-identifies-a-tuned-integration), not an alias or unsupported complex cmd shape.
- Check for an existing `-c`, `-r`, `--continue`, `--resume`, `--session`, `--session-id`, `--fork`, or `--no-session`. These controls intentionally win; remove one only if you want AC to choose automatically.
- Package/config commands and one-shot help, version, export, and model-list modes intentionally remain unchanged.

If the log line appears but Pi opens a new conversation, Pi found no matching session for the current working directory and effective session directory. That is Pi's normal `--continue` behavior. AC does not inspect Pi storage or retry without the flag. Use the accepted separated spelling `--session-dir <dir>` when selecting custom state; Pi 0.80.10 rejects `--session-dir=<dir>`. See [Pi resume behavior](integrations/coding-agents.md#pi-resume-behavior).

## Sessions

### Session is "running" but the agent looks idle

AC marks a session **idle** after 2.5 seconds of PTY silence. If your agent prints a spinner or progress bar, the silence detector may keep it in **running** state. This is intentional — false idles are worse than false runnings.

### Detached window shows nothing

Pop the session back in (right-click → **Reattach**) and pop it out again. xterm.js loses its WebGL context if the host GPU driver resets.

## Inter-agent messaging

### `send: filename contains path separators or traversal`

`--send` takes a **filename only**. Do not pass an absolute path. The CLI resolves the filename against `<room-root>/messaging/`:

```bash
# bad
agentscommander send --send "C:/.../messaging/20260527-150000-wg1-foo-to-wg1-bar-hello.md" ...

# good
agentscommander send --send "20260527-150000-wg1-foo-to-wg1-bar-hello.md" ...
```

### `routing rejected`

The sender does not share a team with the recipient, or the sender is not an orchestrator. Run `list-peers-lean` from the sender's directory to see who is reachable:

```bash
agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

Peers with `"reachable": false` are visible to the sender but not directly addressable.

### Recipient does not receive the message

If `list-peers-lean` shows the recipient as `"sessionStatus": "none"`, the first `--mode wake` only spawns their session — it does not inject the message. Send a second time to actually deliver. Verify with `working: true` after the first send before retrying.

## Telegram bridge

### Bot attaches but receives no output

Check that the bot's chat ID matches a chat you actually started with the bot. Telegram requires that the user message the bot first; the bot cannot DM cold.

Then verify the bridge state in **Sessions → … → Telegram**. A red dot on the bridge means the last poll failed — open the developer console (Help → Toggle DevTools) and look at `telegram_bridge_error` events.

### Bot floods the chat with raw escape codes

Two reasons:
- The session is running a TUI that AC's vt100 cleaner does not recognize. Open an issue with the agent name and what you were running.
- Your coding agent emits non-printable codes outside the cleaner's known set. Update AC (`telegram/redact.rs` is patched regularly).

## Voice-to-text

### Mic button does nothing

Voice requires a Gemini API key in **Settings → Integrations → Voice**. Without a key the button stays disabled. The key is stored locally in `settings.json` — see [`PRIVACY.md`](../PRIVACY.md).

### Transcription is wrong every time

The Gemini transcription model is `gemini-2.5-flash` by default. Switch to `gemini-1.5-pro` under Settings → Integrations → Voice if you need better accuracy at higher cost.

## Logs

If something fails and you cannot tell why, raise the log level. Pick a level in **Settings -> General -> Logging** (it applies live, no restart), set `logLevel` in `settings.json`, or set `RUST_LOG` before launching for per-module filtering from a terminal:

```bash
RUST_LOG=agentscommander=trace agentscommander.exe
```

See [Log filtering](reference/log-filtering.md) for the five levels, the live selector, and precedence rules.

---

Still stuck? Open an [issue](https://github.com/mblua/AgentsCommander/issues) with:

- Your platform and version (`agentscommander --version`)
- The exact error message
- A copy of `<config-dir>/app.log` from the configuration directory selected by that exact version (see [Log filtering](reference/log-filtering.md#where-logs-go))
