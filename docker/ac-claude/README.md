# AgentsCommander Claude-ready container

This image extends the bridge-only image built from `crates/session-bridge/Dockerfile`.
It adds Node.js and the official Claude Code npm package, then copies the existing
`session-bridge` and `agentscommander-api-helper` binaries into `/usr/local/bin`.

No Anthropic credential is baked into the image. Pass authentication at runtime
through `settings.agents[].envs`; AgentsCommander forwards enabled env rows to
the child CLI inside the container.

Official references:

- [Claude Code IAM and authentication](https://code.claude.com/docs/en/iam)
- [Claude Code dev containers](https://code.claude.com/docs/en/devcontainer)

## Build

Run both commands from the repository root. The first command builds the bridge
image by using the existing multi-stage Dockerfile. The second command layers
Claude Code on top of that image.

```bash
docker build -f crates/session-bridge/Dockerfile -t agentscommander/session-bridge:latest .
docker build -f docker/ac-claude/Dockerfile -t agentscommander/ac-claude-ready:local .
```

To reuse a different bridge image tag:

```bash
docker build -f docker/ac-claude/Dockerfile \
  --build-arg BRIDGE_IMAGE=agentscommander/session-bridge:my-tag \
  -t agentscommander/ac-claude-ready:local .
```

## Use from AgentsCommander

Set `AGENTSCOMMANDER_CONTAINER_IMAGE` before launching AgentsCommander:

```powershell
$env:AGENTSCOMMANDER_CONTAINER_IMAGE = "agentscommander/ac-claude-ready:local"
.\agentscommander.exe
```

Use the container backend for the Claude agent and keep the command as `claude`:

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

## Verify

Check the required binaries and Claude Code:

```bash
docker run --rm --entrypoint /bin/sh agentscommander/ac-claude-ready:local \
  -lc 'test -x /usr/local/bin/session-bridge && test -x /usr/local/bin/agentscommander-api-helper && claude --version'
```

Check that no Claude credential was baked into the image:

```bash
docker history --no-trunc agentscommander/ac-claude-ready:local | grep -Ei 'ANTHROPIC|CLAUDE_CODE_OAUTH_TOKEN|API_KEY|AUTH_TOKEN' || true
docker run --rm --entrypoint /bin/sh agentscommander/ac-claude-ready:local \
  -lc 'env | grep -Ei "ANTHROPIC|CLAUDE_CODE_OAUTH_TOKEN|API_KEY|AUTH_TOKEN" || true'
```
