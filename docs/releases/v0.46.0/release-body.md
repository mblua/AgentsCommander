# Agents Commander v0.46.0

### Added

- A **WWW** button next to Home opens an embedded web browser in the central pane, with back, forward, refresh and a URL bar. Agents can drive it with `web-browser navigate|reload|status`. (#3367, #3427)
- Every agent can clear its own live session immediately with `self-clear`, without the idle wait. Pi receives `/new`; a timeout leaves the outcome unknown and is never replayed. Automatic self-clear is unchanged. (#3275, #3279)
- Projects and the Root Agent now receive a default `self-handoff` skill, including existing projects that lack it. User-edited copies are left unchanged. (#3374, #3376)
- `telegram-send-message --text` sends a text message through a configured Telegram bot, without an attachment or an active bridge. (#518, #3104)
- Each Project in the sidebar shows a quiet count of files that differ from its cached `origin/main`, including untracked files. Zero hides the count; unavailable data shows a dash. (#3060, #3066, #3061, #3116)
- Settings > General > Agents > Room activity sets how long a room must stay quiet before it counts as idle: 1 to 3600 seconds, default 15. (#3123, #3127, #3124, #3128, #3125, #3129, #3126, #3130)
- New Team can add agents from other projects. Foreign agents keep their source project, shown as a dimmed `@sourceProject` suffix, and only generic assets are copied into a fresh local Matrix: `Role.md`, the skills tree and `default.*` folders. (#3072, #3082, #3073, #3102, #3142, #3154)
- Message files sent with `--send` are archived under the project's `.ac/message-archive/YYYYMMDD/`, so they survive room deletion. Archive failures warn without stopping the send. (#3081, #3090, #3120, #3121)
- Room creation logs each stage with its elapsed time, so slow room creation can be traced to a stage. (#3262, #3268)

### Changed

- Settings > General > Terminal > Response close (**Close terminal after response**) now takes effect: eligible local members close their terminal after a response and the configured idle time, keep the conversation, and resume when new work arrives. (#2854, #3138, #2855, #3165)
- A room stays active through brief gaps between agents. Room indicators, Non-stop and completion sounds now wait for the idle confirmation delay instead of reporting idle early. (#3086, #3114, #3087, #3115)
- Settings hides container options unless the instance setting `containerOptionsEnabled` is true. Saved container settings are preserved. (#3063, #3077)
- Config seed now prefers Agent Matrix templates over project templates: Matrix profile, then Matrix base, project profile, project base and the catalog default. (#2808, #3143)
- Settings > Coding Agents > Environment no longer shows user/system badges. Variables set by AgentsCommander are marked with a red ring instead. (#3134, #3136)
- SELF-HANDOFF asks only for what is needed to resume: first action, open work, blockers, pending decisions, live state and resume pointers. (#1555, #3369)

### Fixed

- Session snapshots and coordinator clocks are flushed to disk before they replace the previous file, so a host crash no longer leaves them empty. (#3278, #3280)
- The last prompt survives restart, wake and resume, and a late session list no longer overwrites a newer LAST PROMPT. (#3155, #3158, #3156, #3161)
- The first click on a Favorites or group rail button is no longer lost when the rail updates. (#3157, #3160)
- Codex's new `Trust this folder?` prompt is recognized as a blocking menu. (#3095, #3096)
- `send` checks Root Agent message filenames with the receiver's rule before printing `Queued`. `send --help` documents the filename format and that `Queued` does not mean delivered. (#3171, #3285)
- Failed self-handoff operations are reported in the UI and to the agent instead of failing silently. (#3189, #3300)
- `send --command clear` to Codex reports a failed fresh-session start as an error and never replays the clear. (#3371, #3457)
- Restoring a Root session marked as fresh no longer resumes the previous conversation. (#3431, #3452)
- An internal notice is checked again after waiting for the terminal, so a session that exited in the meantime receives nothing. (#3286, #3292)
- Closing a session, purging a room or waiting for Host PTY work no longer stalls unrelated mailbox delivery. (#3271, #3302)
- Mailbox archive files always stay inside `outbox/rejected/` and `outbox/delivered/`. Message ids that are not valid file names on Windows, or are too long, use fallback names instead of causing retry loops or duplicate delivery. (#3351, #3419, #3417, #3423, #3426, #3429)
- A folder or link at a receipt path no longer counts as a settled receipt. (#3475, #3497)

### Security

- Outbox request ids must be canonical UUIDs, Root identity is checked by the physical directory instead of its name, and config repair cannot write before read-only authority is established. (#3318, #3323)

### Notes

- New diagnostics: rail click logging, focus-input PTY debug logs and close-block logs for sessions stuck in Closing. Recovery of stuck sessions is still tracked in #3317. (#3092, #3094, #3093, #3103, #3131, #3133, #3248, #3326)
- Internal preparation for choosing profiles per agents file: profile storage keyed by BASE hash, agent-file source APIs and source references in persisted selections. The UI is still pending. (#2902, #3117, #2905, #3141, #2900, #3144)
- Agent and profile records carry optional `id_hash_debug` metadata that shows the inputs of each identity hash. Existing hashes are unchanged. (#3052, #3089, #3098, #3105, #3099, #3106, #3100, #3108, #3101, #3112)
- About 150 behavior-preserving refactor PRs break module cycles, give disk access and global state a single owner, apply Clippy pedantic fixes and remove dead code. CI adds sanitizing, function-length and Clippy lint-budget gates, and PR regression jobs run faster. (#3170, #3173, #3183, #3185, #3296, #3514, #3519, #3524, #3531)

## Included scope

- feature: feat(cli): send Telegram text without attachments (refs #518) ([#518](https://github.com/mblua/AgentsCommander/issues/518), [PR #3104](https://github.com/mblua/AgentsCommander/pull/3104))
- fix: fix: minimize SELF-HANDOFF resume policy (closes #1555) ([#1555](https://github.com/mblua/AgentsCommander/issues/1555), [PR #3369](https://github.com/mblua/AgentsCommander/pull/3369))
- fix: fix(config-seed): prioritize Matrix templates over project defaults (refs #2808) ([#2808](https://github.com/mblua/AgentsCommander/issues/2808), [PR #3143](https://github.com/mblua/AgentsCommander/pull/3143))
- feature: feat: add guarded response-close lifecycle and resume (refs #2854) ([#2854](https://github.com/mblua/AgentsCommander/issues/2854), [PR #3138](https://github.com/mblua/AgentsCommander/pull/3138))
- feature: feat: activate Settings-controlled response close (refs #2855) ([#2855](https://github.com/mblua/AgentsCommander/issues/2855), [PR #3165](https://github.com/mblua/AgentsCommander/pull/3165))
- maintenance: feat(config): preserve source references in persistence carriers (refs #2900) ([#2900](https://github.com/mblua/AgentsCommander/issues/2900), [PR #3144](https://github.com/mblua/AgentsCommander/pull/3144))
- maintenance: feat(profiles): bind profiles by source-local BASE hash (refs #2902) ([#2902](https://github.com/mblua/AgentsCommander/issues/2902), [PR #3117](https://github.com/mblua/AgentsCommander/pull/3117))
- maintenance: feat(config): add physical agent-file source APIs (refs #2905) ([#2905](https://github.com/mblua/AgentsCommander/issues/2905), [PR #3141](https://github.com/mblua/AgentsCommander/pull/3141))
- maintenance: chore(profiles): preserve pair identity metadata in TypeScript (refs #3052) ([#3052](https://github.com/mblua/AgentsCommander/issues/3052), [PR #3089](https://github.com/mblua/AgentsCommander/pull/3089))
- feature: feat(project): expose Project file difference count (refs #3060) ([#3060](https://github.com/mblua/AgentsCommander/issues/3060), [PR #3066](https://github.com/mblua/AgentsCommander/pull/3066))
- feature: feat: show quiet Project file difference count (refs #3061) ([#3061](https://github.com/mblua/AgentsCommander/issues/3061), [PR #3116](https://github.com/mblua/AgentsCommander/pull/3116))
- feature: feat(settings): hide container options by default (refs #3063) ([#3063](https://github.com/mblua/AgentsCommander/issues/3063), [PR #3077](https://github.com/mblua/AgentsCommander/pull/3077))
- feature: feat(team): import agents using source project identity (refs #3072) ([#3072](https://github.com/mblua/AgentsCommander/issues/3072), [PR #3082](https://github.com/mblua/AgentsCommander/pull/3082))
- feature: feat: preserve cross-project agents in New Team (refs #3073) ([#3073](https://github.com/mblua/AgentsCommander/issues/3073), [PR #3102](https://github.com/mblua/AgentsCommander/pull/3102))
- feature: feat: archive message source snapshots (refs #3081) ([#3081](https://github.com/mblua/AgentsCommander/issues/3081), [PR #3090](https://github.com/mblua/AgentsCommander/pull/3090))
- feature: feat: add shared room inactivity observer (refs #3086) ([#3086](https://github.com/mblua/AgentsCommander/issues/3086), [PR #3114](https://github.com/mblua/AgentsCommander/pull/3114))
- fix: fix: share confirmed room activity across consumers (refs #3087) ([#3087](https://github.com/mblua/AgentsCommander/issues/3087), [PR #3115](https://github.com/mblua/AgentsCommander/pull/3115))
- maintenance: feat: add scoped rail diagnostic ingress (refs #3092) ([#3092](https://github.com/mblua/AgentsCommander/issues/3092), [PR #3094](https://github.com/mblua/AgentsCommander/pull/3094))
- maintenance: chore: record rail gestures and refresh diagnostics (refs #3093) ([#3093](https://github.com/mblua/AgentsCommander/issues/3093), [PR #3103](https://github.com/mblua/AgentsCommander/pull/3103))
- fix: fix: recognize Codex folder-trust blocking menu (refs #3095) ([#3095](https://github.com/mblua/AgentsCommander/issues/3095), [PR #3096](https://github.com/mblua/AgentsCommander/pull/3096))
- maintenance: feat: prepare id_hash_debug storage (refs #3098) ([#3098](https://github.com/mblua/AgentsCommander/issues/3098), [PR #3105](https://github.com/mblua/AgentsCommander/pull/3105))
- maintenance: feat(config): tolerate id hash debug metadata (refs #3099) ([#3099](https://github.com/mblua/AgentsCommander/issues/3099), [PR #3106](https://github.com/mblua/AgentsCommander/pull/3106))
- maintenance: feat(config): derive original id hash debug inputs (refs #3100) ([#3100](https://github.com/mblua/AgentsCommander/issues/3100), [PR #3108](https://github.com/mblua/AgentsCommander/pull/3108))
- maintenance: feat: preserve hash debug metadata in frontend transport (refs #3101) ([#3101](https://github.com/mblua/AgentsCommander/issues/3101), [PR #3112](https://github.com/mblua/AgentsCommander/pull/3112))
- feature: feat: group message archives by day (refs #3120) ([#3120](https://github.com/mblua/AgentsCommander/issues/3120), [PR #3121](https://github.com/mblua/AgentsCommander/pull/3121))
- feature: feat: persist room inactivity confirmation preference (refs #3123) ([#3123](https://github.com/mblua/AgentsCommander/issues/3123), [PR #3127](https://github.com/mblua/AgentsCommander/pull/3127))
- feature: feat: notify room inactivity preference changes (refs #3124) ([#3124](https://github.com/mblua/AgentsCommander/issues/3124), [PR #3128](https://github.com/mblua/AgentsCommander/pull/3128))
- feature: feat: recover room idle settings across realms (refs #3125) ([#3125](https://github.com/mblua/AgentsCommander/issues/3125), [PR #3129](https://github.com/mblua/AgentsCommander/pull/3129))
- feature: feat: configure room inactivity delay in Settings (refs #3126) ([#3126](https://github.com/mblua/AgentsCommander/issues/3126), [PR #3130](https://github.com/mblua/AgentsCommander/pull/3130))
- maintenance: chore(pty): add focus input debug logs (refs #3131) ([#3131](https://github.com/mblua/AgentsCommander/issues/3131), [PR #3133](https://github.com/mblua/AgentsCommander/pull/3133))
- fix: fix(settings): replace environment source badges with red halo (refs #3134) ([#3134](https://github.com/mblua/AgentsCommander/issues/3134), [PR #3136](https://github.com/mblua/AgentsCommander/pull/3136))
- feature: feat: import only generic agent assets (refs #3142) ([#3142](https://github.com/mblua/AgentsCommander/issues/3142), [PR #3154](https://github.com/mblua/AgentsCommander/pull/3154))
- fix: fix(session): retain last prompt across resume (refs #3155) ([#3155](https://github.com/mblua/AgentsCommander/issues/3155), [PR #3158](https://github.com/mblua/AgentsCommander/pull/3158))
- fix: fix(terminal): preserve newer last prompt events (refs #3156) ([#3156](https://github.com/mblua/AgentsCommander/issues/3156), [PR #3161](https://github.com/mblua/AgentsCommander/pull/3161))
- fix: fix(sidebar): preserve first group rail click refs #3157 ([#3157](https://github.com/mblua/AgentsCommander/issues/3157), [PR #3160](https://github.com/mblua/AgentsCommander/pull/3160))
- maintenance: ci: add sanitizing baseline and text gates (refs #3167) ([#3167](https://github.com/mblua/AgentsCommander/issues/3167), [PR #3170](https://github.com/mblua/AgentsCommander/pull/3170))
- maintenance: ci: add layout and edge sanitizing gates (#3168) ([#3168](https://github.com/mblua/AgentsCommander/issues/3168), [PR #3173](https://github.com/mblua/AgentsCommander/pull/3173))
- fix: fix(cli): validate Root message filenames before enqueue (refs #3171) ([#3171](https://github.com/mblua/AgentsCommander/issues/3171), [PR #3285](https://github.com/mblua/AgentsCommander/pull/3285))
- maintenance: ci: versioned allowlist for sanitizing gates (closes #3182) ([#3182](https://github.com/mblua/AgentsCommander/issues/3182), [PR #3183](https://github.com/mblua/AgentsCommander/pull/3183))
- maintenance: ci(workflows): cut PR regression gates wall time (refs #3184) ([#3184](https://github.com/mblua/AgentsCommander/issues/3184), [PR #3185](https://github.com/mblua/AgentsCommander/pull/3185))
- fix: fix(mailbox): report failed self-handoffs to the UI and the agent (refs #3189) ([#3189](https://github.com/mblua/AgentsCommander/issues/3189), [PR #3300](https://github.com/mblua/AgentsCommander/pull/3300))
- maintenance: fix(#3248): reproduce permanent Closing veto and add close-block diagnostics ([#3248](https://github.com/mblua/AgentsCommander/issues/3248), [PR #3326](https://github.com/mblua/AgentsCommander/pull/3326))
- feature: feat: diagnose room creation latency with stage logs (refs #3262) ([#3262](https://github.com/mblua/AgentsCommander/issues/3262), [PR #3268](https://github.com/mblua/AgentsCommander/pull/3268))
- fix: fix(mailbox): bound teardown and Host PTY observation (refs #3271) ([#3271](https://github.com/mblua/AgentsCommander/issues/3271), [PR #3302](https://github.com/mblua/AgentsCommander/pull/3302))
- feature: feat: add immediate self-clear for every agent (refs #3275) ([#3275](https://github.com/mblua/AgentsCommander/issues/3275), [PR #3279](https://github.com/mblua/AgentsCommander/pull/3279))
- fix: fix: sync session snapshots and coordinator clocks before publishing (refs #3278) ([#3278](https://github.com/mblua/AgentsCommander/issues/3278), [PR #3280](https://github.com/mblua/AgentsCommander/pull/3280))
- fix: fix(pty): re-check internal notice admission after writer permit (refs #3286) ([#3286](https://github.com/mblua/AgentsCommander/issues/3286), [PR #3292](https://github.com/mblua/AgentsCommander/pull/3292))
- maintenance: ci: stop committing module-arcs.txt; generate it in CI (#3288) ([#3288](https://github.com/mblua/AgentsCommander/issues/3288), [PR #3296](https://github.com/mblua/AgentsCommander/pull/3296))
- security: fix(security): UUID request IDs, physical Root identity and pre-effect repair authority refs #3318 ([#3318](https://github.com/mblua/AgentsCommander/issues/3318), [PR #3323](https://github.com/mblua/AgentsCommander/pull/3323))
- fix: fix(mailbox): keep reject_message archives inside outbox/rejected (#3351) ([#3351](https://github.com/mblua/AgentsCommander/issues/3351), [PR #3419](https://github.com/mblua/AgentsCommander/pull/3419))
- feature: feat: embedded web browser in the terminal pane (#3367) ([#3367](https://github.com/mblua/AgentsCommander/issues/3367), [PR #3427](https://github.com/mblua/AgentsCommander/pull/3427))
- fix: fix(mailbox): report failed Codex clear and fence replay (refs #3371) ([#3371](https://github.com/mblua/AgentsCommander/issues/3371), [PR #3457](https://github.com/mblua/AgentsCommander/pull/3457))
- feature: feat: seed a default self-handoff skill (closes #3374) ([#3374](https://github.com/mblua/AgentsCommander/issues/3374), [PR #3376](https://github.com/mblua/AgentsCommander/pull/3376))
- fix: fix(mailbox): keep move_to_delivered archives inside outbox/delivered (#3417) ([#3417](https://github.com/mblua/AgentsCommander/issues/3417), [PR #3423](https://github.com/mblua/AgentsCommander/pull/3423))
- fix: fix(mailbox): fall back for Windows-invalid and overlong archive ids (refs #3426) ([#3426](https://github.com/mblua/AgentsCommander/issues/3426), [PR #3429](https://github.com/mblua/AgentsCommander/pull/3429))
- fix: fix(restore): honor persisted Root fresh intent (refs #3431) ([#3431](https://github.com/mblua/AgentsCommander/issues/3431), [PR #3452](https://github.com/mblua/AgentsCommander/pull/3452))
- fix: fix(mailbox): require regular settled receipt files (refs #3475) ([#3475](https://github.com/mblua/AgentsCommander/issues/3475), [PR #3497](https://github.com/mblua/AgentsCommander/pull/3497))
- maintenance: ci: reject new or growing Rust functions above Clippy too_many_lines (refs #3507) ([#3507](https://github.com/mblua/AgentsCommander/issues/3507), [PR #3514](https://github.com/mblua/AgentsCommander/pull/3514))
- maintenance: ci: enforce ten additional Clippy lints (refs #3512) ([#3512](https://github.com/mblua/AgentsCommander/issues/3512), [PR #3519](https://github.com/mblua/AgentsCommander/pull/3519))
- maintenance: ci: prevent growth in seven Clippy lint budgets (refs #3528) ([#3528](https://github.com/mblua/AgentsCommander/issues/3528), [PR #3531](https://github.com/mblua/AgentsCommander/pull/3531))

## Install from npm

```text
npx @mblua/agentscommander@0.46.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/3605
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.45.1`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
