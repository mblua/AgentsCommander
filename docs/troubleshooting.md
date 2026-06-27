# Troubleshooting

For developers hitting an error. Skim the headings for the symptom that matches yours.

## Installation

### Windows: SmartScreen blocks the installer

The Windows release is signed by SignPath, but a freshly-released asset may not have reputation yet. Click **More info → Run anyway**. To verify the signature first:

```powershell
Get-AuthenticodeSignature "Agents Commander_X.Y.Z_x64-setup.exe"
```

`Status` should read `Valid`. See [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md).

### Linux: `.AppImage` will not execute

```bash
chmod +x agentscommander_*_amd64.AppImage
./agentscommander_*_amd64.AppImage
```

If the AppImage fails to start with `error while loading shared libraries: libwebkit2gtk-4.1.so.0`, install the WebKit runtime:

```bash
sudo apt install libwebkit2gtk-4.1-0 libappindicator3-1 librsvg2-2
```

### macOS: "AgentsCommander cannot be opened"

macOS releases are unsigned for now (see [issue #320](https://github.com/mblua/AgentsCommander/issues/320)). Allow the app once via System Settings → Privacy & Security, then it launches normally.

We are looking for macOS testers. If you can help reproduce or fix macOS-specific issues, comment on [#320](https://github.com/mblua/AgentsCommander/issues/320).

## Coding-agent detection

### "No coding agents detected"

AC scans `PATH` for `claude`, `codex`, and `gemini` executables. Verify each one is on PATH:

```bash
where claude     # Windows
which codex      # Linux/macOS
which gemini
```

If the binary is installed but not on PATH, either add it or edit `settings.json` and point the `command` field of the matching entry under `agents` at the full path. See [Settings reference](reference/settings.md).

### Claude Code wrapper not detected

AC matches by **executable basename prefix**: `claude`, `codex`, `gemini`. Wrapper binaries like `claude-mb` or `codex-foo` match because the prefix wins. A wrapper named something completely different (`my-llm`) will be treated as a plain shell — the session works, but agent-specific behavior (resume tokens, idle tuning) is skipped.

## Sessions

### Session is "running" but the agent looks idle

AC marks a session **idle** after 2.5 seconds of PTY silence. If your agent prints a spinner or progress bar, the silence detector may keep it in **running** state. This is intentional — false idles are worse than false runnings.

### Detached window shows nothing

Pop the session back in (right-click → **Reattach**) and pop it out again. xterm.js loses its WebGL context if the host GPU driver resets.

## Inter-agent messaging

### `send: filename contains path separators or traversal`

`--send` takes a **filename only**. Do not pass an absolute path. The CLI resolves the filename against `<workgroup-root>/messaging/`:

```bash
# bad
agentscommander send --send "C:/.../messaging/20260527-150000-wg1-foo-to-wg1-bar-hello.md" ...

# good
agentscommander send --send "20260527-150000-wg1-foo-to-wg1-bar-hello.md" ...
```

### `routing rejected`

The sender does not share a team with the recipient, or the sender is not a coordinator. Run `list-peers-lean` from the sender's directory to see who is reachable:

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
RUST_LOG=agentscommander_lib=trace agentscommander.exe
```

See [Log filtering](reference/log-filtering.md) for the five levels, the live selector, and precedence rules.

---

Still stuck? Open an [issue](https://github.com/mblua/AgentsCommander/issues) with:

- Your platform and version (`agentscommander --version`)
- The exact error message
- A copy of `<config-dir>/app.log` (the persistent log next to the binary; see [Log filtering](reference/log-filtering.md#where-logs-go))
