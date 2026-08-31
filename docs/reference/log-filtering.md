# Log filtering

For developers debugging AgentsCommander or chasing an intermittent issue. How AC sets its log verbosity, how to change it live, and the one escape hatch for fine-grained filters.

AC has one log verbosity setting, `logLevel`, with five levels. You pick a level in Settings or in `settings.json`, and the change applies immediately with no restart. For ad-hoc, per-module filtering from a terminal, the `RUST_LOG` environment variable still works as an escape hatch.

## The five levels

`logLevel` is a single level, not a filter expression. Pick one of:

| Level | Shows |
|---|---|
| `error` | Errors only. |
| `warn` | Warnings and errors. |
| `info` | Normal operational logs. The default. |
| `debug` | Verbose detail for diagnosis. |
| `trace` | Everything, including hot-path noise. |

Each level includes the ones above it: `debug` shows `debug`, `info`, `warn`, and `error`. The level applies to AC's own modules (`agentscommander*`). Third-party crates are pinned at `warn` regardless, so raising AC to `debug` does not flood you with dependency logs.

Anything that is not one of the five names (an empty value, a typo, a legacy filter string like `agentscommander=debug`, or no setting at all) falls back to `info`. An invalid value never silences AC.

## Setting the level live

The level applies the moment you change it. No restart.

**From Settings.** Open **Settings -> General -> Logging** and choose a level from the **Log level** dropdown (Error, Warn, Info, Debug, Trace). AC applies it to the running backend and every open window immediately.

**From `settings.json`.** Set `logLevel` to one of the five names:

```json
{
  "logLevel": "debug"
}
```

See the [settings reference](settings.md#logging) for the file path. If you edit the file while AC is running, reload settings or use the Settings UI, so the change is picked up and not clobbered by the next in-memory save.

To return to the default, set `"logLevel": "info"` (or `null`, or delete the field).

> **One value, not a filter.** A value like `"agentscommander=debug"` is a legacy filter expression, not a level name, so it falls back to `info` and does nothing. Use `"debug"`.

## Downgrade, not delete

Some lines that used to print at `info` were moved **down** to `debug` or `trace` rather than removed. They were too noisy for normal operation but still useful when diagnosing a problem. If the `info` logs are not enough, switch to `debug` (or `trace`) to bring the quieter lines back. Nothing was deleted; it was just made quieter by default.

## The `RUST_LOG` escape hatch

For fine-grained, per-module filtering from a terminal, set the `RUST_LOG` environment variable before launching AC. It uses standard [`env_logger`](https://docs.rs/env_logger/) filter syntax and lets you target individual modules:

```bash
RUST_LOG=info,agentscommander_lib::config::teams=trace agentscommander.exe
```

You should see normal log lines on stderr, plus trace output from the one module you targeted.

The target `agentscommander` is a broad prefix: by env_logger's prefix matching it covers every AC module (all `agentscommander_lib::*`), so `RUST_LOG=agentscommander=trace` raises everything AC logs. Add `::<module>` to scope it to a single subsystem, as in the example above.

`RUST_LOG` takes precedence over `logLevel`, and it behaves differently in two ways:

- It is a full filter expression, not a single level: the same per-module syntax env_logger has always used.
- It **freezes** the live selector. While `RUST_LOG` is set, the Settings level picker cannot change the backend verbosity; AC stays on the `RUST_LOG` filter until you restart without it. The picker still adjusts the in-app console, and AC logs a debug line noting the backend stayed frozen.

Use `RUST_LOG` for a one-off terminal debugging session. Use `logLevel` for a persistent level that survives restarts and works when you launch the GUI from Explorer with no terminal.

**PowerShell:**

```powershell
$env:RUST_LOG="info,agentscommander_lib::pty=trace"; .\agentscommander.exe
```

**cmd.exe:**

```cmd
set RUST_LOG=info,agentscommander_lib::pty=trace && agentscommander.exe
```

> **Malformed `RUST_LOG` filters can suppress logs.** Unlike `logLevel`, which always falls back to `info`, a `RUST_LOG` string that does not parse as a valid `env_logger` expression (a typo, an unrecognized level keyword, a single `:` instead of `::`) can leave AC silent. Verify a candidate filter once by running from a terminal: if AC prints log lines, it parses; if AC is silent, fix the filter before you rely on it.

## Where logs go

| Channel | Destination |
|---|---|
| On-disk log file | `<config-dir>/app.log`, the persistent log. Rotated at 50 MB, keeping up to six files (`app.log` plus `app.log.1` through `app.log.5`). |
| Live runtime logs | stderr of the AC process (visible if you launched from a terminal) |
| In-app console capture | The DevTools console plus a rolling 500-entry in-memory buffer |
| Debug logs export | `<config-dir>/debug-logs.txt`, written by the `save_debug_logs` IPC command with caller-supplied content (a console snapshot export). Not written automatically. |

`<config-dir>` is the active runtime-selected configuration directory, the same directory as `settings.json` (see the [settings reference](settings.md#file-location)). Attach `app.log` to any bug report.

## See also

- [Settings reference: `logLevel`](settings.md#logging) - the field schema
- [`env_logger` filter syntax](https://docs.rs/env_logger/latest/env_logger/#filtering-events) - upstream `RUST_LOG` docs
