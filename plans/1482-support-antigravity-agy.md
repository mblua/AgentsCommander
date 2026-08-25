# Plan #1482: First-class Antigravity (agy) support + legacy Gemini coding-agent runtime removal

Author: ac-architect-v3, workgroup wg-21-ac-dev-team-v3. Full cross-owner change (Rust backend, TS frontend, docs/README/npm/issue template, webpage chip).

Status: READY_FOR_IMPLEMENTATION

Revision: round 2 (2026-08-24) — combined CHANGES_REQUIRED verdicts (dev F1-F3 + grinch G1-G8) applied: F1 §5.10 fixture fixed to an exact-stem Established pair; G1-G7 missed stale lines added (§5.5/§5.6/§5.9/§5.11/§5.13); G8 preserve anchors corrected (settings.md voice fields, architecture.md :549 vs :558); F2 §5.3.5 comment-only reword; F3 §9.5 detector path corrected. Round-1 digest 94911216B04F2366AAA982DA9669547A5B99DAB45B2B29EF4875C0E3C8641C95 superseded.

Issue: [mblua/AgentsCommander#1482](https://github.com/mblua/AgentsCommander/issues/1482), "feat: support Antigravity (agy) as a first-class coding agent" (OPEN).

This is a Full change: (A) first-class Antigravity runtime support per the issue (new `CodingAgentKind`, PTY injection `Established`, resume grammar, catalog preset, mailbox/session support, TS types), (B) removal of the legacy Gemini **coding-agent** runtime while preserving Gemini voice/API/config/security/privacy behavior and tests, and (C) removal of stale Gemini-CLI-as-supported wording in docs/README/npm README/issue template/webpage chip. The architectural basis is the WG14 HTML plan (SHA-256 `5D0BB7CCFFC894B0BFA83FC1FEA77FFFF1585D2A2A612ED015EAC4DD04DE70DE`, byte-identical copy in this architect's replica root) reconciled with the user-confirmed scope: where the HTML plan's conservative "catalog-only Antigravity" step conflicts with the issue's full runtime-kind requirements, **the user's confirmed instruction wins** (full `CodingAgentKind::Antigravity`, `PtyInjectionProfile::Established`, resume tokens). HTML-plan roadmap steps 4–5 (deepen `session/profile.rs` seam, delete `commands/codex_resolver.rs`) are **explicitly out of scope** for this branch (see §4).

---

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `feat/1482-support-antigravity-agy`, based on synced `main`.

At authoring time (2026-08-24 UTC) the committed `HEAD` of the branch is `9cba3852f5ec62a97d9edf452c2aa662fe4665f1` and `git status --porcelain` is empty. Codebase Memory gate: `ready` (project `D-0_repos-AgentsCommander_iac-.ac-wg-21-ac-dev-team-v3-repo-AgentsCommander`, index at the same SHA). Baseline dependency graph measured on this clean tree: **191 modules, 3683 module edges, 1 cyclic SCC (85 members)**; see §11.

Root `.gitignore` ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1482-support-antigravity-agy.md`. Do not remove or weaken the `plans/` ignore rule.

The implementers must repeat the authority ritual: fetch `origin/main`, and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

## 2. Issue and user-confirmed scope (authoritative)

The user (product owner) has confirmed scope and authorized implementation on this branch. The branch must cover EVERYTHING in issue #1482 **and additionally**:

1. **Remove erroneous Gemini references** — stale support residue (docs/README/npm README/issue template/webpage chip) that presents Gemini CLI as a currently supported coding agent — preserving Gemini **voice** mentions (4 in the webpage repo, voice API docs), **historical** mentions (CHANGELOG, plans), and **negative no-Gemini guards** (scripts smoke guards, catalog "no gemini" tests, testing-safety prose).
2. **Remove the legacy Gemini coding-agent runtime in the same branch** (HTML plan Step 3): `CodingAgentKind::Gemini`, `commands/gemini_resolver.rs`, `telegram/gemini_watcher.rs`, resume apply/strip, reader/watcher wiring, TS union member, logical clear/compact mapping, context-target mapping, etc. Preserve Gemini voice/API/config/security/privacy behavior and tests.
3. **Implement everything in #1482**: `CodingAgentKind::Antigravity` detection (`agy`, `agy.exe`, `agy.cmd`, `antigravity`), `PtyInjectionProfile::Established` so `needs_explicit_enter("agy")` returns true (fixes `send_enter=false`), resume tokens `--continue` / `-c` / `--conversation <ID>`, preset/config (binary `agy`, `AGENTS.md`, recommended flags from the issue), mailbox/session support, TS types, verification steps from the issue.

Issue body facts (verbatim requirements): `CodingAgentKind::detect` must recognize `agy`, `agy.exe`, `agy.cmd`, `antigravity`; `PtySubmissionAgent::from_executable` must recognize `agy`/`antigravity`; `as_str()` returns `"agy"`; serde serialization `"antigravity"`; `ANTIGRAVITY_PROFILE` with `resume_tokens: ["--continue"]`, `container_credential: None`, `auto_self_clear_supported: true`; `pty_injection_profile` maps `agy`/`antigravity` to `Established`; mailbox delivery/wakeup/PTY-submission validation must treat `agy` as a supported interactive agent; recommended flags `--dangerously-skip-permissions`, `--model <model>`, `--effort <effort>`; verification steps: agy session launches, `send --mode wake` logs `send_enter=true` and dispatches `\r`, agent executes the prompt, restore/restart injects `--continue`.

## 3. Evidence (measured at 9cba385, not predicted)

- HTML architecture plan: `antigravity-agent-integration-plan-20260821.html` in this architect's replica root (byte-identical to WG14's, SHA-256 `5D0BB7CCFFC894B0BFA83FC1FEA77FFFF1585D2A2A612ED015EAC4DD04DE70DE`). Roadmap steps 0–7; Step 3 = Gemini runtime removal; Step 6 = full verification + cycle gate; Step 7 = optional repo-personal historical filename (out of scope, see §4).
- WG14 evidence reports (read-only, `D:\0_repos\AgentsCommander_iac\.ac\wg-14-dev-v5-team\messaging`): agent-integration-normalization (`20260821-033020-…`), supported-agents-inventory (`20260821-022512-…`), gemini-audit-report runtime (`20260821-004622-…`, 632 literal lines / 91 tracked files), webpage gemini audit (`20260821-003408-…`, 1 stale chip + 4 voice), personal-note audit (`20260821-005929-…`), gemini-product-status (`20260821-002954-…`, Google sources 2026-08-20), antigravity-html-complete (`20260821-173105-…`), full handoff (`20260824-174927-…`).
- Current-code measurements (this plan's own, at 9cba385):
  - `src-tauri/src/session/profile.rs:28-33` — `CodingAgentKind { Claude, Codex, Gemini, Pi }` (`#[serde(rename_all = "snake_case")]`); `detect` at ~764-801 (Pi parser first, then prefix scan Claude > Codex > Gemini); `PtySubmissionAgent` at 619-749 includes `Gemini`; `GEMINI_PROFILE` at 965-971.
  - `src-tauri/src/pty/inject.rs:126-137` — `pty_injection_profile`: `starts_with("claude"|"codex"|"gemini")` → `Established`; `:163-169` `needs_explicit_enter`; `:171-187` logical clear/compact; `:189-202` auto-self-maintenance / self-handoff.
  - `src-tauri/src/commands/session.rs` — `gemini_tokens_have_resume` 307-321, `inject_gemini_resume` 323-379; Gemini context-target arm 1819-1820; Gemini resume gate 2035-2040; `executable_basename` 4386; tests 5024, 8886-8942, 9223.
  - `src-tauri/src/config/sessions_persistence.rs` — `strip_auto_injected_args` 1715-1964 (Gemini stripper 1771-1794, detect match 1800-1808, cmd/direct branches 1828-1930); tests 4016-4064, 4255, 4274.
  - `src-tauri/src/commands/gemini_resolver.rs` (6.7K, `resolve_gemini_home*`, `lookup_chats_dir_for_cwd`, `canonicalize_cwd_for_gemini`); `src-tauri/src/telegram/gemini_watcher.rs` (29.6K, `spawn_watch_task`, `gemini_preamble_extractor`, `extract_gemini_message`, `find_newest_session_jsonl`).
  - `src-tauri/src/commands/telegram.rs:84-97` — `derive_reader` Gemini branch; `src-tauri/src/telegram/bridge.rs:436-454,511-521` — `SessionReaderKind::Gemini` + spawn branch; `:847-855` voice (preserve).
  - `src-tauri/src/config/settings.rs:1712-1757` — `validate_agent_command_text` Gemini ban (`gemini_has_manual_resume` 970-984); `:63` filename doc comment; tests 5773-5783.
  - `src-tauri/src/config/session_context.rs:177-196` — `ManagedContextTarget::Gemini` → `GEMINI.md`, `MANAGED_CONTEXT_FILENAMES` includes `GEMINI.md`; `src-tauri/src/config/agent_command.rs:183` — `Gemini → "GEMINI.md"` (tests 1638, 1654, 1706, 1746, 1768); `src-tauri/src/config/config_seed.rs:232-233` — `Gemini | Pi => return None` (tests 2261-2264).
  - `src/shared/types.ts:120` — `export type CodingAgentKind = "claude" | "codex" | "gemini" | "pi"`; `src/shared/agent-presets.ts` — six-row `FALLBACK_CODING_AGENTS`, generic `definitionToSeed` (86-101); `src/shared/profile-utils.ts:331` (`GEMINI_HOME` in `MANAGED_HOME_ENV_KEYS`), `:405` (gemini → `GEMINI.md` branch); `src/sidebar/components/root-agent-action.ts:13` comment; tests: `agent-presets.test.ts:41-42` (negative no-gemini guard — KEEP), `profile-utils.test.ts:275-277,284-285,488,495-496`, `root-agent-action.test.ts:58,61`.
  - Catalog: `src-tauri/resources/coding-agents/agents.default.json` (six rows, no Gemini); `coding_agents_catalog.rs:986-1041` `EXPECTED_PRESETS` [6], `:1052-1067` six-in-order test, `:1070-1103` drift guard.
  - Mailbox: `phone/mailbox.rs` help texts 823, 9220, 9509, 11477 ("Claude / Codex / Gemini / Cursor"); runtime uses `needs_explicit_enter` (6925, 7094, 7149, 7187, 7237), `detect_pty_submission_agent` (2119-2132), `CodingAgentKind::detect` hint (5497); tests 11485, 20501, 20572, 20860.
  - Misc wiring: `commands/mod.rs:6` exports `gemini_resolver`; `telegram/mod.rs:5` exports `gemini_watcher`; `cli/self_clear.rs:26,44,78` and `cli/send.rs:67` help texts; `cli/agency_templates.rs:748` (`GEMINI.md` in managed-file reject set); `commands/ac_discovery.rs:1532-1536` (`**/__agent_*/GEMINI.md` ignore tuple); `.gitignore:22` (`GEMINI.md`); `pty/container_credentials.rs:567` comment; `session/session.rs:93,137` comments; `config/coordinator_clocks.rs:61` comment; `telegram/jsonl_kernel.rs:20,29,141` comments; `pty/spawn_diagnostics.rs:1798-1804` test; `session/manager.rs:4424` test; `tests/claude_watcher_layering.rs:4,2564-2594` guard; `src-tauri/module-arcs.txt:403,497,955,966-969` (7 Gemini arcs).
  - Stale Gemini-CLI docs/residue (exact lines from the WG14 audit, re-verified at 9cba385): `README.md:59,94,100,117`; `npm/README.md:5,30`; `.github/ISSUE_TEMPLATE/bug_report.yml:86`; `docs/` — agent-matrix-conventions.md:34; agents/creating-agents.md:13,32; agents/inter-agent-messaging.md:140; assets/capture-guide.md:73,278; brand.md:170; comparison.md:9,25,38,69,86; concepts.md:7,11,15; faq.md:7,11; features/coding-agent-profiles.md:29,35; features/container-coding-agents.md:81; features/telegram-bridge.md:38; glossary.md:11,27; integrations/coding-agents.md:3,13,24,35,45,158,170,184; quickstart.md:9,71; reference/cli.md:124,170,601; reference/settings.md:47-49 (agents sample; :56-57/:294-295 are voice fields — preserve); reference/architecture.md:38,143,~535,549,754 (:558 is the voice Gemini-API participant — preserve); security.md:12,123; style-guide.md:58; testing/02-onboarding-and-coding-agents.md:30,42,51,55,176,180,191,201,205,207,213; testing/semantic-ui-automation-affordance-matrix.md:12; troubleshooting.md:45,54,62,64; use-cases.md:5,35; ROADMAP.md:10,23,47.
  - Webpage repo (`repo-agentscommander_webpage` at `85f318d3…` per WG14 audit): `src/components/alternatives/WorkspaceMock.astro:91` stale "Gemini CLI" chip; four preserved voice mentions (`TrustPlatforms.astro:30,32`, `Capabilities.astro:81`, `Workflows.astro:38`).

## 4. Scope

### In scope (this branch, one reviewable change set)

1. Antigravity first-class runtime (Rust + TS + catalog + docs) per §2 item 3.
2. Legacy Gemini coding-agent runtime removal per §2 item 2 (including delete of `gemini_resolver.rs` / `gemini_watcher.rs` and all wiring listed in §5.11).
3. Stale Gemini-CLI-as-supported residue cleanup per §2 item 1, exactly the files listed in §5.13–§5.14, with the preserve sets in §8.2.
4. Regenerated `src-tauri/module-arcs.txt` (7 Gemini arcs disappear).

### Out of scope (binding on the implementers)

- **No** HTML-plan roadmap Step 4 (deepen `session/profile.rs` into the pure apply/strip/resolve seam; move `mangle_cwd_for_claude`; point `sessions_persistence`/`telegram` at the new seam; delete `commands/codex_resolver.rs`). `codex_resolver.rs`, inline Claude resolution, `sessions_persistence -> commands::session::executable_basename`, and the Claude/Pi/Codex resume helpers stay exactly as they are today.
- **No** HTML-plan Step 5 (deep-interface test consolidation).
- **No** HTML-plan Step 7 (the zero-byte `repo-personal/Ideas-DONE/Quitar Gemini CLI.md` stays untouched).
- No new Antigravity transcript watcher/resolver/credentials — Antigravity uses the generic PTY Telegram path (`derive_reader` → `Ok(None)`, same as Pi). No `AGENTS.md`-adjacent provider directories.
- No changes to `dependency-cruiser.config.mjs`, `Cargo.toml`, `package.json` dependencies, IPC command surface, event shapes, or configuration keys.
- No Gemini **voice** removal: `geminiApiKey`/`geminiModel` settings, `voice.rs`, `telegram/output.rs` voice paths, `telegram/redact.rs`, `logging.rs` Gemini-key redaction, `network/mod.rs`, `pty/container_runtime.rs` env redaction, `src/shared/stores/settings.ts` voice, `SettingsModal.tsx` voice UI, voice docs (voice-to-text.md, integrations/voice.md, integrations/telegram.md:56, settings.md:294-295, comparison.md:15, faq.md:55, glossary.md:155, quickstart voice, ROADMAP.md:14, troubleshooting.md:141,145, PRIVACY.md) are all preserved verbatim.
- No global `Gemini` → `Antigravity` string replacement anywhere (product-status evidence: Gemini app/model family and enterprise Gemini CLI remain active).

## 5. Decided solution (exact symbols)

### 5.1 Rust runtime identity — `src-tauri/src/session/profile.rs`

1. Enum: add variant `Antigravity` to `CodingAgentKind` (existing `#[serde(rename_all = "snake_case")]` yields wire value `"antigravity"`); remove variant `Gemini`. Update the module doc header (drop "Gemini CLI" from the opening list).
2. `ANTIGRAVITY_PROFILE` (const, beside `PI_PROFILE`):

```rust
const ANTIGRAVITY_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Antigravity,
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["--continue"],
    container_credential: None,
    auto_self_clear_supported: true,
};
```

Delete `GEMINI_PROFILE` and its `CodingAgentProfile` doc-bullet.
3. `CodingAgentKind::detect`: Pi pass unchanged. Legacy scan becomes **Claude > Codex > Antigravity** (prefix matching retained for claude/codex only; Antigravity matches exact stems `agy` | `antigravity` on the already-stemmed lowercase basename, so `agy.exe`/`agy.cmd`/`agy.bat`/`antigravity.exe` collapse to `agy`/`antigravity` via `file_stem`). Update the function doc comment (remove Gemini, document exact-stem rule). Return `None` for `gemini*` (a user-defined `gemini` command now gets generic behavior).
4. `as_str()`: `CodingAgentKind::Antigravity => "agy"`; delete the Gemini arm.
5. `profile()`: add the Antigravity arm; delete the Gemini arm.
6. `PtySubmissionAgent`: remove variant `Gemini`; add variant `Antigravity`. `from_executable`: `stem == "agy" || stem == "antigravity"` → `Some(Self::Antigravity)`; delete the gemini arm (exact stems only — no `configured_wrapper` prefix allowance for agy). `agrees_with_hint`: replace the Gemini arm with `(Self::Antigravity, Some(CodingAgentKind::Antigravity))`.
7. Tests (same module): `detect_codex_and_gemini_direct` → rework to Codex + Antigravity (add `detect("agy", &[])` and `detect("antigravity", &[])` positive cases, `agy.exe`/`agy.cmd` cases); `pi_serde_and_profile_contract_are_stable` — assert `serde_json::to_string(&CodingAgentKind::Antigravity) == "\"antigravity\""`, `as_str() == "agy"`, profile fields (resume_tokens `["--continue"]`, `container_credential.is_none()`, `auto_self_clear_supported` true), and drop Gemini from the true-assert list; `idle_profiles_keep_existing_defaults` — kind list becomes Claude/Codex/Antigravity/Pi; every `CodingAgentKind::Gemini` literal in tests is replaced (Gemini-as-model-VALUE tokens such as `"--model", "gemini-pro"` in Pi tests may stay as inert values — they no longer detect a kind — but every expectation `Some(CodingAgentKind::Gemini)` is rewritten to the new expected outcome for that exact command shape, per §9.1).

### 5.2 PTY injection — `src-tauri/src/pty/inject.rs`

`pty_injection_profile` (126-137):

```rust
if stem.starts_with("claude")
    || stem.starts_with("codex")
    || matches!(stem.as_str(), "agy" | "antigravity")
{
    PtyInjectionProfile::Established
} else if stem == "agent" {
    PtyInjectionProfile::Cursor
} else if stem == "pi" {
    PtyInjectionProfile::Pi
} else {
    PtyInjectionProfile::Unsupported
}
```

Consequences (no other function body changes): `needs_explicit_enter("agy")`/`("antigravity")` → true (fixes `send_enter=false` at inject.rs:338); `resolve_logical_command_text` → `/clear` and `/compact` for agy (Established arm); `supports_auto_self_maintenance("agy")` → true; `supports_self_handoff_switch("agy")` → true. Update the doc comment at ~237 ("Direct Claude, Codex, Antigravity, Cursor agent, and exact-stem Pi shells…"). Tests: `agent_clis_require_explicit_enter` list — remove `"gemini.cmd"`, add `"agy"`, `"agy.exe"`, `"C:\\tools\\agy.cmd"`, `"antigravity"`; the positive list at ~803 — replace `"gemini-proxy.exe"` with `"agy-proxy.exe"` (prefix wrappers keep Established via the claude/codex-style prefix rule? NO — agy is exact-stem only; `agy-proxy` is NOT Established; use a still-Established wrapper like `"codex-proxy.exe"` instead) — see §9.1 for the exact case list.

### 5.3 Resume apply — `src-tauri/src/commands/session.rs`

1. Delete `gemini_tokens_have_resume` (307-321) and `inject_gemini_resume` (323-379). Add:

```rust
/// Antigravity resume markers: `--continue`/`-c` (most recent conversation)
/// and `--conversation <ID>` / `--conversation=<ID>` (by conversation ID).
fn antigravity_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    tokens[start..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c" || lower == "--conversation"
            || lower.starts_with("--conversation=")
    })
}

fn inject_antigravity_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260/#1482 — resume token sourced from the CodingAgentProfile.
    let &[resume_token] = CodingAgentKind::Antigravity.profile().resume_tokens else {
        debug_assert!(false, "Antigravity resume_tokens must have exactly 1 element");
        return false;
    };
    match executable_basename(shell).as_str() {
        "agy" | "antigravity" => {
            let tokens: Vec<&str> = shell_args.iter().map(String::as_str).collect();
            if antigravity_tokens_have_resume(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, resume_token.to_string());
            true
        }
        "cmd" => {
            // Tokenized and embedded forms, structurally mirroring the deleted
            // inject_gemini_resume cmd arm, but searching for a token whose
            // executable basename is exactly "agy" | "antigravity".
            // (implementer: port the two inner loops verbatim, swapping the
            // executable predicate and the 1-element token slice)
        }
        _ => false,
    }
}
```

2. In `create_session_inner_impl`, replace the Gemini block (2035-2040) with:

```rust
if agent_kind == Some(CodingAgentKind::Antigravity) && !skip_auto_resume {
    if let Some(ref aid) = agent_id {
        if inject_antigravity_resume(&shell, &mut shell_args) {
            log::info!("Auto-injected `agy --continue` for agent '{}'", aid);
        }
    }
}
```

3. Context-target arm (1819-1820): `Some(CodingAgentKind::Gemini) => …Gemini` → `Some(CodingAgentKind::Antigravity) => Some(crate::config::session_context::ManagedContextTarget::Antigravity)`.
4. Comments at 1200, 1546, 3697: remove Gemini from the resume-flag lists (replace with Antigravity where the list enumerates providers).
5. Tests: 5024 shell list `["claude", "codex-wrapper", "gemini.exe"]` → `["claude", "codex-wrapper", "agy"]`; delete the `inject_gemini_resume_*` tests (8886-8942) and add `inject_antigravity_resume_prefixes_direct_agy_args`, `inject_antigravity_resume_inserts_into_cmd_tokenized_wrapper`, `inject_antigravity_resume_inserts_into_embedded_cmd_string`, `inject_antigravity_resume_skips_when_continue_or_conversation_present` (cover `--continue`, `-c`, `--conversation X`, `--conversation=X`); the ~9223-area edit is **comment-only**: `effective_restart_skip_auto_resume_respects_explicit_false` carries a comment listing `gemini --resume latest` — drop Gemini from that comment; the test logic is generic and unchanged (F2).

### 5.4 Resume strip — `src-tauri/src/config/sessions_persistence.rs`

In `strip_auto_injected_args` (1715-1964): delete `strip_gemini_tokens` (1771-1794) and all `is_gemini`/Gemini branches (detect match 1800-1808, cmd scans 1828-1833/1864-1869/1878-1883, direct scans 1915/1927). Add `strip_antigravity_tokens` (single-token loop removing `--continue` only — never `-c`/`--conversation`, which are user-authored) and `is_antigravity` handling mirroring the existing Claude single-token structure: detect match gains `Some(CodingAgentKind::Antigravity) => (false, false, true)`-style flag; cmd tokenized + embedded scans locate the executable token by basename `agy` | `antigravity` and strip after it; direct-exec scan drops `--continue` tokens. Update the function doc comment (1706) and the `is_cmd` position scans. Tests 4016-4064, 4255, 4274: replace Gemini fixtures with Antigravity (`agy --continue`, cmd `/C agy --continue`, embedded `cmd /K agy --continue`, `--conversation`/`-c` preservation cases, round-trip strip(apply(x)) == x for injected `--continue`).

### 5.5 Command validation — `src-tauri/src/config/settings.rs`

Delete `gemini_has_manual_resume` (970-984) and the Gemini ban block (1744-1751). Add, mirroring the Claude block:

```rust
fn antigravity_has_manual_resume(tokens: &[&str], antigravity_idx: usize) -> bool {
    tokens[antigravity_idx + 1..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c"
    })
}
```

and in `validate_agent_command_text`, after the Codex block:

```rust
if let Some(antigravity_idx) = find_provider_token(&tokens, "agy")
    .or_else(|| find_provider_token(&tokens, "antigravity"))
{
    if antigravity_has_manual_resume(&tokens, antigravity_idx) {
        return Err(format!(
            "{context}: Antigravity commands must not include --continue or -c; AgentsCommander injects agy --continue automatically"
        ));
    }
}
```

Also fix the stale comment at `settings.rs:1681` — the composed-effective-command validation note "banned provider flag (Claude --continue/-c or Codex/Gemini manual resume)" becomes "…or Codex manual resume / Antigravity --continue/-c" (G5).

`--conversation <ID>` stays ALLOWED (user-authored resume-by-ID, analog of Claude `--resume <id>`; the injector skip logic honors it). Update doc comment at 63 ("Claude -> CLAUDE.md, Gemini -> GEMINI.md, Codex/Pi/else -> AGENTS.md" → "Claude -> CLAUDE.md, Codex/Pi/Antigravity/else -> AGENTS.md"). Tests 5773-5783: `validate_agent_commands_allows_plain_gemini` → `…_allows_plain_antigravity` (`agy`); `validate_agent_commands_rejects_gemini_resume_latest` → `…_rejects_antigravity_continue` (`agy --continue` and `agy -c` rejected, message contains "must not include --continue or -c"); add `agy --conversation abc` accepted.

### 5.6 Context target + instructions filename

- `src-tauri/src/config/session_context.rs`: `ManagedContextTarget` — remove `Gemini`, add `Antigravity`; `filename()`: `Self::Antigravity => "AGENTS.md"`; `MANAGED_CONTEXT_FILENAMES` (175-176) becomes `&["last_ac_context.md", "CLAUDE.md", "AGENTS.md"]`. Tests at 5650, 7541: replace Gemini with Antigravity (assert `("antigravity"-kind target, "AGENTS.md")`).
- `src-tauri/src/config/agent_command.rs:183`: `Some(CodingAgentKind::Gemini) => "GEMINI.md"` → `Some(CodingAgentKind::Antigravity) => "AGENTS.md"` (merge into the Codex/Pi arm). Two stale doc comments in the same file are corrected (G3, G4): `:280` — "the user-facing built-ins CLAUDE.md / GEMINI.md / AGENTS.md stay allowed" → drop GEMINI.md; `:577` — "the closed enum covers Claude, Codex, Gemini, and Pi" → "…Claude, Codex, Antigravity, and Pi". Tests 1638 (`default_instructions_filename_for_command("gemini -m gpt-5")` → `"agy"` → `"AGENTS.md"`), 1654 (pi --provider gemini value stays Pi), 1706 (`agent("gemini","gemini")` fixture → `agent("agy","agy")` expecting `AGENTS.md`), 1746 (`resolve_target_filename(…, Some(ManagedContextTarget::Gemini))` → `…Antigravity` expecting AGENTS.md), 1768 (any remaining Gemini fixture).

### 5.7 Config seed — `src-tauri/src/config/config_seed.rs`

`compute_config_dir_warning` (232-233): `Some(CodingAgentKind::Gemini | CodingAgentKind::Pi) => return None` → `Some(CodingAgentKind::Antigravity | CodingAgentKind::Pi) => return None` (Antigravity has no AC-managed config-dir env). Tests 2261-2264: the `("gemini", …)` case → `("agy", …)` expecting `None`.

### 5.8 Catalog + presets (7 rows)

- `src-tauri/resources/coding-agents/agents.default.json`: append after `opencode` (six existing rows keep their relative order):

```json
{
  "key": "antigravity",
  "label": "Antigravity",
  "description": "Coding Agent by Google",
  "color": "#4285F4",
  "command": "agy",
  "instructionsFilename": "AGENTS.md",
  "envs": [],
  "isolatedHome": false,
  "removable": true,
  "updateCommands": []
}
```

No `configSeed` (like hermes/pi/cursor). No update command (no verified upstream command; §10 D3).
- `src-tauri/src/config/coding_agents_catalog.rs`: `EXPECTED_PRESETS` (986-1041) → `[(&str; 7)]` with tuple `("antigravity", "Antigravity", "Coding Agent by Google", "#4285F4", "agy", "AGENTS.md", None)`; rename `embedded_default_parses_with_six_agents_in_order` → `…_seven_agents_in_order`, expected keys `["claude", "codex", "hermes", "cursor", "pi", "opencode", "antigravity"]`; `embedded_default_matches_current_presets_exactly` (1070-1103) passes automatically once `EXPECTED_PRESETS` grows; `every_embedded_entry_validates` stays (validates the new row via `validate_definition` — `agy` passes the safe-command validation).
- `src/shared/agent-presets.ts`: append the matching row to `FALLBACK_CODING_AGENTS` (with `updateCommands: []`, `autoUpdate: false`). `definitionToSeed` untouched (generic).
- `src/shared/agent-presets.test.ts`: `EXPECTED_BUILTINS` gains the antigravity entry; keep the "no gemini" negative guard (41-42) verbatim; add an assertion that the antigravity row seeds `{ command: "agy", instructionsFilename: "AGENTS.md" }` via `definitionToSeed`.
- Decisions: recommended flags (`--dangerously-skip-permissions`, `--model <model>`, `--effort <effort>`) are documented in `docs/integrations/coding-agents.md` (§5.13), NOT baked into the seeded `command` (§10 D2). Label/description/color rationale in §10 D1.

### 5.9 Telegram — generic PTY path for Antigravity, Gemini reader removal

- `src-tauri/src/commands/telegram.rs` `derive_reader` (35-104): delete the Gemini arm (84-97); add `Some(CodingAgentKind::Antigravity) => Ok(None)` beside Pi. `resolved_claude_projects_dir`/`effective_codex_home` plumbing unchanged. Fix the function doc at `:23-24` — "the recognized provider has no JSONL reader (Pi)" → "…(Pi, Antigravity)" (G7).
- `src-tauri/src/telegram/bridge.rs`: delete `SessionReaderKind::Gemini` (446-450), the spawn branch (511-521) and its `tasks.push(super::gemini_watcher::spawn_watch_task(…))`. Voice block at 847-855 untouched.
- `src-tauri/src/telegram/mod.rs:5`: remove `pub mod gemini_watcher;`.
- `src-tauri/src/telegram/jsonl_kernel.rs:3,20,29,141,287`: comments no longer mention Gemini — `:3` "reused by claude_watcher, codex_watcher, and gemini_watcher" → the two real watchers; `:20`/`:29` ("Used by the Claude watcher; Codex has its own per-poll discovery"); `:141` drop the Gemini dedupe note; `:287` test doc "backends that dedupe by id are covered in gemini_watcher tests" → drop (G6).
- `src-tauri/tests/claude_watcher_layering.rs`: line 4 comment → "below its three consumers (`telegram::bridge` and the Claude and Codex watchers)"; `production_codex_and_gemini_reach_output_without_bridge` (2564) → rename `production_claude_and_codex_reach_output_without_bridge`, module list `["agentscommander_lib::telegram::claude_watcher", "agentscommander_lib::telegram::codex_watcher"]` — both real adapters stay directly bound to `telegram::output`, never through bridge.

### 5.10 Mailbox + privileged submission (agy = supported interactive agent)

With §5.1 + §5.2 in place, no mailbox **logic** changes are required for Antigravity: `CodingAgentKind::detect("agy", …)` → `Some(Antigravity)` (session `agent_kind` stamped), `detect_pty_submission_agent("agy", …)` → `Some(PtySubmissionAgent::Antigravity)` (provenance passes, mailbox.rs:2119-2132), `needs_explicit_enter("agy")` → true (mailbox 6925/7094/7149/7187/7237 → wake `send_enter=true` + `\r`), logical clear/compact → `/clear`/`/compact` (mailbox 821/9209), self-handoff switch → supported (9503). Required edits: help/error texts 823, 9220, 9509, 11477 — "Claude / Codex / Gemini / Cursor" → "Claude / Codex / Antigravity / Cursor"; comment 8207 (drop Gemini transcript mention); comment 15271 (resume list) and tests 11485 (`"gemini.cmd"` → `"agy.cmd"`), 20501, 20572/20860 (error strings). The 20501 fixture `remote_established_command_branches_preserve_text_and_submission` replaces `assert_wired_clear_and_compact_submission("gemini.exe", "gemini-wrapper.cmd")` with **`assert_wired_clear_and_compact_submission("agy.exe", "antigravity.exe")`** (F1): both legs of the fixture run clear AND compact on both shells, so BOTH must be Established — `agy.exe` (stem `agy`) and `antigravity.exe` (stem `antigravity`) are exact-stem Established; `agy-wrapper.cmd` (stem `agy-wrapper`) would be `Unsupported` and panic at mailbox.rs:20450-20451. (This mirrors the exact-stem-only rule already applied to the inject.rs:803 positive list in §5.2.)

### 5.11 Gemini runtime removal — deletions and wiring

Delete files: `src-tauri/src/commands/gemini_resolver.rs`, `src-tauri/src/telegram/gemini_watcher.rs`.

Wiring removals: `commands/mod.rs:6` (`pub mod gemini_resolver;`); `telegram/mod.rs` (above); `.gitignore:22` (`GEMINI.md` line); `cli/agency_templates.rs:748` — `matches!(file_name.as_str(), "CLAUDE.md" | "AGENTS.md" | "GEMINI.md")` → drop `GEMINI.md`; `commands/ac_discovery.rs:1532-1536` — drop the `**/__agent_*/GEMINI.md` tuple; `pty/container_credentials.rs:567` comment "(Codex, Gemini)" → "(Codex, Antigravity)"; `session/session.rs:93,137` comments (drop `gemini --resume latest`); `config/coordinator_clocks.rs:61` comment (resume list: "Claude/Pi --continue / Codex resume --last / Antigravity --continue"); `cli/self_clear.rs:26,44,78` and `cli/send.rs:67` help texts ("Claude/Codex/Gemini-family" → "Claude/Codex/Antigravity-family"); `pty/spawn_diagnostics.rs:1798-1804` test (`Some(CodingAgentKind::Gemini)` → `Some(CodingAgentKind::Antigravity)`); `session/manager.rs:4424` test list (Gemini → Antigravity); `src-tauri/module-arcs.txt` — regenerate (the 7 arcs at 403, 497, 955, 966-969 disappear; commit the regenerated file).

`settings.rs:63` doc comment (see §5.5). `profile.rs` module doc (see §5.1). `commands/session.rs` comments (see §5.3).

### 5.12 Frontend TypeScript

- `src/shared/types.ts:120`: `export type CodingAgentKind = "claude" | "codex" | "pi" | "antigravity";`. Voice fields 616-617 untouched.
- `src/shared/profile-utils.ts:331`: remove `"GEMINI_HOME"` from `MANAGED_HOME_ENV_KEYS`. `:405`: delete the gemini branch from `defaultInstructionsFilename` (`agy`/`antigravity` fall through to the final `return "AGENTS.md"`).
- `src/sidebar/components/root-agent-action.ts:13` comment: "(`claude --continue`, `codex resume --last`, `antigravity --continue`)".
- Tests: `profile-utils.test.ts:275-277` → `defaultInstructionsFilename("agy")` / `("antigravity")` / `("agy --yolo")` → `"AGENTS.md"`; `:284-285` comment (drop "gemini" from the precedence note; the `"codex --base gemini"` case keeps asserting Codex — value tokens do not detect); `:488` keep (Codex regex wins; value inert); `:495-496` → `suggestedContextRegex("agy")` / `("antigravity")` → `null`. `root-agent-action.test.ts:58,61`: agentId `"gemini"` → `"antigravity"` (arbitrary ID, keeps the restart semantics).

### 5.13 Docs / README / npm README / issue template — exact per-file treatment

Replace Gemini with **Antigravity** in every stale coding-agent-support context below; preserve voice/historical/negative-guard mentions per §8.2. `docs/integrations/coding-agents.md` is the flagship: intro list; Supported table — replace the Gemini row with `| **Antigravity** | \`agy\` | \`--continue\` | Google's agent-first coding CLI. |`; §"How AC identifies a tuned integration" item 2 — "precedence Claude > Codex > Antigravity" + note "Antigravity matches the exact executable stems `agy` / `agy.exe` / `agy.cmd` / `antigravity` (prefix wrappers are not inferred); Gemini no longer has tuned identity"; logical-clear paragraph — "Direct Claude/Codex/Antigravity-family shells"; Installing the CLIs — replace the Gemini bullet with `- **Antigravity:** [antigravity.google](https://antigravity.google) (Antigravity CLI docs)` and add the recommended-flags note (§10 D2); role-template-picker paragraph — "(CLAUDE.md or AGENTS.md)"; "Adding a custom coding agent" — "Claude and Codex retain their legacy prefix-wrapper behavior; Antigravity matches exact stems"; Authentication/CLI-state table — remove the Gemini row, do NOT add an Antigravity row (AC never reads Antigravity host state; its location is not evidenced — no fabricated facts).

| File | Exact change |
|---|---|
| `README.md:59,94,100,117` | Agent lists: `Gemini` → `Antigravity` (e.g. "Claude Code, Codex, Pi, OpenCode, or Antigravity"). |
| `npm/README.md:5,30` | `Gemini` → `Antigravity` in supported-CLI lists. |
| `.github/ISSUE_TEMPLATE/bug_report.yml:86` | Option `Gemini` → `Antigravity`. |
| `docs/quickstart.md:9,71` | Lists: `Gemini` → `Antigravity`. |
| `docs/comparison.md:9,25,38,69,86` | Lists: `Gemini` → `Antigravity` (:15 voice — preserve). |
| `docs/faq.md:7,11` | Lists → Antigravity (:55 voice — preserve). |
| `docs/concepts.md:7,11,15` | Lists and `GEMINI.md` concept text → Antigravity / AGENTS.md. |
| `docs/glossary.md:11,27` | :11 role-file parenthetical "(`CLAUDE.md`, `AGENTS.md`, or `GEMINI.md`)" → "(`CLAUDE.md` or `AGENTS.md`)" (G1); :27 coding-agent definition list → Antigravity (:155 voice — preserve). |
| `docs/brand.md:170` | "Claude Code, Codex, Gemini, and Pi" → "…Codex, Pi, and Antigravity". |
| `docs/style-guide.md:58` | Same list change. |
| `docs/security.md:12` | Threat-model agent list → Antigravity (:13,:123 — `:13` is voice-API network disclosure (preserve); `:123` "AC copies no Codex, Gemini, or Pi credential" → "…Codex, Antigravity, or Pi"). |
| `docs/use-cases.md:5,35` | List → Antigravity; tree example `__agent_reviewer-gemini/` → `__agent_reviewer-antigravity/` (:77 voice — preserve). |
| `docs/troubleshooting.md:45,54,62,64` | `where.exe gemini`/`command -v gemini` → `agy`; detector paragraph: "Claude and Codex use the legacy prefix detector (…claude-mb, codex-foo…); Antigravity matches exact stems agy/antigravity; a later Claude or Codex option value cannot reclassify" (:141,:145 voice — preserve). |
| `docs/features/coding-agent-profiles.md:29,35` | Row list + matrix table row `gemini` → `antigravity`. |
| `docs/features/container-coding-agents.md:81` | "Codex, Gemini, and Pi have no credential descriptor" → "Codex, Antigravity, and Pi". |
| `docs/features/telegram-bridge.md:38` | Remove `| Gemini | JSONL reader |`; table tail: "Anything else" row covers Antigravity/Pi/generic. |
| `docs/agents/creating-agents.md:13,32` | Role-file table: `Gemini | GEMINI.md` → `Antigravity | AGENTS.md`; "(CLAUDE.md, AGENTS.md, or GEMINI.md)" → "(CLAUDE.md or AGENTS.md)". |
| `docs/agents/inter-agent-messaging.md:140` | Table row "Claude, Codex, or Gemini filename stem/prefix" → "Claude, Codex, or Antigravity filename stem". |
| `docs/agent-matrix-conventions.md:34` | Managed files "`CLAUDE.md`, `AGENTS.md`, and `GEMINI.md`" → "`CLAUDE.md` and `AGENTS.md`". |
| `docs/assets/capture-guide.md:73,278` | "…session running Gemini" / "one Codex, one Gemini" → Antigravity. |
| `docs/reference/settings.md:47-49` | Agents-sample JSON: replace the `gemini` agents-array entry (lines 47-49: id/label/command/color) with `{ "id": "antigravity", "label": "Antigravity", "command": "agy", "color": "#4285F4" }`. **Explicit preserve (G8a): `:56-57` (`geminiApiKey`/`geminiModel` in the same sample) and `:294-295` (voice reference table) are voice fields — NOT change sites.** |
| `docs/reference/cli.md:124,170,601` | "Claude/Codex/Gemini-family" → "Claude/Codex/Antigravity-family"; :601 banned list → "Claude `--continue`/`-c`, Codex `resume`/`--last`, and Antigravity `--continue`/`-c`". |
| `docs/reference/architecture.md:38,143,~535,549,754` | Watcher nodes: `:38` "Bridge + Claude/Codex watchers"; `:143` Mermaid `T_WATCH["claude_watcher.rs, codex_watcher.rs,<br/>gemini_watcher.rs"]` → drop `gemini_watcher.rs` (G2); `~535` `AgentFilter (Claude/Codex)`; `:549` prose "Claude, Codex, and Gemini each have a dedicated watcher (`telegram/claude_watcher.rs`, `codex_watcher.rs`, `gemini_watcher.rs`)" → "Claude and Codex each have a dedicated watcher (`telegram/claude_watcher.rs`, `codex_watcher.rs`); Antigravity and Pi use the generic PTY path"; `:754` table row drops `gemini_watcher.rs`. **Explicit preserve (G8b): `:558` is `participant GM as Gemini API` (voice sequence diagram) — NOT a change site.** |
| `docs/testing/02-onboarding-and-coding-agents.md:30,42,51,55,176,180,191,201,205,207,213` | OCA-004 rewording: preset `Gemini CLI` → `Antigravity` (command `agy`); "Do not use a passing Codex run as proof that Cancel, Claude, Antigravity, or Custom Agent works". |
| `docs/testing/semantic-ui-automation-affordance-matrix.md:12` | `onboarding.agentPreset.gemini` → `onboarding.agentPreset.antigravity`. |
| `ROADMAP.md:10,23,47` | Current-capability list "Claude Code · Codex · Gemini · Pi" → "…Codex · Pi · Antigravity"; :23 "alongside Claude Code, Codex, Gemini, and Pi" → Antigravity; :47 "extending beyond Claude to Codex, Gemini, and future agents" → "…Codex, and future agents" (drop Gemini) (:14 voice — preserve). |
| `docs/integrations/rtk_pi/README.md:38` | **No change** — describes rtk's own subprocess-hook processors, not AC's supported-agent claim (§10 D6). |
| `CHANGELOG.md:32,120`, `plans/*`, `docs/testing/06-agent-templates-agency.md:11,199`, `docs/testing/07-terminal-sessions.md:11`, `scripts/smoke-current-app-mockup.mjs:163`, `scripts/smoke-profile-matrix-preview.mjs:57,110-111` | **No change** — historical / negative no-Gemini guards (§8.2). |

### 5.14 Webpage repo — `repo-agentscommander_webpage`

`src/components/alternatives/WorkspaceMock.astro:91`: chip `<span>Gemini CLI</span>` → `<span>Antigravity</span>` (or approved neutral label per WG14 audit; the user-confirmed scope names the Antigravity transition). Preserve `TrustPlatforms.astro:30,32`, `Capabilities.astro:81`, `Workflows.astro:38` (voice) verbatim. No other tracked change (audit-proven exhaustive at `85f318d3…`).

## 6. Affected surfaces, exhaustively

Rust (src-tauri): `session/profile.rs`, `pty/inject.rs`, `commands/session.rs`, `commands/telegram.rs`, `commands/mod.rs`, `commands/gemini_resolver.rs` (del), `commands/ac_discovery.rs`, `config/sessions_persistence.rs`, `config/settings.rs`, `config/agent_command.rs`, `config/config_seed.rs`, `config/session_context.rs`, `config/coordinator_clocks.rs`, `session/session.rs`, `session/manager.rs`, `pty/spawn_diagnostics.rs`, `pty/container_credentials.rs`, `phone/mailbox.rs`, `telegram/bridge.rs`, `telegram/mod.rs`, `telegram/gemini_watcher.rs` (del), `telegram/jsonl_kernel.rs`, `cli/self_clear.rs`, `cli/send.rs`, `cli/agency_templates.rs`, `resources/coding-agents/agents.default.json`, `module-arcs.txt`, `tests/claude_watcher_layering.rs`, `.gitignore`.

TS (src/): `shared/types.ts`, `shared/agent-presets.ts`, `shared/agent-presets.test.ts`, `shared/profile-utils.ts`, `shared/profile-utils.test.ts`, `sidebar/components/root-agent-action.ts`, `sidebar/components/root-agent-action.test.ts`.

Docs/other: `README.md`, `npm/README.md`, `.github/ISSUE_TEMPLATE/bug_report.yml`, the 22 `docs/` files in §5.13, `ROADMAP.md`. Webpage repo: 1 file (§5.14).

Unchanged by design (verify with grep): all voice surfaces (§4 non-goals), `commands/voice.rs`, `network/mod.rs`, `telegram/output.rs`, `telegram/redact.rs`, `logging.rs` key redaction, `pty/container_runtime.rs` env redaction, `src/shared/stores/settings.ts`, `SettingsModal.tsx` voice UI, dependency-cruiser config, `package.json`/`Cargo.toml`/lockfiles, IPC surface.

## 7. Required behavior, edge cases, failure behavior

1. **PTY wake (issue verification steps 1-4)**: session configured with `agy` (catalog preset or custom) → `CodingAgentKind::detect` = Antigravity → `Session.agent_kind` stamped → mailbox wake `send_enter=true` and `\r` dispatched at ~1500/2000 ms → agent executes the prompt; no more stuck `waitingForInput: true`.
2. **Resume (issue verification step 5)**: create/restore/restart of an agy session with resume enabled and no user resume marker injects `--continue` (log `Auto-injected \`agy --continue\``); persisted recipe strips the injected `--continue` so it cannot self-perpetuate; a user-authored `-c` or `--conversation <ID>`/`--conversation=<ID>` survives persistence, suppresses injection, and is honored by agy.
3. **Skip rules**: injection skipped when the command already contains `--continue`, `-c`, `--conversation`, or `--conversation=…` after the executable (case-insensitive). `cmd.exe /C`/`/K` tokenized and embedded single-string forms are supported (mirror of the deleted Gemini injector). Outer `pwsh`/bash evaluator shells get no injection (kind detection still works via basename scan, matching today's legacy behavior).
4. **Generic Telegram path**: `derive_reader` returns `Ok(None)` for Antigravity → bridge `output_task` stabilized-row PTY output + shared input polling; no JSONL watcher, no resolver, no credential copy-in (`container_credential: None`).
5. **Logical commands**: agy/antigravity direct shells map clear/compact → `/clear`/`/compact` (Established); auto-self-maintenance and self-handoff-switch supported; mailbox terminal rejection text updated.
6. **Unknown/generic commands unchanged**: a user-defined `gemini` command now behaves as a plain unknown command (no kind, generic PTY, no resume injection, no GEMINI.md mapping, no watcher). `my-llm`, `pip`, etc. remain `None`.
7. **Catalog**: seven validated rows; `agy` passes `validate_definition`; existing user-owned `agents.json` files are untouched (seed-once; users who want Antigravity add it or re-seed via UI).
8. **Failure behavior**: malformed `agy` commands fail closed exactly as today's detector does (no kind); `inject_antigravity_resume` returns false (no mutation) on unexpected token arity or unsupported shell shapes; validation rejects `agy --continue`/`-c` configs with the §5.5 message before spawn.
9. **Fresh-spawn flag**: `skip_auto_resume` (UI fresh restart / `start_fresh_on_restore`) suppresses Antigravity injection identically to Claude/Codex/Pi.

## 8. Compatibility and security

### 8.1 Compatibility

- **Serialized runtime identity**: `agent_kind` is runtime-only; `PersistedSession` stores `shell`/`shell_args`, and identity is re-derived via `CodingAgentKind::detect` at load (`strip_auto_injected_args`). Removing `"gemini"` from the enum therefore needs **no migration**; old persisted recipes re-derive to `None` (generic) — the correct one-way landing for the removed variant (HTML-plan risk "serialized runtime compatibility" resolved: kind is derived, not persisted).
- Wire union `CodingAgentKind` (TS + serde) loses `"gemini"`, gains `"antigravity"`; same-version IPC only. `activity_log`/`spawn_diagnostics` store the enum — old log rows containing `"gemini"` render as an unknown variant on read (serde enum default) — verify with the round-trip test in §9.1; if the reader is strict, map unknown to `None` in the one log-read path (implementer note: check `activity_log.rs` deserialization; no production reader was found at 9cba385).
- Existing six catalog rows byte-identical; relative order preserved; Antigravity appended last.
- `--session-id` launcher-minted identity (#756) is Claude-only — NOT added for Antigravity (issue does not request it; stripping of it stays Claude-scoped).

### 8.2 Security and privacy (preserve sets)

- **Never** a global `Gemini` → `Antigravity` replacement. Product-status evidence: Gemini app/model family and Gemini CLI (Standard/Enterprise/paid API-key) remain live; only consumer Gemini CLI is stale.
- Preserved verbatim: `geminiApiKey`/`geminiModel` settings + voice transcription (Rust `voice.rs`, `settings.rs` voice sections, `network`, `output.rs`, `redact.rs`, `logging.rs` Gemini-key redaction, `container_runtime.rs` `GEMINI_API_KEY` env redaction, TS voice stores/UI/tooltips), `PRIVACY.md`, voice docs, the 4 webpage voice mentions, historical (`CHANGELOG.md`, `plans/*`), and negative no-Gemini guards (scripts smoke guards, `agent-presets.test.ts:41-42`, testing-safety prose, `.gitignore` negative entries for Gemini are only the removed `GEMINI.md` provider-file ignore — voice/API ignores untouched).
- **No `--dangerously-skip-permissions` in the seeded default command** (§10 D2) — shipping a permission-bypass flag as the default spawn would silently disable Antigravity's own permission prompts for every new user.
- Antigravity adds no credential handling, no transcript reads, no new env injection; `container_credential: None` (container sessions authenticate by the user's own means, like Pi/Codex).
- `agy` is a safe command token; `AGENTS.md` is already in the safe instructions-filename set (`is_safe_instructions_filename`).

## 9. Tests and objective acceptance criteria

### 9.1 Rust tests (targeted)

- `session/profile.rs`: new `detect_antigravity_stems_and_wrapper_exclusions` (agy, agy.exe, agy.cmd, antigravity, `C:\tools\agy.exe`, `cmd /C agy` tokenized; negatives: `agy-proxy`, `my-agy`, `antigravity-pro`, `--model antigravity` as a VALUE for `claude` still detects Claude, `pip`/`pixel` still None, `gemini`/`gemini-bar` now None); `antigravity_serde_profile_contract` (wire `"antigravity"`, `as_str` `"agy"`, profile fields); updated `pi_serde_and_profile_contract_are_stable`, `idle_profiles_keep_existing_defaults`, privileged-detector tests (`detect_pty_submission_agent("agy", &[], Some(Antigravity))` → `Some(PtySubmissionAgent::Antigravity)`; `("antigravity.exe", …)`; `("agyctl", …)` → None; cmd `/C agy` accepted; `cmd /K agy` rejected). All `Some(CodingAgentKind::Gemini)` expectations re-derived per §5.1.7.
- `pty/inject.rs`: positive lists (`needs_explicit_enter` true: agy, agy.exe, agy.cmd, antigravity, codex, claude; `/clear`+`/compact` for agy; auto-self-maintenance + self-handoff true); negative lists (bash, cmd.exe, powershell, agy-proxy, agyctl unchanged).
- `commands/session.rs`: `inject_antigravity_resume_*` per §5.3.5; context-target arm test (`Some(Antigravity)` → `ManagedContextTarget::Antigravity`); update the 5024/9223 fixtures.
- `config/sessions_persistence.rs`: `strip_auto_injected_args` Antigravity cases (direct, tokenized cmd, embedded cmd, `--continue` stripped, `-c`/`--conversation X`/`--conversation=X` preserved, round-trip `strip(apply(cmd)) == cmd`); Gemini strip tests deleted.
- `config/settings.rs`: §5.5 tests.
- `config/agent_command.rs`, `config/config_seed.rs`, `config/session_context.rs`: §5.6/§5.7 tests.
- `config/coding_agents_catalog.rs`: seven-row order + drift tests (§5.8).
- `telegram.rs`/`bridge.rs`: `derive_reader` returns `Ok(None)` for `Some(Antigravity)` (local + container); Gemini reader/watcher spawn code gone (compile-level).
- `session/manager.rs`, `pty/spawn_diagnostics.rs`: kind-list tests updated.
- `tests/claude_watcher_layering.rs`: guard covers claude + codex watchers, direct `telegram::output` dependency, no bridge route (§5.9).
- **Voice preservation**: `cargo test voice` group (commands/voice.rs), settings voice tests, redact/logging/container_runtime redaction tests — unchanged and green.
- Full `cargo test` (src-tauri) green.

### 9.2 TypeScript tests

- `agent-presets.test.ts`: seven built-ins; no-gemini guard intact; antigravity seed shape.
- `profile-utils.test.ts`: agy/antigravity → AGENTS.md; updated precedence cases.
- `root-agent-action.test.ts`: updated IDs.
- `npm run typecheck` (or repo's TS check script) and frontend test suite green; `dependency-cruiser` untouched and green.

### 9.3 Docs / residue acceptance

- Repo-wide case-insensitive `git grep -i gemini` (tracked files): every hit is on the §8.2 preserve list (voice/historical/negative guards) or a §5.13-file correctly rewritten; **zero** hits present Gemini CLI as a currently supported coding agent.
- Webpage repo: `rg -n -i gemini` = exactly the four voice lines (`TrustPlatforms.astro:30,32`, `Capabilities.astro:81`, `Workflows.astro:38`).

### 9.4 Issue verification steps (manual, Windows)

1. Launch an agent session with `agy` (catalog preset).
2. `agentscommander send --to <peer> --mode wake` → log shows `send_enter=true` and `\r` dispatched to the PTY.
3. The agy session starts executing the prompt and produces its response (no stuck `waitingForInput`).
4. Restart/restore the agy session → log `Auto-injected \`agy --continue\``; persisted `shell_args` contain no `--continue`.
5. `agy --conversation <ID>` configured command → no injection, conversation honored, token survives persistence.

### 9.5 Dependency-cycle gate (planning rule 8 — mandatory)

Baseline measured at 9cba385 on a clean tree (this plan's own run): `modulesResolved: 191`, `moduleEdges: 3683`, `moduleCycles: 1` (single cyclic SCC, **85 members**; `gemini_resolver` and `gemini_watcher` are **acyclic nodes outside it** — verified by Tarjan analysis of the emitted graph). The implementer must repeat on the final clean tree:

```
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

(`<VAULT>` = `repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust/` — the detector `01-rust_module-dependency-cycles.mjs` lives one level above `…/rust/Levelization`, verified by both reviewers at this path (F3). Exit 1 is the normal graph-written outcome; exit 3 is failure.)

Green iff ALL:
1. `cyclicSccs` unchanged pre/post (1) and does not increase;
2. every cyclic SCC member set identical set-to-set (the 85 members — the deleted Gemini modules are not members, so no member-set change is possible from this change);
3. zero new `from -> to` pairs cross a previously-clean SCC boundary (this plan adds **zero** new arcs — §11);
4. regenerated `src-tauri/module-arcs.txt` is deterministic and byte-identical to the committed record on the final branch head (the 7 Gemini arcs at lines 403, 497, 955, 966-969 are **expected to disappear**; re-run twice → identical bytes; `git status` empty on the file);
5. structural layering guards green, including the retargeted `claude_watcher_layering` guard (§5.9).

### 9.6 Final acceptance

- All §9.1-§9.5 green; `git status --porcelain` clean except the plan file itself; no unrelated feature/refactor in the diff; the four §8.2 preserve sets verified by review.

## 10. Explicit decisions and accepted residuals

- **D1 — Catalog row facts** (label/description/color): "Antigravity" / "Coding Agent by Google" / `#4285F4` (Google brand blue; the same token was already the Gemini sample color in `docs/reference/settings.md`). The issue pins binary (`agy`) and instructions file (`AGENTS.md`) but not these three; the choices are grounded in the product-status evidence (Antigravity = Google's agent-first platform). Residual: product may re-tune label/color after release — one-line catalog edits, no code.
- **D2 — Recommended flags NOT baked into the seeded command.** The issue's "recommended flags" (`--dangerously-skip-permissions`, `--model <model>`, `--effort <effort>`) are documented in `docs/integrations/coding-agents.md` with a security note (`--dangerously-skip-permissions` disables agy's own permission prompts; `<model>`/`<effort>` are user placeholders). Baking them into `command` would silently disable permission prompts for every new user and cannot be literal (placeholders). Residual: advanced users add flags via coding-agent profile cells.
- **D3 — No update command**: `updateCommands: []` — no verified upstream update command; users update agy themselves. (HTML-plan Step 0 rule: no fabricated packaging facts.)
- **D4 — Exact-stem detection** for agy/antigravity (no `agy-*`/`antigravity-*` prefix wrappers) per the issue's literal list; claude/codex prefix behavior untouched. Residual: `agy --help`/`--version` one-shots still stamp the kind (harmless: injected `--continue` is ignored by the CLI; no one-shot guard — none exists for Claude/Codex either).
- **D5 — Resume grammar**: injected token is always `--continue` (profile `resume_tokens`); `-c` and `--conversation <ID>` are user-authorized forms that suppress injection and survive stripping; `--continue`/`-c` are banned in configured commands (AC-managed), `--conversation` allowed (mirrors Claude `--continue`/`-c` vs `--resume <id>` asymmetry).
- **D6 — `docs/integrations/rtk_pi/README.md:38` preserved**: it documents rtk's own subprocess-hook processors (a third-party package), not an AC supported-agent claim; not stale residue under the user's criterion.
- **D7 — ROADMAP.md:10,23,47 updated** (they assert current/planned AC support — stale under the user's criterion), while CHANGELOG/plans/negative guards stay (true history/guards).
- **D8 — HTML-plan steps 4-5 deferred** (profile-seam deepening, `codex_resolver.rs` deletion): not part of the user-confirmed branch scope; the Gemini removal stands alone without them (verified: `sessions_persistence -> commands::session::executable_basename` and Claude/Codex wiring remain as today, so no intermediate broken state).
- **D9 — Established profile consequences accepted**: mapping agy to `Established` also enables `/clear`+`/compact` logical commands, auto-self-maintenance and self-handoff — the issue's own "e.g. /clear" and `auto_self_clear_supported: true` authorize this; no evidence contradicts slash-command support.
- **D10 — No `--session-id` injection for Antigravity** (#756 is Claude-specific; not requested).
- Residual: HTML-plan step 7 (repo-personal zero-byte historical filename) intentionally untouched.

## 11. Dependency-cycle and layering statement (planning rule 8)

**Enumerated arcs.** New module-to-module import arcs: **ZERO**. Every changed symbol lives in a module that already imports everything it uses — `commands/session.rs` already imports `session::profile` and `config::session_context`; `config::sessions_persistence` already imports `session::profile`; `config::settings`/`agent_command`/`config_seed` already import `session::profile`; `pty::inject`, `session::profile` are self-contained; `commands::telegram`/`telegram::bridge` only delete imports; `phone::mailbox` uses existing `session::profile` imports; TS changes are same-file. Per-arc verdict: no arcs to classify; nothing can join or grow the pre-existing 85-module SCC.

**Removed arcs** (all acyclic-node endpoints): `commands::telegram -> commands::gemini_resolver` (arcs.txt:497), `commands::gemini_resolver -> path_utils` (:403), `telegram::bridge -> telegram::gemini_watcher` (:955), `telegram::gemini_watcher -> commands::gemini_resolver` (:966), `-> network` (:967), `-> jsonl_kernel` (:968), `-> output` (:969). All four endpoints (`gemini_resolver`, `gemini_watcher`, and the shared leaf modules) are outside the cyclic SCC (verified by Tarjan on the 9cba385 graph: `hasGemini false` for the single 85-member SCC), so **no cyclic SCC member set can change**; `jsonl_kernel`/`output`/`network` remain (their non-Gemini consumers stay).

**Measurement**: baseline 191 modules / 3683 edges / 1 cyclic SCC (85 members) at 9cba385; final-tree measurement is acceptance criterion §9.5 (no post tree exists pre-implementation — the criterion, not a claim, is certified).

**Role/layering hygiene**: `session::profile` remains a pure data module (gains one enum variant + const profile; no `tauri`/`AppHandle`/PTY/Telegram dependency); `pty::inject` stays lexical; no lower layer gains a UI transport; Antigravity orchestration is co-located in the existing transport-taking callers (`commands::session`, `config::sessions_persistence`, `config::settings`), exactly where the analogous Claude/Codex/Pi logic already lives. No role inversion.

## 12. Implementation order

1. **ac-dev-rust-v3** (backend, independent): §5.1 profile.rs → §5.2 inject.rs → §5.3 session.rs → §5.4 sessions_persistence.rs → §5.5 settings.rs → §5.6 session_context.rs + agent_command.rs → §5.7 config_seed.rs → §5.8 catalog JSON + coding_agents_catalog.rs → §5.9 telegram + bridge + jsonl_kernel + layering guard → §5.10 mailbox + §5.11 wiring/deletions/`.gitignore`/module-arcs regeneration → §9.1 tests. `cargo test` targeted per commit.
2. **ac-dev-webpage-ui-v3** (frontend, independent via existing types): §5.8 agent-presets.ts + tests, §5.12 types.ts/profile-utils.ts/root-agent-action.ts + tests; §9.2.
3. **Docs** (technical writer or frontend owner): §5.13 + §5.14 (webpage repo commit separate).
4. **Integration**: full targeted suites, voice preservation checks (§9.1/§9.3), manual issue verification (§9.4), dependency-cycle gate (§9.5), force-add and commit this plan (`git add -f plans/1482-support-antigravity-agy.md`), final clean-status check (§9.6).

---

*Authoring note: all line numbers and quoted code refer to `9cba3852f5ec62a97d9edf452c2aa662fe4665f1`. Code sketches in section 5 are normative in behavior and in identifier/argument names; whitespace and comment placement may vary with local style. The webpage-repo and docs changes are exact-file scoped; any additional Gemini-as-supported claim found during implementation is out of scope and must be reported to the coordinator, not silently changed.*
