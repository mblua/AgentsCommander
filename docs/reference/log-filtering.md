# Log filtering

For developers debugging AgentsCommander or chasing an intermittent issue. How AC resolves its runtime log filter and how to set it.

## Resolution chain

AC picks its filter at startup from this chain (first match wins):

1. **`RUST_LOG` environment variable:** used as the filter expression. Preferred for ad-hoc debugging from a terminal.
2. **`settings.logLevel` in `~/.agentscommander*/settings.json`:** used as the filter expression. Persistent across restarts and survives Windows GUI launches (shortcut, double-click).
3. **Default**: `agentscommander=info`.

Filter expressions follow standard [`env_logger`](https://docs.rs/env_logger/) syntax. Examples:

| Filter | What you get |
|---|---|
| `info` | Info-level logs from every crate AC depends on. Loud. |
| `agentscommander=info` | Default. Info+ from AC modules only. |
| `agentscommander=debug` | Debug+ from every AC module. |
| `info,agentscommander_lib::config::teams=trace` | Info baseline + trace for one module. |
| `warn,agentscommander_lib::pty=trace,agentscommander_lib::telegram=trace` | Warnings everywhere + trace from two subsystems. |

## Setting `RUST_LOG`

**Bash / zsh:**

```bash
RUST_LOG=agentscommander=debug agentscommander.exe
```

**PowerShell:**

```powershell
$env:RUST_LOG="agentscommander=debug"; .\agentscommander.exe
```

**One-shot from cmd.exe:**

```cmd
set RUST_LOG=agentscommander=debug && agentscommander.exe
```

## Setting `settings.logLevel`

Open `settings.json` (see [Settings reference](settings.md) for the path), add:

```json
{
  "logLevel": "agentscommander=debug"
}
```

Save, restart AC. The setting persists across launches; useful when you double-click the GUI from Explorer and have no terminal to set `RUST_LOG`.

To revert, set `"logLevel": null` (or delete the field) and restart.

## Where logs go

| Channel | Destination |
|---|---|
| Live runtime logs | stderr of the AC process (visible if you launched from a terminal) |
| In-app console capture | The DevTools console plus a rolling 500-entry buffer in memory |
| Exported logs | **Help → Save debug logs** writes `~/.agentscommander/debug-logs.txt` |

Attach `debug-logs.txt` to any bug report.

## ⚠ Malformed filters silently suppress logs

If the filter does not parse as a valid `env_logger` expression (a typo, an unrecognized level keyword, a single `:` instead of `::`), `env_logger` produces no directives for AC's targets and **every** `agentscommander*` log line is suppressed at runtime.

This is the same behavior the binary had before settings-level filter support landed. It will bite you the same way.

**Verify any filter once before persisting it** by running AC from a terminal:

```bash
RUST_LOG="<your-candidate-filter>" agentscommander.exe
```

If AC prints normal log lines, the filter parses. If AC is silent, the filter is malformed; fix it before pasting into `settings.json`.

## Future work

- Phase 2 of [issue #93](https://github.com/mblua/AgentsCommander/issues/93) (if shipped) will expose the filter in the Settings UI as a dropdown.
- Phase 3 will move to live reload via `tracing-subscriber` so changes take effect without a restart.

Neither phase has shipped yet. Phase 1 (settings-level filter + `RUST_LOG` override) is the current state.

## See also

- [Settings reference: `logLevel`](settings.md)
- [`env_logger` filter syntax](https://docs.rs/env_logger/latest/env_logger/#filtering-events): upstream docs
- [Issue #93](https://github.com/mblua/AgentsCommander/issues/93): the umbrella issue
