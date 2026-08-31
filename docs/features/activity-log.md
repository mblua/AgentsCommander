# Activity log

For developers who want a machine-readable record of when their sessions were working and when they were idle. After this page you can turn the log on, find the file, and read a line without guessing what its fields mean.

The activity log is an append-only JSONL file. AC writes one line per event: a session started working, a session went idle, the app started, the app is alive, the app stopped. It is **off by default** and it records no terminal content.

## What it records

Every line is one event, and there are five kinds:

| Event | When AC writes it |
|---|---|
| `busy` | A session's working state goes from false to true. |
| `idle` | A session's working state goes from true to false. |
| `app_start` | AC starts. The line carries the process id, the app version, what it found of the previous run, and the metrics declaration. |
| `app_alive` | A heartbeat carrying the detector's current view of which sessions are working. It is also the backstop for closing edges no other site emits. |
| `app_stop` | AC stops, with whether the open sessions could be enumerated and how many there were. |

A `metrics` line also appears as the first record of a freshly rotated file. It re-declares the metrics contract so the file stays self-describing after the original `app_start` has been rotated away. It is deliberately not a second `app_start`.

What the log does **not** contain: terminal output, prompts, command text, or anything a session printed. It records edges and identities, not content.

## Turning it on

Set `activityLogEnabled` to `true` in `settings.json`. It is `false` by default, and a missing, null or malformed value is read as `false`.

Writing is best-effort by design. If an append fails, AC logs a warning and keeps running rather than failing the operation that produced the event:

```text
[activity] append to <path> failed (continuing): <error>
```

## Where the file lives

The file is `activity.jsonl`, in the configuration directory selected by the exact binary version, alongside `settings.json` and `sessions.json`. Two copies keep separate logs only when they select different directories.

See [Directory layout](../reference/directory-layout.md) for the rule that picks that directory; this page does not restate it.

The file rotates when it grows past its size limit. On rotation, AC starts the new file with the `metrics` line described above, for the same run as the record that triggered it.

## Record format

One event per line, each a complete JSON object. Here is a heartbeat line:

```json
{"v":1,"at":"2026-08-18T14:32:07.412Z","runId":"9d0d0f6a-6f5c-4d7f-9f5e-2f3b1c8a44e1","event":"app_alive","workingSessionIds":["2f9a1c7e-77d1-4a0a-9c53-0f6f9f2f1a20"]}
```

Every line carries the same three envelope fields, then the event tag, then the fields belonging to that event:

| Field | Present on | Meaning |
|---|---|---|
| `v` | every line | Schema version. Currently `1`. |
| `at` | every line | The timestamp, RFC 3339 in UTC with milliseconds, for example `2026-08-18T14:32:07.412Z`. |
| `runId` | every line | Identifies one run of the app. Every line written between a start and a stop shares it. |
| `event` | every line | Which of the five kinds this is, plus `metrics`. |
| `sessionId` | `busy`, `idle` | The session the edge belongs to. |
| `name` | `busy`, `idle` | The session's name at the time of the edge. |
| `cwd` | `busy`, `idle` | The session's working directory. |
| `agentKind` | `busy`, `idle` | Which coding agent the session runs. Absent when AC has none recorded. |
| `reason` | `busy`, `idle` | Why the edge was recorded. |
| `continuesBlock` | `busy` | Whether this `busy` continues the previous working block rather than starting a new one. |
| `gapMs` | `busy` | The gap since the previous block, in milliseconds. Present exactly when `continuesBlock` is true, and absent otherwise. There is no third case. |
| `idleThresholdMs` | `idle` | Present only when the close waited out the idle threshold. Its absence is what tells a consumer not to subtract anything. |
| `workingSessionIds` | `app_alive` | The sessions the detector currently considers working. |
| `pid` | `app_start` | The process id of the run. |
| `appVersion` | `app_start` | The AC version that wrote the line. |
| `previousRunScan` | `app_start` | What AC found when it looked for the previous run's state. |
| `previousRun` | `app_start` | The previous run's details, or `null` when the scan found nothing readable. |
| `concurrentInstancePid` | `app_start` | Present only when another live process holds the previous `daemon.pid`. Its presence forbids recovery-based closing. |
| `metrics` | `app_start`, `metrics` | The self-describing metrics declaration. |
| `openSessionsEnumerated` | `app_stop` | Whether the open sessions could be counted at shutdown. |
| `openSessionCount` | `app_stop` | How many there were. |

## Settings

| Key | What it controls |
|---|---|
| `activityLogEnabled` | Whether AC writes `activity.jsonl`. `false` by default. |

See [Settings reference](../reference/settings.md#logging) for the logging group this key belongs to.

## Troubleshooting

**"There is no `activity.jsonl`."** `activityLogEnabled` is `false`, which is the default. Note that AC also reads it as `false` when the key is missing, `null`, or the settings file is malformed, so a JSON syntax error elsewhere in the file turns the log off without complaining about the log.

**"The file exists but stopped growing."** Look for `[activity] append to <path> failed (continuing): <error>` in the application log. Appends are best-effort: a failure is warned about and skipped, never retried into a blocking error.

**"I am looking at the wrong file."** A second copy writes a different `activity.jsonl` only if it selected a different configuration directory. Confirm the binary version and selected path before concluding anything from an empty file.

**"An old `app_start` disappeared."** The file rotated. The rotated file opens with a `metrics` line instead, which is why a consumer can still read it without the original `app_start`.

**"I need the application log, not this."** The activity log is not the debug log. `logLevel` and the filter syntax are covered in [Log filtering](../reference/log-filtering.md).

## See also

- [Directory layout](../reference/directory-layout.md) - where `activity.jsonl` sits and why
- [Log filtering](../reference/log-filtering.md) - the separate application log and its levels
- [Settings reference](../reference/settings.md#logging) - `activityLogEnabled` and `logLevel`
