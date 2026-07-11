# AgentsCommander Claude-ready container

This image extends the bridge-only image built from `crates/session-bridge/Dockerfile`.
It adds Node.js and the official Claude Code npm package, then copies the existing
`session-bridge` and `agentscommander-api-helper` binaries into `/usr/local/bin`.

No Anthropic credential is baked into the image. Authentication happens at
runtime, in one of two ways:

- **Host login reuse (the default).** AgentsCommander copies your host
  `~/.claude/.credentials.json` into the container at launch, points
  `CLAUDE_CONFIG_DIR` at it, and deletes it when the session stops. You configure
  nothing; the session starts signed in. See
  [Container coding agents](../../docs/features/container-coding-agents.md).
- **A credential you pass yourself.** Turn host login reuse off
  (`containerCredentialsFromHost: false`) and pass authentication through
  `settings.agents[].envs`; AgentsCommander forwards enabled env rows to the child
  CLI inside the container. The `envs` examples below document that path.

The image runs the bridge as the non-root `node` user. It also configures Git
with `safe.directory=*` because AgentsCommander bind-mounts host workspaces
whose ownership often differs from UID 1000 inside Docker.

Official references:

- [Claude Code IAM and authentication](https://code.claude.com/docs/en/iam)
- [Claude Code dev containers](https://code.claude.com/docs/en/devcontainer)

## Build

Run both commands from the repository root. The first command builds the bridge
image by using the existing multi-stage Dockerfile. The second command layers
Claude Code on top of that image.

```bash
docker build -f crates/session-bridge/Dockerfile -t agentscommander/session-bridge:latest .
docker build -f docker/ac-claude/Dockerfile -t agentscommander/ac-claude:latest .
```

To reuse a different bridge image tag:

```bash
docker build -f docker/ac-claude/Dockerfile \
  --build-arg BRIDGE_IMAGE=agentscommander/session-bridge:my-tag \
  -t agentscommander/ac-claude:latest .
```

## Use from AgentsCommander

Set `AGENTSCOMMANDER_CONTAINER_IMAGE` before launching AgentsCommander:

```powershell
$env:AGENTSCOMMANDER_CONTAINER_IMAGE = "agentscommander/ac-claude:latest"
.\agentscommander.exe
```

Use the container backend for the Claude agent and keep the command as `claude`.
The `envs` row below is needed **only when host login reuse is off**; with the
default on, drop it and AgentsCommander supplies the credential:

```json
{
  "agents": [
    {
      "id": "claude",
      "label": "Claude Code",
      "command": "claude",
      "color": "#E87B35",
      "backend": { "kind": "containerTransport" },
      "envs": [
        {
          "key": "CLAUDE_CODE_OAUTH_TOKEN",
          "value": "paste-token-from-claude-setup-token",
          "source": "user",
          "enabled": true
        }
      ]
    }
  ]
}
```

Use only one primary Claude credential unless your gateway setup requires a
different combination.

- Subscription auth: run `claude setup-token` on a trusted machine, then pass
  the output as `CLAUDE_CODE_OAUTH_TOKEN`.
- API billing: pass `ANTHROPIC_API_KEY` instead.
- Gateway auth: pass `ANTHROPIC_AUTH_TOKEN` and any required base URL settings.

For API billing, the env row is:

```json
{
  "key": "ANTHROPIC_API_KEY",
  "value": "sk-ant-...",
  "source": "user",
  "enabled": true
}
```

Do not place real tokens in this repository or in the Dockerfile. Store them in
the local AgentsCommander settings file or inject them from your shell before
launching AgentsCommander.

## Headless mode

For non-interactive smoke tests or profiles, run Claude with `-p` or `--print`:

```bash
claude -p "Reply with ok"
```

Claude Code skips the workspace trust dialog in non-interactive mode. Avoid
`--bare` when using `CLAUDE_CODE_OAUTH_TOKEN`, because bare mode does not read
OAuth or keychain credentials.

## Verify

Check the required binaries and Claude Code:

```bash
docker run --rm --entrypoint /bin/sh agentscommander/ac-claude:latest \
  -lc 'test -x /usr/local/bin/session-bridge && test -x /usr/local/bin/agentscommander-api-helper && claude --version'
```

Check that no Claude credential was baked into the image:

```bash
docker history --no-trunc agentscommander/ac-claude:latest | grep -Ei 'ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|CLAUDE_CODE_OAUTH_TOKEN|API_KEY=|AUTH_TOKEN=' || true
docker run --rm --entrypoint /bin/sh agentscommander/ac-claude:latest \
  -lc 'env | grep -Ei "ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|CLAUDE_CODE_OAUTH_TOKEN|API_KEY=|AUTH_TOKEN=" || true'
```
