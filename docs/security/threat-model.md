# Threat model

## Scope and source commit

This model treats coding agents as untrusted actors. It covers local sessions, LAN access,
and container sessions. It is an analysis stance, not a change to product policy or an
statement of isolation. Source commit: `4cca370af24d564a71bbe8f75caefc06eacac655` (2026-11-13).
All repository `path:line` citations below refer to that commit, not a moving branch.

Observed entries describe source, not a penetration test. Hypotheses describe possible
consequences, not demonstrated exploits. Issue states were queried on 2026-11-13;
an open issue alone establishes neither the presence nor the absence of a partial fix.
No runtime probes were performed for this document. Policy and support choices remain USER
items. Existing [security guidance](../security.md) and [disclosure policy](../../SECURITY.md)
are separate documents; this model does not amend them.

**O01 — observed policy conflict.** Existing guidance calls agents trusted and says AC adds no
sandbox (`docs/security.md:12,34`); it also describes enabled listeners as loopback-only
(`docs/security.md:21`). Those are policy statements, not executable control evidence.
O03 and O09 below describe the configurable binds. U01 and U02 leave the wording decisions open.

## Assets and actors

Assets to consider: project and configuration files; agent prompts, session state and terminal
content; master, root, web, session and API client credentials; Telegram bot tokens;
Gemini/Jev keys; copied provider credentials; PTY-write, session-lifecycle and settings-write
authority; service and session availability. This is an inventory of protection interests,
not a claim that every actor can reach every asset.

Actors for analysis:

- User/operator, including mistakes in bind, mount, credential and disclosure settings.
- Coding agents, treated as untrusted even when intentionally launched by the user.
- Other local processes running as the same OS user; compromised same-user software.
- LAN clients of the optional web server and control-plane API, with or without credentials.
- Container processes, including compromised images or agents and holders of copied credentials.
- Telegram participants and external dependency publishers as additional ingress actors.

## Boundaries

### Web server: LAN client to daemon

**Observed controls. O02:** Web enablement uses a default-false boolean and its bind defaults
to `127.0.0.1` (`src-tauri/src/config/settings.rs:569-576`;
`src-tauri/src/config/settings/field.rs:363-365`).

**O03:** The listener parses a `SocketAddr` and binds it without a loopback condition in that
path (`src-tauri/src/web.rs:97-112`). The router registers `/ws` and `/api/sessions`
(`src-tauri/src/web.rs:144-147`).

**O04:** The WS and sessions handlers compare a query parameter named `token` only when
`!cfg!(debug_assertions)` (`src-tauri/src/web.rs:214-219,256-261`). The comparison rejects
unequal lengths, then folds byte XORs for equal-length inputs
(`src-tauri/src/web/auth.rs:11-20`). A UUID-v4 web token is constructed at startup
(`src-tauri/src/app_run.rs:372`), and its persistence helper uses `std::fs::write` on
`web-token.txt` (`src-tauri/src/filesystem/documents.rs:414-416`).

**Limits. O05:** This listener is a raw `TcpListener` served with `axum::serve`, with no TLS
layer in the cited server path (`src-tauri/src/web.rs:105-106,175-199`). The command dispatch
includes PTY writes and session creation/destruction (`src-tauri/src/web/commands.rs:246-260`);
settings reads and updates are also dispatched
(`src-tauri/src/web/commands/settings_dispatch.rs:53-62`). The HTTP session-view struct has
no session-token field (`src-tauri/src/web.rs:236-248`); this is a limit on that view only,
not on the WS command surface.

**H01 — hypothesis:** A reachable debug listener, leaked URL token or observed unencrypted
traffic could expose session actuation or settings to an unintended client. Reachability,
firewalls, reverse proxies and deployment TLS were not verified.

**Gaps:** G01 (#908), G02 (#1357), G03 (#1379), listed below; no fix claimed.

### Control-plane API: LAN/container client to daemon

**Observed controls. O06:** API enablement defaults false and its bind defaults to loopback
(`src-tauri/src/config/settings.rs:579-589`;
`src-tauri/src/config/settings/field.rs:375-377`). The parser accepts an `IpAddr` and nonzero
port (`src-tauri/src/config/settings/load/validate.rs:71-83`); the listener binds that address
(`src-tauri/src/api.rs:398-402`).

**O07:** The ordinary authenticator extracts an `Authorization: Bearer` token, checks the
source-IP lockout, authenticates the registry entry and checks the requested scope
(`src-tauri/src/api/handlers.rs:26-37,132-181`). Registry authentication hashes the presented
secret and compares an unrevoked, unexpired entry (`src-tauri/src/api/auth.rs:327-333`);
SHA-256 is used for that hash (`src-tauri/src/api/auth.rs:163-178`). Minting stores
`token_hash`, bound root and scopes (`src-tauri/src/api/auth.rs:567-578`). The default lockout
parameters are 10 failures within 10 seconds and 60 seconds of lockout
(`src-tauri/src/api/auth.rs:825-827`).

**O08:** API sender resolution derives an FQN from the credential's bound root and rejects
Root identities (`src-tauri/src/api/identity.rs:21-35`).

**Limits. O09:** Non-loopback addresses reach a warning function, rather than a rejection
(`src-tauri/src/api.rs:398-401,503-511`). The source evidence here does not establish a host
firewall policy, transport encryption or protection against a same-user process reading a
client secret. The web query-token contract is separate from the API bearer contract.

**H02 — hypothesis:** Widening the bind for containers could also admit unintended LAN peers;
a stolen scoped API secret could enable the actions allowed by that scope.

**Gaps:** G04 (#793) tracks API follow-ups; G02 (#1357) concerns host credential-file access.

### CLI tokens and ordinary messaging: local process to mailbox

**Observed controls. O10:** CLI token validation accepts a matching persisted root/master token
or a parseable UUID (`src-tauri/src/cli.rs:389-423`); send invokes it
(`src-tauri/src/cli/send.rs:326`). The ordinary mailbox looks up the session token and compares
`msg.from` with a name derived from the session working directory
(`src-tauri/src/phone/mailbox/outbox_auth.rs:465-469,488-505`).

**Limits:** The CLI shape check alone is not proof of live identity. The cited mailbox checks
are narrower than a claim of isolation from another process under the same OS user.
The policy's self-reported-root discussion is recorded separately
(`docs/security.md:50-54,159`), not used as sole control evidence.

**H03 — hypothesis:** A same-user process able to read a live credential or mutate reachable
agent files could impersonate an ordinary sender. This document does not demonstrate an
exploit or infer the current exploitability of #139 from its open state.

**Gaps:** G05 (#139); ordinary-message authority is not evidence of privileged PTY authority.

### Ordinary PTY input: user/web/Telegram/message text to terminal

**Observed controls. O11:** Web PTY dispatch reaches `pty_write_command`
(`src-tauri/src/web/commands.rs:260,450-477`); Telegram delivery calls
`runtime.inject_text(session_id, text)` (`src-tauri/src/telegram/bridge/poll.rs:416`). These
are ordinary text entry points, distinct from the privileged operation below.

**Limits:** No common scalar-validation claim is asserted for user keyboard, web,
Telegram or ordinary-message delivery by this review. G06 is a tracking link, not proof
that a fix has or has not landed on one particular path.

**H04 — hypothesis:** Adversarial terminal text or instructions could affect an agent or its
terminal state even when the enclosing message route is authorized.

**Gaps:** G06 (#4309).

### Privileged exact PTY input: verified actuation request to one target

**Observed controls. O12:** Host ingress resolves a unique live session by token and checks
for a local-process backend (`src-tauri/src/phone/mailbox/pty_validation.rs:46-65`), verifies
the PTY route and sender FQN, then verifies the outbox identity
(`src-tauri/src/phone/mailbox/pty_validation.rs:87-97,19-28`).

**O13:** API ingress uses fresh PTY authentication, duplicate-key-rejecting JSON parsing,
live authority verification and a verified PTY route
(`src-tauri/src/api/handlers/pty_input.rs:129-171`). Later API authority validation checks the
scope, session binding, credential generation, backend, physical root identity and runtime
credential hash (`src-tauri/src/phone/mailbox/pty_authority.rs:102-149`).

**O14:** The exact-text validator rejects empty input, inputs above `PTY_INPUT_MAX_BYTES`
and forbidden scalars (`src-tauri/src/pty/inject.rs:67-94`); the scalar predicate enumerates
control/bidi/separator values (`src-tauri/src/pty_input.rs:14-30`).

**Limits:** These executable checks describe selected ingress/revalidation controls; they
are not a claim that ordinary text paths inherit them. Full lifecycle and replay semantics
are outside this source survey. The dedicated policy is at `docs/security.md:56-78`.

**H05 — hypothesis:** Correctly authorized instructions could still induce unwanted agent
work; byte acceptance alone would not establish model understanding or completion.

**Gaps:** No issue yet for that model-consumption hypothesis. G05 and G06 track separate
ordinary-plane concerns and are not presented as bypasses of this authority path.

### Terminal snapshots: verified requester to terminal disclosure

**Observed controls. O15:** Snapshot requester proof resolves live requesters, distinguishes
local/container backends, checks coordinator/Root roles and verifies physical identities
(`src-tauri/src/pty/terminal_snapshot/authority.rs:34-87`). The snapshot admission path reads
strict disk and in-memory disclosure settings, rejects either disabled gate, then verifies
the route before capture (`src-tauri/src/pty/terminal_snapshot.rs:527-552`).

**O16:** The disclosure setting defaults true
(`src-tauri/src/config/settings.rs:596-597`;
`src-tauri/src/config/settings/field.rs:427-429`).

**Limits:** Snapshot disclosure is a read boundary, separate from both ordinary messaging
and PTY writes. No secret-redaction or forensic-erasure claim is made here;
full fidelity, persistence and cleanup behavior was not independently traced for this model.
The policy discussion is at `docs/security.md:80-119`.

**H06 — hypothesis:** A permitted viewport disclosure could contain credentials, source or
personal data, regardless of whether the requester is authorized.

**Gaps:** No issue yet assigned here for the residual content-disclosure hypothesis.
U03 leaves the default and retention policy open.

### Tauri IPC: window content to native authority

**Observed configuration. O17:** One capability names eight window patterns, including
`terminal-*` and `screenshot-overlay-*`, and lists window creation, event emission/listening
and file-dialog permissions (`src-tauri/capabilities/default.json:4-42`). The complete
`app` configuration has `withGlobalTauri: false` and an empty windows list, with no explicit
`security` or `csp` key anywhere in the file (`src-tauri/tauri.conf.json:1-38`).

**Limits:** This records configuration only. Framework default CSP behavior and the effective
permission set of each runtime window were not verified; no exploit or universal command
access is inferred from a permission name.

**H07 — hypothesis:** Compromised window content could misuse native permissions available to
that window, so shared capability scope and CSP merit separate evaluation.

**Gaps:** G07 (#4330) and G08 (#4286).

### Telegram: chat participant to PTY, terminal output to chat/log

**Observed controls. O18:** Incoming updates are filtered by configured chat ID
(`src-tauri/src/telegram/bridge/poll.rs:332-339`). The parser retains a sender first name and
chat ID (`src-tauri/src/telegram/api.rs:158-172`); accepted text is passed to `deliver_text`
(`src-tauri/src/telegram/bridge/poll.rs:341-378`) and then to PTY injection (`:416`).

**O19:** Bridge logging calls `redact` before preview truncation
(`src-tauri/src/telegram/output.rs:100-122`). The redactor replaces matched spans
(`src-tauri/src/telegram/redact.rs:67-80`); its fast-path markers include `/bot`, `key=`,
container prefix, `TOKEN=`, `--token` and `Bearer ` (`:126-136`).

**Limits:** No sender-ID allowlist check appears in the cited incoming parser/delivery path
(O18); this is a bounded source observation. Redaction is pattern based, not proof that
arbitrary secrets or terminal content are removed.

**H08 — hypothesis:** Another participant in the configured group chat could submit text to
the agent. Unrecognized secret formats could remain in outbound material. No group-chat
probe or exhaustive output-sink audit was performed.

**Gaps:** No issue yet identified for the group-sender hypothesis. G06 concerns ordinary
injection; U04 leaves chat membership and sender-policy choices open.

### Settings secrets: in-memory settings to browser and disk

**Observed controls. O20:** The browser `get_settings` path serializes the shared snapshot
helper (`src-tauri/src/web/commands/settings_dispatch.rs:53-59`). Its builder clones settings
and clears `root_token` (`src-tauri/src/commands/config/settings_snapshot.rs:340-350`), whose
field has `skip_serializing_if = "Option::is_none"`
(`src-tauri/src/config/settings.rs:654-655`).

**Limits. O21:** That clone retains Gemini/Jev string fields and Telegram bot configuration
(`src-tauri/src/config/settings.rs:380-382,397-398,476-483`); the bot configuration derives
`Serialize` and has a token field without a skip attribute
(`src-tauri/src/telegram/types.rs:3-9`). The cited builder clears root token only, so these
fields are part of the serialized snapshot when configured (O20).

**O22:** The settings writer serializes the settings object
(`src-tauri/src/config/settings/save/locked.rs:552-566`), passes the output object to the
atomic writer (`:692-693`), formats JSON bytes and writes those bytes through a temporary
file (`src-tauri/src/config/settings/save.rs:128-133,207-226,257-268`;
`src-tauri/src/filesystem/settings_file.rs:43-44`). This is JSON storage, not encryption at
that write boundary; no whole-system at-rest or ACL assurance is inferred.

**H09 — hypothesis:** Browser access or a same-user file read could disclose configured
service credentials; protecting `rootToken` alone would not address those other fields.

**Gaps:** G09 (#1356), G10 (#4486), G02 (#1357). The Jev-field observation is based on source,
not on whether the older issue title mentions that key.

### Container filesystem: container process to mounted host data/credentials

**Observed controls. O23:** Docker command construction adds a host-root bind mount and one
bind mount per `request.repo_mounts`, with no read-only option in those mount specifications
(`src-tauri/src/pty/docker_runtime.rs:176-194`).

**O24:** The Claude profile identifies `.claude/.credentials.json` and first-run state flags
(`src-tauri/src/session/profile.rs:293-305`). Credential-plan resolution derives a host
source and destination under the mount root
(`src-tauri/src/pty/container_credentials.rs:41-66`); copy-in checks destination reparse
paths, copies the file, and attempts Unix private permissions
(`src-tauri/src/pty/container_credentials.rs:92-116`;
`src-tauri/src/filesystem/documents.rs:278-285`).

**Limits. O25:** The private-permission call is Unix-conditional and failure returns to the
copy flow after logging (`src-tauri/src/pty/container_credentials.rs:112-128`). Removal checks
the destination and attempts file deletion (`:135-154`). This source survey does not claim
an owner-only Windows ACL, crash cleanup or forensic erasure. It does not claim all session
stop paths were audited.

**H10 — hypothesis:** A compromised container could modify writable mounted repositories
or read a copied credential; broad inherited ACLs or interrupted cleanup could prolong exposure.
The provider credential's account scope and refresh behavior were not independently verified.

**Gaps:** G11 (#933), G12 (#1791); U05 leaves mount/credential isolation policy open.

### Container network and environment: container process to host/API

**Observed controls and limits. O26:** The Docker command passes API URL, credential-variable
names and bridge environment data (`src-tauri/src/pty/docker_runtime.rs:139-171`), and keeps
secret environment separately in the returned command specification (`:203-215`). The
complete `build_run_command` function adds no `--network`, `--user`, `--cap-drop` or
`--read-only` argument (`:121-217`); that is an observed omission in this function, not a
measurement of Docker/image defaults or a claim of isolation.

**H11 — hypothesis:** Image-selected users and deployment-selected Docker networking could
allow unintended egress or access to an API widened for containers. Actual container network,
image user, host routing and firewall behavior were not tested.

**Gaps:** G04 (#793); no issue yet assigned here to image-user/network-policy choices (U05).

### Supply chain: dependency publisher to application

**Observed configuration. O27:** The repository workflow invokes `cargo deny --locked check`
(`.github/workflows/cargo-deny.yml:42-44`). That command's presence is not proof of any
particular run result, dependency's security or coverage of npm dependencies.

**Limits:** Advisory completeness, release integrity and transitive dependency behavior were
not evaluated for this documentation change.

**H12 — hypothesis:** A malicious or newly vulnerable dependency could cross the privileges
of its consuming application, independently of a previously green advisory check.

**Gaps:** G13 (#4490). G14 (#4315) is a closed tracking item, not an outstanding gap or proof
of implementation; O27 is the executable configuration evidence.

## Gap register

The following states come from `gh issue view <number> --json state` queries on 2026-11-13.
For each OPEN entry: **open as of 2026-11-13; no fix claimed**. That wording does not assert
that no partial mitigation exists. Hypotheses marked **no issue yet** above have no issue
assigned by this document.

| ID | Tracking issue | State and scope |
| --- | --- | --- |
| G01 | [#908](https://github.com/mblua/AgentsCommander/issues/908) | OPEN; web debug authentication |
| G02 | [#1357](https://github.com/mblua/AgentsCommander/issues/1357) | OPEN; Windows credential-file ACLs |
| G03 | [#1379](https://github.com/mblua/AgentsCommander/issues/1379) | OPEN; web bind UI |
| G04 | [#793](https://github.com/mblua/AgentsCommander/issues/793) | OPEN; API follow-ups |
| G05 | [#139](https://github.com/mblua/AgentsCommander/issues/139) | OPEN; ordinary CLI/message identity |
| G06 | [#4309](https://github.com/mblua/AgentsCommander/issues/4309) | OPEN; ordinary external PTY text |
| G07 | [#4330](https://github.com/mblua/AgentsCommander/issues/4330) | OPEN; per-window capabilities |
| G08 | [#4286](https://github.com/mblua/AgentsCommander/issues/4286) | OPEN; explicit compatible CSP |
| G09 | [#1356](https://github.com/mblua/AgentsCommander/issues/1356) | OPEN; browser settings secrets |
| G10 | [#4486](https://github.com/mblua/AgentsCommander/issues/4486) | OPEN; OS keychain storage |
| G11 | [#933](https://github.com/mblua/AgentsCommander/issues/933) | OPEN; copied credential hardening |
| G12 | [#1791](https://github.com/mblua/AgentsCommander/issues/1791) | OPEN; container mount documentation |
| G13 | [#4490](https://github.com/mblua/AgentsCommander/issues/4490) | OPEN; npm advisory checks |
| G14 | [#4315](https://github.com/mblua/AgentsCommander/issues/4315) | CLOSED as of 2026-11-13; cargo-deny tracking; no fix inferred from state |

## USER decisions (open, not adopted here)

- **U01:** Decide how the untrusted-agent analysis stance relates to the existing trusted-agent
  wording; any change to `docs/security.md` belongs with its owner.
- **U02:** Decide the permitted LAN exposure, TLS/reverse-proxy expectations and wording for
  configurable binds versus the existing loopback-only statement.
- **U03:** Decide terminal-disclosure defaults, authorized audiences, output retention and
  any desired secret filtering; no new guarantee is adopted here.
- **U04:** Decide whether group-chat membership is sufficient authority or sender-level
  filtering is desired, and which Telegram disclosure risks are acceptable.
- **U05:** Decide acceptable container mounts, network/image-user restrictions and copied
  credential handling; no isolation guarantee or provider-account policy is adopted here.
- **U06:** Decide any product security guarantee, support policy, deadline or additional
  documentation links with the relevant owners; this document adds none.

## Claim table

Each ID covers the named entry above, including its stated scope. Hypothesis citations are
motivation only, not proof of an exploit. Gap citations establish tracking state only.
Independent PASS/FAIL results belong in a review bound to the exact PR head and source SHA,
not in this table. Shortened Rust module paths in this table are relative to `src-tauri/src/`.

| Claim ID | Type | Citation |
| --- | --- | --- |
| O01 | observed | `docs/security.md:12,21,34` (policy conflict only) |
| O02 | observed | `config/settings.rs:569-576`; `config/settings/field.rs:363-365` |
| O03 | observed | `web.rs:97-112,144-147` |
| O04 | observed | `web.rs:214-219,256-261`; `web/auth.rs:11-20`; `app_run.rs:372`; `filesystem/documents.rs:414-416` |
| O05 | observed | `web.rs:105-106,175-199,236-248`; `web/commands.rs:246-260`; `web/commands/settings_dispatch.rs:53-62` |
| O06 | observed | `config/settings.rs:579-589`; `config/settings/field.rs:375-377`; `config/settings/load/validate.rs:71-83`; `api.rs:398-402` |
| O07 | observed | `api/handlers.rs:26-37,132-181`; `api/auth.rs:163-178,327-333,567-578,825-827` |
| O08 | observed | `api/identity.rs:21-35` |
| O09 | observed | `api.rs:398-401,503-511` |
| O10 | observed | `cli.rs:389-423`; `cli/send.rs:326`; `phone/mailbox/outbox_auth.rs:465-469,488-505` |
| O11 | observed | `web/commands.rs:260,450-477`; `telegram/bridge/poll.rs:416` |
| O12 | observed | `phone/mailbox/pty_validation.rs:19-28,46-65,87-97` |
| O13 | observed | `api/handlers/pty_input.rs:129-171`; `phone/mailbox/pty_authority.rs:102-149` |
| O14 | observed | `pty/inject.rs:67-94`; `pty_input.rs:14-30` |
| O15 | observed | `pty/terminal_snapshot/authority.rs:34-87`; `pty/terminal_snapshot.rs:527-552` |
| O16 | observed | `config/settings.rs:596-597`; `config/settings/field.rs:427-429` |
| O17 | observed | `src-tauri/capabilities/default.json:4-42`; `src-tauri/tauri.conf.json:1-38` |
| O18 | observed | `telegram/bridge/poll.rs:332-339,341-378,416`; `telegram/api.rs:158-172` |
| O19 | observed | `telegram/output.rs:100-122`; `telegram/redact.rs:67-80,126-136` |
| O20 | observed | `web/commands/settings_dispatch.rs:53-59`; `commands/config/settings_snapshot.rs:340-350`; `config/settings.rs:654-655` |
| O21 | observed | O20; `config/settings.rs:380-382,397-398,476-483`; `telegram/types.rs:3-9` |
| O22 | observed | `config/settings/save/locked.rs:552-566,692-693`; `config/settings/save.rs:128-133,207-226,257-268`; `filesystem/settings_file.rs:43-44` |
| O23 | observed | `pty/docker_runtime.rs:176-194` |
| O24 | observed | `session/profile.rs:293-305`; `pty/container_credentials.rs:41-66,92-116`; `filesystem/documents.rs:278-285` |
| O25 | observed | `pty/container_credentials.rs:112-128,135-154` |
| O26 | observed | `pty/docker_runtime.rs:121-217` |
| O27 | observed | `.github/workflows/cargo-deny.yml:42-44` |
| H01 | hypothesis | O03-O05; no deployment verification |
| H02 | hypothesis | O06-O09; no LAN probe |
| H03 | hypothesis | O10; no impersonation demonstration |
| H04 | hypothesis | O11; no injection probe |
| H05 | hypothesis | O12-O14; no model-consumption proof; no issue yet |
| H06 | hypothesis | O15-O16; no content audit; no issue yet |
| H07 | hypothesis | O17; no runtime capability/CSP assessment |
| H08 | hypothesis | O18-O19; no group-chat probe; no issue yet |
| H09 | hypothesis | O20-O22; no credential-disclosure probe |
| H10 | hypothesis | O23-O25; no mount/credential attack probe |
| H11 | hypothesis | O06,O26; no deployment network/image assessment; no issue yet |
| H12 | hypothesis | O27; no dependency evaluation |
| G01 | gap | issue #908 state query, 2026-11-13 |
| G02 | gap | issue #1357 state query, 2026-11-13 |
| G03 | gap | issue #1379 state query, 2026-11-13 |
| G04 | gap | issue #793 state query, 2026-11-13 |
| G05 | gap | issue #139 state query, 2026-11-13 |
| G06 | gap | issue #4309 state query, 2026-11-13 |
| G07 | gap | issue #4330 state query, 2026-11-13 |
| G08 | gap | issue #4286 state query, 2026-11-13 |
| G09 | gap | issue #1356 state query, 2026-11-13 |
| G10 | gap | issue #4486 state query, 2026-11-13 |
| G11 | gap | issue #933 state query, 2026-11-13 |
| G12 | gap | issue #1791 state query, 2026-11-13 |
| G13 | gap | issue #4490 state query, 2026-11-13 |
| G14 | gap | issue #4315 CLOSED state query, 2026-11-13; historical tracking only |
| U01 | USER | open wording decision; O01 |
| U02 | USER | open network policy decision; O01,O03,O06,O09 |
| U03 | USER | open disclosure policy decision; O15-O16,H06 |
| U04 | USER | open Telegram policy decision; O18-O19,H08 |
| U05 | USER | open container policy decision; O23-O26,H10-H11 |
| U06 | USER | open guarantee/support decision; scope exclusions |
