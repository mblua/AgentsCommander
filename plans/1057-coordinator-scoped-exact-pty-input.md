# Draft implementation plan: #1057 coordinator-scoped exact PTY input

**Issue:** https://github.com/mblua/AgentsCommander/issues/1057
**Branch:** `feature/1057-coordinator-pty-input`
**Base and current HEAD inspected:** `ebadc7f4dccbb3e337a2848fa8640b4473e40252`
**Delivery class:** FULL
Status: READY_FOR_IMPLEMENTATION

## 1. Objective and current evidence

Add one privileged operation that submits validated, byte-identical UTF-8 text to exactly one coding-agent PTY:

- a live, identity-verified workgroup coordinator may target one verified non-coordinator member replica in the same exact project and workgroup;
- a live Root Agent may target one verified workgroup coordinator replica;
- a container-backed coordinator receives the same capability through a dedicated HTTP scope and helper path;
- all other sender/target combinations fail before session mutation or PTY input.

This is PTY actuation, not shell execution and not ordinary messaging. The accepted value is never supplied to `Command`, `cmd.exe /C`, `sh -c`, PowerShell, argv, env, a path, or a host/container shell evaluator. It goes only to the already-running target PTY through `PtyManager`.

Step 1 is `CHANGE_REQUIRED`. On this HEAD:

- `cli/send.rs::SendArgs` has only `send` and allowlisted `command` payloads.
- `phone/types.rs::OutboxMessage` has no typed/versioned PTY-input payload.
- `phone/mailbox.rs::process_message` rejects Root actions and routes ordinary actions through broader messaging rules.
- `pty/inject.rs::inject_text_into_session` owns the 1500 ms plus 500 ms Enter sequence, but exposes only `Result<(), String>` and takes a separate PTY lock for each phase.
- `commands/pty.rs::pty_write` and both web PTY input paths can interleave user bytes between those phases.
- `api/message_store.rs` has only standard-message `queued/delivering/retry/delivered/poisoned` states and retains standard bodies.
- the API has no PTY-input scope, route, DTO, status lookup, or no-replay state.
- container tokens always receive the same three existing scopes, regardless of coordinator identity.
- `agentscommander-api-helper` supports only `list-peers-lean` and ordinary `send`.

The standard message plane, legacy `clear|compact` command plane, frontend, and dependencies do not provide this contract and must not be widened to approximate it.

## 2. Scope

### 2.1 In scope

1. Host `send --pty-input` and `send --pty-input-stdin` parsing, validation, strict payload construction, authorization preflight, durable confirmation, and terminal output.
2. A typed version-1 wire payload and metadata-only terminal result.
3. A read-only, path-safe, exact-workgroup authorization resolver shared by host and API paths.
4. A dedicated PTY-input operation table and state machine with an `actuating` transaction before the first PTY write.
5. Strict target selection, lifecycle, coding-agent eligibility, sustained post-spawn readiness, and local/container backend parity.
6. Per-session writer serialization covering the complete text and staggered Enter window.
7. Dedicated API scope, POST route, GET status route, strict DTOs, idempotency, dispatcher integration, and helper polling.
8. Metadata-only audit, terminal artifact redaction, generated coordinator/Root context, and user/developer documentation.
9. Positive, negative, failure, durability, compatibility, and optional Docker coverage for every issue matrix row.

### 2.2 Out of scope

- Any UI, clipboard reader, context menu, terminal IPC, response capture, or model-consumption proof.
- Binary input, arbitrary control keys, caller-selected Enter behavior, session-ID targeting, wildcard/team/workgroup targeting, broadcast, or fan-out.
- A force/busy bypass, deferred text for a busy live target, or response waiting.
- Coordinator-to-coordinator, cross-workgroup, cross-project, coordinator-to-Root, or Root-to-worker PTY actuation.
- Expanding legacy `--command` beyond `clear|compact` or tightening its existing sender authorization.
- General repairs for #139 or ordinary-message wake/consumption defects.
- New crates, a frontend type, or a broad mailbox/API refactor.

## 3. Binding architecture decisions

### 3.1 Dedicated operation, never an overloaded message

`action: "pty-input"` is dispatched before standard message framing and before the generic action switch. It never calls `format_pty_wrap`, never populates a standard body, never uses `command`, and never passes through `/api/v1/send`.

The standard DB `messages` table and its retry behavior stay unchanged. A new `pty_input_operations` table in the same SQLite database owns privileged state because its no-replay and redaction invariants differ from ordinary delivery.

### 3.2 One shared engine

`MailboxPoller::dispatch_pty_input_operation` is the single lifecycle and actuation engine. It accepts a claimed operation descriptor, the shared `Arc<MessageStore>`, the combined in-/cross-process operation and target ownership state, and, for `container_api`, the live `Arc<ApiClientStore>`. The claimed descriptor contains IDs and metadata only; payload bytes are returned only by the committing `actuating` transaction.

It is called by:

- the specialized filesystem marker handler for `host_cli`; and
- the dedicated PTY branch in `api/dispatcher.rs` for `container_api`.

The API must not recreate lifecycle, idle, session selection, writer, Enter, boundary, or status logic. API code authenticates, constructs a trusted authority reference, enqueues, and calls the same engine. The engine returns a typed dispatch disposition; it never returns a privileged failure to the mailbox's generic retry/reject machinery, which can retain raw `OutboxMessage` JSON.

`MessageStoreState` also owns `Arc<PtyInputActiveOperations>` and `Arc<PtyInputTargetLocks>`. A combined RAII claim holds the operation-stripe OS lock, active-set entry, and canonical-target stripe/exact locks from claim through terminal/artifact handling. Recovery requires both absence from the local active set and successful acquisition of the deterministic operation stripe lock, so another daemon or a machine-suspended task cannot be terminalized and later write after a public terminal result.

### 3.3 Source planes are daemon-assigned

Only these values exist:

- `host_cli`: a local-process coordinator or the local Root Agent using its live session UUID token;
- `container_api`: a container coordinator using an automatically minted API token bound to its live container session.

A request cannot choose its source plane. Root is valid only on `host_cli`. A container session cannot use the filesystem plane for this operation, and a local/manual API client cannot impersonate `container_api`.

### 3.4 Fixed constants

- Payload version: `1`.
- Enter mode: exactly `agent-submit`.
- Maximum accepted text: `65_536` UTF-8 bytes, inclusive.
- Host wire/API operation TTL: exactly 10 minutes from `issuedAt`.
- Future clock skew accepted at host-wire ingestion: at most 30 seconds.
- Post-spawn readiness: 2 seconds continuously idle, polled every 500 ms, with a 90-second maximum. Hitting the maximum rejects with zero PTY writes; it never injects on the cap.
- Preparation lease: 120 seconds, renewed every 30 seconds while an owned destroy, spawn, readiness, or other long pre-actuation await is still running. Renewal requires the same status and lease owner. A failed renewal stops before `actuating` and leaves safe lease recovery to the store.
- Runtime `actuating` orphan grace: 15 seconds. The required actuation normally completes in about 2 seconds. Runtime/startup recovery terminalizes only ownerless rows whose deterministic operation-stripe OS lock it acquires; runtime additionally requires age beyond the grace and absence from the local active set. Startup does not assume another daemon has no live owner.
- Pre-actuation attempts: at most 5, with 5, 10, 20, and 40 second retry delays after attempts 1 through 4. Attempt 5 terminalizes as `rejected`.
- API dispatcher PTY-input batch size: 1 per tick. Recovery/expiry/full-row-compaction selects at most 64 IDs per maintenance batch and never materializes an unbounded result set. Ordinary-message batch behavior is unchanged.
- Maximum decoded text remains 65,536 bytes. The dedicated HTTP raw-envelope ceiling is `6 * 65_536 + 16 KiB` so legal text represented entirely with `\uXXXX` escapes is not rejected before semantic validation. The existing `/send` handler keeps its current smaller handler-level ceiling.
- Every request/state timestamp uses exactly `chrono::SecondsFormat::Millis` with `use_z = true` (`YYYY-MM-DDTHH:MM:SS.sssZ`). Parsing must round-trip byte-for-byte to that representation; offsets, omitted milliseconds, excess precision, leap-second spellings, and `+00:00` are invalid on the host wire. Internal PTY-operation timestamps use the same fixed-width form so SQLite lexical ordering is chronological.
- Host request/result/marker reads are bounded: request envelope `PTY_INPUT_HOST_ENVELOPE_MAX_BYTES = 6 * 65_536 + 16 KiB`, marker/result envelope 16 KiB. Stdin readers take at most 65,537 bytes. Identity-bearing team/replica/local/settings JSON reads cap at 1 MiB; privileged `api-clients.json` reads cap at 4 MiB and 4,096 client rows; project candidates/team identities cap at 1,024. Exceeding a cap fails closed. No privileged path uses an unbounded read or Serde body extractor.
- Transactional admission limits are 16 nonterminal operations per canonical sender, 512 nonterminal operations globally, and 16 MiB of aggregate nonterminal payload bytes. Exact duplicates are looked up before limits and still return their original operation. New over-limit work rejects as `capacity_exceeded` (HTTP 429); an expiry sweep runs before counting so dead work cannot consume capacity.
- Cross-process ownership uses 4,096 fixed lazy operation lock stripes (`operation-0000.lock`..`operation-0fff.lock`, indexed by the first 12 SHA-256 bits of canonical injection ID) and 1,024 fixed target stripes (`target-000.lock`..`target-3ff.lock`, first 10 SHA-256 bits of canonical FQN) below `config_dir()/pty-input-locks/`. Empty stripe files contain no payload, identity, token, path, or nonce and are never deleted/replaced, avoiding Unix unlink/recreate split-lock races and unbounded file growth. Lazy creation uses no-follow `create_new`/open validation, Unix mode 0600, and inherited per-user Windows ACLs; pre-existing nonempty/multi-link/reparse stripe objects fail closed. After acquiring a stripe, re-resolve the directory entry and require it still names the opened object, closing open-versus-path replacement. Hash collisions only serialize unrelated work. Rust `File::try_lock`/RAII guards, not timestamps, prove ownership across process suspension.
- Privileged `api-clients.lock` acquisition is a bounded two-second `try_lock` loop and occurs before taking SessionManager/IdleDetector state. Contention before `actuating` is retryable; contention after the boundary is indeterminate. No suspended registry writer may freeze global session state.
- A host confirmation tag is lowercase SHA-256 over the domain string `ac-pty-input-confirmation-v1`, the injection ID, op ID, and nonce using length-prefixed fields. It is not authority and is never accepted from API input; it only correlates the CLI's request with a terminal artifact without disclosing the nonce. The CLI keeps it in memory and in the request only; it never prints or logs it.
- Full terminal rows and transition-audit detail retain seven days (unconfirmed host rows may retain their full result for 30 days for artifact repair), then compact transactionally to a payload-free `pty_input_tombstones` row. Tombstones retain the sender/op ID uniqueness key, stable sender-incarnation fingerprint, injection ID, nonce hash, request fingerprint, confirmation tag when host-originated, and the metadata-only terminal result indefinitely. Reaping can never make an op ID or nonce reusable: exact retries return the tombstone forever and changed semantics/physical sender generations conflict forever. This deliberately accepts minimal append-only metadata growth because the issue's no-second-injection contract has no idempotency expiry.

## 4. Public contracts

### 4.1 Host CLI

The four payload forms are one required Clap group with `multiple = false`:

```text
agentscommander send --to <canonical-fqn> --send <filename> ...
agentscommander send --to <canonical-fqn> --command clear|compact ...
agentscommander send --to <canonical-fqn> --pty-input <text> --mode wake [--agent <configured-id>] [--confirm-timeout <seconds>]
agentscommander send --to <canonical-fqn> --pty-input-stdin --mode wake [--agent <configured-id>] [--confirm-timeout <seconds>]
```

Rules:

1. Exactly one of `--send`, `--command`, `--pty-input`, or `--pty-input-stdin` is required. All six pairs fail in Clap before `execute`.
2. `--pty-input` uses `allow_hyphen_values = true` and is parsed as `OsString`, then converted with strict UTF-8 after Clap so a conversion error reports only `invalid_text` and never echoes the value. `--pty-input-stdin` is a flag and locks stdin, wraps the lock in `Read::take(PTY_INPUT_MAX_BYTES as u64 + 1)`, calls `Read::read_to_end` exactly once, rejects the 65,537th byte, then uses `String::from_utf8`; do not use unbounded reads, `read_to_string`, lossy decoding, trimming, or line reads. The helper uses the identical bounded-read shape.
3. PTY input conflicts with `--get-output` and with the testing/escape-hatch `--outbox` flag. The privileged filesystem request must use the canonical outbox derived from the live sender root. `--timeout` remains only for the legacy response path.
4. `--mode` must be `wake` as today.
5. `--to` must already be the exact qualified `<project>:<wg-N-team>/<agent>` returned by `list-peers-lean`. PTY input rejects bare, origin, workgroup-local, filesystem-directory, Root URI, wildcard, and merely resolvable aliases. Ordinary sends retain current target compatibility.
6. `--agent` remains a configured coding-agent entry ID and affects only a required spawn/respawn. `auto` is the default; any explicit value must pass the existing `coding_agent_mutations::validate_custom_agent_id` grammar exactly (`^[a-z0-9][a-z0-9_-]{0,63}$`, 1..=64 bytes). It never selects a session ID, backend, executable, argv, or provider directly.
7. `execute` determines the payload variant before target resolution. Standard payloads keep the existing `resolve_agent_target` plus broad routing path. PTY payloads bypass that path, require exact-FQN equality from the narrow resolver, and never call `can_communicate`.
8. Host preflight uses the same read-only hierarchy resolver as the daemon for immediate UX, but the daemon remains authoritative.
9. A master/root credential without a live session UUID is rejected for PTY input even though it remains valid for existing host-authority verbs. Concretely, a PTY branch rejects `validate_cli_token(...).is_root == true`; a live Root session token is UUID-shaped and is authorized only by the daemon's Root session lookup.
10. CLI help explicitly warns that the caller's shell performs quoting and expansion before AC receives an argument. It recommends stdin for multiline, leading-hyphen, clipboard, and process-list-sensitive values.
11. Build `issuedAt`, `expiresAt`, and legacy `timestamp` from one UTC instant and one canonical RFC3339 `Z` string; generate UUID-v4 injection/op ID and a distinct UUID-v4 nonce once. `agent == "auto"` serializes as no nested `agentId`.
12. The CLI prints `Queued` with the injection ID only after successful atomic publication, before waiting, and explicitly states that queued is not injected. The pre-publication line is labeled `Operation ID`, never `Queued`. Terminal output uses only `Injected`, `Rejected`, or `Indeterminate` and never prints the text.
13. Confirmation timeout exits 1 without canceling the operation. It prints the stable injection ID, says not to resubmit under a new ID, and names the metadata-only terminal artifact locations to inspect later.
14. PTY input requires `--root` to be byte-equivalent after canonical resolution to the verified replica root (or canonical Root directory), not merely a descendant. The CLI creates no privileged directory through a symlink/reparse component and never falls back to the app outbox.
15. Before publishing, generate and print/flush only the stable injection/op ID; retain the nonce-derived confirmation tag in memory without displaying it. Serialize once into a payload-bearing buffer that has no `Debug`, write with no-follow `create_new` (Unix mode 0600, inherited per-user Windows ACL) to a unique non-`.json` sibling, flush and `sync_all`, atomically rename to `<injectionId>.json`, then best-effort fsync the parent. The poller ignores temporary suffixes. A publish/fsync ambiguity is reported as `Indeterminate` with the same ID; the CLI checks whether the final file or a correlated artifact exists and never generates a replacement ID.
16. Stale privileged temp files are metadata-checked and deleted, never archived, by the specialized poller only after the request TTL and only after obtaining their operation lock. This bounds crash residue without exposing plaintext to generic rejection.
17. `--confirm-timeout` is parsed with a checked upper bound of 3,600 seconds. Confirmation opens artifacts no-follow, reads at most 16 KiB, strictly validates the directory/status pair plus version, IDs, verified sender/target, length, digest, source plane, and confirmation tag, and derives rejection text from the fixed reason enum. It never trusts or prints an arbitrary `.reason.txt` body.

### 4.2 Host typed wire

Add one optional field to `OutboxMessage`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub pty_input: Option<PtyInputWirePayload>
```

The JSON for the new action is:

```json
{
  "id": "<injection UUID>",
  "token": "<live session UUID>",
  "from": "<claimed sender, checked but not trusted>",
  "to": "<canonical target FQN>",
  "body": "",
  "mode": "wake",
  "getOutput": false,
  "preferredAgent": "",
  "priority": "normal",
  "timestamp": "<same instant as issuedAt>",
  "action": "pty-input",
  "ptyInput": {
    "version": 1,
    "text": "exact text",
    "enter": "agent-submit",
    "injectionId": "<same UUID as id>",
    "opId": "<same UUID as id on host plane>",
    "issuedAt": "<RFC3339 UTC>",
    "expiresAt": "<issuedAt + 10 minutes>",
    "nonce": "<different UUID v4>",
    "agentId": "<optional configured agent ID; absent means auto>"
  }
}
```

`PtyInputWirePayload` uses `#[serde(rename_all = "camelCase", deny_unknown_fields)]`. `enter` is a Serde enum with only `agent-submit`. IDs are canonical lowercase hyphenated UUID v4, not nil, another UUID version, or an alternate spelling. The daemon validates byte-for-byte equality of the canonical `timestamp` and `issuedAt` strings, fixed TTL, skew, expiry, distinct nonce, and host equality rules above.

Do not classify through `serde_json::Value` alone because it silently collapses duplicate object keys. Open the source no-follow, validate the opened handle, and read at most `PTY_INPUT_HOST_ENVELOPE_MAX_BYTES + 1`; any larger canonical outbox JSON is rejected through the metadata-only path regardless of claimed action, so a huge handcrafted privileged file cannot force allocation or hide its discriminator after padding. Before BOM/UTF-8 decoding, probe the retained bytes for ASCII, UTF-16LE, and UTF-16BE encodings of the privileged key/value tokens. Then use a top-level map visitor that records every key, flags duplicates, and marks the input privileged if any occurrence has key `ptyInput`, any `action` occurrence has string value `pty-input`, or any `kind` occurrence has string value `pty-input-marker`. For a privileged candidate, reject BOMs/non-UTF-8, reject duplicate keys at every nested object layer, and deserialize the retained value into a dedicated `PtyInputHostEnvelope` with `deny_unknown_fields` at the top level. Its required fields are exactly `id`, `token`, `from`, `to`, `body`, `mode`, `getOutput`, `preferredAgent`, `priority`, `timestamp`, `action`, and `ptyInput`; it has no serde defaults. Forbidden legacy/action parameters are therefore rejected before conversion to the backward-compatible `OutboxMessage`.

Well-formed inputs classified as standard continue from the same already-opened, bounded byte snapshot through the existing parser so valid standard behavior and old duplicate-key behavior are unchanged. Any syntactically malformed, truncated, invalid-encoding, or over-limit outbox document, whether or not the raw probe recognizes a privileged token, uses the specialized metadata-only malformed path and can never reach `reject_raw_file`. JSON permits escaped key/value spellings such as `"pty\\u0049nput"` and `"pty\\u002dinput"`; once parsing has failed, a finite substring probe cannot prove a malformed file is ordinary. Conservatively redacting every invalid document is the only decisive way to prevent an obfuscated privileged payload/token from being archived. This changes only invalid outbox-file diagnostics, not any valid standard message. Valid UTF-8/UTF-16/BOM standard messages retain their existing decode behavior; privileged candidates reject BOM/non-UTF-8.

Payload-bearing request/enqueue structs must not derive a plaintext `Debug`. Replace `OutboxMessage`'s derived `Debug` with a manual implementation that always redacts `token` and represents both standard `body` and PTY text by length/digest only; a redacted nested implementation alone is insufficient because the existing outer derive exposes the token. Give `PtyInputWirePayload`, `ContainerApiToken`, `ContainerStartRequest`, `ApiClient`, registry snapshots, fresh-auth guards, session-transport query/hello carriers, and runtime credential-binding types manual redacted implementations or no `Debug`; API request/enqueue types and helper payload/request types implement no `Debug`. Bearer secrets/hashes, tickets, confirmation tags, child env, argv, host roots, and payloads therefore cannot appear through formatting. Sentinel tests exercise `{value:?}`, every public error conversion, and captured logs.

For `action == "pty-input"`, all of these are mandatory:

- `ptyInput` present, version 1, and valid metadata;
- empty standard `body`;
- no `command`, `requestId`, `senderAgent`, `target`, `force`, `timeoutSecs`, `switchCodingAgent`, `switchProfile`, `dryRun`, or `quietPeriodMs`;
- `getOutput == false`, mode `wake`, and no existing-field provider/session selection (`preferredAgent` must be empty);
- only the exact privileged top-level key set enumerated for `PtyInputHostEnvelope`; known legacy keys are still forbidden mixtures.

A `ptyInput` field without the action, the action without the field, another action with the field, and every mixed privileged/legacy form fail closed. The duplicate-aware probe classifies before strict typed deserialization, so an unknown nested field/enum is rejected through the redacted PTY-input artifact path rather than preserved as a raw rejected file.

Old standard JSON remains valid because the field defaults to `None`. An old daemon ignores the new optional field but sees the unknown `pty-input` action and rejects before mode delivery, so the empty body cannot be delivered as an ordinary message.

All existing `OutboxMessage` literals receive only the mechanical `pty_input: None` addition. Do not refactor their constructors.

### 4.3 Text validator

`pty/inject.rs::validate_pty_input_text` is the single in-process validator used by host CLI preflight, host daemon ingestion, and API ingestion. Daemon/API validation is always repeated. The helper is a separate workspace crate and cannot call this function without adding a forbidden dependency edge, so it keeps a small mirrored validator for UX only. Both implementations consume one checked-in `crates/session-bridge/tests/fixtures/pty_input_validation.json` table via `include_str!`; host and helper tests must execute every fixture row. The server remains authoritative if the mirror ever drifts.

It accepts valid UTF-8 with byte length `1..=65_536`, including whitespace-only values, printable Unicode, LF (`U+000A`), and TAB (`U+0009`). It preserves the resulting bytes exactly and performs no trim, Unicode normalization, line-ending normalization, wrapping, prefix, suffix, or source-label insertion.

It rejects:

- CR (`U+000D`), NUL (`U+0000`), ESC (`U+001B`), and DEL (`U+007F`);
- every other C0 control in `U+0001..U+001F` except LF and TAB;
- every C1 control in `U+0080..U+009F`;
- `U+2028` and `U+2029`;
- the Unicode `Bidi_Control` set: `U+061C`, `U+200E`, `U+200F`, `U+202A..U+202E`, and `U+2066..U+2069`.

Shell metacharacters, quotes, backticks, `$()`, redirection symbols, pipes, ampersands, semicolons, slashes, and leading hyphens are ordinary accepted text. Validator errors report only a stable reason code, byte/scalar offset, and code point, never a text preview.

The canonical limit moves to `pty/backend.rs::PTY_INPUT_MAX_BYTES`. `container_backend::MAX_TRANSPORT_FRAME_BYTES` aliases it, preventing host target validation and the container one-frame ceiling from drifting. The helper keeps a same-value constant pinned by a contract test because it is a separate crate.

### 4.4 Public result DTO

Add typed status/result definitions in `phone/types.rs`:

- `PtyInputPublicStatus`: `queued`, `actuating`, `injected`, `rejected`, `indeterminate`;
- `PtyInputReason { code, detail }` with fixed, non-payload-bearing detail;
- `PtyInputQueueMarker`, a metadata-only host outbox record containing only marker version, injection ID, and op ID after daemon ingestion;
- `PtyInputResult` with version, a safe valid injection ID, optional validated op ID and verified canonical sender/target, public status, terminal boolean, optional byte length/SHA-256, source plane, selected session/backend when known, optional issued/expiry/queued/actuating/terminal timestamps, and optional reason. Optional fields use `skip_serializing_if`; a pre-enqueue malformed/authorization rejection uses the validated request ID or a server-generated artifact ID and omits any field that was not independently parsed/verified instead of echoing caller claims. Every enqueued/status operation has op ID, sender/target, digest/length, and request timestamps.

No result contains text, token, nonce, unverified identity, shell-escaped text, command/argv/env, host path, or a preview. Host filesystem artifacts wrap the public result in `PtyInputHostArtifact { result, confirmation_tag }`; the tag is present only for a fully parsed CLI request, is compared to the CLI-held expected value, is not emitted in API status, and is not an authorization input. A malformed request whose tag cannot be derived may receive a server-ID rejection artifact, but the CLI treats it as uncorrelated/indeterminate rather than trusting it.

Internal `preparing` and `retry` both serialize publicly as `queued`. `actuating` is public and nonterminal so a status caller knows that replay is forbidden. Only `injected`, `rejected`, and `indeterminate` are terminal.

Definitions:

- `injected`: the backend accepted the exact text write and the required first `\r` write. It does not assert model consumption, understanding, or completion.
- `rejected`: the operation terminalized before the `actuating` transaction, so zero PTY writes occurred.
- `indeterminate`: the `actuating` transaction committed but complete text-plus-first-Enter submission cannot be durably proven.

## 5. Authorization and identity

### 5.1 New narrow resolver

Add these symbols to `config/teams.rs`:

- `PtyInputAuthorityKind::{Coordinator, Root}`;
- `VerifiedPtyInputIdentity` for canonical project/workgroup/agent, replica root, matrix root, and coordinator/member role;
- `VerifiedPtyInputRoute` containing canonical sender/target identities and kind;
- `verify_pty_input_coordinator_root(root: &Path)` for container scope minting;
- `verify_pty_input_route(sender_cwd, sender_is_root, target_fqn, project_paths)` for ingress and revalidation.

This resolver is read-only. It must use `replica_identity::read_wg_replica_config_read_only`, never the repair writer, and it must not call `can_communicate`, `resolve_agent_target`, `discover_teams`, or the repairing `resolve_wg_coordinator_replica` path.

Add a strict PTY FQN parser: exactly one `:`, an exact project folder name, local form `wg-<ASCII digits>-<nonempty team>/<agent>`, exactly one `/`, and a validated ASCII alphanumeric/hyphen agent. After resolving disk identity, reconstruct the canonical FQN and require byte-for-byte equality with caller input. Case aliases, origin/bare/local aliases, extra separators, filesystem prefixes, Root URI, and wildcard-like strings reject rather than normalize.

For every identity-bearing project, Project AC Root, `_team_*`, `wg-*`, `__agent_*`, `_agent_*`, config, local-config, and privileged outbox/artifact path, it:

1. walks each expected component with `symlink_metadata` and rejects symlinks plus `FILE_ATTRIBUTE_REPARSE_POINT` on Windows, not merely a symlink at the final component; opened mutable/config/lock/source/artifact files must also have one hard link;
2. canonicalizes each accepted anchor once, retains the verbatim form for I/O, strips it only for display, and compares Windows paths with ordinal case-insensitive semantics plus object IDs (exact component bytes on Unix);
3. requires canonical descendants to remain under the expected project/Project AC Root/replica root;
4. derives names from the canonical layout and read-only replica identity, never from a caller role label or directory spelling alone;
5. parses the workgroup team suffix and the matching `_team_<team>/config.json`, requiring regular non-reparse files before and after each read;
6. treats team `coordinator`/`agents[]` strings only as opaque identity references: use `agent_bare_name_from_ref`, never open or canonicalize the referenced path, then require the corresponding real local `_agent_<name>` directory. This preserves legacy relative/stale absolute references without following an escape;
7. rejects any mismatch, missing file, non-regular file, duplicate bare identity, ambiguous identity, changed metadata during read, or post-canonicalization escape;
8. represents every opened project/workspace/workgroup/replica/matrix/config anchor as `VerifiedPathIdentity { canonical_path, object_id, metadata, content_sha256: Option<[u8; 32]> }`, where object ID is volume+file ID on Windows and device+inode on Unix. Security reads validate the opened handle before and after the bounded read. These fingerprints are stored with queued authority and must match at dispatch/final revalidation; every route carries a generic CWD identity and verified WG routes additionally carry an optional replica-anchor fingerprint captured at registration, so replacing a directory at the same spelling cannot retarget an old PTY;
9. recursively rejects duplicate JSON object keys in identity-bearing team, replica, local, settings, and API-client documents without rejecting unrelated unknown compatibility fields. It derives role and membership from the retained duplicate-free snapshot only;
10. deduplicates repeated settings entries that resolve to the same project object ID, but rejects two different project directories whose basename is equal under Windows ordinal case-insensitive comparison (exact bytes on Unix). Never implement path comparison with lossy strings or Unicode lowercase. The FQN parser caps the whole value at 1,024 UTF-8 bytes, rejects control/bidi/path syntax in the project component, and requires the workgroup/team/agent segments to match their actual validated directory entries.

A target session's trusted CWD must canonicalize to the verified replica root or a real non-reparse descendant of it; string-only `agent_fqn_from_path` is not enough for privileged selection. Backend route kind must equal `SessionInfo.backend_kind`. Coordinator role is scoped to the matching workgroup's team config, not broad `is_any_coordinator` status from another team.

Coordinator route:

- sender session CWD must resolve to a real replica whose identity is the matching team's coordinator;
- target must be a real replica under the same canonical project and exact same `wg-*` directory;
- target identity must occur in that team's `agents[]` and must not equal the coordinator;
- self, Root, an origin agent, another coordinator, another project/workgroup, and spoofed/non-member replicas reject.

Root route:

- sender session must have `is_root_agent == true`, use the canonical configured Root directory, be a local-process session, and pass regular-directory/reparse checks;
- target must resolve from current trusted `settings.projectPaths` to the verified coordinator replica of its exact workgroup;
- worker, origin coordinator, spoofed coordinator, Root URI, and Root itself reject.

Make `root_agent.rs` expose a narrow read-only `verify_live_root_agent_path` wrapper around its existing canonical-path and reparse logic. Do not weaken the standard Root messaging validator.

### 5.2 Live authority sources

Host ingress uses a new `SessionManager::find_unique_live_by_token`; zero or more than one matching non-pending session fails closed, and the canonical UUID-v4 token string must round-trip exactly. It stores the uniquely matched session ID. The record must be non-exited, have `backend_kind == LocalProcess`, and have a live PTY route of the same backend kind. A coordinator sender requires the route's optional verified replica anchor to match; Root requires its generic CWD object identity to match the canonical Root anchor and must have no forged WG authority. The privileged action is rejected from the instance app-outbox and from any noncanonical/custom outbox. It never authorizes from `msg.from`, outbox placement, master/root credentials, target spelling, or a CWD fallback. `msg.from` must still equal the session-derived canonical FQN, but the derived value is what is persisted. The outbox path is only a confinement check: after identity is derived from the live token, canonicalize the expected `<session cwd>/<local-dir>/outbox`, require the source file's parent and filename `<injectionId>.json` to match it, and reject every symlink/reparse component.

API ingress requires all of:

- a bearer client with the distinct `pty-input` scope;
- `ApiClient.bound_session_id` and `credential_generation`, optional backward-compatible registry fields populated together only by container session minting;
- a current, non-exited `SessionManager` record with that ID, container backend, live PTY route, and canonical CWD equal to `boundRoot`;
- a fresh coordinator verification from that live CWD.

Manual `api-client mint` writes `boundSessionId: null` and `credentialGeneration: null`. Automatic container minting writes canonical UUID-v4 values for both, and `ContainerTransportBackend` stores `{client_id, credential_generation, bound_session_id, bound_root_object_id, credential_token_hash}` in the pending/attaching/active route state. The hash is compared in constant time and is never logged, exposed, or accepted as a bearer value. The transport ticket consume/hello path and PTY API ingress must match the entire live binding. Without the runtime-held hash, a handcrafted registry row could copy the visible client/generation/session values, replace `tokenHash` with a manual token's hash, and impersonate automatic provenance. Even if a manual token requests the `pty-input` scope or a registry fixture handcrafts every serialized binding field, the server returns 403. A worker's automatically minted token never receives the scope and still fails the fresh role gate if its scopes are tampered.

Privileged authentication bypasses the existing mtime-only cache: `ApiClientStore::authenticate_pty_input_fresh` and `load_active_binding_fresh` lock the dedicated regular non-reparse `config_dir()/api-clients.lock` (stable and never replaced/deleted), then bounded-open `api-clients.json` no-follow, reject duplicate client IDs/generations/token hashes, and read the current unrevoked/unexpired record every time. Locking the replaceable registry file itself is forbidden because atomic replacement would switch inodes. Every registry mutation takes the dedicated lock exclusively across read-modify-write, file fsync, atomic replace, and parent fsync, closing same-mtime revocation and cross-process lost-update races without a new dependency. The ordinary endpoint cache behavior stays unchanged.

### 5.3 Enforcement and post-await revalidation

There are three daemon checks:

1. **Ingress:** validate the live sender and exact route before enqueue. This is before any target-session lookup or lifecycle mutation.
2. **Dispatch start:** re-read the host session or active API client, live sender session, current project/team/replica configs, and target identity before enumerating target sessions. Check `RestoreInProgress`, request expiry, and `PurgeGuard::blocks_agent` here.
3. **Immediately before actuation:** acquire the selected route-generation permit without holding other guards; re-run sender authority, identity fingerprints, restore/purge/expiry, and verify canonical target, route/backend/anchor, non-exited state, strict submission agent, and all four readiness legs. Renew the lease, then commit `actuating`. Because SQLite is offloaded/awaited, repeat authority/client/target/restore/purge/expiry/session/route and four-leg readiness after the transaction and inside the synchronous first-write boundary.

Build ordinary check results as owned safe snapshots and drop both the outer `RwLockReadGuard<SessionManager>` and internal state guard before every later await. After the post-commit check, acquire any API `ApiClientFreshGuard` first through the bounded dedicated-lock path and re-read the exact binding; never wait for that cross-process lock while SessionManager is held. Then call a mailbox-level `prepare_pty_input_boundary` orchestrator using narrow SessionManager/IdleDetector methods rather than making the session domain depend on API/filesystem modules. With only target/input permits and the optional fresh-auth guard held, it performs the last bounded identity check, takes the internal SessionManager write guard, enters an atomic `IdleDetector::prepare_pty_input_boundary` that holds tuning/resize/activity/idle state in one documented order, acquires a generation/anchor-checked per-route `PtyRouteWriteGuard`, validates the exact session and four readiness legs, and marks SessionManager plus IdleDetector busy. It then declares authorization linearized and releases session/idle/auth guards while returning the route guard. The caller immediately invokes the synchronous backend write through that route guard with no await or intervening user code. Route removal/replacement for that session is blocked, but no global route registry or SessionManager/IdleDetector state is held across a potentially blocked 65,536-byte OS write. Later revocation cannot retroactively cancel an already-linearized write. If a post-commit check fails, write zero bytes, terminalize `indeterminate/final_revalidation_failed`, and never replay.

For API operations, each dispatch check uses the fresh locked registry read and verifies unrevoked/unexpired status, `pty-input` scope, original client ID/generation, `boundSessionId`, canonical bound-root object ID, canonical sender, constant-time equality with the runtime-held credential token hash, and the exact active `ContainerTransportBackend` binding. It then re-reads that exact SessionManager ID and requires a non-exited container session, live container PTY route, and matching route-anchor identity. Revocation, registry replacement, handcrafted serialized provenance, role/config change, credential/session reuse, route divergence, or teardown therefore stops a queued operation before side effects.

## 6. Durable operation store and state machine

### 6.1 Shared store initialization

Add `MessageStoreState` in `api/message_store.rs`, initialized exactly once before the Tauri builder chain in `lib.rs`, managed before `.setup`, and available before either API start path or the mailbox poller. It contains either the shared `Arc<MessageStore>`, `Arc<PtyInputActiveOperations>`, and `Arc<PtyInputTargetLocks>`, or a safe recorded initialization code. A store error does not abort unrelated app startup or ordinary messaging; specialized host requests get a redacted pre-actuation rejection and API startup reports its existing readiness error without opening a second store.

`api::start_server` uses `app_handle.try_state::<MessageStoreState>()` and clones the managed store instead of calling `MessageStore::at_config_dir`. The startup block in `lib.rs` and dynamic `commands/config.rs::start_api_server` therefore share one migrated/recovered connection. Update their mock-app helpers to manage a temporary ready/error state; missing state is a fail-soft readiness error, never a panic. Pass the same `Arc<ApiClientStore>` held by `ApiState` into the PTY dispatcher for post-enqueue revocation/scope/session revalidation.

`PtyInputActiveOperations` is a poison-tolerant `Mutex<HashSet<String>>`, and `PtyInputTargetLocks` is a keyed in-process async lock map paired with the target OS stripes. The authoritative RAII claim combines (in order) the operation's exclusive OS stripe, active-set registration, the target's exclusive OS stripe, and the exact-FQN keyed async target guard. Acquire the operation stripe before the conditional SQL claim and both target locks before any target enumeration/destroy/spawn. Keep both through lifecycle, route permit, actuation, boundary metadata, terminalization attempt, and host artifact bookkeeping.

The same logical-target create gate is also a central participant in every `create_session_inner` path for a strict WG-replica CWD, including user, standard mailbox/API wake, restore, and privileged create. An ordinary create acquires target stripe/exact ownership before `mark_spawning` or its SelectionCoordinator create ticket and holds it through finalization/rollback; a privileged create passes a crate-private proof of its already-held permit so it never re-enters the lock. Under the gate, every background/restore/privileged create rechecks SessionManager records, PTY routes, and `PtyManager::archive_liveness` pending-spawn CWDs immediately before `create_pending_session`; an appeared live/pending target returns a typed race disposition to its caller instead of creating beside it. Explicit user intent may still create a second same-CWD session sequentially, preserving that public capability, but concurrent user creation cannot land inside the privileged missing-target window. Non-WG/Root/origin creates retain existing behavior; different targets remain concurrent except harmless stripe collisions. `PtyInputTargetLocks` uses explicit waiter/holder reference counts removed only when the last reservation drops, so a waiter cannot race map eviction and verified targets do not leak process memory.

Lock order is globally: operation stripe OS lock (privileged only) -> active-set entry -> target stripe OS/exact async lock -> SelectionCoordinator create ticket when spawning -> route input gate -> optional bounded final `ApiClientFreshGuard` and identity recheck -> final SessionManager state -> IdleDetector tuning/resize/activity/idle locks -> per-route lifecycle/write guard. Registry writers acquire only the dedicated registry lock and never SessionManager/target/input state. Session/idle/auth guards are released in reverse order before the route guard performs the synchronous backend write; only the operation/target/input ownership and per-route guard may span that write. No global route registry or outer `PtyManager` mutex spans it. SQL locks are never held while acquiring an operation/target/input lock; candidate IDs are selected, the SQL transaction is dropped, then locks are tried and a conditional transaction is opened. No reverse acquisition is allowed. Stripe files are stable installation-scoped synchronization objects and are never unlinked; DB-row/tombstone compaction does not alter them.

### 6.2 Schema migration, version 2

Migration 1 and the existing `messages`/`message_audit` schema stay byte-for-byte in behavior. Migration 2 adds `pty_input_operations` and `pty_input_audit` in one transaction, then records schema version 2.

`pty_input_operations` columns are:

| Column | Contract |
| --- | --- |
| `injection_id TEXT PRIMARY KEY` | daemon/CLI-issued UUID v4 |
| `sender_fqn TEXT NOT NULL` | canonical live sender |
| `target_fqn TEXT NOT NULL` | canonical exact target |
| `op_id TEXT NOT NULL` | idempotency key; unique with sender |
| `nonce_sha256 TEXT NOT NULL` | hash of UUID-v4 nonce; unique with sender; raw nonce is never stored |
| `request_fingerprint TEXT NOT NULL` | domain-separated SHA-256 of all normalized idempotency semantics; persists after payload/profile clearing |
| `confirmation_tag TEXT NULL` | host-only request/artifact correlation hash; NULL for API |
| `version INTEGER NOT NULL` | exactly 1 |
| `enter_mode TEXT NOT NULL` | exactly `agent-submit` |
| `requested_agent_id TEXT NULL` | validated configured ID used only if spawning; cleared at `actuating` or any terminal state |
| `payload BLOB NULL` | exact UTF-8 bytes while safely replayable; NULL at/after actuation or any terminal state |
| `payload_sha256 TEXT NOT NULL` | lowercase 64-hex digest |
| `payload_bytes INTEGER NOT NULL` | 1 through 65,536 |
| `source_plane TEXT NOT NULL` | `host_cli` or `container_api` |
| `sender_incarnation_fingerprint TEXT NOT NULL` | hash of the verified physical sender/root anchor object identity; persists into the permanent tombstone so a replacement directory cannot inherit an old op ID |
| `sender_identity_fingerprint TEXT NULL`, `target_identity_fingerprint TEXT NULL` | queued physical/config authority generations; cleared at boundary/terminal |
| `authority_session_id TEXT NULL` | live sender session reference while queued/preparing; cleared at `actuating` or rejection |
| `authority_client_id TEXT NULL`, `authority_client_generation TEXT NULL` | API runtime credential binding while queued; NULL on host and cleared at boundary/rejection |
| `status TEXT NOT NULL` | internal state below |
| `attempt INTEGER NOT NULL` | bounded preparation attempts |
| `next_attempt_at TEXT NOT NULL` | retry schedule |
| `lease_owner TEXT NULL`, `lease_until TEXT NULL` | preparation only |
| `selected_session_id TEXT NULL` | target session chosen by the engine |
| `selected_backend TEXT NULL` | `localProcess` or `containerTransport` |
| `issued_at TEXT NOT NULL`, `expires_at TEXT NOT NULL` | fixed request validity window |
| `queued_at TEXT NOT NULL`, `preparing_at TEXT NULL`, `actuating_at TEXT NULL`, `terminal_at TEXT NULL`, `host_artifact_at TEXT NULL`, `updated_at TEXT NOT NULL` | state/artifact timestamps |
| `reason_code TEXT NULL`, `reason_detail TEXT NULL` | fixed structured reason, never raw underlying payload/error text |

Constraints/indexes:

- `UNIQUE(sender_fqn, op_id)`;
- `UNIQUE(sender_fqn, nonce_sha256)`;
- due index on `(source_plane, status, next_attempt_at, lease_until)`;
- checks for version, Enter mode, byte/digest/fingerprint shapes, source plane, known internal status/reason values, fixed-width timestamps, host/API authority-column agreement, payload present only in replayable states, payload NULL at/after `actuating`, terminal timestamp presence, and lease presence only in `preparing`;
- transition methods use conditional updates and require exactly one changed row before inserting the audit event. Idempotent repeats must match the existing terminal transition; zero/multiple-row updates are typed store corruption, never success.

Migration refuses an unknown future schema version. Open SQLite with `rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW` in addition to the required read/write/create flags, from a verified non-reparse parent; reject unsafe pre-existing database, `-wal`, `-shm`, and rollback-journal objects and verify every sidecar SQLite creates before admitting privileged work. The DB/config directory and fixed lock stripes are likewise opened no-follow and checked as regular non-reparse descendants; an unsafe store disables only the specialized operation/API readiness path with a fixed code. Touched production store/lock code returns typed poison/overflow/join errors and introduces no `unwrap`, recoverable `expect`, silent `let _ =`, or `unwrap_or_default` fallback.

`pty_input_audit` stores one row per state transition with only: event ID, injection/op IDs, canonical sender/target, version, byte length, SHA-256, source plane, selected session/backend when known, status, reason code, and timestamp. It has no payload/token/nonce/path/error-preview column.

Migration 2 also creates `pty_input_tombstones`. Its primary/unique keys cover `(sender_fqn, op_id)`, injection ID, and `(sender_fqn, nonce_sha256)`. It stores only the stable sender-incarnation hash, request fingerprint, host confirmation tag when present, and all fields needed to reconstruct the metadata-only terminal result; it has no payload, requested profile, authority session/client, mutable identity snapshot, token, raw nonce, path, or raw error. A terminal transition upserts the tombstone in the same transaction. After seven days (30 days for an unconfirmed host artifact), compaction under the operation lock verifies byte-equivalence with the tombstone, removes the full operation/audit detail, and leaves the tombstone indefinitely. Exact duplicate enqueue and GET status query both consult live rows then tombstones in the same transaction. A current sender whose FQN matches but physical incarnation hash differs gets conflict on POST and not-found on GET, never old metadata or a new injection. Artifact publication conditionally records `host_artifact_at`; a crash between publication and that update is repaired from either the full row or tombstone. Standard-message retention is unchanged.

### 6.3 State machine

```text
new request
  -> queued
queued | retry | expired preparing lease
  -> preparing (lease + attempt increment)
preparing
  -> retry       only for a classified transient failure before actuating
  -> rejected    permanent failure, expiry, or attempt exhaustion; payload NULL
  -> actuating   atomic boundary; payload returned to memory and set NULL in DB
actuating
  -> injected
  -> indeterminate

No transition leaves injected/rejected/indeterminate.
No transition from actuating returns to queued/preparing/retry.
```

Public mapping:

- `queued`, `preparing`, `retry` -> `queued`;
- `actuating` -> `actuating`;
- terminal states map one-to-one.

### 6.4 Transactions

1. **Enqueue/idempotency transaction:** validate before opening the transaction; canonicalize absent/`auto` agent selection to the same value; compute a domain-separated, length-prefixed `request_fingerprint`; run the lock-aware expiry sweep outside any enqueue transaction; then use `TransactionBehavior::Immediate` so cross-process duplicate/tombstone lookup, nonterminal quota counts, and insert serialize as one writer decision. Look up `(sender, opId)` in both live rows and permanent tombstones before limits; then enforce admission limits against nonterminal rows and insert `queued` plus its audit row. API fingerprints cover source plane, sender, target, version, Enter mode, payload digest/length, and normalized agent ID, but exclude server-issued nonce/times/injection ID so a retry can reproduce them. Host fingerprints additionally cover its fixed injection/op IDs, nonce hash, issued/expiry strings, and confirmation tag. The separately stored stable sender-incarnation hash must also match. Exact matches return the original injection ID/current or tombstoned terminal state forever. Any semantic/incarnation mismatch is `409 idempotency_conflict` or a redacted host rejection. Nonce-hash/injection-ID collision against either table outside that exact operation fails closed.
2. **Claim transaction:** select candidate IDs without retaining a SQL transaction, acquire the candidate's deterministic operation-stripe OS lock, then conditionally move exactly one still-due source-plane row to `preparing`, set a unique UUID-v4 lease owner/expiry, and increment attempt. Container candidates order by `next_attempt_at, queued_at, injection_id`; host claims use the marker's exact injection ID, so a raw file is not dispatchable. A locked candidate is skipped without burning an attempt. Start a `PreparationHeartbeatGuard` task ticking every 30 seconds; renewal requires the same owner, `preparing`, and an unexpired lease. Heartbeat failure is delivered over a watched state and stops the engine before `actuating`. Every normal pre-boundary exit calls `finish()` to cancel and await the task; `Drop` cancels and aborts as a panic/cancellation fallback, so dropping a Tokio `JoinHandle` can never leave an orphan heartbeat. Immediately before the actuating transaction, renew once, call a boundary-specific `finish()` and await it, then perform no unrelated await. A heartbeat that observes the already-committed `actuating` state is clean completion, not a false lease failure. The claim never returns payload bytes.
3. **Actuating transaction:** require the same unexpired lease owner and `preparing` state after the heartbeat has been joined. Inside the transaction revalidate payload length, SHA-256, strict UTF-8/text rules, version/Enter/source fields, request fingerprint, and authority-column consistency before selecting bytes. Then set `status = actuating`, `actuating_at`, selected session/backend, `payload = NULL`, `requested_agent_id = NULL`, mutable identity fingerprints and authority references = NULL, and clear the lease; retain the stable sender-incarnation fingerprint; write the audit row; commit. Return bytes only after a definitely successful commit. If commit/spawn-blocking completion is ambiguous, query under the same operation stripe lock: `actuating` means zero-write `indeterminate`, replayable state means safe pre-boundary failure, and an unreadable state fails closed. The engine never calls a backend on an ambiguous commit.
4. **Terminal transaction:** from `actuating`, conditionally set `injected` or `indeterminate`, terminal timestamp and fixed typed reason/warning, and write the audit event. From a pre-actuation state, `rejected` also clears payload, requested agent ID, mutable identity fingerprints, and authority references. Every terminal path inserts the byte-equivalent permanent tombstone in the same transaction before commit. A repeated identical terminalization/tombstone is idempotent without another audit row; a conflicting terminal status or tombstone is corruption, never overwritten. DTO construction validates every retained ID/enum/digest/timestamp before exposing it.
5. **Recovery, run at store initialization:** select recovery candidates, release SQL, and acquire each row's operation stripe lock before a conditional transaction. A lock held by another process proves a live/suspended owner and is skipped. For acquired rows, `actuating` becomes `indeterminate/daemon_restart_after_actuation`; unexpired `preparing` becomes immediately due `retry`; expired queued/retry/preparing becomes `rejected/expired`; every terminalized row clears payload/profile/authority/mutable fingerprints and writes both its audit event and permanent tombstone atomically. Apply expiry first. Startup never assumes its process-local active set describes other daemons.
6. **Runtime orphan recovery:** every API dispatcher tick and mailbox cycle finds `actuating` rows older than 15 seconds, skips local active IDs, and tries its operation stripe lock. Only a row absent locally *and* stripe-lockable cross-process becomes `indeterminate/runtime_actuation_orphan`. Register/lock before `actuating` and release only after terminal/artifact handling. A machine-suspended or blocking-write task retains its OS lock and can never be published terminal before it resumes; a panic, cancellation, process death, or failed terminal SQL releases the lock and is recovered. Actuating rows are never leasable.

7. **Runtime expiry/orphan-host reconciliation:** the same select-release-lock-conditional pattern acquires each row's operation stripe before terminalizing expired queued/retry/preparing rows, even when their raw host file/marker vanished; a live preparing owner is skipped. Reconciliation clears payload and frees admission quota only after ownership is proven. Host rows without a correlated artifact remain queryable and are not mistaken for confirmed delivery; if a valid marker later reappears, it materializes the stored terminal truth. Orphan privileged temp files are scrubbed under their operation lock after expiry.

SQLite/WAL may physically contain historical page bytes like any plaintext queue, but no live or terminal SQL row, audit row, result artifact, event, or log retains the payload after the boundary. Documentation must identify the nonterminal queue/outbox as sensitive. Do not claim forensic secure erasure from an SSD or SQLite WAL.

### 6.5 Retry boundary

Only these pre-actuation conditions are retryable:

- restore or purge temporarily blocks the target;
- a target record/PTY route races away before selection is committed;
- a preparation lease is lost before `actuating`;
- spawn infrastructure fails and rollback proves no live/ambiguous replacement remains;
- a transient store operation fails before the boundary.

Busy, unsupported/plain shell, invalid/missing supported profile, identity/authority change, expired request, ambiguous unsafe lifecycle state, post-spawn readiness timeout, and any authorization/routing failure are terminal `rejected` with zero writes.

After `actuating`, text-write failure, first-Enter failure, daemon stop, task cancellation, lease expiry, terminalization failure, or a final revalidation failure is `indeterminate` and never retryable. Failure of only the redundant second Enter remains `injected` with reason code `redundant_enter_failed` for audit/status.

### 6.6 Typed failure taxonomy

Add `PtyInputFailure { code: PtyInputReasonCode, class: PtyInputFailureClass }`; do not classify by substring or retain a legacy error string. `PtyInputFailureClass` is exactly `RetryBeforeBoundary`, `RejectBeforeBoundary`, or `IndeterminateAfterBoundary`. `PtyInputReasonCode` is a Serde snake-case enum with these stable families:

| Class | Codes |
| --- | --- |
| Validation/reject | `invalid_envelope`, `mixed_payload`, `unsupported_version`, `invalid_enter_mode`, `invalid_id`, `invalid_nonce`, `invalid_timestamp`, `expired`, `invalid_target`, `invalid_text`, `payload_too_large`, `idempotency_conflict`, `capacity_exceeded` |
| Authority/reject | `session_token_required`, `invalid_session_token`, `ambiguous_session_token`, `sender_session_not_live`, `sender_backend_not_local`, `sender_identity_invalid`, `sender_not_coordinator`, `root_identity_invalid`, `target_not_member`, `target_is_coordinator`, `target_out_of_scope`, `unsafe_path`, `api_scope_required`, `api_client_unbound`, `api_client_stale`, `api_binding_mismatch`, `authority_changed` |
| Lifecycle/reject | `busy`, `resize_unsettled`, `untracked_readiness`, `unsupported_session`, `nonpersistent_live_session`, `inconsistent_session`, `unsupported_profile`, `readiness_timeout`, `store_corrupt` |
| Retry before boundary | `restore_in_progress`, `purge_in_progress`, `session_race`, `lease_lost`, `spawn_failed_safe`, `store_transient` |
| Indeterminate | `final_revalidation_failed`, `text_write_failed`, `required_enter_failed`, `daemon_restart_after_actuation`, `runtime_actuation_orphan`, `terminal_store_failed` |
| Injected warning | `redundant_enter_failed`, `boundary_metadata_failed` |
| Maintenance/audit only | `artifact_unclaimed` |

Provide one exhaustive `safe_detail(code) -> &'static str` match. Public details, reason files, audit, events, HTTP errors, and logs use only this mapping. Tests pin every enum serialization and ensure no mapping accepts caller/OS/library text. Attempt exhaustion converts the last retry code to terminal `rejected` while preserving that fixed code; it does not invent a raw `last_error`.

## 7. Target lifecycle and PTY submission

### 7.1 Deterministic persistent-session selection

Add a pure `select_pty_input_candidate` policy plus mailbox adapters. Acquire the canonical-target lifecycle lock before the first enumeration and hold it through terminalization. Filter by exact target FQN and discard legacy `[temp]` records from eligibility.

For non-exited records, probe the PTY route and trusted submission kind. A selectable idle candidate has all of: persistent, live route, route/backend agreement, route replica-anchor fingerprint equal to the verified target, non-exited, supported coding agent, and `waiting_for_input == true`. `SessionStatus::Active|Running|Idle` is only the ranking bucket; it never overrides readiness. Selection is followed by the strict IdleDetector gate below; a missing/untracked snapshot never means idle. Rank eligible records by:

1. `Active`, then `Running`, then `Idle`;
2. newer `createdAt`;
3. UUID lexical order as the final stable tie-break.

Select one only. Never iterate writes and never fan out. If no eligible live record exists:

- any supported but non-idle live record makes the request terminal `busy`;
- otherwise a live plain/unsupported record makes it terminal `unsupported_session`;
- a non-exited record with no route is re-probed once and treated as a retryable race; after bounded retries it rejects `inconsistent_session` and never spawns alongside the phantom;
- a live temporary record prevents a duplicate spawn and rejects `nonpersistent_live_session`;
- any `PtyManager::archive_liveness` pending-spawn mark whose canonical CWD belongs to the target is a retryable `session_race` and is re-enumerated under the shared create gate; it is never treated as missing.

If there are no live records, select only the newest persistent exited record, with UUID as tie-break. Parse `SessionInfo.created_at` as RFC3339; an invalid timestamp ranks oldest, and UUID lexical order breaks equality. Extra exited records are neither written nor destroyed. A route/backend mismatch, duplicate live route, or non-exited record whose canonical CWD cannot be proven under the target root is `inconsistent_session`, never a spawn invitation.

### 7.2 Lifecycle table

| Target state | Decided behavior |
| --- | --- |
| Live, idle, supported | Select one, acquire its writer permit, revalidate, begin actuation, submit once. |
| Live busy/non-idle | Terminal `rejected/busy`; zero writes, no wait/defer, no spawn. |
| Live unsupported/plain shell | Terminal `rejected/unsupported_session`; zero writes, no spawn. |
| Exited persistent | Resolve and validate a supported spawn profile before mutation; destroy the selected record, respawn persistent on its configured backend with `skip_auto_resume = false`, carry its Telegram/communication intent through existing helpers, wait for strict sustained idle, then submit once. |
| Missing | Use the verified target replica root, resolve a supported configured profile, spawn persistent on its configured backend with `skip_auto_resume = true`, wait for strict sustained idle, then submit once. |
| Missing/exited without supported profile | Reject before destroy/create. No unintended session remains. |
| Multiple records | Apply the selector above and touch at most one. Never fan out. |
| Race after selection | Before boundary, retry/reject without a write. After boundary, indeterminate without replay. |

Spawn profile resolution order is fixed:

1. explicit non-`auto` `agentId`, which must exist and be supported when a spawn is actually needed; an invalid explicit override does not silently fall back;
2. selected exited record's configured `agent_id`, if still valid and supported;
3. target replica `currentCodingAgent`;
4. target local config `lastCodingAgent`;
5. first configured `settings.agents` entry whose effective target-CWD command is supported.

Read current/last config only through the already verified target root and reject symlink/reparse config files. There is no sender-agent fallback. For each candidate, call `build_configured_agent_spawn_for_cwd(&settings, id, verified_target_root, None)` first, then run the shared trusted submission detector on the returned effective shell/args. Do not use `normalize_agent_for_wake`, raw command strings, or sender settings as a privileged fallback. Preserve the returned `AgentSpawnCommand.backend`, profile/env, mounts, and trusted agent ID through `create_session_inner`.

Lifecycle mutation rules remove the standard wake path's ambiguous best-effort behavior:

1. Snapshot and validate the profile, authority, selected record, and all matching records before destroy/create. Any await between that snapshot and a mutation forces another synchronous strict authority/target check plus owned session snapshot immediately before submitting `background_destroy_session_inner` or `create_session_inner`. Renew the preparation lease around every long await.
2. For exited restart, capture Telegram/communication intent from only the selected record. After `background_destroy_session_inner`, re-list and probe. Spawn only when the selected record/route is gone and no non-exited target record appeared. A failed destroy with a surviving/ambiguous record is retryable only within the bounded policy and never spawns beside it.
3. For missing spawn, call `create_session_inner` once with `CreateSelectionIntent::Background`, the verified target root, persistent readable name, resolved `AgentSpawnCommand`, and fixed resume flag. On an error, re-list/probe before classifying `spawn_failed_safe`; if any replacement or pending create may exist, reject `inconsistent_session` rather than retrying another spawn.
4. After a successful spawn, verify returned session CWD, backend, route, and submission agent against the prevalidated plan before carrying Telegram/communication intent. A mismatch is terminal and must not be injected.
5. Readiness uses a dedicated `evaluate_pty_input_readiness` over one correlated `SessionInfo` plus one `IdleDetector::purge_readiness` snapshot. Ready requires mirror `waiting_for_input`, watcher idle, `activity_age >= idle_threshold + 2s`, and no resize or `last_resize_age >= resize_grace + 2s`, with checked duration addition. Missing activity/tuning/route is `untracked_readiness`, never ready. A pre-existing live candidate that is genuinely busy rejects immediately; only a fresh-idle/resize-settling candidate may wait, and if it becomes busy it rejects rather than deferring stale text. A session spawned by this operation may ride startup churn, polling every 500 ms until all four legs hold or 90 seconds/expiry; it never injects on the cap. Every poll verifies route generation/anchor, session existence, authority, and lease heartbeat.
6. A successfully created persistent session remains visible if later readiness, expiry, or authority revalidation rejects; do not destroy a healthy session merely to roll back text delivery. Only the existing create cancellation/rollback machinery cleans a partial failed create. Record this distinction in tests.

### 7.3 Trusted coding-agent eligibility

Add `PtySubmissionAgent::{Claude, Codex, Gemini, CursorAgent}` in `session/profile.rs` and methods on `Session`/`SessionInfo` in `session/session.rs`.

- `Session::agent_kind` is an identity hint, not sufficient proof: today's prefix-scanning detector can classify `bash -c "echo claude"`. A privileged match requires the hint (when present) to agree with a new executable-position grammar over trusted effective shell/args.
- Direct Claude/Codex/Gemini executables and configured direct wrapper basenames such as `claude-*` are accepted only when they occupy the executable position. A Windows `cmd.exe /C` wrapper is accepted only when its first and only command is the agent executable/wrapper plus literal arguments: reject `CALL`/`START`, command grouping, CR/LF, quotes that do not round-trip under Windows command-line parsing, every cmd metacharacter/operator, and `%...%`/`!...!`/caret expansion that could synthesize a follow-on command after validation. `cmd.exe /K` is always rejected for privileged input: once the coding agent exits, `/K` leaves an idle `cmd` prompt behind while stale session metadata still says Codex/Claude/Gemini, turning literal PTY text plus Enter into host-shell execution. Mentions in arbitrary args, `echo claude`, compound preambles, PowerShell/bash evaluators, and conflicting kinds reject.
- A missing legacy kind may be reconstructed only when that strict launch grammar independently proves the kind; raw `CodingAgentKind::detect` alone is not enough.
- Cursor is the exact executable basename `agent`/`agent.exe`/`agent.cmd` in the same direct/first-command positions. Prefixes such as `agentctl` and plain shells are not Cursor.
- Spawn eligibility uses the identical detector on `build_configured_agent_spawn_for_cwd`'s effective command before session creation.

Do not use caller labels or arbitrary `needs_explicit_enter` shell spelling for privileged eligibility. Keep legacy command eligibility behavior otherwise unchanged.

### 7.4 Per-session writer serialization

Change `SpawnRegistry.routes` from `HashMap<Uuid, SessionBackendKind>` to route entries containing `{ kind, generation, input_gate: Arc<tokio::sync::Mutex<()>>, lifecycle_gate: Arc<std::sync::Mutex<()>>, canonical_cwd_identity, verified_replica_anchor: Option<Fingerprint> }`, with a checked monotonically increasing route generation. Every ordinary/root/origin/ad-hoc session gets a generic canonical CWD object identity; only a strictly verified WG route gets `Some(replica_anchor)`. Privileged coordinator/member selection requires `Some` equal to the current verified replica, while Root uses its generic canonical Root anchor. Requiring a replica fingerprint for every route would brick origin, Root, and arbitrary-shell compatibility. `record_route` rejects an already-present UUID instead of silently replacing its gates; a real replacement must remove the old route first. Generation overflow fails route registration rather than wrapping.

Expose these exact facade contracts:

```rust
pub struct PtyInputPermit {
    session_id: Uuid,
    route_generation: u64,
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    backend: Arc<dyn PtyBackend>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

pub async fn acquire_input_writer(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyInputPermit, AppError>;

pub struct PtyRouteWriteGuard<'a> { /* private per-route lifecycle lock + backend; never the global registry */ }

pub fn lock_route_for_write(
    permit: &PtyInputPermit,
) -> Result<PtyRouteWriteGuard<'_>, AppError>;

pub fn write_with_permit(
    permit: &PtyInputPermit,
    bytes: &[u8],
) -> Result<(), AppError>; // convenience path; no outer PtyManager guard
```

`acquire_input_writer` locks the outer manager only long enough to clone the route's input/lifecycle gates, generation, registry, and backend; it drops the outer/global locks, awaits `lock_owned`, then validates generation/anchor briefly through the registry. `lock_route_for_write` uses nonblocking/bounded acquisition of only that route's lifecycle gate, briefly rechecks registry ID/generation/anchor, drops the registry, and exposes a payload-blind backend `write`; it never waits on that gate while SessionManager/IdleDetector guards are held. Route removal/re-registration takes the same per-route gate, but backend kill/transport teardown may invalidate and terminate a blocked backend first and defer only registry removal, so teardown can unblock a stuck OS write. A container backend error callback must never synchronously wait on the lifecycle gate held by its own write. The convenience `write_with_permit` uses this guard. Thus remove/recreate cannot land between check and write, a blocked write cannot freeze unrelated route registration/removal, and the outer manager is never needed. Privatize/remove the old public `PtyManager::write`; production code must not have a permitless facade method. Direct `PtyBackend::write` remains backend-internal and test-only outside the facade.

Every production PTY input writer obtains the same permit:

- Tauri `commands/pty.rs::pty_write`;
- web binary PTY input in `web/mod.rs`;
- web JSON `pty_write` in `web/commands.rs`;
- canonical text injection in `pty/inject.rs`, which also covers loops, Telegram, mailbox bodies, self-clear/switch, and resume prompts;
- mailbox graceful-exit direct input.

The complete global order is the order in section 6.1. No SessionManager/IdleDetector/outer-PtyManager guard is held while awaiting a permit. Once a permit is held, owned snapshots may be awaited; `prepare_pty_input_boundary` holds final session/readiness guards only long enough to validate, stamp busy, and obtain `PtyRouteWriteGuard`, then releases them before its immediate synchronous first write. Writes never reacquire the outer manager. The route guard is released before every sleep, DB/filesystem operation, bookkeeping, or later await. Resize/kill do not require an input permit; route removal/kill racing after the first write makes a later checked Enter fail and the already-actuating result indeterminate.

User writers re-check purge after waking. After backend acceptance and before releasing the input gate, every user/automated writer synchronously stamps IdleDetector busy (scheduling the mirror transition) and updates synchronous voice state, so a privileged waiter cannot observe stale idle. The gate is then released before async persistence/badge bookkeeping; those later operations cannot affect readiness because the activity stamp is already authoritative. A privileged operation holds one permit from final checks through the `actuating` commit, post-commit checks, text, sleeps, Enters, boundary metadata, terminal transaction attempt, and artifact state update. At the first-write linearization closure it synchronously marks the target busy in both SessionManager and IdleDetector before calling the backend. Thus user bytes and every automated writer cannot splice into the sequence, and a second operation cannot observe the stale pre-submit idle bit and inject back-to-back.

### 7.5 Phase-aware exact submission

Refactor `pty/inject.rs` without changing the public behavior of `inject_text_into_session`:

- ordinary/legacy injection acquires a permit and retains its current direct-shell Enter detection and nonfatal second Enter;
- a new `submit_exact_agent_input_with_permit` accepts already-validated bytes and returns exactly `AgentSubmitOutcome::{TextWriteFailed, RequiredEnterFailed, Submitted { redundant_enter_failed: bool }}`. It consumes backend errors into the phase enum and never returns/logs a payload-bearing error string.

Resolve the legacy shell before permit acquisition, then acquire the permit and run `inject_text_into_session_with_pre_write_check`'s closure immediately before its first checked write. This preserves the loop stale-delivery check at the real serialized write boundary without holding SessionManager or PTY guards across await.

Privileged sequence:

0. inside the final no-await SessionManager/IdleDetector preparation, re-check strict readiness, acquire `PtyRouteWriteGuard`, transition the target busy, release the global guards, and linearize the operation;
1. immediately perform exactly one `route_write_guard.write(accepted_bytes)` call;
2. `tokio::time::sleep(1500 ms)`;
3. exactly one required `PtyManager::write_with_permit(&permit, b"\r")`;
4. `tokio::time::sleep(500 ms)`;
5. exactly one best-effort redundant `PtyManager::write_with_permit(&permit, b"\r")`.

A synchronous backend write that has not returned keeps the operation stripe, target stripe/exact lock, input gate, and per-route lifecycle guard and public state `actuating`; age recovery must not lie and terminalize it. `LocalProcessBackend::write` must clone the per-session writer under the `ptys` map and release that global map before `write_all + flush`; kill/teardown can then remove and terminate the child without waiting for the writer/lifecycle registry removal, closing the pipe and unblocking the write. Container teardown similarly invalidates the active sender first and defers route removal. Tests use a genuinely blocking writer/transport and prove bounded teardown, unrelated-session progress, retained operation ownership, and no detached write after a terminal result.

No Enter is appended to or embedded in the text. Local `write_all + flush` and one container binary frame remain the backend acceptance boundaries. A 65,536-byte container payload is one accepted frame; 65,537 never reaches a backend.

Boundary metadata is part of completing an injected operation and runs while the input permit is still held, after the required first Enter (and redundant attempt) but before the terminal SQL transaction. Refactor the existing helpers to return a payload-free `BoundaryMetadataOutcome` while preserving legacy best-effort behavior; the privileged caller emits only fixed `boundary_metadata_failed` metadata and never inherits their raw path/I/O logging:

- exact bytes `/clear`: await the existing `stamp_fresh_boundary_to_session`, the same best-effort helper used by legacy `--command clear`;
- exact bytes `/compact`: apply neither a fresh stamp nor a post-boundary drop, matching legacy compact semantics;
- every other accepted text, including whitespace variants of slash commands: await `note_post_boundary_content_to_session`.

Only text plus required-Enter success reaches this step. If the task dies during metadata, the row remains `actuating` and recovers as indeterminate; never publish `injected` and then risk crashing before the required `/clear` effect is attempted. First-Enter/text failure applies no metadata. The terminal `injected` transaction follows metadata; its failure still leaves an unreplayable `actuating` row for indeterminate recovery.

## 8. Dedicated API and container parity

### 8.1 Scope and token provenance

Add `auth::SCOPE_PTY_INPUT = "pty-input"` to `VALID_SCOPES`. Existing `send` never implies it.

Add backward-compatible optional `ApiClient.bound_session_id` and `ApiClient.credential_generation`; the public/manual `MintRequest` always supplies both as `None`, while an internal-only `mint_for_container_session` requires canonical UUID-v4 values. Update every manual call site (`cli/api_client.rs`, `commands/config.rs::mint_api_client_with_path`, auth/config tests, helper fixtures) explicitly. Manual help explains that requesting the scope is not authority; actuation additionally requires the matching live runtime container binding.

`ContainerApiTokenManager::mint_for_session` always creates a fresh credential generation and session binding. Its unique client ID is exactly `container-<session UUID>-<generation UUID>`; under the registry lock it revokes any prior automatic record with that `boundSessionId` plus legacy `container-<session UUID>` records, prunes superseded revoked/expired automatic generations, and retains one inert historical `container:` witness when needed so existing `has_container_clients` cleanup semantics remain true. It never prunes manual or live/unrevoked automatic clients; if those alone exceed the 4,096-row bound, mint fails closed with a fixed capacity code rather than silently dropping authority records. It appends `pty-input` only when `verify_pty_input_coordinator_root(bound_root)` succeeds. It returns client ID, generation, secret, and the secret's SHA-256 binding; `ContainerTransportBackend` installs all non-plaintext binding fields before launch and requires client ID, generation, session, root object ID, and constant-time token-hash equality during ticket consumption/hello and every PTY authority check. Workers retain exactly the existing three automatic scopes. Scope detection failure is fail-closed and logs only client/session ID plus a fixed reason code, not paths or role/config contents.

### 8.2 Strict API DTOs and routes

Add `api/handlers/pty_input.rs` and register:

```text
POST /api/v1/pty-input
GET  /api/v1/pty-input/{opId}
```

POST request, with `deny_unknown_fields` at every layer:

```json
{
  "apiVersion": "1",
  "opId": "<UUID v4>",
  "to": "<canonical FQN>",
  "ptyInput": {
    "version": 1,
    "text": "exact text",
    "enter": "agent-submit"
  },
  "agentId": "<optional configured ID>"
}
```

The API does not accept `from`, token, root, source plane, injection ID, issued/expiry, nonce, session ID, backend, provider, command, action, content type, Enter override, `getOutput`, or response fields. The server derives sender identity and issues injection ID, nonce, one canonical `issuedAt`, and fixed expiry.

Handler order is fixed: raw-body ceiling; exactly one syntactically valid Authorization header; fresh privileged auth/scope/runtime binding while preserving the existing failed-auth lockout and per-client rate gates (with no unverified `boundFqn` audit); reject query parameters, duplicate Content-Type/Content-Encoding headers, any media type except one `application/json` with no parameter or `charset=utf-8`, and any content encoding except absent or one `identity`; strict duplicate-rejecting decode; exact `apiVersion == "1"`; canonical UUID-v4 `opId`; semantic text/target validation; fresh live authority/route verification; then enqueue. Normalize absent or exact `agentId == "auto"` to `None`; any other value must pass `validate_custom_agent_id` exactly (1..=64 ASCII bytes, `^[a-z0-9][a-z0-9_-]{0,63}$`) and is resolved only if spawning. Every DTO layer denies unknown fields, and parse/validation errors map to fixed details rather than Serde/payload debug output. Raise the router ceiling to max(existing `/send`, escaped PTY envelope), retaining `/send`'s smaller handler cap.

POST responses:

- 202 for a new or existing nonterminal operation;
- 200 for an exact duplicate already terminal;
- 400 for malformed DTO/version/ID/target/control/field combinations;
- 401 for absent/invalid/revoked/expired bearer identity;
- 403 for missing scope, manual/unbound token, worker/non-coordinator, or unauthorized route;
- 409 for an opId reused with different semantics;
- 413 for more than 65,536 text bytes;
- 429 for existing lockout/rate behavior or transactional PTY admission capacity;
- 500 for store failures.

GET rejects a query string, authenticates through the same fresh scope/runtime binding, validates the decoded path as canonical UUID-v4, and re-resolves current authority before looking up `(senderFqn, opId)`. It returns 200 metadata-only status or 404. It cannot inspect another sender's operation or use a manual, duplicate, cached-revoked, stale, rebound, or worker token. Add `NotFound`, `Conflict`, and fixed PTY-specific mappings to `ApiError`; existing route mappings remain unchanged.

### 8.3 Dispatcher

The existing standard-message `dispatch_due_with` remains unchanged in signature and semantics. `start_dispatcher` additionally receives the same `Arc<ApiClientStore>` as `ApiState`. Each tick snapshots active-operation IDs, runs runtime PTY orphan recovery, then, when restore/purge permits, claims at most one due `container_api` PTY operation and invokes the shared mailbox engine with that client store.

It never builds a standard `OutboxMessage`, never marks `delivered/poisoned`, and never calls standard retry methods for PTY input. Shutdown before `actuating` leaves only a reclaimable preparation lease; abort after `actuating` drops the RAII active guard and the next mailbox/API orphan sweep makes the row indeterminate.

### 8.4 Container helper

Extend the helper with a distinct metadata-only lookup command plus the existing `send` parser's four mutually exclusive submission forms:

```text
agentscommander-api-helper pty-input-status --op-id <canonical-UUID-v4>
```

The status command accepts no target, payload, agent, mode, timeout, or host credential flags; it calls the same authenticated GET route, prints only the strict `PtyInputResult`, and exits 0 for any found status (including rejected/indeterminate), 1 for auth/not-found/invalid response. It is the required post-timeout lookup surface and never POSTs.

- existing `--send` and `--message` remain ordinary `/send` behavior;
- `--pty-input <text>` and `--pty-input-stdin` use `/pty-input`;
- accept `--agent`, enforce `--mode wake`, and default `--confirm-timeout` to the host CLI's 90 seconds for PTY input;
- reject `--get-output`, `--outbox`, and host-only `--token`/`--root` rather than silently ignoring them;
- parse with `args_os`, convert PTY values with fixed non-echoing UTF-8 errors, and use the same 65,537-byte `take(...).read_to_end` stdin bound; run the mirrored fixture-pinned validator, then let the server repeat validation and authority.

Generated coordinator context lists separate local-host and `AGENTSCOMMANDER_TRANSPORT=api` command lines, so a container is never instructed to pass absent host session credentials to the helper.

Generate one canonical UUID-v4 opId and one immutable serialized request before any network call, print/flush the opId first, and retain both for the full invocation. On an ambiguous connect/reset/5xx after POST, poll the same GET first; 404 permits only a bounded retry of the identical bytes and same opId (250/500 ms backoff), never a new operation. A definitive 4xx is not retried. Poll status every 250 ms and stop only on `injected`, `rejected`, `indeterminate`, or confirmation timeout. A terminal duplicate returns immediately; a queued duplicate follows the same poll. Timeout never POSTs again and prints the exact `pty-input-status --op-id <same-id>` command; submission exits 0 only for `injected`.

No request/response log or error includes the text or bearer token. PTY helper responses are read through a 16 KiB cap into strict versioned success/error DTOs; an invalid/oversized/non-JSON error prints only HTTP status plus `invalid_server_response`, never the raw response body. HTTP/JSON tests use a tiny `tokio::net::TcpListener` fixture already supportable by current dependencies; do not add Axum or a new mock-server crate to `session-bridge`.

## 9. Redaction, artifacts, events, and audit

### 9.1 Host artifacts and specialized poll control flow

Never pass a privileged candidate or marker to generic `move_to_delivered`, `reject_message`, `reject_raw_file`, the generic `retry_tracker`, or its max-attempt fallback, because those can retain serialized payloads and raw error strings. Change `process_message` to classify first and return an internal `StandardResult` or fully handled `PtyInputPollResult`; the outer poll applies existing retry behavior only to `StandardResult`. A retryable privileged filesystem I/O failure leaves the source in place, logs a fixed code once, and is revisited without generic relocation. No lifecycle claim occurs until a marker exists.

Require the opened source handle and every outbox/artifact directory component to be regular, non-reparse, confined, and stable by object ID. Require the raw source filename stem to equal its canonical UUID-v4 injection ID. The CLI publishes the raw request atomically as section 4.1 requires, so the daemon never rejects a partial writer. After a valid request is transactionally enqueued, write and fsync a sibling temporary marker, atomically replace the raw source with no backup (`rename` on Unix, the existing no-backup `ReplaceFileW` pattern on Windows), and best-effort fsync the parent before lifecycle work. The strict marker shape is:

```json
{"kind":"pty-input-marker","version":1,"injectionId":"<uuid>","opId":"<uuid>"}
```

`PtyInputQueueMarker` denies unknown fields and contains no sender, target, token, nonce, text, path, or profile. On every marker poll, derive the marker outbox owner through the same strict real-path layout, load the row by injection ID, and require marker op ID, `source_plane == host_cli`, and row sender to match that owner. This prevents a copied/tampered marker from exposing another operation's result in the wrong outbox. Run runtime orphan recovery every mailbox poll cycle, not only when a marker happens to exist.

Marker handling is state-based and idempotent:

- queued/retry/due preparation: claim this exact row and call the shared engine;
- preparing/actuating with a current owner: leave the marker and return handled;
- injected/rejected/indeterminate: materialize the terminal artifact, then remove the marker;
- missing/mismatched row: write a fixed redacted rejection only under a safe server-generated ID when the request ID is unusable, then remove the source.

Add `write_pty_input_terminal_artifact` that uses the same no-reparse atomic writer and then removes the queue marker:

- injected: `outbox/delivered/<injectionId>.json`;
- rejected: `outbox/rejected/<injectionId>.json` plus `<injectionId>.reason.txt` containing only fixed code/detail for existing CLI polling compatibility;
- indeterminate: `outbox/indeterminate/<injectionId>.json`.

`wait_for_pty_input_confirmation` checks all three directories before timeout and strictly validates the `PtyInputHostArtifact` plus confirmation tag and expected immutable metadata. Artifacts contain only the public result and tag. A parseable malformed privileged JSON uses the specialized redacted path, computes digest/length only when isolated, never trusts an ID for a filename until UUID validation, writes no raw copy, and removes the source after artifact publication. A pre-existing artifact is never blindly accepted: the daemon opens it no-follow and requires byte-equivalent safe metadata to the terminal DB row or permanent tombstone; mismatch is fixed-code tampering, atomically replaced, and never used to delete/activate the source. Windows replacement uses the existing no-backup `ReplaceFileW`/atomic-publish pattern rather than `std::fs::rename` over an existing destination, so no plaintext backup is created. If artifact write succeeded but source deletion or `host_artifact_at` update failed, the next poll repairs those steps idempotently.

A crash or I/O failure between DB enqueue and marker replacement can leave the original transient request; the next poll resolves the same fingerprint and retries marker replacement before lifecycle. Tampering it into a conflict rejects the file but does not strand plaintext: the runtime expiry sweep terminalizes and clears the original row. A crash after DB terminalization but before artifact creation leaves the marker; the next poll recreates the exact metadata artifact and records `host_artifact_at`. Document that raw host outbox, ignored temp request, and queued SQL payload are sensitive only until specialized marker/expiry/actuating redaction.

### 9.2 Logs and events

Privileged log lines and preflight/publish errors contain only injection/op IDs, verified canonical sender/target, byte length, digest, source plane, selected session/backend, status, attempt, and fixed reason code. The one intentional path disclosure is the authorized host CLI's timeout instruction naming its own metadata-only artifact directories; logs/events/API errors never contain that path. They never use standard mailbox `first_100`, `escape_debug`, Serde/value/HTTP dumps, full `SessionInfo: Debug`, raw errors, tokens/hashes/tickets, paths, argv/env, or text. Internal adapters convert legacy `Result<_, String>` and boundary-metadata errors to typed fixed codes at the call boundary and discard the raw string from public/retained/logged output. Every event/audit/artifact write failure is handled and logged by a fixed code rather than `let _ = ...`; failure of the secondary event/audit sink never changes the transactional outcome.

Emit `pty_input_status` only with `PtyInputResult`. Do not reuse `message_delivered` with a payload. Existing frontend has no listener and needs no change.

### 9.3 Audit

Every enqueue, duplicate, claim, retry, actuating, and terminal transition writes the metadata-only SQLite audit row in the same transaction as state. Lease heartbeats are not semantic transitions and add no audit row. Validation/authorization rejections that cannot create a trusted operation row still emit one metadata-only host/API audit event with the safe digest/length when available and a fixed reason code; untrusted claimed sender/target values are recorded as absent, never canonical. Extend `api/audit.rs` with a serializable `PtyInputAuditMetadata` writer using the same rotation/fail-soft policy and exactly the allowed metadata fields. The existing general request audit stays unchanged.

A write failure in the secondary audit log never changes operation outcome. Failure of the transactional SQLite state/audit write before actuation prevents a PTY write; after actuation it cannot cause replay and results in indeterminate recovery.

## 10. Compatibility and security invariants

1. Existing `--send`, `--command clear|compact`, filename validation, file notification formatting, response markers, broad standard routing, wake behavior, and standard API `/send` remain behaviorally unchanged.
2. Standard `messages` rows and old serialized Outbox JSON remain valid. No migration rewrites them.
3. Legacy slash commands gain writer serialization only; their public syntax, idle requirement, shell eligibility, timing, `/clear` stamp, and `/compact` behavior stay intact.
4. PTY input is never inferred from a standard body or text beginning with `/`.
5. Master/root credentials retain existing verbs but confer no PTY-input identity.
6. Root remains excluded from HTTP and container execution.
7. Authorization uses no role prose, caller `from`, outbox placement, broad `can_communicate`, manual API scope alone, stale token, CWD fallback, or target spelling.
8. Ordinary standard-message plaintext retention remains documented and unchanged. PTY-input terminal rows/artifacts are redacted. PTY text never enters argv or env. The pre-existing container bearer credential necessarily exists inside the helper environment, but Docker launch passes its value through `Command::env` plus name-only `--env AGENTSCOMMANDER_API_TOKEN`, never `KEY=<secret>` in host argv/diagnostics; the same applies to `AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN`.
9. No frontend/TypeScript change and no dependency/Cargo manifest change.

## 11. Exact implementation surfaces

### 11.1 Core contract and CLI

- `src-tauri/src/phone/types.rs`
  - add `PtyInputWirePayload`, Enter/status/reason/result/host-artifact DTOs, `OutboxMessage::pty_input`, confirmation-tag/fingerprint helpers, manual redacted `Debug`, strict Serde tests, and legacy JSON tests.
- `src-tauri/src/cli/send.rs`
  - add Clap group/flags, bounded stdin reader, exact-root preflight, fixed timestamp/UUID/tag construction, no-reparse atomic request publish/temp cleanup contract, correlated confirmation/output, help, and parsing tests.
- Mechanical `pty_input: None` additions only in:
  - `src-tauri/src/cli/{close_session,purge_wg,raise_hand,self_clear,self_switch}.rs`;
  - standard constructors in `src-tauri/src/api/{actuation,dispatcher}.rs`;
  - existing mailbox/type test fixtures.
- `src-tauri/src/cli/api_client.rs`
  - set manual `bound_session_id = None` and `credential_generation = None`, update scope help/tests.
- `src-tauri/src/path_identity.rs` (new), exported privately from `src-tauri/src/lib.rs`
  - implement opened-handle no-follow/reparse checks, Windows volume+file ID and ordinal case comparison, Unix device+inode identity, bounded stable reads, descendant checks, and non-lossy display separation used by resolver/outbox/store/auth paths.

### 11.2 Identity, lifecycle, and PTY

- `src-tauri/src/config/teams.rs`
  - add the narrow read-only identity/route resolver, strict FQN/config duplicate handling, project de-duplication/ambiguity rules, identity fingerprints, and exhaustive fixture tests. Leave `can_communicate` unchanged.
- `src-tauri/src/config/root_agent.rs`
  - expose strict live Root path verification and the code-owned canonical-Root capability constant; do not alter seeded/customizable Root supplement bytes.
- `src-tauri/src/phone/mailbox.rs`
  - duplicate-aware privileged probe, strict host envelope/marker dispatch before generic parsing, generic-retry bypass, canonical outbox/artifact confinement, host enqueue/status artifacts, shared operation engine, selector/lifecycle/readiness/lease heartbeat/revalidation, fixed error classification, redacted events/audit tests.
- `src-tauri/src/session/profile.rs`, `src-tauri/src/session/session.rs`
  - executable-position `PtySubmissionAgent` detection/methods and wrapper/Cursor/plain-shell/false-positive tests.
- `src-tauri/src/session/manager.rs`, `src-tauri/src/session/selection.rs`, `src-tauri/src/commands/session.rs`, `src-tauri/src/pty/idle_detector.rs`
  - unique live-token lookup, crate-private pre-held logical-target create permits across every WG create/finalizer, final no-await boundary preparation, correlated four-leg readiness guard, synchronous automated-busy transition, release-before-backend-write discipline, and mutation/create/blocked-write/race tests.
- `src-tauri/src/pty/backend.rs`
  - canonical 65,536-byte constant and one-write contract documentation.
- `src-tauri/src/pty/manager.rs`
  - checked route generation plus generic CWD identity/optional replica anchor, owned input and per-route lifecycle gates, duplicate-route rejection, async permit acquisition, `PtyRouteWriteGuard` plus convenience write (no outer/global-registry final lock), removal of public permitless write, route cleanup/path-replacement/blocked-write/race/serialization tests.
- Mechanical route-registration/removal adaptations in `src-tauri/src/{commands/ac_discovery.rs,commands/resource_monitor.rs,commands/window.rs,session/auto_close.rs,session/selection.rs}` and `src-tauri/src/lib.rs::install_container_route_remover`
  - pass generic/optional verified route-anchor identity, handle duplicate/overflow/deferred-removal errors without unwrap, and prove no path holds the outer manager/global registry while awaiting SessionManager, an input permit, or a blocking backend write.
- `src-tauri/src/pty/inject.rs`
  - exact validator, permit-held canonical injection, phase-aware privileged submit, fake-clock/failure tests.
- `src-tauri/src/commands/pty.rs`
  - use permit for user write/recheck and centralize post-success `/clear`/`/compact`/content boundary effect tests.
- `src-tauri/src/web/mod.rs`, `src-tauri/src/web/commands.rs`
  - route both web user-input paths through the permit; no protocol/frontend change.
- `src-tauri/src/pty/local_backend.rs`, `src-tauri/src/pty/container_backend.rs`
  - exact one-write/one-frame boundary tests; local writes release the global PTY map before blocking and teardown can break the pipe; container cap aliases the canonical constant; container state carries and exposes the active client/generation/session/root/token-hash binding and defers self-recursive route removal.
- `src-tauri/src/pty/container_runtime.rs`, `src-tauri/src/pty/docker_runtime.rs`
  - manual redacted `Debug` for token/ticket/env/path-bearing start requests; extend `DockerCommandSpec` with a non-`Debug` secret-env map, render only name-only Docker `--env` args, apply values with `Command::env`, and ensure recorded/timeout/error diagnostics expose names only; otherwise preserve runtime behavior.

### 11.3 Persistence and API

- `src-tauri/src/api/message_store.rs`
  - managed shared state, migration 2 constraints plus permanent compact tombstones, request/incarnation fingerprints and quotas, fixed operation/target lock stripes plus exact ref-counted target lock map, heartbeat boundary handoff, transactions/ambiguous-commit reconciliation, cross-process recovery, expiry/artifact-aware compaction, live/tombstone status, offloaded wrappers, and state-machine tests.
- `src-tauri/tests/pty_input_cross_process.rs` (new)
  - child-process lock/recovery tests proving a second daemon cannot reclaim a suspended preparing/actuating owner or concurrently lifecycle-lock the same missing target.
- `src-tauri/src/lib.rs`
  - initialize/manage shared store, active-operation set, and target-lock map before `.setup`, API, or poller start. No command registration or frontend IPC.
- `src-tauri/src/commands/config.rs`
  - dynamic API start uses the managed store; manual UI/API client mint sets both binding fields `None`; update mock-app fixtures.
- `src-tauri/src/api/auth.rs`
  - scope, optional bound session/generation, stable dedicated cross-process registry lock, bounded `ApiClientFreshGuard`/no-cache privileged lookup usable by the final synchronous boundary, duplicate rejection, bounded automatic-generation compaction with the existing container-history witness preserved, automatic-vs-manual mint separation, and legacy-cache tests.
- `src-tauri/src/api/identity.rs`
  - live container-session identity/role resolution and negative tests.
- `src-tauri/src/api/schema.rs`
  - strict request/status response DTOs and JSON contract tests.
- `src-tauri/src/api/error.rs`
  - 404/409 stable mappings.
- `src-tauri/src/api/handlers/mod.rs`
  - export focused handler and strict single-bearer extraction used by PTY routes without changing legacy-route auth behavior.
- `src-tauri/src/api/handlers/session_transport.rs`
  - authenticate through the fresh dedicated-lock lookup and bind ticket consumption/hello to the pending route's exact automatic client ID, credential generation, session/root object IDs, and runtime-held token hash; manual/unbound/substituted clients cannot consume a runtime session ticket.
- `src-tauri/src/api/handlers/pty_input.rs` (new)
  - POST/GET handlers only: body bound, auth/scope, strict decode, live authority, enqueue/status mapping.
- `src-tauri/src/api/mod.rs`
  - routes, managed shared-store/client-store dispatcher wiring, and the max(existing-send, escaped-PTY-envelope) global body ceiling; retain `/send`'s existing handler limit and server lifecycle.
- `src-tauri/src/api/dispatcher.rs`
  - separate PTY claim/dispatch path with no standard status reuse.
- `src-tauri/src/api/actuation.rs`
  - API-to-shared-engine adapter and no standard-body construction for PTY input.
- `src-tauri/src/api/audit.rs`
  - metadata-only PTY audit record.
- `src-tauri/src/pty/container_tokens.rs`, plus `src-tauri/src/pty/container_backend.rs` fixtures
  - unique session+generation client IDs, bound session/generation/root/token-hash provenance, prior-generation and legacy-ID revocation/compaction with one inert history witness, coordinator-only automatic scope, safe Debug, runtime-binding accessor, and mint fixture updates.

### 11.4 Container helper, generated contexts, and docs

- `crates/session-bridge/src/bin/agentscommander-api-helper.rs`
  - matching flags/stdin, distinct `pty-input-status --op-id` GET-only lookup, mirrored validator, strict request/response bounds, polling, terminal output, parser/HTTP tests.
- `crates/session-bridge/tests/fixtures/pty_input_validation.json` (new)
  - byte/scalar/control/boundary cases consumed by both host and helper validator tests.
- `crates/session-bridge/tests/docker_bridge_e2e.rs`
  - add an opt-in helper container POST/status scenario while retaining bridge coverage.
- `src-tauri/src/config/session_context.rs`
  - append a code-owned `PTY_INPUT_COORDINATOR_CONTEXT` only when `verify_pty_input_coordinator_root(cwd)` succeeds. Do not put the grant unconditionally in `get_default_coordinator_template`: origin coordinators also receive that customizable template, so doing so would violate the authorization contract. Keep coordinator seeded version 4 unchanged and test verified WG/custom-template inclusion plus worker, origin coordinator, Root, and spoofed-replica exclusion.
- `src-tauri/src/config/root_agent.rs` and `src-tauri/src/config/session_context.rs`
  - expose the exact Root-only capability block as code-owned runtime prose and append it from `render_root_runtime_prologue_inner` only when the existing strict canonical `is_root_agent` gate is true. Do not put this authority grant in editable `ROOT_ROLE_MD` or bump seeded Root version 5: a customized `Context.root-agent.md` is intentionally preserved and would otherwise omit the new capability, while a default supplement plus code-owned append could duplicate it. Test current/default/custom Root supplements unchanged, canonical Root inclusion exactly once, and same-named/spoofed path exclusion.

The code-owned coordinator section uses this exact operational copy, with normal project spelling substituted only by existing context rendering:

```text
## Privileged PTY Input

This capability is present only because this session is an identity-verified workgroup coordinator replica. You may ask AgentsCommander to submit validated text to exactly one non-coordinator member in this same project and workgroup. Resolve the exact target with `list-peers-lean`. This writes text into the target coding-agent PTY; it never directly executes a host or container OS shell command.

Local host session:
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<agent_name>" --pty-input-stdin --mode wake

Container session with `AGENTSCOMMANDER_TRANSPORT=api`:
"<AGENTSCOMMANDER_BINARY_PATH>" send --to "<agent_name>" --pty-input-stdin --mode wake
"<AGENTSCOMMANDER_BINARY_PATH>" pty-input-status --op-id "<operation_id>"

Prefer stdin for multiline or sensitive text. `Queued` is not `Injected`. If confirmation times out, keep the reported operation ID and inspect its status; do not create a new operation to retry it.
```

The code-owned canonical-Root runtime section uses this exact copy:

```text
## Privileged PTY Input to Workgroup Coordinators

As the live local Root Agent, you may ask AgentsCommander to submit validated text only to an identity-verified workgroup coordinator replica returned by `list-peers-lean`. Worker replicas, origin coordinators, Root itself, and coordinator-to-coordinator requests from any non-Root sender are not valid targets. This writes text into the target coding-agent PTY; it never directly executes a host or container OS shell command.

"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<coordinator_name>" --pty-input-stdin --mode wake

Prefer stdin for multiline or sensitive text. `Queued` is not `Injected`. If confirmation times out, keep the reported injection ID and inspect the metadata-only outbox artifact; do not submit the text again under a new ID.
```
- `docs/reference/cli.md`
  - all four forms, exact validation, quoting, lifecycle, terminal meanings, timeout/no-cancel.
- `docs/agents/inter-agent-messaging.md`
  - separate privileged actuation section and authorization matrix; ordinary file messaging remains distinct.
- `src-tauri/src/api/README.md`
  - scope, exact POST/GET schemas/status/HTTP meanings, container-only live binding, nonterminal plaintext sensitivity.
- `docs/security.md`
  - authority sources, Root host-only rule, no shell evaluator, no-replay boundary, audit/redaction, and #139 residual threat model.

## 12. Implementation order

### Phase 1: MVP foundations

1. Add failing contract tests for bounded text/envelopes, strict pairing/mixing, authority/path matrix, operation migration/state transitions, multi-process/target ownership, phase outcomes, and writer interleaving.
2. Add typed payload/result/artifact contracts, manual redaction, canonical byte/timestamp/tag/fingerprint helpers.
3. Add no-follow path identity primitives, read-only hierarchy resolver, route-anchor identity, and live Root verification.
4. Add migration 2, shared store/fixed lock-stripe/exact-target-lock state, quotas, idempotent enqueue, heartbeat/claim, actuating, terminal, recovery, expiry, and query methods.
5. Add per-session permit plus final session/readiness boundary and refactor every production facade writer before introducing privileged writes.
6. Add phase-aware exact submit and strict executable-position coding-agent eligibility.

Exit gate: pure/unit tests prove validation, exact state transitions, no replay, and serialization. No public request can actuate yet.

### Phase 2: Full features

7. Add host CLI flags/payload/confirmation and early mailbox ingress.
8. Add shared target selector, strict lifecycle, post-await revalidation, actuation, fresh-boundary behavior, redacted artifacts/events.
9. Add API scope, bound session identity, coordinator-only auto-mint, strict POST/GET, and dispatcher adapter.
10. Add helper flags/stdin/polling and local/container target tests.

Exit gate: all positive/negative authorization, lifecycle, backend, failure, status, and idempotency rows pass end to end.

### Phase 3: Polish

11. Add metadata audit and sentinel scans for logs/events/artifacts/queried terminal rows.
12. Add the verified-WG-only dynamic coordinator section and canonical-Root-only code-owned runtime section without changing seeded customizable defaults, and run grant-leak/exactly-once/custom-preservation tests.
13. Update CLI/API/inter-agent/security docs and release CLI smoke assertions.
14. Run full CI, the Docker E2E when the runner has Docker, cross-process lock integration tests, and the Windows ConPTY manual matrix. No readiness certification may omit the recorded Docker availability/skip reason or any Windows row.

### Phase 4: Extras

None. UI, force mode, response capture, binary/control input, and broader authorization remain out of scope.

## 13. Required automated test matrix

### 13.1 Parsing and payload

- Four single valid forms: `send`, `command`, argument PTY input, stdin PTY input.
- Zero forms and all six pairs fail.
- PTY input plus `get-output` or custom `outbox` fails; mode other than wake fails.
- Argument and stdin preserve accepted Unicode, LF, TAB, leading/trailing spaces, whitespace-only, shell metacharacters, and hyphen-leading values byte-for-byte.
- Empty and invalid UTF-8 stdin fail; an infinite/oversize reader stops after 65,537 bytes with one bounded read and no allocation proportional to the source.
- Table-driven rejection for CR, NUL, ESC, DEL, every disallowed C0/C1 boundary, both Unicode line separators, every listed bidi control/range boundary.
- 65,536 UTF-8 bytes succeed; 65,537 fail in CLI, helper, daemon, API, store, and container backend before write.
- Version/Enter/unknown nested or top-level field, duplicate JSON key, missing action/payload pair, body/command/action-parameter/getOutput/requestId/senderAgent/preferredAgent mixture, malformed UUID version/spelling, nil/duplicate ID, bad nonce, bad timestamp/TTL/expiry, and caller-selected privileged fields fail closed.
- Every malformed/truncated/invalid-encoding outbox file, including escaped-key/value obfuscations such as `pty\u0049nput` and `pty\u002dinput`, and every over-limit envelope takes the bounded metadata-only path regardless of probe classification; no partial CLI file or stale temp is generically archived. Legal bounded standard JSON keeps existing behavior.
- Exact canonical FQN succeeds; case alias, bare/local/origin/path/wildcard/session-ID/extra-separator forms reject without normalization.
- An API request whose 65,536 decoded ASCII bytes are represented with worst-case `\uXXXX` escapes passes the raw envelope bound; decoded 65,537 still returns 413. Duplicate Authorization/content headers, query parameters, wrong content type/encoding, duplicate nested keys, and overlong/invalid `agentId` reject with fixed payload-free details. The helper never echoes an oversized/malformed/adversarial response body.
- A legacy Outbox JSON still parses; a simulated old decoder sees unknown `pty-input` action and cannot deliver the empty body.

### 13.2 Authorization with zero target mutation/write on every negative

Positive:

1. live verified coordinator token to a verified non-coordinator member in the same exact project/workgroup;
2. live local Root session token to a verified workgroup coordinator;
3. auto-bound container coordinator with `pty-input` scope through API.

Negative:

- worker sender to any target;
- forged coordinator directory/role/config mismatch;
- origin coordinator;
- coordinator to self, Root, coordinator, cross-workgroup, cross-project, origin, spoofed/non-member;
- Root to worker, origin coordinator, spoofed coordinator, Root URI/self;
- malformed, stale, wrong-owner, duplicate/ambiguous, or exited-session token;
- master/root credential alone;
- tokenless or handcrafted outbox, app-outbox placement, custom/wrong owner's outbox, filename/ID mismatch, and a container session token attempting the filesystem plane;
- API token with only `send` scope;
- manual coordinator/worker token carrying requested `pty-input` scope but no bound live container session;
- auto-bound worker even if registry scope is handcrafted;
- revoked/expired/wrong-session/wrong-root API client, including same-mtime revoke, concurrent mint/revoke, duplicate registry IDs/hashes, reused credential generation, handcrafted bound fields/token-hash substitution, mismatch with the active transport binding, automatic-generation compaction preserving one #992 history witness, and manual/live rows exhausting the 4,096-row cap;
- symlink/junction/reparse, multi-link file, or handle-swapped project, workspace, workgroup, sender, target, matrix, team/replica/local/settings/API config, DB/sidecar, lock, outbox, marker, artifact directory, or escape; same-spelling path replacement and different-project same-casefold basename also reject;
- role/membership/target identity changed at dispatch start;
- each of those changed again during a scripted await and caught immediately before actuation; a mutation scheduled on another runtime thread between the final snapshot and write is blocked by the final SessionManager/IdleDetector boundary.

Each test asserts no destroy, create, target lookup before authority where observable, input permit write, or backend frame.

### 13.3 Lifecycle and selection

- One idle supported local session and one idle supported container session each receive one exact text write plus two canonical Enter attempts.
- Busy/non-idle live target rejects terminally with no deferred text and no duplicate spawn.
- Plain bash/cmd/PowerShell, every `cmd /K <agent>` launch (including a scripted agent-exit-to-idle-cmd fallback), `cmd /C` with `CALL`/`START`, malformed quotes, `%`/`!`/caret expansion, or any literal/synthesizable follow-on operator, `bash -c "echo claude"`, a compound preamble mentioning an agent, conflicting kind metadata, and unrecognized wrappers reject with zero write/spawn; direct and conservatively literal `cmd /C` wrappers remain covered positively.
- Claude/Codex/Gemini direct and supported Windows wrappers, plus exact Cursor `agent`, are eligible; `agentctl` is not.
- Exited persistent target: selected once, validated profile before destroy, one destroy, one configured-backend respawn with resume intent, sustained idle, one submit.
- Missing target: valid explicit override and each auto fallback path spawn once; no profile/unsupported profile leaves no session.
- Multiple live candidates choose only the best eligible; a busy first record cannot hide a later eligible record; no candidate is written twice.
- Multiple exited candidates destroy/respawn only the deterministic selected record.
- Phantom and selection/destroy/spawn races never fan out or spawn beside a non-exited phantom.
- Four-leg readiness requires mirror idle, watcher idle, activity age `idle_threshold + 2s`, and resize age `resize_grace + 2s`. Missing activity fails closed; spawned startup may settle, while a live candidate that becomes busy rejects; 90-second cap never injects.
- Destroy failure is followed by re-list/probe and never spawns beside an ambiguous survivor; partial spawn ambiguity never retries another create.
- A healthy newly spawned persistent session remains after readiness/expiry/authority rejection, while a partial failed create uses existing rollback.

### 13.4 Concurrency and exact submission

- A user write queued during privileged text/Enter sleeps appears strictly after the redundant Enter, never between phases; its voice/session bookkeeping corresponds to the successful serialized write.
- Two sessions can write concurrently, proving lock granularity is per session.
- Removing/recreating a route with the same UUID changes generation and CWD-anchor proof: old queued permits and same-spelling directory replacements fail. Duplicate `record_route` and generation overflow cannot replace/wrap a live gate.
- Concurrent host/API operations and an ordinary API/filesystem wake or user create for one missing/exited WG target share the target create gate: at most one create/finalizer runs, the privileged pre-held permit does not self-deadlock, pending-spawn marks are visible, and every non-user create rechecks live/pending state under the gate immediately before `create_pending_session`. A stale standard wake cannot resume after the privileged release and create beside the new live route. Explicit sequential user same-CWD creates and different targets retain compatibility/concurrency.
- Tauri, web binary, web JSON, standard injector, loop/pre-write-check path, legacy slash command, and graceful exit all use the same permit.
- Text is one backend call/frame, not chunked; Enter is absent from the text call.
- Text-write failure and first-Enter failure map to indeterminate; second-Enter-only failure maps to injected.
- Exact `/clear` stamps fresh through the existing helper; exact `/compact` does not; whitespace variants are opaque content.

### 13.5 Durability, idempotency, status, and audit

- Migration 1 database upgrades to version 2 without changing old message rows; recovery/expiry/compaction remains bounded to 64 IDs per batch under a large terminal-history fixture.
- Exact duplicate `(sender, opId)` returns the original injection/status and creates no row/write before or after terminal compaction; force compaction past both retention windows, restart the daemon, and prove the permanent tombstone still returns the same result. Changed source/target/text/profile/host nonce/time/tag or a same-FQN replacement sender anchor conflicts forever and never injects.
- Failure before actuating retries only under the fixed bounded policy; attempt 5 rejects and clears payload.
- Actuating transaction commits and clears SQL payload before the fake backend observes the first write.
- Reopen after queued/preparing is safely retryable only when its operation stripe is free; reopen after actuating is indeterminate only when its stripe is free and is never leasable. A second process skips a live/suspended owner.
- A panic/cancellation or terminal SQL failure while the daemon remains live is recovered after the 15-second runtime grace as indeterminate and never leasable.
- Runtime/startup recovery skips a deliberately suspended task by both local active set and cross-process operation stripe; after the combined guard drops, the same row becomes indeterminate without any later write. A blocked backend write retains ownership and a teardown unblocks it before terminalization. Forced operation/target stripe collisions only serialize, stripe files are never unlinked/replaced, and exact target-map entries disappear after the last guard.
- Preparation lease heartbeat spans a scripted >120-second lifecycle await; ownership is retained, normal finish joins it, and panic/cancellation aborts it with no later renewal. A tick racing the actuating handoff cannot report a false lease loss: final renewal, boundary-specific join, and transaction ordering are pinned. Lost renewal stops before boundary, and expired preparing can be reclaimed; actuating is never reclaimed.
- Confirmation timeout does not cancel; helper connection-reset/5xx retry uses the identical opId/body; a fresh helper invocation of `pty-input-status --op-id` performs GET only; repeated status/POST never injects twice. Host publish ambiguity reports the preprinted stable ID.
- Raw request to marker, marker to each terminal artifact, crash windows, vanished/tampered/copied marker, wrong confirmation tag, mismatched/pre-existing artifact, DB-terminal-before-artifact, artifact-before-DB-flag, and deletion failure are idempotent and never enter generic rejection. Artifact-confirmed full rows use `host_artifact_at`; unclaimed full rows retain 30 days then compact under a free operation stripe, but their permanent tombstones keep IDs/nonces non-reusable and can regenerate terminal truth if the exact marker/request later returns. Missing-marker expiry clears payload/quota.
- API and host results distinguish queued, actuating, injected, rejected, and indeterminate exactly.
- Unique plaintext/token/path/ticket sentinels are absent from every manual `Debug`, parse/auth/store/backend error, log capture, event, terminal artifact/reason file, queried terminal row, audit row, and Docker command argv. Digest/length/source/session/backend/status/timestamps are exact; the API token remains only in its pre-existing credential delivery channel, never in this operation's payload/DB/artifacts.
- Terminalization failure cannot requeue/replay; lock-aware recovery makes an ownerless orphan indeterminate. Future schema, invalid row constraints, payload/digest corruption, ambiguous commit, quota exhaustion, and timestamp rollback/overflow fail closed without a write.

### 13.6 Compatibility

- Existing CLI `--send` and `--command clear|compact` parsing/output tests remain green.
- Existing standard file framing, route/wake, API `/send`, DB retry/retention, response markers, and local/container ordinary messages remain green.
- Legacy `/clear` and exact PTY `/clear` both call the same stamp helper; legacy and exact `/compact` do not stamp. A crash during exact `/clear` metadata remains indeterminate, never already-injected.
- Old JSON without `ptyInput` remains valid; new action fails closed under a legacy-dispatch fixture.
- Local host CLI and container helper report identical submission terminal meanings/exit codes; the helper's separate status-only command exits 0 for any found metadata state and provably never POSTs.
- Generated worker, origin-agent, origin-coordinator, spoofed-replica, and Root-as-coordinator-template context contains no WG coordinator grant; verified WG coordinators get the dynamic section once. Canonical Root gets its separate code-owned grant exactly once even with a customized supplement; same-named/spoofed Root paths do not. Seeded Root v5 and customized Root/coordinator templates remain byte-preserved.

## 14. Objective acceptance criteria

Implementation is acceptable only when all statements are true:

1. A live verified coordinator can submit argument/stdin text to one exact verified same-workgroup member.
2. A live local Root Agent can do so only to one verified workgroup coordinator.
3. Every unauthorized combination rejects before lifecycle or input side effects.
4. Accepted bytes arrive once, unchanged, followed only by the 1500 ms required Enter and 500 ms best-effort Enter.
5. Live idle/busy/unsupported, exited, missing, duplicate, phantom, and race cases match section 7 exactly.
6. Local/container sender and target combinations share the byte cap, authority, lifecycle, status, audit, and no-replay engine.
7. Nothing after actuating automatically replays text.
8. Logs, events, terminal artifacts/rows, and audit disclose no text or token.
9. Existing standard messaging and legacy command clients retain their public behavior.
10. Every issue matrix row has an automated assertion, with Docker/ConPTY manual evidence only where the real platform boundary cannot be simulated.
11. Help, generated authorized-role context, API docs, inter-agent docs, and security docs state that this writes to a coding-agent PTY and never directly executes a host/container OS shell command.

## 15. Verification gates

Run from the repository root unless a command says otherwise. First require `git diff --check` and a test-owned source inventory proving every production PTY write reaches `write_with_permit` (no public permitless facade or direct backend call outside backend internals):

```text
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins --tests
npm run test:debt
npm run typecheck
npm test
npm run build
```

Also reproduce the exact current Rust CI working-directory gates and the real child-process ownership test:

```text
cd src-tauri
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --lib --bins --tests
cargo test --test pty_input_cross_process -- --nocapture
```

Session-bridge/container coverage:

```text
cargo test -p session-bridge --all-targets
AGENTSCOMMANDER_RUN_DOCKER_E2E=1 cargo test -p session-bridge --test docker_bridge_e2e -- --nocapture
```

The Docker command is a merge gate when Docker is available. The evidence record must contain runner OS, `docker version` result, image/build identity, scenario and exit status. When Docker is objectively unavailable, record that command failure verbatim (without secrets) and require the non-Docker helper HTTP suite, host container-frame tests, and Windows manual container-sender rows; an unexplained skip is a failed gate.

Windows manual ConPTY/API matrix:

1. local coordinator -> local member;
2. local coordinator -> container member;
3. container coordinator helper -> local member;
4. container coordinator helper -> container member;
5. local Root -> local and container coordinator targets;
6. multiline Unicode/LF/TAB/space/hyphen/shell-metacharacter sentinel at exactly 65,536 bytes;
7. busy and plain-shell zero-write cases;
8. confirmation timeout followed by status lookup with no resubmit;
9. kill/restart immediately after the actuating test hook and verify indeterminate/no replay;
10. scan app log, API audit, terminal artifacts, and queried terminal SQL rows for the unique sentinel/token.

Run the Windows release CLI smoke after adding parser/help assertions:

```text
npm run build:prod
npm run smoke:cli-release-windows
```

## 16. Dependencies and blast radius

No dependency or Cargo manifest change. Existing `sha2`, `uuid`, `chrono`, `rusqlite`, Tokio, Axum, Reqwest, and Serde cover the design.

The large file count comes from security choke points, mechanical `OutboxMessage` literals, all PTY writer surfaces, generated-context authority safety, and local/container parity. It does not authorize unrelated cleanup, module movement, ordinary-message changes, UI work, or security work beyond this operation.

## 17. Dev-rust enrichment rationale and residual risk

| Enrichment | Why it is required |
| --- | --- |
| Duplicate-aware host envelope plus privileged malformed probe | `serde_json::Value` drops duplicate keys, while generic mailbox rejection moves raw JSON. Without the specialized path, a mixed/handcrafted payload can be misclassified or retained with plaintext. |
| Canonical outbox/marker ownership and generic-retry bypass | The current poller's max-attempt fallback calls `reject_raw_file`. A privileged I/O failure must never fall into that retention path, and copied markers must not expose another operation's metadata. |
| Local live host-session and container-bound API source checks | Current generic token/master paths and manual API scopes are intentionally broader. Backend/session binding is what prevents master, stale, tokenless, worker, and container-filesystem authority gaps from #139. |
| Cross-process operation + target RAII ownership | A process-local active set cannot see a second daemon, and a route gate does not exist for a missing target. fixed OS operation stripes prevent false orphan recovery across suspension/processes; target stripes plus exact in-process locks prevent concurrent host/API destroy/spawn/write races without unbounded lock objects. |
| Lease heartbeat | Destroy, spawn, selection-coordinator admission, and 90-second readiness can outlast a fixed 120-second lease. Renewal prevents a second preparation owner or a safe operation failing only because its own lifecycle wait consumed the lease. |
| Route-generation/anchor permit plus final boundary preparation | A gate keyed only by UUID can outlive route/path replacement, and an owned snapshot can go stale on another runtime thread. Checked object IDs plus stable session/readiness preparation returning a locked route guard close the gap without holding global state across a potentially blocking backend write. |
| Explicit lock order and permit-owned route/backend | `PtyManager` is behind a standard mutex while SessionManager and input gates are async. Cloning registry/backend into the permit removes the outer-manager acquisition from the final guarded write; the stated order prevents inversion and guards never cross later awaits. |
| Strict lifecycle rollback decisions | Current ordinary wake can spawn after a failed exited destroy and inject on readiness cap. Both are explicitly forbidden for privileged actuation, so re-list/probe and reject-on-cap behavior must be separate. |
| Boundary metadata before terminal `injected` | Publishing `injected` before attempting the shared `/clear` stamp creates a crash window where the promised fresh boundary never runs. Keeping the row actuating until metadata returns yields conservative indeterminate recovery instead. |
| Separate helper validator with a shared fixture and stable-op retry | The helper is a separate crate and a true shared function would add a forbidden dependency edge. Common fixtures plus authoritative validation provide semantic parity; immutable request bytes/opId make ambiguous HTTP retry idempotent. |
| Escaped-JSON HTTP ceiling | A semantic 65,536-byte string can exceed the current raw request ceiling when represented with `\uXXXX`. A larger global extractor ceiling plus unchanged `/send` handler cap accepts the contract without widening ordinary send semantics. |
| Dynamic verified-WG coordinator context | `get_default_coordinator_template` is also appended to origin coordinators. An unconditional seeded grant would advertise authority to a forbidden sender; code-owned strict-WG insertion avoids that leak and makes a coordinator version bump unnecessary. |
| Managed single DB/store and complete mint call-site list | Host operations exist even when the optional API server is off, and dynamic API start currently opens its own store. One managed recovered store is required for no-replay, and every `MintRequest` literal must compile with explicit session/generation provenance. |
| Persisted request fingerprint + permanent compact tombstone | Clearing payload/profile at `actuating` otherwise makes a terminal duplicate impossible to compare, and deleting a seven-day terminal row would make the same opId injectable again. Fingerprints plus incarnation-bound tombstones preserve exact idempotency forever without retaining text/profile/raw nonce. |
| Four-leg readiness + synchronous busy stamp | `waiting_for_input` is a lagged mirror and resize freezes activity. Correlated activity/watcher/mirror/resize proof prevents false idle, while the busy stamp prevents a queued second operation from observing stale idle. |
| Bounded atomic ingress and artifact correlation | Unbounded stdin/files, partial `.json` publication, missing-marker rows, and copied/forged artifacts create OOM, false rejection, plaintext retention, or false success. Bounds, atomic publish, expiry sweeps, confirmation tags, and artifact-aware retention close each window. |
| Fresh runtime-bound API credential proof | Mtime caching, process-local registry writes, and serialized `boundSessionId` alone do not prove a current automatic container credential. Fresh locked reads plus backend client/generation/session/root and constant-time token-hash binding make revocation and provenance authoritative. |

No unresolved product or implementation choice is intentionally left in this draft. Residual threat-model boundary: an administrator or fully compromised same-OS-user account that can read another process's live environment/memory remains outside this issue, as does proving model consumption (#1001); `docs/security.md` must state both without diluting the strict live-token/runtime-binding checks here. Permanent compact idempotency tombstones are an intentional storage consequence of the issue's unbounded "same opId never injects twice" promise, not an unresolved retention choice. Residual execution risk is limited to evidence still to be produced: Windows handle/reparse and ConPTY behavior, SQLite crash/compaction hooks, cross-process locks, container transport, blocked-write teardown, and generated-context authority gating. Architect consensus accepts this Plan Contract as implementation-ready; the listed implementation and evidence gates remain mandatory.

## 18. Grinch Review: findings resolved in this draft

1. **CRITICAL: cross-process false orphan recovery.**
   - **What:** The process-local active-ID set was the only live-owner proof.
   - **Why:** Daemon B could mark daemon A's suspended `actuating` row terminal, after which A could resume and write bytes behind a public terminal result.
   - **Fix:** Sections 3.2, 3.4, 6.1, and 6.4 now require deterministic operation-stripe OS locks for claim/recovery and child-process tests. Recovery needs both local absence and a lock it actually acquires.

2. **CRITICAL: concurrent missing-target lifecycle had no universal create lock.**
   - **What:** A per-session input gate does not exist before a missing target is spawned, and a lock used only by new host/API operations would not stop an ordinary wake or user create.
   - **Why:** Two privileged operations, or one privileged operation plus standard delivery/user creation, could all observe no session, spawn duplicates, then select/write divergent records.
   - **Fix:** A canonical-FQN target stripe plus keyed exact async gate is held before privileged enumeration through terminal handling and is also acquired centrally by every WG `create_session_inner` path. Privileged create passes a pre-held permit; same-target races serialize while different targets and sequential same-CWD creates remain concurrent/compatible.

3. **CRITICAL: terminal idempotency could be lost after clearing or retention reaping.**
   - **What:** `requested_agent_id` and payload are cleared at `actuating`, while the prior seven-day terminal-row reaper deleted the only `(sender, opId)` witness.
   - **Why:** A changed profile could reuse a live terminal opId, and even an identical API opId could create a second injection after row retention expired.
   - **Fix:** Migration 2 persists a domain-separated request fingerprint, nonce hash, stable sender-incarnation hash, and an indefinite compact terminal tombstone. Live and compacted duplicates are compared forever; full-row/audit compaction can never make an ID reusable.

4. **HIGH: cached/manually forged API provenance and revocation races.**
   - **What:** `boundSessionId` in JSON plus an mtime-gated cache did not prove automatic minting, and registry writes were only process-local serialized. Matching only copied client/generation/session fields would still let a handcrafted row substitute a manual token hash.
   - **Why:** Same-mtime revoke, lost concurrent updates, reused client IDs, or handcrafted binding fields/hash substitution could retain privileged authority.
   - **Fix:** Bounded reads under a stable dedicated lock (never the atomically replaced registry inode), unique credential generations, constant-time equality with a runtime-held token hash, and the exact live `ContainerTransportBackend` binding are mandatory at ingress/dispatch/final synchronous linearization and transport hello.

5. **HIGH: dropped-snapshot and same-spelling path ABA.**
   - **What:** Revalidation returned owned `SessionInfo` and canonical path strings, then dropped guards before write.
   - **Why:** Another runtime thread or directory replacement could swap session state/target identity between the check and first byte.
   - **Fix:** Open-handle object identities are carried by authority and route entries; final preparation validates/stamps under SessionManager/IdleDetector guards and returns a generation/anchor-locked route guard, then releases global state before the immediate write.

6. **HIGH: `waiting_for_input` alone is a false-idle oracle.**
   - **What:** The original plan reused the lagged mirror and ignored resize-frozen activity.
   - **Why:** A startup/repaint window can report idle before paste readiness; a second operation can also observe stale idle immediately after the first.
   - **Fix:** Four-leg activity/watcher/mirror/resize readiness, reject-on-cap, and a synchronous automated-busy stamp now define the boundary.

7. **HIGH: unbounded/partial/obfuscated ingress and stranded plaintext.**
   - **What:** stdin/raw-file reads and CLI `.json` publication were not fully bounded/atomic; vanished markers left host rows indefinitely queued; a malformed JSON discriminator escaped as `pty\u0049nput` could evade a literal token probe and fall into raw archival.
   - **Why:** A pipe or handcrafted file could exhaust memory, the poller could reject a partial write, a lost marker could retain payload/quota forever, and an obfuscated privileged payload/token could be retained by `reject_raw_file`.
   - **Fix:** 65,537-byte stdin bounds, absolute envelope/result bounds, atomic temp+fsync publish, metadata-only handling for every malformed/invalid-encoding document, specialized stale-temp cleanup, runtime expiry, and admission quotas are binding.

8. **HIGH: artifact/reaper crash windows could lie.**
   - **What:** The CLI trusted ID/status only, and the seven-day reaper could delete terminal DB truth before a host artifact existed.
   - **Why:** A copied/pre-existing artifact could report false success; artifact failure followed by reaping could turn a real terminal operation into `not_found`/rejection.
   - **Fix:** Nonce-derived confirmation tags plus full immutable metadata correlation, no-follow/no-backup replacement of mismatches, `host_artifact_at`, permanent terminal tombstones, and artifact-aware full-row compaction close the windows.

9. **MEDIUM: outer `Debug` and container launch leaked secrets.**
   - **What:** Redacting only the nested PTY payload left `OutboxMessage.token`, `ContainerApiToken.secret`, and `ContainerStartRequest` visible; Docker used `KEY=value` argv.
   - **Why:** An error/debug/command diagnostic could expose the text or a bearer credential.
   - **Fix:** Explicit manual-redacted or deliberately non-`Debug` type contracts cover every named payload/credential carrier, sentinel tests cover all error surfaces, and Docker receives bearer/ticket through name-only env arguments with redacted diagnostics.

10. **CRITICAL: trusted agent detection could actuate a surviving host shell.**
    - **What:** Current `CodingAgentKind::detect` prefix-scans every argument, so `bash -c "echo claude"` can look like Claude; even an initially real `cmd /K codex` session falls back to an idle `cmd` prompt after Codex exits while metadata remains Codex.
    - **Why:** Privileged literal text plus the required Enter could execute directly in bash/cmd, violating the central non-shell-evaluator boundary.
    - **Fix:** A strict executable-position grammar corroborates `agent_kind`, rejects compound/mention-only evaluators, and categorically rejects `/K`; only non-persistent-shell `cmd /C` wrappers are eligible.

11. **MEDIUM: DB/lease transition and heartbeat handoff ambiguity was under-specified.**
    - **What:** Commit errors, zero-row updates, corrupt payloads, future schemas, a suspended preparing owner, and a heartbeat tick racing the `preparing -> actuating` transition had no exact disposition.
    - **Why:** Blind retry after an actually committed boundary can replay; blind recovery can steal live work; a normal actuation could be falsely reported as lease loss when its own transaction clears the lease.
    - **Fix:** Conditional row-count checks, schema/row constraints, in-transaction payload revalidation, ambiguous-commit query, final renewal plus joined heartbeat handoff, and operation-stripe-aware preparation recovery are explicit.

12. **MEDIUM: helper retry could create a new injection after an ambiguous POST.**
    - **What:** The opId was retained only after an ordinary successful request flow.
    - **Why:** A connection reset after enqueue invites a rerun with a new ID and duplicate PTY actuation.
    - **Fix:** The helper prints/flushed one opId before networking, serializes once, probes status after ambiguity, and retries only identical bytes under that ID.

13. **MEDIUM: one lock file per operation either leaks files or permits split locks on cleanup.**
    - **What:** ID-named files were to be deleted after row reaping.
    - **Why:** Unix unlink/recreate can give old waiters and new claimants different locked inodes for the same ID; never deleting creates unbounded filesystem state.
    - **Fix:** Cross-process ownership now uses fixed 4,096 operation and 1,024 target stripes that are never replaced. Hash collisions only serialize; exact in-process target entries are ref-counted and removed.

14. **HIGH: a global route-registry guard could wedge unrelated PTYs and teardown.**
    - **What:** The prior `PtyRouteWriteGuard` held the whole route registry across a potentially blocking OS write, and every route was assumed to have a WG-replica anchor.
    - **Why:** One blocked 65,536-byte local write could freeze all route registration/removal; container error callbacks could wait on their own held registry; requiring a replica anchor would break Root, origin, and ad-hoc sessions; kill could not reach the child to unblock the write.
    - **Fix:** Route entries now carry generic CWD identity plus an optional WG anchor and a per-route lifecycle gate. The global registry is released before backend I/O, backend teardown may invalidate/kill before deferred route removal, and local write releases the global PTY map before blocking.

15. **HIGH: editable/custom Root context could silently omit the authority contract.**
    - **What:** Adding the grant only to seeded `ROOT_ROLE_MD` reached pristine defaults but intentionally preserved customized Root supplements unchanged.
    - **Why:** A valid live Root using a custom context would receive no generated capability instructions, while putting the grant in a broadly reused template risks leakage or duplication.
    - **Fix:** The Root grant is a separate code-owned runtime block gated by the existing canonical Root-path predicate. Seeded/custom bytes remain unchanged; canonical Root receives it once and same-named/spoofed paths never do.

16. **MEDIUM: the proposed agent-ID bound did not match the repository's validator.**
    - **What:** The draft said 1..=128 bytes in an unspecified grammar, but `coding_agent_mutations::validate_custom_agent_id` is exactly `^[a-z0-9][a-z0-9_-]{0,63}$`.
    - **Why:** Host and helper/API could disagree, accept IDs normal configuration rejects, or produce divergent idempotency fingerprints.
    - **Fix:** All three ingress planes call/mirror the exact existing 64-byte grammar and normalize only exact `auto` to absence.

**Remaining unresolved choice/blocker:** none at Plan Contract level. Platform behavior is an evidence gate, not an alternative design; failure of any required Windows, cross-process, Docker-when-available, crash, or redaction gate blocks readiness/merge.

**Plan Contract certification:** **READY_FOR_IMPLEMENTATION**. Architect consensus accepts every binding resolution above; implementation and merge remain gated by Section 15 evidence.
