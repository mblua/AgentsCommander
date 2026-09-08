# Use codebase-memory-mcp with Claude Code

For Claude Code users who want AgentsCommander to pass a codebase-memory MCP configuration into each local launch.

[`codebase-memory-mcp`](https://github.com/DeusData/codebase-memory-mcp) is third-party software from DeusData. It indexes a codebase into a persistent knowledge graph and gives the coding agent queries for structure, call paths, and changes. It is not part of AgentsCommander; this page does not endorse it or provide support for it.

## Understand the boundary

AgentsCommander starts `claude`; Claude Code speaks MCP to the configured server. AgentsCommander does not expose or consume MCP itself.

## Install on Windows

The upstream project provides these Windows installation routes. They all expose the binary as `codebase-memory-mcp`:

| Route | Upstream instructions or manifest |
|---|---|
| PowerShell installer | [Windows quick start](https://github.com/DeusData/codebase-memory-mcp#quick-start) |
| npm | [npm wrapper](https://github.com/DeusData/codebase-memory-mcp/blob/main/pkg/npm/README.md#installation) |
| PyPI | [Python wrapper](https://github.com/DeusData/codebase-memory-mcp/blob/main/pkg/pypi/README.md#installation) |
| Scoop | [Scoop manifest](https://github.com/DeusData/codebase-memory-mcp/blob/main/pkg/scoop/codebase-memory-mcp.json) |
| Winget | [Package ID `DeusData.CodebaseMemoryMcp`](https://github.com/DeusData/codebase-memory-mcp/tree/main/pkg/winget/manifests/d/DeusData/CodebaseMemoryMcp) |
| Chocolatey | [Package ID `codebase-memory-mcp`](https://github.com/DeusData/codebase-memory-mcp/blob/main/pkg/chocolatey/codebase-memory-mcp.nuspec) |

Follow one upstream route, then verify what Windows resolves:

```powershell
Get-Command codebase-memory-mcp | Select-Object Name, CommandType, Source
codebase-memory-mcp --version
```

For a usable installation, `Get-Command` prints the resolved executable or package-manager shim and the version command exits 0 with the installed version.

## Choose how Claude Code loads the server

For an AgentsCommander profile, use these forms in this order.

### 1. Load a JSON file from the profile

This form keeps `--strict-mcp-config` isolation and avoids putting JSON quotes in the profile. Save this JSON as `codebase-memory-mcp.json` in each replica root that uses the profile:

```json
{
  "mcpServers": {
    "codebase-memory-mcp": {
      "command": "codebase-memory-mcp",
      "args": []
    }
  }
}
```

Put only these parameters in the profile cell:

```text
--mcp-config %AC_REPLICA_ROOT%/codebase-memory-mcp.json --strict-mcp-config
```

`--mcp-config` accepts several values and consumes following bare words. The next token must therefore be another `--flag`; here, `--strict-mcp-config` ends that run.

`%AC_REPLICA_ROOT%` is valid here because it occupies a separate path token. If the configured default shell is `cmd.exe`, its expanded path must not contain whitespace or `cmd.exe` metacharacters; a quoted path still becomes one token containing whitespace, which that adapter rejects.

### 2. Configure Claude Code once

Let the upstream installer or Claude Code's own configuration workflow register the server, then pass no MCP parameters in the profile cell. This avoids JSON quoting on every host-shell path, but you give up `--strict-mcp-config` isolation from other configured MCP servers.

### 3. Pass inline JSON

The maintainer uses this working command when starting Claude Code directly:

```powershell
claude --strict-mcp-config --mcp-config '{"mcpServers":{"codebase-memory-mcp":{"type":"stdio","command":"codebase-memory-mcp","args":[],"env":{}}}}' --dangerously-skip-permissions --model claude-opus-5 --effort high
```

It opens an interactive Claude Code session. `--dangerously-skip-permissions` bypasses permission checks; remove it unless you deliberately want that behavior.

For an AgentsCommander profile, remove the leading `claude` because the cell contains parameters, not the binary. A focused inline cell is:

```text
--mcp-config '{"mcpServers":{"codebase-memory-mcp":{"type":"stdio","command":"codebase-memory-mcp","args":[],"env":{}}}}' --strict-mcp-config
```

AgentsCommander's tokenizer treats single and double quotes as equivalent grouping characters, so the single quotes are valid. The later host-shell adapter still decides whether inline JSON reaches `claude`; see [Windows launch failures](#windows-launch-failures).

## Add the AgentsCommander profile

First check whether Windows resolves Claude Code to a native executable or a batch shim:

```powershell
Get-Command claude | Select-Object Name, CommandType, Source
```

A native install reports `claude.exe` with command type `Application`. If the source ends in `.cmd`, do not use inline JSON in a PowerShell-hosted profile; it produces the silent failure described below.

Then configure a profile:

1. Open **Settings -> Coding Agents** and find the `claude` row.
2. Add or edit a profile cell.
3. Keep `claude` as the coding agent's base command.
4. Set the cell's params to `--mcp-config %AC_REPLICA_ROOT%/codebase-memory-mcp.json --strict-mcp-config`.
5. Leave the binary out of the cell, save, and check the launch preview.

The concrete cell is therefore:

| Setting | Value |
|---|---|
| Coding-agent base command | `claude` |
| Profile-cell params | `--mcp-config %AC_REPLICA_ROOT%/codebase-memory-mcp.json --strict-mcp-config` |
| Effective command preview | `claude --mcp-config %AC_REPLICA_ROOT%/codebase-memory-mcp.json --strict-mcp-config` |

AgentsCommander trims both parts and joins them with one space. Assign the profile to one replica or select it for one launch as described in [Coding Agent Profiles](../features/coding-agent-profiles.md).

## Keep MCP sources isolated

`--strict-mcp-config` tells Claude Code to use only servers supplied by `--mcp-config` and ignore its other MCP configuration sources, which is why the examples pair the flags.

## Set the cache directory from the profile

Profile-cell environment variables reach the child process and override a same-named variable from the coding agent's base environment. Add `CBM_CACHE_DIR` when you want to choose where codebase-memory stores indexes:

| Use | `CBM_CACHE_DIR` value |
|---|---|
| One cache shared by concurrent replicas | `%AC_WORKSPACE_ROOT%/project-shared/cbm-cache` |
| A replica-specific cache used one at a time | `%AC_REPLICA_ROOT%/.cache/codebase-memory-mcp` |

Current upstream behavior allows only one canonical cache root per account while any codebase-memory process is active. Point concurrent replicas at the same root; close all active codebase-memory sessions and commands before switching to another value. See the upstream [environment-variable reference](https://github.com/DeusData/codebase-memory-mcp/blob/main/docs/CONFIGURATION.md#4-environment-variables).

## Keep profile parameters on resume

When AgentsCommander resumes Claude Code, it appends `--continue` after every profile parameter; it does not replace the profile parameters. On a fresh launch it may append `--session-id <uuid>` instead. Both injected options start with `--`, so either safely terminates the variadic `--mcp-config` value list.

Claude detection comes from the base `claude` program token and remains active with either MCP form.

## Know the limits

### Windows launch failures

Inline JSON behaves differently after AgentsCommander hands the parsed arguments to the configured default shell:

| Default shell and Claude binary | Inline JSON result |
|---|---|
| PowerShell or pwsh with native `claude.exe` | Works |
| PowerShell or pwsh with an npm `.cmd` shim | **Exits 1 with a blank pane and no message** |
| `cmd.exe` | Rejected before PTY creation with `adapter_error` |
| Git Bash | Works |

The blank-pane case is easy to misdiagnose: PowerShell found Claude, but AgentsCommander's batch branch refuses an argument containing JSON quotes and exits without child output. Use the JSON-file form or configure Claude Code once. Changing the outer single quotes to double quotes does not help because AgentsCommander's tokenizer produces the same logical argument.

### Configuration traps

- Do not put `%AC_REPLICA_ROOT%` inside an inline JSON string. Command-argument expansion produces Windows backslashes, making that JSON invalid; Claude Code can then report the misleading `MCP config file not found: ...`. An AgentsCommander placeholder is valid as the separate `--mcp-config` path token when its expanded path also satisfies the host-shell rules.
- Do not put `%USERPROFILE%` or any other unknown `%...%` pair in the cell. AgentsCommander rejects it with `unknown placeholder marker in value` before launch.
- Keep profile parameters well below about 10 KB on Windows. The generated PowerShell script expands the data and approaches Windows' 32,767-character command limit; use a JSON file for a large multi-server configuration. AgentsCommander has no separate length cap here.
- This evidence covers local-process launches only. This page makes no claim about the container runtime.

### What was verified

- The maintainer ran the full inline command above directly with Claude Code and confirmed it starts correctly.
- The AgentsCommander team ran its tokenizer and local PowerShell/Git Bash argument adapters and confirmed the intact JSON argument reaches the real `claude.exe` on the supported paths in the table.
- Nobody on the team launched an AgentsCommander session for this issue and observed codebase-memory tools inside that pane. The profile recipe documents argument and environment delivery, not that unobserved end-to-end result.

## See also

- [Coding Agent Profiles](../features/coding-agent-profiles.md) - how profile cells compose parameters and environment variables
- [Coding agents](coding-agents.md) - Claude Code detection, auto-resume, and host-managed state
