# #1005 S2 checklist (issue #1010)

Author: dev-rust (wg-25). Branch `feat/1010-1005-s2-messaging`, base main @ 83a093a.
Per plan section 4.5. Sources: `render_inter_agent_messaging_block` (session_context.rs:2881 at base), `peer_name_format` (:3069-3072), `send_message_instructions` (:3074-3112).

## Grinch blind OLD-side inventory (4.5(a0)) - RESERVED

> Grinch: append your blind OLD-text inventory HERE, committed before reading the
> mapping tables below. Old texts: `git show 83a093a:src-tauri/src/config/session_context.rs`
> lines 2881-2911 (shell), 3069-3072 (peer formats), 3074-3112 (send instructions, three variants).

---

## Harvested test needles (4.4)

All kept verbatim in the rewrite unless marked SWAPPED:

| Needle | Guarding test |
|---|---|
| `### Incoming Message Notifications` + before-`### Send` ordering | notification subsection test |
| `Process this inter-agent message` | same + wire coupling tests |
| `operational inter-agent message` | same |
| `within your role, authority, and write restrictions` | same |
| `do not stop at a summary unless it asks only for one` | same |
| `If the task finishes or blocks` | same |
| `filename ONLY` | `default_context_embeds_filename_only_warning` |
| `BAD:` | same test - SWAPPED to `never a path` (plan-specified cut of the BAD line; the test's point, the filename-only warning, is preserved by `filename ONLY` + `never a path` + `GOOD:`) |
| `GOOD:` | same |
| `<project>:<workgroup>/<agent>` / `<project>/<agent>` | `default_context_embeds_fqn_format_and_filesystem_warning` |
| `filesystem directory name is NEVER` | same |
| `__agent_` | same |
| `list-peers-lean` | same + root tests |
| `verified WG coordinator replicas only` | root documents-coordinators-only test |
| `Origin coordinators and non-coordinator WG replicas are not valid Root Agent targets in #277` | same (single carrier now: peer_name_format) |
| `Use only the JSON `name` values returned by `list-peers-lean`` | same (kept in Root send instructions) |
| `Root messaging is **file-based**` (count == 1) | root context/count tests |
| `This session has no messaging directory` | None-mode omits test |
| absent: `walk up from your root` (None mode) | same (WG variant's walk-up phrasing appears only in WG mode) |
| absent: `workgroup messaging directory` (Root mode) | root omits test (Root text says "Root Agent messaging directory") |
| `## Inter-Agent Messaging` heading | #658 + legacy set (byte-identical) |
| `### Send a message to another agent`, `### List available peers` subheadings | legacy heading set (byte-identical) |

## Em-dash constraint map (4.2)

- Pinned-free targets in S2 scope: none (A3 is not in the :4810-4814 pin set).
- Rewritten A3 texts are U+2014-free (verified by scan): shell, both peer formats, all three send variants. Old em-dashes were punctuation, not anchors: peer-format `shape — e.g.` became `shape, e.g.`; `filename ONLY — never a path` became `filename ONLY, never a path` (both needles preserved).
- The six G5 em-dash needle tests (:4316/:4360/:4390/:4658/:4702/:4848 base numbering) assert the A2 `Narrow exception —` headers, which S2 does NOT touch; all six pass unchanged. Their paired-anchor pivot remains S3 work as mapped.
- Surviving keep-exact em-dashes: A2 texts (S3 scope), A6 per-repo line, frozen legacy corpus - all untouched.

## G11 keep-exact couplings (verified at implementation time)

| Documented string in A3 | Emitting code | Status |
|---|---|---|
| `[Message from <peer>] Process this inter-agent message: <path>` | `format_pty_wrap` (messaging.rs:30-32) + `FILE_NOTIFICATION_PREFIX` (messaging.rs:13) | byte-equal (rendering of `\n[Message from {from}] {body}\n\r` with the prefix body) |
| `filename '...' contains path separators or traversal` | `MessagingError` display (messaging.rs:128: `filename '{0}' contains path separators or traversal`) | byte-equal modulo the `{0}` -> `...` placeholder ellipsis, as before |
| send command line (`"<AGENTSCOMMANDER_BINARY_PATH>" send --token ... --send <filename> --mode wake`) | CLI `send` syntax | byte-identical to old text, quoting preserved |
| list-peers-lean command line | CLI syntax | byte-identical |
| filename patterns `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` / `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md` | messaging filename validators | byte-identical |
| `{{PEER_NAME_FORMAT}}` / `{{SEND_MESSAGE_INSTRUCTIONS}}` token names | TemplateTokenSignals | untouched |

## Mapping tables

### A3 shell (`render_inter_agent_messaging_block`)

| # | Class | Old statement | New carrier |
|---|---|---|---|
| 1 | ANCHOR | `## Inter-Agent Messaging` + three subheadings | byte-identical |
| 2 | ANCHOR | wire notification line | byte-identical, backticked |
| 3 | RULE | treat notification as operational inter-agent message | "...is an operational inter-agent message" |
| 4 | PROCEDURE | read `<path>`, follow the file's task instructions | "read `<path>` and follow its task instructions" |
| 5 | RULE | act within role, authority, write restrictions | needle kept verbatim |
| 6 | RULE | do not stop at a summary unless it asks only for one | needle kept verbatim |
| 7 | RULE | on finish or block, reply to sender with concrete result or blocker via the send flow | "If the task finishes or blocks, reply to the sender with a concrete result or blocker via the send flow below." |
| 8 | RULE | MANDATORY: resolve exact name via list-peers-lean before sending; never guess | "**MANDATORY**: resolve the exact agent name via `list-peers-lean` before every send... Never guess agent names." |
| 9 | IDENTITY | peer-name source of truth = list-peers-lean JSON `name` field | "its JSON `name` field is the only authoritative source" (merged with row 8: same rule stated once) |
| 10 | RULE | filesystem dir names never valid `--to`; `__agent_*`/`_agent_*` are paths not peer names | "A filesystem directory name is NEVER a valid `--to` value (`__agent_*` replica and `_agent_*` matrix dirs are on-disk paths, not peer names)." |
| 11 | RULE | empty array -> do NOT scan `__agent_*` siblings; stop and report | "If `list-peers-lean` returns an empty array, do NOT fall back to scanning `__agent_*` siblings on disk; stop and report the empty result." |
| 12 | IDENTITY | peer name format label (canonical FQN, what list-peers-lean emits) | "**Peer name format** (canonical FQN, the `list-peers-lean` `name` field):" |
| 13 | IDENTITY | recipient gets notification with path, reads file from disk | "The recipient gets a notification with the file path and reads the file from disk." |
| 14 | RULE | no `--get-output` (blocks; non-interactive only) | kept verbatim |
| 15 | RULE | wait for the reply after sending | kept verbatim |
| 16 | ANCHOR | list-peers-lean command block | byte-identical |

### peer_name_format

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | IDENTITY | WG replica FQN shape + example | kept, em-dash -> comma |
| 2 | IDENTITY | origin agent shape + example | kept, em-dash -> comma |
| 3 | IDENTITY | Root: verified WG coordinator replicas only, shape + example | kept, em-dash -> comma |
| 4 | RULE | Root: #277 exclusions (origin coordinators, non-coordinator replicas) | kept here; the DUPLICATE copy at the end of the Root send instructions is DROPPED (this row is the surviving carrier) |

### send_message_instructions (WG)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | IDENTITY | file-based to avoid PTY truncation; two steps | kept ("Messaging is **file-based** to avoid PTY truncation. Two steps:") |
| 2 | PROCEDURE | step 1: write file in `<workgroup-root>/messaging/`; walk up to `wg-<N>-*` | "...at `<workgroup-root>/messaging/` (walk up from your root to the parent `wg-<N>-*` folder)." |
| 3 | ANCHOR | filename pattern + UTC + kebab <= 50 | "Filename pattern: `...` (UTC timestamp, sanitized kebab-case slug <=50 chars)." (pattern bytes identical; `\u{2264}` source escape renders the same <= char) |
| 4 | ANCHOR | step 2 send command line | byte-identical |
| 5 | RULE | `--send` takes filename ONLY, never a path | "**IMPORTANT: `--send` takes the filename ONLY, never a path**" |
| 6 | (example) | BAD line | DROPPED: plan-specified cut; the prohibition (row 5), the failure signature (row 7), and the GOOD example carry the rule |
| 7 | IDENTITY | passing a path triggers the exact error string | "(a path fails with `filename '...' contains path separators or traversal`)" |
| 8 | (example) | GOOD line | kept inline: "e.g. GOOD: `--send \"...\"`" |
| 9 | (explanation) | CLI resolves filename against messaging/ automatically | DROPPED: explanatory, no normative force; rows 2+5 carry where the file goes and what to pass |

### send_message_instructions (Root)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | RULE | use only JSON `name` values from list-peers-lean | needle kept verbatim (leading sentence) |
| 2 | IDENTITY | Root sessions list verified WG coordinators only | "...it returns verified WG coordinator replicas only." |
| 3 | IDENTITY | Root messaging file-based, two steps | "Root messaging is **file-based** to avoid PTY truncation. Two steps:" (count==1 preserved) |
| 4 | PROCEDURE | step 1 write to Root messaging dir ({path} fence) | kept, fence + placeholder untouched |
| 5 | ANCHOR | root filename pattern + UTC + kebab <= 50 | kept |
| 6 | ANCHOR | send command line (coordinator_name) | byte-identical |
| 7 | RULE | filename ONLY, never a path | kept (comma for em-dash) |
| 8 | RULE | #277 exclusions | DROPPED here: duplicate of peer_name_format row 4 (surviving carrier named) |

### send_message_instructions (None)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | IDENTITY | no messaging directory; `--send` requires `wg-<N>-*` ancestor or canonical Root dir; this root is neither | "This session has no messaging directory: `--send` requires your `--root` to sit under a `wg-<N>-*` ancestor or be the canonical Root Agent directory, and this root is neither." |
| 2 | RULE | do NOT walk up the filesystem looking for one | kept verbatim |
| 3 | GRANT | can still RECEIVE; read the notified absolute-path file, act on it | "You can still RECEIVE messages: when AgentsCommander hands you an absolute path in an incoming `[Message from <peer>]` notification, read that file, act on it..." |
| 4 | RULE | report result in-session, not through `send --send` | "...and report your result in this session rather than through `send --send`." |

## S1 advisory folded (zero-cost)

Grinch LOW-1: A5 intro first sentence restores "using" before "Claude Code-compatible
YAML frontmatter" (+6 chars, single-line const edit, no test or freeze impact: the
frozen legacy intro and two-sided compare are unaffected because the CURRENT intro
participates in the compare symmetrically). Done in the S2 rewrite commit.

## Measurements (harness)

Baseline @ 83a093a (= S1 head values):

| item | chars | ~tokens |
|---|---|---|
| block: inter-agent messaging (A3, replica) | 2662 | 665 |
| block: skills section (A5, synthetic 2 skills) | 1027 | 256 |
| profile: WG replica | 10507 | 2626 |
| profile: coordinator (+auto-clear) | 12940 / 15759 | 3235 / 3939 |
| profile: Root Agent (+auto-clear) | 16957 / 19776 | 4239 / 4944 |
| (other rows unchanged; see S1 checklist) | | |

Head @ 9c2dbaf:

| item | chars | ~tokens | delta chars |
|---|---|---|---|
| block: inter-agent messaging (A3, replica) | 2287 | 571 | -375 |
| block: skills section (A5, synthetic 2 skills) | 1033 | 258 | +6 (LOW-1) |
| profile: WG replica | 10138 | 2534 | -369 |
| profile: coordinator | 12571 | 3142 | -369 |
| profile: coordinator + auto_self_clear | 15390 | 3847 | -369 |
| profile: Root Agent | 16656 | 4164 | -301 |
| profile: Root Agent + auto_self_clear | 19475 | 4868 | -301 |
| (A2/A4/A6/A8/A9/B1/B3 rows unchanged) | | | 0 |

Net: -92 tok per replica/coordinator boot, -75 tok per Root boot.

## Deviation from plan (state + why)

Plan section 2 estimated S2 at ~250 tok/agent (2.9K -> 1.8K chars). Achieved -92
(replica). Same cause as S1 (recorded there): plan targets came from source-escape
char counts (the "~2.9K" A3 baseline is 2,662 actual), and the needle harvest locks
most operative sentences verbatim: the #711 paragraph alone carries five asserted
needles. The plan-specified cuts (BAD line, auto-resolve sentence, paragraph merge,
duplicate #277 line) were all taken. One test needle swapped (`BAD:` -> `never a
path`), recorded above; no other test changed.
