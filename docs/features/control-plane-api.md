# Control-plane API

For developers running a containerized coding agent, or any machine client that has to speak the inter-agent control plane over HTTP instead of the filesystem. After this page you can turn the API on, mint a scoped client token, and know which endpoint does what and what each one refuses.

The control-plane API is a local HTTP server hosted inside the AC daemon. It lets a machine client send messages, list its peers, attach a session transport, and, with the right live binding, actuate PTY input or read a terminal snapshot. It is **off by default**: no listening socket appears unless you opt in.

## What it is

The API is a sibling of the web server, not the same thing. The web server serves the AgentsCommander interface to a human in a browser; the control-plane API serves a program. They have separate ports, separate bind addresses and separate credentials, and neither configures the other.

The filesystem messaging path stays fully live alongside it. An API send is stored in the same durable queue and dispatched through the same actuation as a filesystem send, so a container and a host agent reach a peer the same way.

Its first consumer is a Dockerized coding agent: the daemon starts the container with the API URL, a minted token and a session id in its environment, and never mounts the host config directory, the host `messaging/` directory or the Docker socket into it.

## Turning it on

Set `apiServerEnabled` to `true`. The port comes from `apiServerPort`, whose default is profile-aware per binary suffix and deliberately distinct from the web port so development and production builds do not collide.

`apiServerBind` defaults to `127.0.0.1`. Widen it **only** when you are serving a container, and know what you are doing when you do: **any non-loopback bind logs a loud startup warning**, and the token, not the interface, is the trust boundary.

Docker Desktop containers cannot reach a daemon bound only to `127.0.0.1`. For local container sessions the documented approach is to bind a Docker-reachable interface and limit access with host firewall rules scoped to the local Docker or WSL subnet.

A bind or port change takes effect by saving settings and stopping and starting the API server. A full daemon restart is not required.

## Authentication

Every request except `healthz` needs a bearer token:

```text
Authorization: Bearer <client-token>
```

Tokens are minted host-side with the [`api-client`](../reference/cli.md#api-client) CLI verb, which requires the master or root token. A container has no master token by construction and cannot mint its own.

```bash
agentscommander api-client mint \
  --token "$MASTER_TOKEN" \
  --root "D:\path\to\wg-8-dev-v5-team\__agent_dev-rust" \
  --scopes send,list-peers-lean \
  --label "CI bot"
```

Four properties matter more than the syntax:

- **A token is bound to exactly one replica.** The caller's identity is derived at request time from that bound root, never from the request body. The Root Agent is rejected, both at mint time and at request time.
- **The secret is printed once.** The registry stores only a SHA-256 hash, in a host-only file that is never mounted into a container. `list` never shows secrets or hashes.
- **Scopes are `send`, `list-peers-lean`, `session-transport`, `pty-input` and `terminal-snapshot`**, and they do not imply one another.
- **The two privileged scopes never work on their own.** A manually minted `pty-input` or `terminal-snapshot` scope gains no live authority: both additionally require a matching automatically bound container credential, checked fresh on every request. Workers never receive them automatically.

Revocation takes effect on the next request. Authentication is unconditional in every build profile, and a per-source-IP failed-auth lockout throttles unauthenticated probing.

## Endpoints

All routes live under `/api/v1`.

| Route | Handler | What it does |
|---|---|---|
| `POST /api/v1/send` | `send` | Durable inline send to a peer FQN. `opId` is the idempotency key, enforced per sender, so a replay returns the same queued message id and never creates a second row. Inline payloads are capped at 256 KiB. Answers `202` with `{ "status": "queued", "messageId": "..." }`. |
| `GET /api/v1/peers` | `list_peers` | `list-peers-lean` for the caller's bound replica. Reachability is computed from the bound identity, not from anything in the request. |
| `GET /api/v1/session-transport` | `session_transport` | A WebSocket upgrade carrying one container session's terminal transport. Authorization happens before the upgrade, and the socket is bound to that session, its backend and its bound root, with a fixed maximum frame size. |
| `POST /api/v1/pty-input` | `pty_input` | Privileged exact PTY actuation into one live, automatically bound container coordinator's same-workgroup member. Requires the `pty-input` scope. `text` is 1 through 65,536 decoded UTF-8 bytes. `opId` is permanently idempotent for that sender incarnation. |
| `GET /api/v1/pty-input/{opId}` | `pty_input` | Metadata-only status for the authenticated sender's own operation. Statuses are `queued`, `actuating`, `injected`, `rejected` and `indeterminate`. Returns `200` or `404`. |
| `POST /api/v1/terminal-snapshot` | `terminal_snapshot` | Reads one live backend terminal viewport as JSON or PNG. Requires the `terminal-snapshot` scope **and** the default-off `terminalSnapshotsEnabled` gate. See [Terminal snapshots](terminal-snapshots.md). |
| `GET /api/v1/windows/{window_id}/screenshot` | `window_screenshot` | Returns the PNG bytes of exactly one live native window. **Windows only**: the route is absent from other builds rather than emulated. It enforces the `pty-input` scope and the same fresh bound-credential guard. See [Window capture](window-capture.md). |
| `GET /api/v1/healthz` | `health` | Unauthenticated liveness. The body is exactly `{"ok":true}`. |

Two notes on reading that table. `injected` proves the backend accepted the exact text and the required first Enter; it does not prove the model consumed or completed anything. `indeterminate` means actuation began and complete submission cannot be proven, and it is never replayed automatically: keep the same operation id and look the status up rather than retrying under a new one.

## Authorization and audit

Scope is necessary and never sufficient. Authority is based on live physical identity, and the [security model](../security.md) is the binding description; the short version:

- A live container coordinator may target one verified non-coordinator member in the same exact project and workgroup.
- Manual API clients, workers, stale sessions, cross-workgroup and cross-project routes, aliases and wildcards have no privileged authority, whatever their scope list says.
- Target names must be exact canonical FQNs.

**Audit is metadata-only.** Every mint, revoke and authenticated request appends a record to a host-only audit log with a size cap and one rotation. Terminal snapshots emit their own metadata-only event instead of the generic one. Secrets and hashes are never logged, and neither is terminal text, JSON, PNG data, a raw nonce, an output path, or arbitrary parser errors.

Two limits stated plainly, because they are easy to over-read. The audit is **fail-soft operational metadata, not compliance-grade fail-closed audit**. And the message bus database on the host holds queued message bodies and replayable nonterminal PTY-input text in plaintext until the operation commits or rejects; clearing those bytes is redaction of live application state, not forensic secure erasure.

## Settings

| Key | What it controls |
|---|---|
| `apiServerEnabled` | Whether the control-plane API server runs. `false` by default. |
| `apiServerPort` | The listening port. Profile-aware default per binary suffix, distinct from the web port. |
| `apiServerBind` | The bind address. `"127.0.0.1"` by default. Any non-loopback bind logs a loud startup warning at startup. |

See [Settings reference](../reference/settings.md#control-plane-api-server-opt-in) for the field types.

## Troubleshooting

**"Every request comes back 401."** The token is absent, revoked or expired. Revocation takes effect on the next request, so a token that worked a moment ago can stop between calls. `api-client list` shows which clients are registered.

**"The token authenticates but a privileged call returns 403."** The scope is missing, or the call is privileged and the client is manually minted. `pty-input` and `terminal-snapshot` require a live automatically bound container credential in addition to the scope; a hand-minted token holding those scopes never gains authority.

**"A container cannot reach the API at all."** Check `apiServerBind`. A daemon bound only to `127.0.0.1` is unreachable from a Docker Desktop container; that is the documented cause, and the fix is a Docker-reachable bind plus firewall rules limiting access to the local Docker or WSL subnet.

**"A PTY-input retry returned 409."** The same `opId` was reused with a changed target, text, profile, source semantics or sender identity. That combination can never create a second injection under that id. Use a new `opId` for new work, and the original one only for an exact retry.

**"The status says `indeterminate`."** Actuation began and completion cannot be proven. Do not replay it under a new id. Look the same operation id up; a new id would risk a second injection of the same text.

**"Is the server even up?"** `GET /api/v1/healthz` needs no token and answers exactly `{"ok":true}`.

## See also

- [Terminal snapshots](terminal-snapshots.md) - the full contract behind the snapshot endpoint
- [Window capture](window-capture.md) - the window screenshot route, its limits and its audit
- [Security model](../security.md) - the authority matrix these endpoints enforce
- [`api-client` CLI reference](../reference/cli.md#api-client) - minting, revoking and listing clients
