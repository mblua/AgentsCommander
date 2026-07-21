# Coding agents

For developers configuring which coding-agent CLIs AgentsCommander launches and how. Covers Claude Code, Codex, Gemini, Pi Coding Agent, the Agents Agency role-template picker, and adding your own custom agent.

AgentsCommander is **not** a coding agent. It spawns coding-agent processes and routes between them. You bring the CLIs; AC commands them.

## Supported coding agents

| Coding agent | Binary | Resume tokens AC injects | Notes |
|---|---|---|---|
| **Claude Code** | `claude` (or wrappers like `claude-mb`) | `--continue` | Anthropic's official CLI. |
| **Codex** | `codex` | `resume --last` | OpenAI's coding agent CLI. |
| **Gemini** | `gemini` | `--resume latest` | Google's CLI. |
| **Pi Coding Agent** | `pi`, `pi.exe`, or `pi.cmd` in a supported command position | `--continue` | Earendil Works' coding agent CLI. |

> **OpenCode** runs today through the custom coding-agent path (see [Adding a custom coding agent](#adding-a-custom-coding-agent)). OpenCode is provider-agnostic, so you can point it at any provider or model, including OpenRouter Fusion. It does not yet have a first-class tuned integration (resume tokens, idle tuning); that work is tracked as [#315](https://github.com/mblua/AgentsCommander/issues/315).

## How AC identifies a tuned integration

AC applies an exact Pi command-position pass before its legacy provider detector. That pass has three outcomes:

1. **Supported Pi command:** The direct executable leaf is exactly `pi`, `pi.exe`, or `pi.cmd`, compared case-insensitively, or Pi is the first command under `cmd`/`cmd.exe` with `/C` or `/K` as the first argument. Full, UNC, and Windows verbatim paths work. AC supports tokenized cmd arguments and one embedded command string. Pi may be the first segment of a supported compound command; AC inspects and mutates only that segment.
2. **Genuinely non-Pi command:** AC runs the legacy detector. It scans the shell and whitespace-separated argument tokens by executable basename prefix, with precedence Claude > Codex > Gemini. This preserves wrappers such as `claude-foo`, `codex-bar`, and `gemini-bar`.
3. **Malformed or unsupported Pi-shaped command:** AC fails closed with no coding-agent kind and does not run the legacy detector. Examples include `pi.md`, `pi.bat`, `npx pi`, `call pi`, `start pi`, `/S /C pi`, grouped Pi, Pi after a compound separator, `pi>out`, an unclosed cmd quote, a dangling outside-quote caret, or NUL/CR/LF in parsed cmd text. A later `--model claude-*` or similar value cannot reclassify that command.

AC treats the runtime shell as an already-decoded executable value. It does not trim it or remove literal quotes; configuration parsing removes syntactic outer quotes once before detection. For a supported embedded cmd string, AC splices after the raw executable range and preserves the remaining quotes, carets, whitespace, metacharacters, redirection, and later command bytes. Tokenized cmd arguments support standalone separator elements, but an attached unescaped `&` or `|` in a tokenized Pi segment is unsupported.

Pi aliases and arbitrary wrappers such as `my-pi` are not inferred. Prefix lookalikes such as `pip`, `pipx`, `ping`, and `pixel`, plus ordinary values such as `echo pi`, are genuine non-Pi shapes. PowerShell command text and environment-expanded executable names are also outside the supported Pi command shapes. Use an exact Pi executable, directly or as the first command in the supported cmd forms.

The full enum is in `session/profile.rs::CodingAgentKind`.

### Logical clear capability is separate from tuned integration

Remote logical clear and canonical text submission use an operation-specific direct-shell capability table, independent of `CodingAgentKind`. Direct Claude/Codex/Gemini-family shells and Cursor exact stem `agent` map logical clear to `/clear`. An exact-stem direct Pi shell maps it to `/new` and uses the same delayed double-Enter submission timing. Pi compact and Pi-origin `self-handoff-and-switch` remain unsupported.

Pi is a tuned `CodingAgentKind` for the auto-resume, profile, and wire behavior described below, but that identity does not authorize logical PTY actions. The logical-clear rule is lexical trusted configuration: a direct shell whose final file stem is exactly `pi` matches, including `pi`, `pi.exe`, and the stock `pi.cmd` shim; file-stem extraction discards any final extension. An outer `cmd`/`pwsh` wrapper does not match this operation-specific rule, even when tuned Pi detection supports its command shape. This is not binary attestation. AgentsCommander does not version-probe or semantically acknowledge production clears; stock Pi 0.80.10 is the validated control, and a successful action records PTY write receipt.

## Installing the CLIs

AC does not install the coding-agent binaries. Use the upstream installers:

- **Claude Code:** [docs.claude.com/en/docs/claude-code](https://docs.claude.com/en/docs/claude-code)
- **Codex:** [github.com/openai/codex](https://github.com/openai/codex)
- **Gemini:** [github.com/google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli)
- **Pi Coding Agent:** [github.com/earendil-works/pi](https://github.com/earendil-works/pi)

Install Pi with npm:

```bash
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

Or use the upstream installer:

```bash
curl -fsSL https://pi.dev/install.sh | sh
```

Run `pi --help` to verify the install. It exits successfully and lists `--continue, -c` as `Continue previous session`.

After installation, each CLI handles its own authentication (login flow, API key, or both). Pi auto-resume does not touch credentials or CLI-managed state. The separate generic config-seed feature runs only when configured. Container credential copy-in is Claude Code only today; see [Container coding agents](../features/container-coding-agents.md).

## How AC finds them

On startup AC reads `settings.json → agents[]`. Each entry has:

```json
{
  "id": "claude",
  "label": "Claude Code",
  "command": "claude",
  "color": "#E87B35"
}
```

| Field | Meaning |
|---|---|
| `id` | Stable internal id used by `create-agent --launch <id>`. |
| `label` | Display name in the launcher dropdown. |
| `command` | The binary to spawn. Resolved against `PATH` unless absolute. |
| `color` | Sidebar accent color for sessions launched with this agent. |

The default coding-agent catalog includes a Pi entry with command `pi` and instructions file `AGENTS.md`.

## Switching the coding agent per session

When you launch a session AC shows a dropdown listing every entry in `agents[]`. Pick one. The choice is remembered as the session's `lastCodingAgent` so subsequent wakeups use the same CLI without asking.

You can change the choice for a session later: right-click → **Launch with…** → pick a different agent.

## Pi resume behavior

AC uses Pi's direct `--continue` option, not `--resume`, which opens Pi's interactive session selector. AC injects one `--continue` immediately after the Pi executable only when all of these conditions hold:

- The exact command-position pass identifies a supported Pi command.
- The launch resolves to a configured Coding Agents entry. A heuristic session label or matching shell basename alone does not authorize mutation.
- AC's final lifecycle decision requests known state rather than a fresh start.
- Pi's first command segment is conversational and contains no lexical user-authored session control.

Resume-intent launches include eligible restores, dormant or closed-session reopens, qualified mailbox wakes, and Loop deliveries. A Loop deliberately requests resume even on a cold spawn; Pi creates a new persisted session when no cwd match exists. Fresh creates, default or explicit fresh restarts, and a coordinator's final fresh override leave the configured command unchanged.

| Configured command | Eligible known-state runtime command |
|---|---|
| `pi --model x` | `pi --continue --model x` |
| `cmd.exe /C "pi --model x&&echo done"` | `cmd.exe /C "pi --continue --model x&&echo done"` |

AC changes only the runtime argv. It does not rewrite or persist the configured recipe, and a second application sees the injected selector and adds nothing. Pi then selects its most recent session for the current working directory and effective session directory. If no matching session exists, Pi creates a new persisted session.

### User options and non-conversation commands win

Pi option and subcommand matching is case-sensitive. AC leaves a known-state command unchanged in either case below:

- The first Pi argument is a management command: `install`, `remove`, `uninstall`, `update`, `list`, or `config`.
- Pi's first command segment contains `--help`, `-h`, `--version`, `-v`, `--export`, or `--list-models`. The conservative `--export=...` and `--list-models=...` forms also veto injection.

AC then checks Pi's first command segment lexically for an explicit session selector or disabler. Whole-token `-c` and `-r` veto injection. The exact long options `--continue`, `--resume`, `--session`, `--session-id`, `--fork`, and `--no-session` also veto it, as do their `--name=value` forms. A selector-looking decoded token anywhere in that segment vetoes, even when it appears as another option's value. Short-option bundles and prefix lookalikes do not match. Selectors in a later compound segment do not control the first Pi command.

Conversational `--print`/`-p`, JSON, and RPC modes remain eligible for known-state continuation.

### Session directories

Pi 0.80.10 accepts the separated spelling `--session-dir <dir>`. This option is not a session selector, so an eligible launch receives `--continue` and preserves the directory and value:

```text
pi --session-dir ./state
pi --continue --session-dir ./state
```

AC also distinguishes `--session-dir=<dir>` from `--session` and applies the ordinary injection policy. It preserves that joined spelling apart from the one insertion, but Pi 0.80.10 rejects the joined form as an unknown option. Use the separated spelling.

### No AC-side Pi state probe or fallback

AC does not inspect `~/.pi`, `~/.pi/agent/`, `PI_CODING_AGENT_SESSION_DIR`, a `--session-dir` path, settings, or session headers. A Pi command containing `--provider claude` or a `claude-*` model also does not run AC's Claude projects-directory probe. Pi owns cwd matching and storage errors.

If Pi cannot continue because of its version, configuration, permissions, storage, or session data, its normal child output reports the error. AC does not remove `--continue`, retry without it, or launch a second process. Pi 0.80.10 is the verified compatibility target; older versions that lack `--continue` fail visibly.

### Pi integration limits

- Telegram uses the generic PTY reader for Pi. AC has no Pi-specific JSONL transcript reader.
- Pi has no Pi-specific config-seed convention or factory seed. The generic [config-seed feature](../features/config-seed.md) still works when you explicitly configure a destination.
- AC does not copy Pi credentials or state into containers, map host `~/.pi/agent/`, translate `PI_CODING_AGENT_SESSION_DIR`, or provision `--session-dir` paths.
- Generated auto-self-clear instructions, explicit-Enter submission, and mailbox logical clear use the separate exact-stem direct-shell capability above. An eligible direct Pi shell receives them when settings allow; tuned outer-`cmd` Pi auto-resume shapes do not. Pi compact remains unsupported.

## Profiles: launch variants per coding agent

Each coding agent can have several **profiles** (lettered launch variants: a cheap one, a max-effort one, an isolated-config one). A profile adds parameters and env vars on top of the agent's base command, and you assign one per agent or per session. This is a separate feature from the tuned `CodingAgentKind` integration above. See [Coding Agent Profiles](../features/coding-agent-profiles.md).

## Role-template picker

When you create a new agent through the UI you can pick a role template. The picker shows two sources:

1. **Agency templates** — read from the validated offline cache at `<config-dir>/agency-agents_templates`, refreshed only by `agency-templates update`.
2. **Local templates** — read from `<config-dir>/agent-templates/<folder>/` (override the path via `settings.agentTemplatesPath`).

Each template provides metadata (name, description, category, accent color) and a markdown role body. AC writes the body into the new agent's `Role.md` and `CLAUDE.md`.

> AC's role-template picker can use a downloaded cache of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). If you author a new role and want it discoverable in AC by default, submit it upstream to the agency-agents catalog, then refresh the cache with `agency-templates update`.

## Adding a custom coding agent

To make AC recognise a new CLI (e.g. a custom wrapper) under the **Coding Agents** dropdown:

1. Open **Settings → Coding Agents → Add agent**.
2. Fill in `id`, `label`, `command`, and accent `color`.
3. Save.

The new entry appears in the launcher dropdown immediately. AC spawns the configured command as-is unless the launch matches a tuned integration. Claude, Codex, and Gemini retain their legacy prefix-wrapper behavior. Pi tuning requires the exact supported command position described above; naming a wrapper or custom row `my-pi` does not enable Pi resume behavior.

For deeper integration (a new `CodingAgentKind` with its own resume tokens and idle tuning), you need to add a variant to `src-tauri/src/session/profile.rs` and rebuild. OpenCode already runs through the custom-agent steps above; its first-class tuned integration is tracked on the [roadmap](../../ROADMAP.md) ([#315](https://github.com/mblua/AgentsCommander/issues/315)) as the canonical example of how a new `CodingAgentKind` is added.

> **"Profile" here does not mean the profile matrix.** A tuned `CodingAgentKind` is how AC drives one CLI (resume tokens, idle tuning). The lettered launch variants (A/B/C) are a separate feature: see [Coding Agent Profiles](../features/coding-agent-profiles.md).

## Authentication and CLI state

AC does not store coding-agent credentials of its own. Each CLI manages its credentials and state on the host:

| CLI | Host-managed state |
|---|---|
| Claude Code | `~/.claude/` |
| Codex | `~/.codex/` |
| Gemini | `~/.gemini/` |
| Pi Coding Agent | `~/.pi/agent/` |

Under the **local-process** runtime, Pi auto-resume never reads, copies, or writes those host credentials or live state. The separate generic config-seed feature can copy a user-configured template into a replica, but Pi has no Pi-specific seed convention or factory seed.

Under the **Container** runtime there is one deliberate exception, on by default. AC copies the Claude Code host credential file (`~/.claude/.credentials.json`) into the replica config dir so the container starts signed in, and deletes it when the session stops. Claude Code is the only supported copy-in provider today. AC does not copy or provision Pi state or credentials. The Claude copy puts a full-account token in plaintext inside the workspace tree and is governed by the `containerCredentialsFromHost` setting. Read [Container coding agents](../features/container-coding-agents.md) and [Security model](../security.md#container-coding-agents-copied-host-credentials) before you rely on it.

## See also

- [Coding Agent Profiles](../features/coding-agent-profiles.md): lettered launch variants (A/B/C) per coding agent
- [Container coding agents](../features/container-coding-agents.md): host login reuse, and why container agents cannot reach repos yet
- [Creating agents](../agents/creating-agents.md) — make a new agent dir
- [Settings reference](../reference/settings.md) — full schema for `agents[]`
- [Roadmap: coding agents](../../ROADMAP.md): OpenCode first-class integration, Nvidia agent, more
