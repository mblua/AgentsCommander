# Coding agents

For developers configuring which coding-agent CLIs AgentsCommander launches and how. Covers Claude Code, Codex, Antigravity, Pi Coding Agent, the Agents Agency role-template picker, and adding your own custom agent.

AgentsCommander is **not** a coding agent. It spawns coding-agent processes and routes between them. You bring the CLIs; AC commands them.

## Supported coding agents

| Coding agent | Binary | Resume tokens AC injects | Notes |
|---|---|---|---|
| **Claude Code** | `claude` (or wrappers like `claude-mb`) | `--continue` | Anthropic's official CLI. |
| **Codex** | `codex` | `resume --last` | OpenAI's coding agent CLI. |
| **Antigravity** | `agy` | `--continue` | Google's agent-first coding CLI. |
| **Pi Coding Agent** | `pi`, `pi.exe`, or `pi.cmd` in a supported command position | `--continue` | Earendil Works' coding agent CLI. |
| **Cursor** | `agent` (catalog entry) | — | Shipped in the default catalog (id `cursor`, instructions `AGENTS.md`). No tuned `CodingAgentKind`: it uses the generic path and the exact-stem logical-clear rule below. |

> **OpenCode** runs today through the custom coding-agent path (see [Adding a custom coding agent](#adding-a-custom-coding-agent)). Configure it with any provider or model OpenCode supports; AgentsCommander launches and coordinates the CLI without restricting that choice. OpenCode does not yet have a first-class tuned integration (resume tokens, idle tuning); that work is tracked as [#315](https://github.com/mblua/AgentsCommander/issues/315).

## How AC identifies a tuned integration

AC applies an exact Pi command-position pass before its legacy provider detector. That pass has three outcomes:

1. **Supported Pi command:** The direct executable leaf is exactly `pi`, `pi.exe`, or `pi.cmd`, compared case-insensitively, or Pi is the first command under `cmd`/`cmd.exe` with `/C` or `/K` as the first argument. Full, UNC, and Windows verbatim paths work. AC supports tokenized cmd arguments and one embedded command string. Pi may be the first segment of a supported compound command; AC inspects and mutates only that segment.
2. **Genuinely non-Pi command:** AC runs the legacy detector. It scans the shell and whitespace-separated argument tokens by executable basename prefix, with precedence Claude > Codex > Antigravity. This preserves wrappers such as `claude-foo` and `codex-bar`. Antigravity matches the exact executable stems `agy` / `agy.exe` / `agy.cmd` / `antigravity` (prefix wrappers are not inferred); Gemini no longer has tuned identity.
3. **Malformed or unsupported Pi-shaped command:** AC fails closed with no coding-agent kind and does not run the legacy detector. Examples include `pi.md`, `pi.bat`, `npx pi`, `call pi`, `start pi`, `/S /C pi`, grouped Pi, Pi after a compound separator, `pi>out`, an unclosed cmd quote, a dangling outside-quote caret, or NUL/CR/LF in parsed cmd text. A later `--model claude-*` or similar value cannot reclassify that command.

AC treats the runtime shell as an already-decoded executable value. It does not trim it or remove literal quotes; configuration parsing removes syntactic outer quotes once before detection. For a supported embedded cmd string, AC splices after the raw executable range and preserves the remaining quotes, carets, whitespace, metacharacters, redirection, and later command bytes. Tokenized cmd arguments support standalone separator elements, but an attached unescaped `&` or `|` in a tokenized Pi segment is unsupported.

Pi aliases and arbitrary wrappers such as `my-pi` are not inferred. Prefix lookalikes such as `pip`, `pipx`, `ping`, and `pixel`, plus ordinary values such as `echo pi`, are genuine non-Pi shapes. PowerShell command text and environment-expanded executable names are also outside the supported Pi command shapes. Use an exact Pi executable, directly or as the first command in the supported cmd forms.

The full enum is in `session/profile.rs::CodingAgentKind`.

### Logical clear capability is separate from tuned integration

Remote logical clear and canonical text submission use an operation-specific direct-shell capability table, independent of `CodingAgentKind`. Direct Claude/Codex/Antigravity-family shells and Cursor exact stem `agent` map logical clear to `/clear`. An exact-stem direct Pi shell maps it to `/new` and uses the same delayed double-Enter submission timing. An exact-stem direct Pi shell is also a supported `self-handoff-and-switch` source. Pi compact remains unsupported.

Pi is a tuned `CodingAgentKind` for the auto-resume, profile, and wire behavior described below, but that identity does not authorize logical PTY actions. The logical-clear rule is lexical trusted configuration: a direct shell whose final file stem is exactly `pi` matches, including `pi`, `pi.exe`, and the stock `pi.cmd` shim; file-stem extraction discards any final extension. An outer `cmd`/`pwsh` wrapper does not match this operation-specific rule, even when tuned Pi detection supports its command shape. This is not binary attestation. AgentsCommander does not version-probe or semantically acknowledge production clears; stock Pi 0.80.10 is the validated control, and a successful action records PTY write receipt.

## Installing the CLIs

AC does not install the coding-agent binaries. Use the upstream installers:

- **Claude Code:** [docs.claude.com/en/docs/claude-code](https://docs.claude.com/en/docs/claude-code)
- **Codex:** [github.com/openai/codex](https://github.com/openai/codex)
- **Antigravity:** [antigravity.google](https://antigravity.google) (Antigravity CLI docs)

Antigravity's recommended flags (`--dangerously-skip-permissions`, `--model <model>`, `--effort <effort>`) are **not** baked into the seeded catalog command: `--dangerously-skip-permissions` disables agy's own permission prompts, and `<model>`/`<effort>` are user placeholders. Add them per agent via the coding-agent profile cells.
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

**Where `updateCommands` lives.** The catalog is a separate manifest from `settings.json`. It is `<project>/.ac/coding-agents/agents.json`, an AC-managed snapshot that startup and project registration initialize or refresh; your overrides live beside it in `<project>/.ac/coding-agents/agents.local.json`. Each catalog definition can carry `updateCommands`, the commands AC runs to update that tool, and `autoUpdate`. **Neither is a `settings.json` key**, and the CLI exposes the catalog read-only (`coding-agent catalog`). What you set in `settings.json` is your answer to the startup prompt, `agentAutoUpdateByCommand`, described in [Coding agent auto-update](../features/agent-auto-update.md); the catalog's `autoUpdate` field is inert.

Commands resolve only from the persisted catalog. AC never substitutes the shipped defaults at read time, so a legacy catalog's absent `updateCommands` stays empty until a supported restart migrates it; `cursor` and `muse` intentionally ship no update command (`cursor`'s CLI self-updates with the desktop app). See [Managed catalog: base, local overrides, and migration](#managed-catalog-base-local-overrides-and-migration) for the schema, refresh and recovery rules.

In the Settings > Coding Agents **Auto-update** table, one row appears per command whose first effective entry has a non-empty `updateCommands` (see [Coding agent auto-update](../features/agent-auto-update.md)). There is no `versionCommand` field: AC detects installed versions with a built-in `--version` probe for the built-in commands (`claude`, `codex`, `hermes`, `pi`, `opencode`, `agy`), only for bare command names resolved through PATH; a custom entry or an explicit path shows `Installed` without a version.

## Managed catalog: base, local overrides, and migration

`<project>/.ac/coding-agents/agents.json` is the file AgentsCommander manages; `<project>/.ac/coding-agents/agents.local.json` is yours. Startup and every project registration initialize or refresh the managed base. Ordinary reads and **Reload catalog** never write. The embedded catalog is seed material only. With no registered project, AC reads an existing instance catalog read-only (`<config_dir>/coding-agents/agents.json`) or reports it unavailable; it never initializes the instance.

**Existing registrations are snapshots.** Adding an agent from the catalog copies `label`, `command`, `color`, `envs`, `isolatedHome`, and, when present, `instructionsFilename` and `configSeed` into `settings.agents[]` (the CLI's `add --from-catalog` does the same). Later catalog changes do not rewrite registered agents.

### The local overrides file

`agents.local.json` is strict. Its root is an object with `schemaVersion: 1`, an `agents` array, and an optional `order` array of unique keys. Anything AC does not recognize — an unknown field at the root, in a row or in a nested object; a duplicate JSON member; a duplicate key; an unsupported `schemaVersion` — disables the **whole** local layer with a `localInvalid` warning naming the path and reason. The base stays readable and usable; AC never applies a partial local file or rewrites it.

A row with an existing `key` patches that base entry. `label`, `description`, `color`, `command`, `instructionsFilename`, `envs`, `isolatedHome`, `configSeed`, `removable`, `updateCommands` and `autoUpdate` are accepted. A field you omit is inherited; `false`, `""` and `[]` are explicit values. `null` is accepted only for `instructionsFilename` (clear it) and `configSeed` (clear the whole object). `configSeed` merges by presence (`enabled`, `dest` only); an object after a missing or `null` base starts from the defaults `enabled: true`, `dest: ""`. `envs` and `updateCommands` replace the whole array in the order you write; env rows are never merged by key. Each env object accepts only `key`, `value`, `source` (`user` or `system`/`agentsCommander`) and `enabled`.

A row with a new `key` must be complete: `label`, `description`, `color`, `command`, `envs`, `isolatedHome`, `removable`, `updateCommands` and `autoUpdate` are all required; `instructionsFilename` and `configSeed` may be absent or `null`. New definitions append after the base entries unless `order` places them.

A tombstone is a row with `remove: true` and only `key` and `remove`. Removing a nonremovable base entry invalidates the whole layer; `remove: false` is rejected — omit `remove` for an ordinary patch. An unknown key's tombstone is valid and kept for a future shipped key.

`order` lists surviving keys first; unlisted survivors keep base-then-local-add order. Unknown or removed keys in `order` are ignored, so you can pre-position a key a future AC version adds. After composition AC applies its built-in support table; an unsupported built-in stays suppressed.

This example patches Codex to an explicit empty command list and a partial seed, removes the Pi entry (Pi is removable), adds a complete custom agent, and pre-positions a future key:

```json
{
  "schemaVersion": 1,
  "agents": [
    {
      "key": "codex",
      "command": "codex --model gpt-5",
      "updateCommands": [],
      "configSeed": { "enabled": false }
    },
    { "key": "pi", "remove": true },
    {
      "key": "my-cli",
      "label": "My CLI",
      "description": "Internal wrapper",
      "color": "#6366f1",
      "command": "my-cli --fast",
      "envs": [],
      "isolatedHome": false,
      "removable": true,
      "updateCommands": ["my-cli self-update"],
      "autoUpdate": false
    }
  ],
  "order": ["codex", "my-cli", "future-tool"]
}
```

When several catalog entries share one command, the updater uses the **first effective entry** for that command — its label, color and exact sequence — even when that sequence is `[]`; later duplicates are ignored. Settings consent stays keyed by the exact command string and is never inherited by a changed command.

### The managed base

A base AC owns carries a `managed` marker beside `schemaVersion: 1` and `agents`:

| Field | Value |
|---|---|
| `owner` | `agentscommander` |
| `version` | `1` |
| `revision` | SHA-256 of the deterministic compact UTF-8 serialization of the supported shipped definitions |
| `contentSha256` | The same SHA-256; identifies the exact content AC published |

An unrecognized owner or version means the file is not AC's: it stays readable but is never refreshed or migrated. A formatting-only edit does not change `contentSha256` and does not pin the file. A semantic edit stops `contentSha256` matching the entries; the file stays readable with a `managedBaseEdited` warning, and AC never auto-refreshes or auto-migrates it. To customize entries, use `agents.local.json`, not `agents.json`.

A refresh replaces only a verified managed base: it can bring new or updated shipped definitions into the composed view you read, and it never edits `agents.local.json`. A fresh project writes the base and a creation-only stub `{"schemaVersion":1,"agents":[]}` in `agents.local.json`, and only when the local path does not exist at all: any existing entry is preserved and never overwritten. An existing valid regular file is composed as your overrides with no warning; a directory, link, unreadable or schema-invalid local is preserved and surfaces `localInvalid` with the path and reason when read. If the stub cannot be created, the usable base remains and a startup log names the local path.

### Migration, sidecars, and recovery

If `agents.json` has no recognized managed marker it is legacy user-owned data. On a supported restart AC migrates it before claiming ownership, and requires `agents.local.json` to be completely absent: an existing local file of any kind blocks the transfer, even an empty stub AC may not have written. A readable legacy base plus a valid local file stays usable with a `migrationPending` warning while the transfer is blocked. A legacy file stays readable at all times — AC does not extract values or insert defaults while reading.

An existing local file does **not** block fresh initialization when there is no project base and no instance legacy source: AC writes the managed base and leaves your local file in place.

The one-time extraction is computed and strictly validated before any write:

- for a shipped key whose command is unchanged, explicitly present fields are pinned as local values; an absent `updateCommands` inherits the persisted default sequence;
- a custom key or a changed command is materialized completely; an absent `updateCommands` becomes `[]`;
- shipped keys absent from the legacy catalog become `remove: true` tombstones (an empty legacy catalog tombstones every supported shipped key), so future AC versions can still add new defaults.

AC then publishes an immutable byte-exact backup `agents.migration-v1.backup.json`, a journal `.agents.migration-v1.json`, the extracted local file, and finally the managed base — the local file before the base. The journal records the version, source kind (`project` or `instance`), source path and SHA-256, the local file's SHA-256 and byte length, and the complete intended managed base with its revision and content digest. A restart resumes an interrupted migration only when the recorded source, backup and local bytes still match; a changed source or local file, a missing backup, or an inconsistent sidecar stops recovery and preserves every byte, with the conflict path and reason reported. An instance source is read-only: only its bytes are imported, into the project's base and backup, and only when the project has no base at all. `agents.local.json` is never imported from the instance.

Warnings keep a failed state visible without changing bytes:

| Warning | Meaning |
|---|---|
| `baseUnavailable` | no readable persisted catalog exists at the selected path; AC substitutes nothing |
| `baseInvalid` | the persisted base is corrupt or invalid; its bytes are preserved |
| `invalidDefinition` | a catalog entry did not validate and was omitted from the read, or a built-in is suppressed by this build's support table |
| `duplicateKey` | a catalog entry's key duplicates an earlier entry and was omitted |
| `localInvalid` | the local file is not valid under the strict schema; the base still applies |
| `migrationPending` | the base is legacy or unmanaged, an entry is missing `updateCommands`, or the base carries unrecognized fields, so a supported restart can migrate it; also a journal or a local layer is waiting for a managed base |
| `migrationConflict` | the base carries an unrecognized managed ownership marker, or a sidecar, source or local file changed during migration, or an existing local/sidecar blocks it; nothing was overwritten |
| `managedBaseEdited` | the base no longer matches its content digest; it stays readable and is never auto-refreshed |
| `refreshFailed` | the persisted revision differs from this build, so the entries stay usable and a restart retries the refresh; initialization also logs it when it cannot create the local stub or inspect a sidecar, without adding it to this report |
| `publicationUntracked` | the base is verified managed but the seed manifest does not record it yet; the next initialization records it without republishing |

The table describes the warnings a read report can carry. A failed initialization logs the same codes, and some failures reach only the log: a blocked recovery logs `migrationConflict`, and a local-stub or sidecar failure logs `refreshFailed`, without appearing in the report.

**Reload catalog** (Settings and the New Agent picker) re-reads the files after you edit them by hand. A restart is what retries the managed-base refresh and migration. The config-seed **Re-seed default configuration** button is unrelated: it writes only the `_seed/` master under the primary registered project's `.ac/coding-agents/`, or the legacy `<config_dir>/coding-agents/_seed/` when no project is registered (see [Config seed](../features/config-seed.md#the-factory-default-and-the-re-seed-button)).

If migration cannot resume, reconcile it by hand:

1. Stop AgentsCommander.
2. Keep `agents.json`, `agents.local.json`, `agents.migration-v1.backup.json` and `.agents.migration-v1.json` — do not delete them.
3. Prepare a valid `agents.local.json` with the customizations you want to keep.
4. Move a conflicting base and transaction sidecars aside to archival names of your choice.
5. Make sure no instance legacy catalog will silently re-enter migration.
6. Restart AC: it initializes a fresh managed base and preserves your reconciled local file.

There is no repair command that deletes or overwrites these files. An unsupported lock or hard-link filesystem stops migration safely instead of publishing a partial local file; move the project to a supported local filesystem or reconcile by hand.

## Switching the coding agent per session

When you launch a session AC shows a dropdown listing every entry in `agents[]`. Pick one. The choice is remembered as the session's `lastCodingAgent` so subsequent wakeups use the same CLI without asking.

You can change the choice for a session later: right-click → **Launch with…** → pick a different agent.

## Pi resume behavior

AC uses Pi's direct `--continue` option, not `--resume`, which opens Pi's interactive session selector. AC injects one `--continue` immediately after the Pi executable only when all of these conditions hold:

- The exact command-position pass identifies a supported Pi command.
- The launch resolves to a configured Coding Agents entry. A heuristic session label or matching shell basename alone does not authorize mutation.
- AC's final lifecycle decision requests known state rather than a fresh start.
- Pi's first command segment is conversational and contains no lexical user-authored session control.

Resume-intent launches include eligible restores, dormant or closed-session reopens, qualified mailbox wakes, and Loop deliveries. A Loop deliberately requests resume even on a cold spawn; Pi creates a new persisted session when no cwd match exists. Fresh creates, default or explicit fresh restarts, and an orchestrator's final fresh override leave the configured command unchanged.

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

Each template provides metadata (name, description, category, accent color) and a markdown role body. AC writes the body into the new agent's `Role.md`; the coding-agent role file (`CLAUDE.md` or `AGENTS.md`) is materialized from `Role.md` plus AC context at launch.

> AC's role-template picker can use a downloaded cache of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). If you author a new role and want it discoverable in AC by default, submit it upstream to the agency-agents catalog, then refresh the cache with `agency-templates update`.

## Adding a custom coding agent

To make AC recognise a new CLI (e.g. a custom wrapper) under the **Coding Agents** dropdown:

1. Open **Settings → Coding Agents → Add agent**.
2. Fill in `id`, `label`, `command`, and accent `color`.
3. Save.

The new entry appears in the launcher dropdown immediately. AC spawns the configured command as-is unless the launch matches a tuned integration. Claude and Codex retain their legacy prefix-wrapper behavior; Antigravity matches exact stems. Pi tuning requires the exact supported command position described above; naming a wrapper or custom row `my-pi` does not enable Pi resume behavior.

For deeper integration (a new `CodingAgentKind` with its own resume tokens and idle tuning), you need to add a variant to `src-tauri/src/session/profile.rs` and rebuild. OpenCode already runs through the custom-agent steps above; its first-class tuned integration is tracked on the [roadmap](../../ROADMAP.md) ([#315](https://github.com/mblua/AgentsCommander/issues/315)) as the canonical example of how a new `CodingAgentKind` is added.

> **"Profile" here does not mean the profile matrix.** A tuned `CodingAgentKind` is how AC drives one CLI (resume tokens, idle tuning). The lettered launch variants (A/B/C) are a separate feature: see [Coding Agent Profiles](../features/coding-agent-profiles.md).

## Authentication and CLI state

AC does not store coding-agent credentials of its own. Each CLI manages its credentials and state on the host:

| CLI | Host-managed state |
|---|---|
| Claude Code | `~/.claude/` |
| Codex | `~/.codex/` |
| Pi Coding Agent | `~/.pi/agent/` |

Under the **local-process** runtime, Pi auto-resume never reads, copies, or writes those host credentials or live state. The separate generic config-seed feature can copy a user-configured template into a replica, but Pi has no Pi-specific seed convention or factory seed.

Under the **Container** runtime there is one deliberate exception, on by default. AC copies the Claude Code host credential file (`~/.claude/.credentials.json`) into the replica config dir so the container starts signed in, and deletes it when the session stops. Claude Code is the only supported copy-in provider today. AC does not copy or provision Pi state or credentials. The Claude copy puts a full-account token in plaintext inside the project tree and is governed by the `containerCredentialsFromHost` setting. Read [Container coding agents](../features/container-coding-agents.md) and [Security model](../security.md#container-coding-agents-copied-host-credentials) before you rely on it.

## See also

- [codebase-memory-mcp with Claude Code](codebase-memory-mcp.md): pass file-based or inline MCP configuration through a Claude Code profile
- [Coding Agent Profiles](../features/coding-agent-profiles.md): lettered launch variants (A/B/C) per coding agent
- [Container coding agents](../features/container-coding-agents.md): host login reuse, and how a container mounts that agent's work repos
- [Creating agents](../agents/creating-agents.md) — make a new agent dir
- [Settings reference](../reference/settings.md) — full schema for `agents[]`
- [Roadmap: coding agents](../../ROADMAP.md): OpenCode first-class integration, Nvidia agent, more
