# Window capture

Window capture lets you capture exactly one live native window as a PNG file on Windows, either from the CLI (local, in-process, no authorization boundary) or through the authenticated control-plane API. It is Windows-only: the CLI verbs and the HTTP route are compiled only on Windows and are deliberately absent on other targets.

Use this feature when you need the current pixels of a live native desktop window. It is not a terminal snapshot (that reads the backend terminal viewport without touching OS pixels), not a monitor-wide screenshot, and not the in-app overlay/hotkey flow (see [Screenshot capture](screenshot-capture.md)).

## Before you start

- A live interactive Windows desktop session (the verbs and the route require the native Windows capture stack).
- The canonical decimal window id of the target window, discovered with `window-list`.
- For the API path: an automatically bound container credential with live fresh-guard authority. A manually minted API token never gains this capability.

Window captures can contain passwords, tokens, source code, and personal data. The CLI writes exactly the captured pixels with no redaction.

## Window identity

`window_id` is the canonical ASCII decimal rendering of the live target's `xcap::Window::id().to_string()` value. It is accepted only when all of these rules hold:

1. It matches `^(0|[1-9][0-9]*)$`.
2. It has at most 20 decimal digits.
3. It parses as an unsigned 64-bit integer.

The id is a best-effort current-holder selector, not a stable object identity: the capture selects the matching live window from one fresh `Window::all()` snapshot. If the snapshot has no matching object, the operation reports `window_not_found`. Windows may destroy or reuse a native handle after matching, so a closed or recycled id is not guaranteed to stay `window_not_found`. There is no generation, title, process-name, PID, or geometry selector.

Discover the current ids from the CLI:

```bash
agentscommander window-list
```

Each line is `id<TAB>title`. Titles are printed verbatim, with no sanitization; a title containing a tab or newline breaks the line contract for downstream parsers.

## Capture from the CLI

The CLI path is local and in-process. It reuses the exact same bounded capture worker as the API route but performs no HTTP request, no authentication, and no audit recording: it is a one-shot local process acting on the invoking user's own behalf.

```bash
agentscommander window-screenshot \
  --window-id 983044 \
  --output "C:\path\shot.png"
```

| Flag | Required | Description |
|---|---|---|
| `--window-id` | Yes | Canonical decimal window id as printed by `window-list`. |
| `--output` | Yes | Destination PNG file path. Overwritten if it exists; parent directories are not created. |

Success exits 0 and writes no stdout; the output file contains exactly the raw PNG bytes produced by the capture worker (same encoder and limits as the API path). A minimized window yields `capture_unavailable`. Capture-side failures leave an existing output file untouched; a failed write may leave a partial file and can destroy prior content of an existing `--output`.

After dispatch, every failure exits 1, writes no normal stdout, and writes exactly one stderr line:

```text
window_screenshot_error code=<code> detail=<detail>
```

| Code | HTTP | Condition |
|---|---:|---|
| `invalid_window_id` | 400 | `--window-id` is empty, non-decimal, signed, whitespace-padded, leading-zero, over 20 digits, or over `u64::MAX`. |
| `window_not_found` | 404 | The canonical id matches no live window in the enumeration snapshot. |
| `capture_busy` | 429 | Capture capacity is full (local one-shot limiter; kept for completeness). |
| `capture_too_large` | 413 | The window exceeds the advisory pixel limit or the encoded PNG exceeds the hard 16 MiB bound. |
| `capture_unavailable` | 503 | Enumeration, minimized/inaccessible window, capture, encode, or runtime failure. |
| `output_write_failed` | Not emitted | The output file could not be written (missing parent, permission, disk full). |

The HTTP column mirrors the API route's stable codes; `output_write_failed` is CLI-only. Standard `--help` and pre-dispatch Clap syntax failures keep normal Clap output and exit behavior. Enumeration failure for `window-list` exits 1 with `window_list_error code=window_list_unavailable detail=<error>`. On non-Windows targets the subcommands do not exist and are not listed by `--help`.

**No token required**: the verbs read no `--root`, token, registry, or config state and record no API audit.

## Capture from the API

`GET /api/v1/windows/{window_id}/screenshot` returns the raw PNG bytes for exactly one live native window. The route exists only on Windows; on other builds it is deliberately absent rather than emulated.

The request has no body and no defined query parameters. It requires exactly one `Authorization` header in the strict form:

```text
Authorization: Bearer <nonempty-token>
```

The route parameter is a matcher only: the handler authenticates first, then validates the literal raw path segment itself with no percent-decoding. A malformed, percent-encoded, or noncanonical segment is an authenticated 400 `invalid_window_id` with no capture attempt. A structurally nonmatching URL is the router's ordinary 404 before the handler.

### Authorization

The route uses the existing strict protected-path model (`authenticate_pty_input_fresh`): lockout checks, strict Bearer parsing, the fresh-guard bound-credential registry, `SCOPE_PTY_INPUT` scope enforcement, and bound-session and credential-generation enforcement. Freshness is checked twice per request: once before validation and admission (the credential-registry lock is dropped before any await), and again after the active capture slot is acquired (dropped before capture). A revocation completed while a request waits blocks its launch; a capture launched after the second check is authorized as of launch and is not retroactively cancelled. Only an automatically bound container credential satisfies the fresh guard; a manually minted token never gains live authority.

### Success response

```text
200 OK
Content-Type: image/png
Content-Length: <exact PNG byte count>
Cache-Control: no-store

<raw PNG bytes>
```

It is not JSON, base64, a data URL, a multipart response, or a persisted artifact.

### Error responses

Failures use the canonical `ApiError` JSON envelope with the stable machine code. No native error, title, token, image data, process information, or host path is returned.

| Status | Code | Condition |
|---|---:|---|
| 400 | `invalid_window_id` | After successful auth, the raw path segment is malformed, percent-encoded, noncanonical, or out of range. |
| 404 | `window_not_found` | Valid canonical id matches no live window in the current snapshot. |
| 429 | `capture_busy` | The local capture queue is full (3 admitted, 1 active). |
| 413 | `capture_too_large` | The window reports over the advisory pixel limit, its captured image exceeds it, or its PNG exceeds the hard 16 MiB encoded bound. |
| 503 | `capture_unavailable` | Enumeration, minimized/inaccessible window, capture, PNG encoding, or blocking-task join fails. |

### Audit

Every final route-specific outcome records exactly one API audit entry with `event` and `operation` set to `window_screenshot` and a redacted status: `succeeded`, `invalid_window_id`, `window_not_found`, `capture_busy`, `capture_too_large`, or `capture_unavailable`. The entry omits the window id, title, PID, token, credential, native diagnostic, and image bytes. Strict-authentication and freshness failures exit through the pre-existing protected-path helper and record no screenshot audit. Audit is fail-soft and rotated like all API audit records.

## Fixed limits

| Resource | Limit |
|---|---|
| Admitted requests | 3 (1 active + 2 queued) |
| Active capture workers | 1 |
| Advisory source pixels | 16,777,216 |
| Encoded PNG | 16 MiB (hard) |

The advisory pixel check is a best-effort refusal before requesting a bitmap, not a pre-allocation memory guarantee. CLI captures use a fresh one-shot limiter per process, so concurrent CLI invocations never contend with each other or with the API queue.

## See also

- [CLI reference](../reference/cli.md#window-list)
- [CLI reference](../reference/cli.md#window-screenshot)
- [Architecture reference](../reference/architecture.md)
