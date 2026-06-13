# 05 Inter-Agent Messaging

These cases validate CLI-driven and GUI-observable inter-agent messaging, including peer discovery, fire-and-forget delivery, wake routing, file-based message delivery, invalid peer handling, and visible evidence in recipient sessions.

Use clearly disposable test projects, workgroups, peers, message files, sessions, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. MSG-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only disposable peers created for the current test run in the expected disposable workgroup. Before any send, capture `list-peers-lean` JSON showing the recipient `name`, expected workgroup/team, `reachable:true`, sender root, and recipient root. Do not send to existing live team members.

Required evidence categories for this suite: peer discovery JSON, sender command stdout/stderr, message filename and file content, recipient terminal/session screenshot, sender and recipient roots, and any mailbox/outbox/log artifact needed to correlate one message end to end.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| MSG-001 | NOT RUN | TBD | Peer discovery checks not executed in this run. |
| MSG-002 | NOT RUN | TBD | Running peer delivery not executed in this run. |
| MSG-003 | NOT RUN | TBD | Wake routing checks not executed in this run. |
| MSG-004 | NOT RUN | TBD | File-based message delivery not executed in this run. |
| MSG-005 | NOT RUN | TBD | Invalid peer handling not executed in this run. |
| MSG-006 | NOT RUN | TBD | Evidence/correlation checks not executed in this run. |

Residual test data:

- TBD during execution.

Automation gaps observed:

- TBD during execution.

### MSG-001: Peer discovery returns canonical peer names

Purpose:

Verify that peer discovery returns canonical peer names and enough routing state to choose a safe disposable recipient.

Preconditions:

- `agentscommander_testeable.exe` is launched and verified with `window-info`.
- A disposable workgroup exists with a sender session and at least one disposable recipient peer.
- The sender and recipient roots are known or visible from session/context evidence.
- No live workgroup peers are used.

Steps:

1. Capture the target app and selected disposable workgroup.
2. From the sender session/root, run `list-peers-lean` with the current session token and root.
3. Save stdout/stderr as peer discovery evidence.
4. Identify the recipient peer using the JSON `name` field only.
5. Confirm the selected recipient belongs to the expected disposable workgroup/team and has `reachable:true`.
6. Record the sender root and recipient root in the evidence notes.
7. Capture the GUI peer/session list if visible and compare it to the peer JSON.

Expected Result:

Peer discovery returns canonical names such as `<project>:<workgroup>/<agent>`, and the safe recipient is explicitly identified by `name`, workgroup/team, `reachable:true`, sender root, and recipient root.

Evidence Required:

- `MSG-001-window-info.json`.
- `MSG-001-peer-discovery.json` or command log containing `list-peers-lean` output.
- `MSG-001-workgroup-peers.png` showing visible disposable peers if available.
- Notes showing the canonical recipient `name`, sender root, and recipient root.

Pass/Fail Criteria:

PASS if a disposable `reachable:true` peer is identified by canonical `name`. PARTIAL if peer JSON is valid but GUI comparison is unavailable. FAIL if testers must infer names from filesystem directory names or the peer belongs to a live workgroup. BLOCKED if no safe disposable peer is available.

### MSG-002: Message sent to a running peer appears in the recipient session

Purpose:

Verify that a filename-only message sent with `--mode wake` reaches an already running disposable peer.

Preconditions:

- Depends on MSG-001.
- The recipient peer is already running or waiting for input, and peer JSON shows `reachable:true`.
- A valid markdown message filename can be created in the disposable workgroup `messaging/` directory.
- The sender root and recipient root are recorded before sending.

Steps:

1. Capture pre-send `list-peers-lean` JSON showing recipient `name`, expected workgroup, and `reachable:true`.
2. Create a short markdown message file in the disposable workgroup `messaging/` directory using the required timestamped filename pattern.
3. Save the message content as evidence and record the exact filename.
4. Run `send --to "<canonical-peer-name>" --send <filename> --mode wake` from the sender root. Use filename-only `--send <filename>`, never a path.
5. Save sender command stdout/stderr.
6. Capture the recipient session showing the file notification or message handling.
7. Capture any sender outbox or mailbox artifact that correlates the send.

Expected Result:

The running disposable recipient receives a notification for the exact message file, and sender/recipient evidence can correlate the canonical peer name, filename, sender root, and recipient root.

Evidence Required:

- `MSG-002-pre-send-peers.json`.
- `MSG-002-message-file.md` or copy of message content.
- `MSG-002-send-command.log`.
- `MSG-002-recipient-notification.png`.
- Sender root and recipient root notes.

Pass/Fail Criteria:

PASS if the running recipient receives the exact message file notification and correlation evidence is complete. PARTIAL if delivery occurs but one correlation artifact is missing. FAIL if `--send` requires a path, the wrong peer receives the message, or delivery mutates a live peer. BLOCKED if no running disposable `reachable:true` recipient exists.

### MSG-003: Wake routing starts a stopped or absent peer predictably

Purpose:

Verify the documented wake behavior for a cold disposable peer whose `sessionStatus` is `none` or otherwise not running.

Preconditions:

- Depends on MSG-001.
- A disposable recipient peer exists and peer JSON shows `reachable:true`.
- The recipient is cold or absent, with `sessionStatus` recorded as `none` or another documented non-running state.
- The tester can safely send two harmless messages if the first send only starts the peer.

Steps:

1. Capture pre-send peer JSON showing recipient `name`, `reachable:true`, sender root, recipient root, and `sessionStatus: "none"` when applicable.
2. Create a harmless markdown message file in the workgroup `messaging/` directory.
3. Run `send --to "<canonical-peer-name>" --send <filename> --mode wake` using filename-only `--send <filename>`.
4. Save stdout/stderr and capture the GUI/session state after the first send.
5. If the first send only starts the peer, wait until peer JSON shows the recipient is up or `working:true`.
6. Send the same message content or a second clearly named message file with `--mode wake`.
7. Capture the recipient notification after the second-send delivery.

Expected Result:

For a cold peer, the first wake starts the session when documented, and second-send delivery reaches the now-running disposable recipient. The case must not use `--get-output`.

Evidence Required:

- `MSG-003-cold-peer-before.json` showing `sessionStatus` and `reachable:true`.
- `MSG-003-first-send.log`.
- `MSG-003-peer-after-first-send.json` or screenshot showing wake/start state.
- `MSG-003-second-send.log` when second send is required.
- `MSG-003-recipient-after-second-send.png`.

Pass/Fail Criteria:

PASS if cold-peer behavior matches documented first-send spawn and second-send delivery. PARTIAL if wake succeeds but second-send evidence is incomplete. FAIL if wake routes to the wrong peer, delivery occurs without a traceable file, or `--get-output` is needed. BLOCKED if no safe cold disposable peer can be prepared.

### MSG-004: File-based message content is delivered without PTY truncation

Purpose:

Verify that file-based message delivery preserves message content by having the recipient read the file from disk rather than relying on long PTY injection.

Preconditions:

- Depends on MSG-001.
- A disposable running recipient peer with `reachable:true` is available.
- A harmless multiline markdown message can be written under the workgroup `messaging/` directory.

Steps:

1. Capture peer JSON showing the recipient canonical `name`, `reachable:true`, sender root, and recipient root.
2. Write a multiline markdown message file with a unique marker, several paragraphs, and a small table or list.
3. Save the exact message filename and content as evidence.
4. Send with `send --to "<canonical-peer-name>" --send <filename> --mode wake`; pass the filename only.
5. Capture sender stdout/stderr.
6. Capture recipient session output showing the file notification path.
7. From the recipient side or evidence notes, confirm the recipient can open/read the exact file path and marker.

Expected Result:

The recipient receives a file notification that points to the full message file, and the exact content remains available on disk without PTY truncation.

Evidence Required:

- `MSG-004-pre-send-peers.json`.
- `MSG-004-message-file.md` with multiline content.
- `MSG-004-send.log`.
- `MSG-004-recipient-file-notification.png`.
- Optional recipient-side `Get-Content` or equivalent log showing the unique marker.

Pass/Fail Criteria:

PASS if the exact file notification and message content are traceable end to end. PARTIAL if notification appears but recipient-side file-read evidence is unavailable. FAIL if content is truncated, path is wrong, or message is injected as raw PTY text instead of file notification. BLOCKED if no safe disposable recipient can read message files.

### MSG-005: Invalid peer or malformed send target fails safely

Purpose:

Verify that harmless invalid messaging inputs fail clearly without creating, waking, or mutating unintended sessions.

Preconditions:

- Depends on MSG-001.
- A disposable sender root is available.
- Baseline peer/session state has been captured.
- The invalid command is chosen to avoid targeting any real peer.

Steps:

1. Capture baseline `list-peers-lean` JSON and GUI session state.
2. Prepare a harmless invalid target, such as a peer name not present in the JSON.
3. Optionally prepare a path-valued `--send` argument to confirm the filename-only rule, without using a real recipient.
4. Run the invalid `send` command and save stdout/stderr.
5. Capture peer/session state after the failed command.
6. Confirm no new recipient session, outbox delivery, or message mutation occurred.

Expected Result:

The CLI rejects invalid peer or malformed filename/path input with a clear error, and no unintended disposable or live session state changes.

Evidence Required:

- `MSG-005-before-peers.json`.
- `MSG-005-invalid-send.log`.
- `MSG-005-after-peers.json`.
- `MSG-005-after-gui-state.png` if GUI state is visible.

Pass/Fail Criteria:

PASS if the invalid command fails clearly and state remains unchanged. PARTIAL if failure is safe but the error text is incomplete. FAIL if an invalid target creates, wakes, or mutates a session. BLOCKED if a harmless invalid command cannot be run without risk to live peers.

### MSG-006: Messaging evidence correlates sender, recipient, and message file

Purpose:

Verify that a single message can be traced from sender command to recipient notification using concrete artifacts.

Preconditions:

- Depends on MSG-002 or MSG-004.
- At least one disposable send has been performed in the current run.
- Sender root, recipient root, canonical recipient `name`, message filename, and command log are available.

Steps:

1. Collect the sender command log for one completed send.
2. Collect the message file content and filename.
3. Collect the peer JSON that identified the recipient `name` and `reachable:true`.
4. Collect recipient session evidence showing the notification or read action.
5. Collect any available outbox, mailbox, or conversation artifact for the same filename.
6. Compare timestamps, filename, sender root, recipient root, and canonical peer name across the artifacts.
7. Record any missing artifact or mismatch in the case notes.

Expected Result:

The evidence set ties one message to one sender, one recipient, one message filename, and the expected disposable workgroup without relying on filesystem directory names as `--to` values.

Evidence Required:

- `MSG-006-correlation-summary.md` or notes file.
- Peer discovery JSON from the same send.
- Sender command log.
- Message file content.
- Recipient notification screenshot.
- Optional outbox/mailbox/conversation artifact.

Pass/Fail Criteria:

PASS if all required artifacts correlate to one disposable message. PARTIAL if delivery is proven but one optional mailbox/outbox artifact is absent. FAIL if artifacts conflict, peer identity is inferred from directory names, or a live peer is involved. BLOCKED if no completed safe disposable send exists to correlate.
