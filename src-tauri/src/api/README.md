# Control-plane API (#791, increment 1)

A local, opt-in HTTP API hosted inside the daemon (a sibling of `web/`) that
lets a machine client (first: a Dockerized coding agent) speak the inter-agent
control-plane over a token instead of the filesystem outbox. The filesystem
messaging path stays fully live; this is a strangler-fig second front door into
the SAME actuation (`deliver_wake`).

## Enabling

Off by default. Set in settings:

- `apiServerEnabled: true`
- `apiServerBind` (default `127.0.0.1`) - widen to a Docker/WSL-reachable
  interface ONLY when serving a container. Any non-loopback bind logs a loud
  startup warning; the token, not the interface, is the trust boundary.
- `apiServerPort` - profile-aware default (`profile::api_server_port`), distinct
  from the web port so dev/prod builds do not collide.

A bind/port change requires a daemon restart (or a future stop/start command).

## Auth

Every request except `healthz` needs `Authorization: Bearer <client-token>`.
Tokens are minted host-side and stored HASHED (SHA-256) in
`api-clients.json` (host-only `config_dir()`, never mounted into a container):

```
agentscommander api-client mint --token <MASTER/ROOT> \
  --root <replica-working-dir> --scopes send,list-peers-lean [--label ..] [--expires <rfc3339>]
agentscommander api-client revoke --token <MASTER/ROOT> --client-id <id>
agentscommander api-client list   --token <MASTER/ROOT>
```

Mint prints the secret ONCE. The registry is read THROUGH to disk on every
request (mtime-gated), so a revoke takes effect on the next request. Identity is
the token's bound replica: `from` is derived at request time from `boundRoot`,
never from the request body. The Root Agent is rejected (mint-time + request-time).
Auth is unconditional in all build profiles (no debug bypass). A per-source-IP
failed-auth lockout throttles unauthenticated probing.

## Endpoints (`/api/v1`)

- `POST /api/v1/send` - mirrors `send --mode wake --send <file>` (no `--command`).
  Body (`deny_unknown_fields`; `from`/`root`/`token`/`command`/`action` rejected):
  ```json
  { "apiVersion": "1", "opId": "<uuid>", "to": "<fqn>",
    "message": { "send": "<bare-filename-in-messaging/>" } }
  ```
  `inline` is reserved and rejected with 400 in v1. `opId` is the idempotency
  key: a replay returns the prior result and never re-delivers (persisted across
  restarts in `api-idempotency.json`). Response: `200` delivered, `422` rejected,
  `400/401/403/429` on client/auth/routing/lockout failures.
- `GET /api/v1/peers[?peer=<fqn>...]` - `list-peers-lean` for the caller's bound
  replica; `reachable` is computed from the bound identity.
- `GET /api/v1/healthz` - unauthenticated liveness, body exactly `{"ok":true}`.

## Container runtime contract

S4 starts the container through the backend Docker runtime. The daemon passes
only scoped bridge configuration and never mounts the host config directory,
the host `messaging/` directory, or the Docker socket.

- `AGENTSCOMMANDER_API_URL=http://host.docker.internal:<apiServerPort>`
- `AGENTSCOMMANDER_API_TOKEN=<the minted client secret>`
- `AGENTSCOMMANDER_SESSION_ID=<uuid>`
- `AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN=<one-time ticket>`
- `AGENTSCOMMANDER_ROOT=<host replica root bound to the token>`

Docker Desktop containers cannot reach a daemon bound only to `127.0.0.1`.
For local container sessions, enable the API server and set `apiServerBind` to
a Docker-reachable interface such as `0.0.0.0`, with host firewall rules limiting
access to the local Docker/WSL subnet.

## Versioning

URL-versioned (`/api/v1`). Additive changes (new optional fields, new verbs)
stay in v1; a breaking change introduces `/api/v2` mounted alongside. The
`apiVersion` envelope field is a redundant explicit echo.

## Audit

Every mint/revoke and authenticated request appends `(ts, clientId, boundFqn,
op, outcome)` to `api-audit.log` (host-only, 10 MB cap + one rotation, never
fails closed). Secrets and hashes are never logged.
