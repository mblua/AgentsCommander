# Terminal snapshots

Terminal snapshots let an authorized Root Agent or workgroup Orchestrator read one live backend terminal viewport as versioned JSON or a deterministic PNG without changing the target terminal.

Use this feature when you need the current terminal state of a hidden, minimized, detached, or never-mounted session. A snapshot is not a transcript, frontend screenshot, or request to wake an agent.

## Before you start

You need:

- AgentsCommander running with the target session already live;
- **Settings > General > Terminal snapshots > Allow authorized terminal snapshots** enabled;
- a live requester session token, not a stored Root or master token;
- the exact canonical target name from snapshot target discovery; and
- for a container requester, the automatically provided API URL and token.

Terminal screens can contain passwords, tokens, source code, prompts, and personal data. The setting is off by default. AgentsCommander does not redact snapshot content.

## Enable terminal snapshots

In the app, open **Settings > General > Terminal snapshots**, select **Allow authorized terminal snapshots**, and save.

The corresponding `settings.json` field is:

```json
{
  "terminalSnapshotsEnabled": true
}
```

The default is `false`. Missing, malformed, duplicated, unreadable, linked, or wrongly typed security settings fail closed. AgentsCommander changes this field through a dedicated compare-and-set operation, so a stale Settings window cannot silently re-enable a concurrently disabled gate. Direct edits from another process remain last-writer authority.

See the [settings reference](../reference/settings.md#terminal-snapshots).

## Discover eligible targets

Use the capability-specific lean view before a host request:

```bash
agentscommander list-peers-lean \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --snapshot-targets
```

Pass the returned `name` exactly to `terminal-snapshot --to`.

This discovery view is not authorization and does not reveal session liveness:

- Root receives verified workgroup Orchestrators and members from active registered projects.
- A verified workgroup Orchestrator receives non-Orchestrator members from the same physical project and workgroup.
- Workers and origin agents receive `[]`.
- Identity-only entries report `working=false`, `sessionStatus="unknown"`, `waitingForInput=false`, no `contextPercent`, and no `roleSummary`. `reachable` continues to describe ordinary messaging, not snapshot permission.
- The view does not read `sessions.json`, create peer directories, or change default `list-peers-lean` behavior.

Use `--peer <exact-fqn>` to filter this view. `list-peers --snapshot-targets` is not supported.

## Capture from the host

### JSON

```bash
agentscommander terminal-snapshot \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "project:wg-1-team/member"
```

`--format json` is optional because JSON is the default. Success writes exactly one compact ASCII-only JSON document followed by LF to stdout.

### PNG

```bash
agentscommander terminal-snapshot \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "project:wg-1-team/member" \
  --format png \
  --output "/absolute/new/snapshot.png"
```

PNG requires an absolute path to a new `.png` file. AgentsCommander validates the response and complete PNG before it opens the output. It never overwrites an existing file. On Unix, it creates the file with mode `0600`. On every platform, it rejects linked or replaced path objects, including a link anywhere in the output path's ancestor chain, and unsafe Windows device, stream, and alias forms.

Success writes metadata JSON followed by LF to stdout. It never writes PNG bytes or base64 to stdout.

A write, sync, or final identity failure returns `output_failed` and can leave an incomplete caller-owned file. AgentsCommander does not delete that path because another process could have replaced the name. If the PNG completed but the later stdout receipt fails, the completed file also remains.

On macOS, `--output` with a non-UTF-8 leaf name exits 1 with `output_failed` and creates nothing, where the identical command succeeds on Linux. The filesystem refuses to create an entry with those bytes. AgentsCommander imposes no encoding constraint of its own and passes the path through unchanged.

### Timeout

`--timeout <seconds>` accepts whole numbers from 5 through 60 and defaults to 15. One deadline covers request wait, response read, schema and PNG validation, and the check before output creation. A filesystem write already in progress cannot be interrupted portably.

The daemon's disclosure deadline is 10 seconds, and a host request authorizes for at most `min(timeout, 30)` seconds. A blocking OS call can finish later, but it cannot authorize publication after the deadline. A timed-out host client attempts to cancel only its own still-unclaimed request. Retrying creates a new point-in-time read with a new request ID.

## Capture from a container Orchestrator

An automatically bound container Orchestrator receives:

- `AGENTSCOMMANDER_API_URL`
- `AGENTSCOMMANDER_API_TOKEN`

Use the helper without supplying either secret on the command line:

```bash
agentscommander-api-helper terminal-snapshot \
  --to "project:wg-1-team/member"
```

For PNG:

```bash
agentscommander-api-helper terminal-snapshot \
  --to "project:wg-1-team/member" \
  --format png \
  --output "/workspace/evidence/snapshot.png" \
  --timeout 15
```

The helper accepts the same format, timeout, stdout, and output-file contract as the host command. It sends exactly one `POST /api/v1/terminal-snapshot` request, bypasses ambient proxies, does not follow redirects or retry protocol NACKs, requests identity encoding, and applies one absolute deadline.

A manual API token does not gain this capability even if its registry scopes include `terminal-snapshot`. Root is host-only.

## Authorization

Terminal snapshots use a separate read capability. They do not broaden ordinary messaging or privileged PTY input.

| Requester | Authorized target | Plane |
|---|---|---|
| Live verified canonical Root Agent | Any verified workgroup Orchestrator or member in active registered project paths | Host only |
| Live verified workgroup Orchestrator | One verified non-Orchestrator member in the same exact project and workgroup | Host |
| Automatically bound live container Orchestrator with `terminal-snapshot` scope | One verified non-Orchestrator member in the same exact project and workgroup | Container API |
| Worker, origin agent, origin Orchestrator, manual API client, stale session, or static Root/master credential | None | None |

Orchestrator-to-Orchestrator, Orchestrator-to-Root, self, cross-workgroup, cross-project, Root-to-Root, Root-to-origin, aliases, wildcards, filesystem directory names, and session IDs are not authorized targets.

AgentsCommander verifies the physical requester-to-target route before it looks up a target session, parser, backend, or liveness. Shape-valid unauthorized and nonexistent targets therefore return the same `not_authorized` response and do not expose target liveness. Local filesystem cache timing is outside that no-liveness guarantee.

After authorization, the service selects one live persistent target session. It prefers `Active`, then `Running`, then `Idle`; within a status it chooses the newest creation time, then the lowest UUID bytes. The caller cannot select a session or fan out. A busy live TUI is valid. Temporary and exited-only rows are not eligible.

The service repeats the privacy, requester, route, target, session, backend, generation, restore, purge, and shutdown checks before it releases content. An authority change discards the prepared result. Output or resize after capture does not invalidate a legitimate point-in-time screen.

## What the snapshot represents

A snapshot contains the current active viewport held by the backend `vt100` 0.15.2 mirror:

- all rows and all cells, including blank, background-only, and inverse-only cells;
- exact represented cell text, width, foreground, background, and four exposed styles;
- dimensions, row-wrap flags, active normal or alternate buffer, cursor state, parser error count, and output-read sequence;
- the selected session ID and backend kind; and
- fixed fidelity metadata that states what is omitted or unsupported.

The backend mirror has zero scrollback rows. The snapshot does not contain frontend scrollback, a user-scrolled viewport, selection, xterm theme state, window chrome, overlays, title, or exact WebView pixels.

Capture is atomic at one completed backend output-read chunk and accepted resize boundary. `sequence` counts output chunks, not bytes or application frames. `applicationFrameAtomic` is therefore always `false`. A fast TUI redraw can be internally coherent while still showing a semantically intermediate application frame.

A hidden, minimized, detached, or never-mounted frontend does not affect capture. AgentsCommander never focuses, selects, wakes, spawns, resizes, repaints, writes to, or submits input to the target. It never calls the OS window, monitor, desktop, or existing interactive screenshot path.

## JSON schema version 1

The top-level document is closed and versioned. This one-row example is structurally complete:

```json
{
  "schemaVersion": 1,
  "requestId": "22222222-2222-4222-8222-222222222222",
  "capturedAt": "2026-07-31T03:30:00.123Z",
  "requester": "project:wg-1-team/coordinator",
  "target": "project:wg-1-team/member",
  "session": {
    "id": "11111111-1111-4111-8111-111111111111",
    "backend": "localProcess"
  },
  "screen": {
    "dimensions": { "rows": 1, "columns": 2 },
    "sequence": 42,
    "activeBuffer": "normal",
    "cursor": {
      "row": 0,
      "column": 1,
      "visible": true,
      "inBounds": true
    },
    "parserErrors": 0,
    "lines": [
      {
        "wrapped": false,
        "cells": [
          {
            "text": "A",
            "width": "narrow",
            "foreground": { "kind": "default" },
            "background": { "kind": "indexed", "index": 4 },
            "style": {
              "bold": false,
              "italic": false,
              "underline": false,
              "inverse": false
            }
          },
          {
            "text": "",
            "width": "narrow",
            "foreground": { "kind": "rgb", "red": 51, "green": 255, "blue": 153 },
            "background": { "kind": "default" },
            "style": {
              "bold": false,
              "italic": false,
              "underline": false,
              "inverse": false
            }
          }
        ]
      }
    ]
  },
  "fidelity": {
    "scope": "currentBackendViewport",
    "backendParser": "vt100-0.15.2",
    "backendScrollbackRows": 0,
    "atomicAtOutputSequence": true,
    "applicationFrameAtomic": false,
    "allActiveViewportCellsIncluded": true,
    "includesFrontendState": false,
    "exactFrontendPixels": false,
    "parserHadErrors": false,
    "parserErrorCoverage": "replacementC1AndUnhandledControls",
    "omitted": [
      "applicationCursorMode",
      "applicationKeypadMode",
      "audibleBellCount",
      "bracketedPasteMode",
      "iconName",
      "inactiveBuffer",
      "mouseProtocolEncoding",
      "mouseProtocolMode",
      "title",
      "visualBellCount"
    ],
    "unsupported": [
      "blink",
      "colorEmoji",
      "cursorBlinkPhase",
      "cursorShape",
      "faint",
      "frontendFontMetrics",
      "frontendOverlays",
      "frontendScrollOffset",
      "frontendScrollback",
      "hyperlinks",
      "ligatures",
      "overline",
      "selection",
      "strikethrough",
      "terminalImages",
      "uiChrome"
    ]
  }
}
```

### Field rules

| Field | Contract |
|---|---|
| `schemaVersion` | Exactly `1`. |
| `requestId` | Canonical lowercase UUID v4 generated for this read. |
| `capturedAt` | Canonical UTC RFC 3339 timestamp with exactly millisecond precision. |
| `requester` | Exact workgroup FQN, or `agentscommander://root-agent` on host Root responses. |
| `target` | Exact canonical workgroup FQN. |
| `session.backend` | `localProcess` or `containerTransport`. |
| `screen.dimensions` | Nonzero, at most 200 rows, 400 columns, and 40,000 cells. |
| `screen.sequence` | Saturating count of processed PTY output chunks. Resize does not increment it. |
| `screen.activeBuffer` | `normal` or `alternate`. Only that visible grid is included. |
| `screen.cursor` | Backend row, column, visibility, and whether row and column are inside the viewport. A wrap-pending column can equal `columns`. |
| `screen.parserErrors` | The parser's represented error count. A nonzero value still produces a successful snapshot. It does not count every ignored terminal extension. |
| `lines` and `cells` | `lines.length == rows`; every `cells.length == columns`; no blank cells are omitted. |
| `cell.text` | Exact `vt100::Cell::contents()`, at most six Unicode scalars and 24 UTF-8 bytes. No trim, normalization, shaping, or raw ANSI replay. |
| `cell.width` | `narrow`, `wideLead`, or `wideContinuation`. A lead is followed by one empty continuation. |
| `foreground`, `background` | `default`, `indexed` with `index` 0 through 255, or `rgb` with channels 0 through 255. |
| `style` | Exactly `bold`, `italic`, `underline`, and `inverse`. |
| `fidelity` | Closed version-1 constants. Clients must not treat the arrays as open-ended hints. |

Every object rejects unknown or duplicate fields when decoded by snapshot clients. Host and helper stdout escape all controls, non-ASCII scalars, U+007F, and Unicode line or bidi hazards into lowercase `\uXXXX` sequences. Parsing recovers the original cell text.

## Deterministic PNG profile

PNG is rendered from the same retained immutable screen model used for JSON. The service does not read the parser a second time.

| Property | Version-1 value |
|---|---|
| Renderer | `ac-terminal-png-v1` |
| Font | Embedded DejaVu Sans Mono 2.37 regular, SHA-256 `b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582` |
| Font size | 16.0 px |
| Rasterizer | `fontdue` 0.9.3 with `collection_index=0`, scale 16.0, substitutions off, one thread, no SIMD or platform font API |
| Cell | 10 by 20 px, baseline 15 px |
| Padding | 8 px on every side |
| Palette | `ac-dark-v1`; default foreground `#e8e8e8`, default background `#0a0a0f` |
| Cursor | Fixed nonblinking block `#00d4ff`, cursor text `#0a0a0f` |
| Output | Opaque RGB8, noninterlaced, lossless PNG |
| Encoder | `png` 0.18.1, `Fast` compression, `Sub` filter |
| Chunks | PNG signature, one IHDR, consecutive IDAT chunks, and one final IEND only |

Indexed colors 0 through 15 are fixed:

```text
0 #1a1a2e   1 #ff3b5c   2 #33ff99   3 #ffcc33
4 #3399ff   5 #ff33cc   6 #33ccff   7 #e8e8e8
8 #4a4a5e   9 #ff6699  10 #66ffbb  11 #ffdd66
12 #66bbff 13 #ff66dd  14 #66ddff  15 #ffffff
```

Indices 16 through 231 use the standard 6 by 6 by 6 cube with levels `0, 95, 135, 175, 215, 255`. Indices 232 through 255 use `8 + 10 * (index - 232)` grayscale. RGB values are direct. Inverse swaps foreground and background; bold does not brighten indexed colors.

The renderer paints the default background, cell backgrounds, cursor, glyph masks, and underline in that order. A valid wide lead paints and clips one two-cell visual span; its continuation remains present as a raw JSON cell but is not repainted independently. Bold redraws the same glyph mask at `x + 1`; italic shears by integer `((19 - local_y) / 4)` pixels; underline fills cell-local row 17. Antialiasing per color channel is `(foreground * alpha + current_destination * (255 - alpha) + 127) / 255`. Every write clips to its one-cell or wide two-cell span and owning row. No installed font, GPU, platform text API, shaping, ligature, fallback font, alpha, timestamp, title, path, or ancillary metadata affects the bytes.

A missing glyph uses U+FFFD, or a fixed hollow box when that font glyph is unavailable, and increments `renderer.fallbackGlyphCount`. This is declared visual loss, not a silent substitution.

The PNG receipt contains all document identity, capture, session, screen metadata, and the full fidelity object, but no line or cell payload. It adds:

| Field | Contract |
|---|---|
| `format` | Exactly `png`. |
| `png.bytes` | Decoded PNG byte length, at most 16 MiB. |
| `png.pixelWidth` | `columns * 10 + 16`. |
| `png.pixelHeight` | `rows * 20 + 16`. |
| `renderer` | The fixed profile above plus `fallbackGlyphCount`. |

## Fixed limits and admission

The service rejects rather than truncates:

| Resource | Limit |
|---|---|
| Rows, columns, cells | 200, 400, 40,000 |
| Pixel side and pixels | 4,096 and 8,200,000 |
| RGB working bytes | 24,288,768 |
| JSON document and decoded PNG | 16 MiB each |
| Success transport envelope | 24 MiB |
| Request and API error body | 8 KiB each |
| Requester rate | 6 admitted attempts per rolling 60 seconds |
| Target rate | 12 admitted attempts per rolling 60 seconds |
| Source ingress rate | 30 attempts per rolling 60 seconds |
| Concurrent work | 8 ingress tasks, 1 per requester, 1 per target, 2 expensive operations globally |

Rate history resets when the daemon restarts.

## HTTP contract

The container helper calls `POST /api/v1/terminal-snapshot` with `Authorization: Bearer`, `Content-Type: application/json`, and this strict body:

```json
{
  "apiVersion": "1",
  "requestId": "22222222-2222-4222-8222-222222222222",
  "to": "project:wg-1-team/member",
  "format": "json"
}
```

Success is HTTP 200. Every handler-produced success or error has:

```text
Content-Type: application/json; charset=utf-8
Cache-Control: no-store
Pragma: no-cache
```

The route rejects query strings, duplicate authorization or content headers, non-JSON content, non-identity encoding, mismatched lengths, unknown or duplicate body fields, and bodies over 8 KiB. The operation is a fresh point-in-time read and is not idempotent. `requestId` correlates one response; reusing one through the API is another rate-limited read.

See the [control-plane API reference](../../src-tauri/src/api/README.md#terminal-snapshots) for transport details.

## Stable errors

After command dispatch, host and helper failures exit 1, write no normal stdout, and write exactly this shape to stderr:

```text
terminal_snapshot_error code=<code> detail=<fixed-detail>
```

Standard host Clap help and syntax errors keep normal Clap output and exit behavior. If an OS error occurs after a stdout write has already begun, safe partial ASCII bytes cannot be retracted; the command reports `output_failed` and does not attempt a second stdout document.

| Code | HTTP | Fixed detail | Recovery |
|---|---:|---|---|
| `invalid_request` | 400 | `The terminal snapshot request is invalid.` | Correct flags, target syntax, headers, or body. |
| `requester_unavailable` | 401 | `A unique live terminal snapshot requester is required.` | Restart or respawn the requester and use its current live token. |
| `terminal_snapshots_disabled` | 403 | `Terminal snapshots are disabled.` | Enable the setting and correct any malformed security settings. |
| `not_authorized` | 403 | `The terminal snapshot route is not authorized.` | Use an allowed requester-to-target route. Do not probe target liveness. |
| `target_unavailable` | 404 | `The authorized target has no eligible live session.` | Start an eligible persistent target session, then make a new request. |
| `snapshot_unavailable` | 409 | `The authorized target screen is temporarily unavailable.` | Retry later with a new request after route, parser, restore, or purge state recovers. |
| `snapshot_too_large` | 413 | `The terminal snapshot exceeds a fixed resource limit.` | Reduce the target viewport dimensions. |
| `authority_changed` | 409 | `Terminal snapshot authority changed before disclosure.` | Rediscover the target and retry only after identity, setting, and route state stabilizes. |
| `rate_limited` | 429 | `The terminal snapshot rate or concurrency limit was reached.` | Wait for the rolling window or current operation to finish. |
| `snapshot_timeout` | 504 | `The terminal snapshot did not complete before its deadline.` | Check daemon health, then issue a new point-in-time request if needed. |
| `service_unavailable` | 503 | `A terminal snapshot security dependency is temporarily unavailable.` | Retry after settings, registry, lock, or daemon contention clears. |
| `render_failed` | 500 | `The deterministic terminal renderer failed.` | Retry once; report a reproducible screen and renderer version without sharing secret content. |
| `unsafe_path` | 403 | `A terminal snapshot path failed confinement checks.` | Choose an absolute new `.png` path under an existing parent, with no link anywhere in its ancestor chain. |
| `output_failed` | Not emitted | `The requested terminal snapshot output could not be completed safely.` | Inspect the requested path for an incomplete file; choose a new path for any retry. |
| `response_unavailable` | 500 | `The terminal snapshot response could not be published or validated.` | Check transport and protocol compatibility; do not trust or persist the rejected bytes. |
| `internal` | 500 | `An internal terminal snapshot invariant failed.` | Preserve metadata-only diagnostics and report the failure. |

## Output lifetime and cleanup

### Host mailbox

Host transport uses dedicated transient directories below the verified requester root:

```text
<requester-root>/<agent-local-dir>/outbox/terminal-snapshot-requests/
<requester-root>/<agent-local-dir>/terminal-snapshot-responses/
```

It does not put snapshot content in workgroup `messaging/`, ordinary delivered or rejected message state, conversations, the standard message database, or PTY-input state.

The CLI removes a validated response after use. The daemon tracks protocol requests, processing files, temporary files, cancellations, and unread responses and sweeps identity-stable entries after 60 seconds. Startup performs a bounded sweep of discoverable verified Root and replica directories. A crash followed by removal of the only registered or archived project path can leave a protocol file undiscoverable. In that residual case, stop AgentsCommander and inspect the two exact directories above. Remove only protocol-shaped files that you own and recognize; an identity mismatch is intentionally left untouched.

Unix transient directories use mode `0700` and files use `0600`. Windows files inherit same-user ACLs. Neither mode is a defense against a fully compromised same-OS-user account, and deletion is not forensic secure erasure.

### API and caller output

The API prepares content in bounded memory and returns it with no-store headers. It does not send the snapshot to a third-party service. Memory is not locked or zeroized and can appear in swap or a crash dump.

A requested final PNG is durable caller-owned content. It remains until you delete it. A failed final write can leave an incomplete file. AgentsCommander never performs path-racy cleanup or claims secure erasure.

Snapshot audit records operational metadata only, such as verified identities, format, selected session/backend, dimensions, sequence, capture time, payload byte count, status, and fixed reason code. Audit is fail-soft and rotated, not compliance-grade fail-closed logging. It never stores cell text, JSON documents, PNG/base64, ANSI, title, token, nonce, output path, or content hash.

## See also

- [CLI reference](../reference/cli.md#terminal-snapshot)
- [Settings reference](../reference/settings.md#terminal-snapshots)
- [Security model](../security.md#authorized-terminal-snapshots)
- [Inter-agent messaging](../agents/inter-agent-messaging.md#terminal-snapshots-are-a-separate-read-plane)
- [Container coding agents](container-coding-agents.md#terminal-snapshots-from-a-container-orchestrator)
- [Terminal session test plan](../testing/07-terminal-sessions.md#terminal-snapshot-evidence-contract)
