# Phase #1862 — publish the Muse resume contract and evidence

Class: patterned
Owner: AgentsCommander_iac:room-9-ac-dev-team-v4/ac-technical-writer-v4
Child issue: [#1862 — Docs: publish the Muse beta evidence matrix](https://github.com/mblua/AgentsCommander/issues/1862)
Parent epic: [#1854 — Add a beta Muse Code catalog preset](https://github.com/mblua/AgentsCommander/issues/1854)
Repository: /home/mblua/0_repos/AgentsCommander_iac/.ac/room-9-ac-dev-team-v4/repo-AgentsCommander
Branch: feature/1862-muse-beta-documentation
Branch base: then-current main, recorded when the coordinator creates this branch
Design evidence base: 5c7dd08841a5846f483ba59a0d0dd94ea6410c4d
Depends on: #1861 landed on main and green, transitively after #1860 and #1873
Total phase files: 6

## Objective

Document Muse as a first-class beta runtime kind with a narrowly guarded workspace-latest automatic resume contract. Publish an auditable 14-row matrix that separates unit evidence, the Muse 1.0.3 Linux narrative and its missing raw proof, and live end-to-end evidence. Correct stale updater and supported-agent statements without claiming untested store, platform, authentication, or identity behavior.

This phase changes documentation only. It adds no code, schema, dependency, lockfile, workflow, version, package, or release.

## Exact six-file scope

Only these documentation files may differ from PHASE_BASE_SHA:

1. docs/faq.md
2. docs/features/agent-auto-update.md
3. docs/integrations/coding-agents.md
4. docs/testing/README.md
5. docs/testing/coding-agent-compatibility-muse.md
6. docs/testing/coding-agent-tests-template.md

The five frozen plans and all #1860/#1873/#1861 bytes are tracked base content. Verify their accepted hashes and commits but never edit, stage, or recommit them.

## Pre-mutation evidence gate

1. After consensus, the coordinator freezes the recomputed five-plan digest set, replaces live #1854/#1862 bodies with exact revised epic/phase bytes, keeps #1862 open/linked, and sends an immutable sync carrying all five digests. Extracted epic/phase bodies must compare byte-identical before branching.
2. After #1861 lands green, branch from updated main and record PHASE_BASE_SHA; this is `MATRIX_CODE_SHA`. Never branch from a feature branch.
3. Require exact root, HEAD == PHASE_BASE_SHA, clean state, issue-sync proof, unchanged plan digests, and accepted predecessor deltas: #1860 nine files, #1873 ten files including both wire sides, and #1861 six files.
4. Require the exact system-delivered ac-tech-lead-v4 canonical attestations for #1860, #1873, and #1861. They supply attested candidate/CI SHA, UTC, OS/toolchain, log hashes, test names/counts, runtime argv/probe facts, and frontend lifecycle facts. Producer payloads, private paths, and receipts fail.
5. Fetch origin/main before writing and classify PHASE_BASE_SHA..origin/main. Changes to owned docs, landed Muse behavior/evidence, or applicable workflows require the smallest relevant refresh and tech-lead review.
6. Require nonempty AGENTSCOMMANDER_ROOT; create its `.evidence` parent and use only `mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1862.XXXXXX"`, proving the result stays below that root. Record base hashes for the five existing owned pages and prove only the Muse matrix is absent. After committing, the writer sends only candidate/base/branch as an untrusted validation request and stops.

Wrong root/branch/base, dirty state, missing or inconsistent evidence, plan drift, fabricated fields, or relevant unreviewed drift blocks mutation.

## Settled support statement

Muse Code is the eighth built-in beta preset and a serde-stable CodingAgentKind. AgentsCommander launches configured muse normally for fresh intent. For resume intent it appends resume --last only when the resolved configured launch is an exact direct local-process muse executable, configured argv is empty, and the compiled host is macOS or Linux.

The #1873 product commit lands Rust `CodingAgentKind::Muse` and TypeScript `"muse"` together; `SessionInfo::from` exposes that value as `agentKind`.

The selector means newest retained Muse session for the launch cwd, not exact AgentsCommander-session identity. Absolute direct paths ending in exact muse qualify. Any configured argument, wrapper, ad-hoc launch, recipe mismatch, Windows host, or container suppresses automatic injection.

If `PtyBackend::spawn` returns `Err`, it returns one create diagnostic with no created event and reverts/removes the pending row. With no row to restart, the user fixes the cause and launches/creates again. If spawn succeeds and later exits nonzero, its created row remains and one child-initiated `[pty] child-exit` diagnostic surfaces with the code; only that row offers fresh `Restart Session`. Each timing makes one process attempt and one diagnostic, with no automatic/plain Muse fallback and no AgentsCommander parsing of vendor text or exit codes. Configured and persisted args remain unchanged while only effective spawn args receive AC tokens. Manual selectors, prompts, root/workspace options, and --no-session-log round-trip and suppress injection.

AGENTS.md is the default instruction filename. Idle tuning is default. The built-in Muse preset has no configSeed/factory seed or automatic Muse credential flow, but a user-configured `configSeed` and generic agent/profile environment values still work through provider-neutral paths with no Muse/AgentKind gate. There is no context watcher, Telegram support, logical/privileged PTY command, auto-self-clear, version probe, install/auth/update, or container/Windows resume.

Live Root agent-picker selection is a fresh replacement, not PTY reuse: its omitted flag serializes to null, the backend defaults fresh, tears down the old runtime, and spawns configured plain Muse once. A dormant Root explicitly sends false and is eligible for resume --last; no prior Root uses fresh creation.

The Linux Muse 1.0.3 (1.0.3-R2198.1), 2026-09-08 feasibility message contains parser/no-history narrative, not a qualifying raw successful retained-session transcript. Successful manual continuation claims are retracted. Workspace-latest remains the required selector semantics; vendor continuation and AC live resume remain PENDING. It proves no store path/lifetime, identity, grouping, macOS, container, or authenticated TUI behavior.

Treat reported UUID/picker/root-flag/conflict/parser outcomes as narrative only until their command/stdout/stderr/exit/version/date/OS artifacts are retained. Do not publish manual PASS from narrative or substitute synthetic effective-argv tests for missing vendor proof.

Use these exact upstream links as vendor context:

- https://dev.meta.ai/docs/muse-code.md
- https://dev.meta.ai/docs/muse-code/configuration.md
- https://dev.meta.ai/docs/muse-code/interactive.md
- https://api.meta.ai/muse-launcher.sh

The Meta documentation was login-gated during intake. It does not replace or
upgrade the versioned executable evidence.

## Required documentation edits

### docs/integrations/coding-agents.md

- Include Muse Code in the opening coverage and catalog table.
- The Muse row says beta, binary muse, automatic resume resume --last under the
  exact trusted local macOS/Linux empty-argv guard, and AGENTS.md instructions.
- Add a Muse subsection containing the continuity, precedence, lifecycle,
  one-attempt failure, timing-specific recovery, platform, persistence, and
  unsupported-capability statements above. State the absent built-in seed/
  credential defaults separately from supported configured seed/generic env.
- Explain fresh create/explicit restart/first Root/live Root replacement/
  missing-PTY Root/cold mailbox/cold Loop versus startup restore/dormant reopen/
  dormant Root/known mailbox/known Loop. Only paths that truly reuse a live
  runtime, such as live Loop reuse, spawn nothing.
- Distinguish immediate spawn error/removal/user re-create from successful spawn
  followed by nonzero exit/retained-row fresh Restart Session, including exact
  event/diagnostic semantics, one attempt, and no automatic/plain fallback.
- Link the new compatibility matrix and official Muse documentation. State
  AgentsCommander does not install, authenticate, or update Muse.
- Require exact absence of the current stale sentence
  `` `cursor` intentionally ships no update command (its CLI self-updates with the desktop app). ``
  and exact presence of this replacement:

~~~markdown
`cursor` and `muse` intentionally ship no AgentsCommander update command: Cursor CLI updates with the desktop app, while the Muse launcher owns self-update.
~~~

- Preserve every existing provider statement and command.

### docs/features/agent-auto-update.md

- Replace both Cursor-only exceptions. Require exact substrings `Cursor and Muse ship no AgentsCommander update command, so neither is listed.` and ``the agent is `cursor` or `muse`, which ship none by design``.
- State that the Muse launcher owns self-update while AgentsCommander neither
  configures nor launches a Muse update command.
- Preserve every other update-table, version-check, prompt, and troubleshooting
  contract; do not add Muse to built-in version probes.

### docs/faq.md

- The introductory CLI list contains exact text
  `(Claude Code, Codex, Antigravity, Pi, and Muse Code)`.
- The supported-agent answer contains exact text
  `Claude Code, Codex, Antigravity, Pi, and Muse Code have first-class tuned integrations.`
- Identify Muse as beta and link its coding-agent section/matrix. Summarize only
  guarded launch-cwd workspace-latest `muse resume --last`; do not imply an AC/
  Muse UUID guarantee or upgrade general macOS product support.
- Preserve the OpenCode/Nvidia roadmap statements and every other FAQ answer.

### docs/testing/coding-agent-compatibility-muse.md

Use title Muse Code compatibility test matrix (beta). Identify #1854/#1862,
the workspace-latest boundary, and one authoritative row code version:
`MATRIX_CODE_SHA`, exactly equal to this phase's PHASE_BASE_SHA after #1861 has
landed. Include this exact scope sentence:

This issue-specific 14-row matrix extends the reusable 11-item checklist; it
does not replace it.

Define:

- PASS (unit): successful repository test at an exact commit.
- PASS (manual): successful direct vendor observation with version/date/OS.
- PASS (unit + manual): the composed boundary is explicit; it is not a live
  AgentsCommander TUI claim.
- PASS (live): observed through the built application.
- NOT SUPPORTED IN INITIAL BETA: deliberate exclusion.
- UNKNOWN: no established contract.
- PENDING: specified live evidence has not occurred.

Add official upstream links in a section titled Upstream facts, not
AgentsCommander evidence. Vendor statements alone never upgrade an AC result.

Create exactly these rows and outcomes:

1. Agent identity/detection — PASS (unit): direct and absolute exact muse;
   prefix/argument/wrapper negatives; serde/profile stability.
2. Privileged PTY injection wake — NOT SUPPORTED IN INITIAL BETA.
3. AgentsCommander auto resume — PENDING (vendor/live); PASS (unit) only after
   #1873 attests effective argv and one-attempt/no-fallback failure behavior.
   No qualifying successful vendor transcript exists in checked sources.
4. Resume history visibility — UNKNOWN: no qualifying four-screen/full-visible-
   history observation tied to exact version/date/OS exists.
5. User-authored resume markers — PASS (unit) after configured-argv preservation
   tests; vendor parser behavior remains PENDING without raw probe artifacts.
6. Telegram input/output — NOT SUPPORTED IN INITIAL BETA.
7. Catalog preset — PASS (unit): exact #1860 and #1861 data/tests.
8. Logical commands — NOT SUPPORTED IN INITIAL BETA.
9. Transcript/JSONL watcher — NOT SUPPORTED IN INITIAL BETA.
10. Automatic resume on macOS — PENDING: code is eligible, no live observation.
11. Automatic resume on Linux — PENDING (vendor/live); PASS (unit) only for
    attested effective argv. No successful vendor or authenticated AC smoke proof.
12. Create/restore/restart/reopen/Root/mailbox/Loop lifecycle — PASS (unit):
    name the #1873 backend and #1861 frontend tests for every row, including
    fresh live-Root replacement versus dormant-Root resume.
13. Instructions/config/credentials — AGENTS.md, user-configured `configSeed`,
    and generic agent/profile env PASS (unit) through provider-neutral paths;
    built-in seed/factory, automatic Muse credential flow, and isolated home NOT
    SUPPORTED IN INITIAL BETA.
14. Managed update — NOT SUPPORTED IN INITIAL BETA: empty commands, auto-update
    false, frontend exclusion; launcher self-update remains separate.

Each PASS cell names evidence type; notes carry exact commit, UTC date, OS,
tool/version, tests/counts, and direct lead-attestation path/hash where applicable;
a private log path is diagnostic only.
Every row uses MATRIX_CODE_SHA. PENDING/unsupported cells never invent runtime
date, OS, version, or observation.

### Evidence provenance convention

`MATRIX_CODE_SHA == PHASE_BASE_SHA` is the sole row code version: the landed code snapshot before docs change. `DOC_EVIDENCE_SHA == DOC_CANDIDATE_SHA` is the distinct #1862 docs commit independently checked and used for PR-head CI; it is external attestation provenance and cannot be embedded in its own commit. #1860/#1873/#1861 branch, PR, and merge SHAs appear only inside execution notes, never as alternate row versions. Evidence notes itemize separate executions:

- #1860: catalog commit and Cargo logs; all four product/test paths; catalog
  suite, exact web route, CLI catalog filter, full lib, and both exact
  instruction-resolver tests; exact counts and row values.
- #1873: atomic Rust/TypeScript wire commit and Rust/frontend logs; effective/
  configured/persisted argv and provider-neutral configured seed/env; fresh
  live-Root replacement; separate removed-row spawn-error and retained-row
  successful-spawn/nonzero-exit event, diagnostic, count, recovery, and no-
  fallback proofs; unsupported boundary, Grinch, full/layering, typecheck/build,
  check/clippy/arc results; separate Muse 1.0.3 Linux narrative provenance and missing raw proof; no manual PASS.
- #1861: frontend commit/date/OS/log; all four targeted tests; dependency,
  typecheck/build results; catalog/update and lifecycle intent assertions.

Do not collapse distinct dates/hosts into one field. A missing source, count,
SHA, or observation downgrades the affected result; never fill it from intent.

### docs/testing/README.md

Link coding-agent-compatibility-muse.md as the first-class beta
workspace-latest matrix. Preserve the template/Antigravity links and the rule
that unobserved rows remain pending.

### docs/testing/coding-agent-tests-template.md

- Name Antigravity and Muse as existing examples.
- Use Cursor and Muse as the empty-update examples; state Antigravity ships
  agy update.
- Make resume evidence distinguish configured, effective, and persisted argv;
  fresh versus resume intent; provider/version; workspace selector semantics;
  unsupported transport/platform; spawn error versus post-spawn nonzero exit;
  timing-specific recovery; provider-neutral configured seed/env; one-attempt/
  no-fallback behavior; live Root replacement; and true live reuse.
- Add the Muse matrix to See also.
- Keep the historical Antigravity matrix outside scope.

## Optional live evidence boundary

No interactive Muse test is authorized. Rows 3/10/11 retain PENDING vendor/live
outcomes; unit evidence describes only the tested AC boundary. A future user-
authorized tester may add manual/live PASS only with raw command/stdout/stderr/
exit, exact AC SHA/build, UTC, OS/architecture, Muse version, secret-free auth
precondition, harmless prompt/response, retained-session continuation, no empty
second turn, and clean termination. Missing proof cannot be filled from intent.

## Verification

The producer may run this for feedback. Acceptance requires ac-tech-lead-v4 to
extract and independently run this frozen body (SHA-256
`AF12D972EBBE0933479B65B9E702D3795235212BD692D0482258781AB581FB66`) from a fresh noninteractive shell. It treats the requested SHA only as expected input, requires current branch/commit equality, then exports candidate, symbolic ref, and complete HEAD plus branch checkout-history SHA-256. The lead binds LEAD_CANDIDATE_SHA to the expected DOC_CANDIDATE_SHA before entry and retains LEAD_BRANCH_REF/LEAD_CHECKOUT_SHA256 for CI/delivery guards:

~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
if test -z "${LEAD_CHECKOUT_SHA256:-}"; then LEAD_CANDIDATE_SHA="$(git rev-parse HEAD)" || exit 1; bind_checkout; fi
assert_checkout
command -v rg git sha256sum date uname >/dev/null
: "${AGENTSCOMMANDER_ROOT:?}" "${PHASE_BASE_SHA:?}" "${DOC_CANDIDATE_SHA:?}"
MATRIX_CODE_SHA="$PHASE_BASE_SHA"; DOC_EVIDENCE_SHA="$DOC_CANDIDATE_SHA"
printf '%s\n' "$MATRIX_CODE_SHA" "$DOC_EVIDENCE_SHA" | rg -x '[0-9a-f]{40}' >/dev/null
test "$MATRIX_CODE_SHA" != "$DOC_EVIDENCE_SHA"
mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"
DOC_LOG_DIR="$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1862.XXXXXX")"
case "$DOC_LOG_DIR" in "$AGENTSCOMMANDER_ROOT"/*) ;; *) exit 1;; esac
exec 3>&1 4>&2; exec > >(tee "$DOC_LOG_DIR/verification.log") 2>&1; DOC_TEE_PID=$!
assert_doc_candidate() { assert_checkout; test "$DOC_CANDIDATE_SHA" = "$LEAD_CANDIDATE_SHA" || exit 1; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; }
doc_exit() { local rc=$?; trap - EXIT; assert_doc_candidate || rc=1; exec 1>&3 2>&4; wait "$DOC_TEE_PID" || rc=1; exit "$rc"; }
trap doc_exit EXIT
assert_doc_candidate; git merge-base --is-ancestor "$PHASE_BASE_SHA" "$DOC_CANDIDATE_SHA"

for target in \
  docs/faq.md \
  docs/features/agent-auto-update.md \
  docs/integrations/coding-agents.md \
  docs/testing/coding-agent-compatibility-muse.md \
  docs/testing/README.md \
  docs/testing/coding-agent-tests-template.md
do
  test -f "$target" && test -r "$target"
done

rg -nF -- 'Muse Code' docs/integrations/coding-agents.md
rg -nF -- 'muse resume --last' docs/integrations/coding-agents.md
rg -nF -- 'fresh replacement' docs/integrations/coding-agents.md
rg -nF -- 'post-spawn' docs/integrations/coding-agents.md
rg -nF -- 'user-configured `configSeed`' docs/integrations/coding-agents.md
rg -nF -- 'generic agent/profile environment' docs/integrations/coding-agents.md
rg -nF -- 'coding-agent-compatibility-muse.md' docs/integrations/coding-agents.md
rg -nF -- 'coding-agent-compatibility-muse.md' docs/testing/README.md
rg -nF -- 'coding-agent-compatibility-muse.md' docs/testing/coding-agent-tests-template.md
rg -nF -- '`cursor` and `muse` intentionally ship no AgentsCommander update command: Cursor CLI updates with the desktop app, while the Muse launcher owns self-update.' docs/integrations/coding-agents.md
rg -nF -- 'This issue-specific 14-row matrix extends the reusable 11-item checklist; it does not replace it.' \
  docs/testing/coding-agent-compatibility-muse.md
rg -nF -- 'PENDING (vendor/live)' \
  docs/testing/coding-agent-compatibility-muse.md
rg -nF -- 'Cursor and Muse' docs/testing/coding-agent-tests-template.md
rg -nF -- 'agy update' docs/testing/coding-agent-tests-template.md
rg -nF -- 'Cursor and Muse ship no AgentsCommander update command, so neither is listed.' \
  docs/features/agent-auto-update.md
rg -nF -- 'the agent is `cursor` or `muse`, which ship none by design' \
  docs/features/agent-auto-update.md
rg -nF -- '(Claude Code, Codex, Antigravity, Pi, and Muse Code)' docs/faq.md
rg -nF -- 'Claude Code, Codex, Antigravity, Pi, and Muse Code have first-class tuned integrations.' \
  docs/faq.md
rg -nF -- 'workspace-latest' docs/faq.md
rg -nF -- "$MATRIX_CODE_SHA" docs/testing/coding-agent-compatibility-muse.md
if rg -nF -- "$DOC_EVIDENCE_SHA" docs/testing/coding-agent-compatibility-muse.md; then exit 1; else test "$?" -eq 1; fi
assert_doc_candidate

git ls-files -co --exclude-standard -- '*.md' ':!plans/**' >"$DOC_LOG_DIR/public-markdown.raw" || exit 1
LC_ALL=C sort -u "$DOC_LOG_DIR/public-markdown.raw" >"$DOC_LOG_DIR/public-markdown.list" || exit 1
mapfile -t PUBLIC_MARKDOWN <"$DOC_LOG_DIR/public-markdown.list" || exit 1
test "${#PUBLIC_MARKDOWN[@]}" -gt 6
printf '%s\n' "${PUBLIC_MARKDOWN[@]}" >"$DOC_LOG_DIR/public-markdown.list"
rg -xF 'README.md' "$DOC_LOG_DIR/public-markdown.list"
rg -xF 'docs/faq.md' "$DOC_LOG_DIR/public-markdown.list"
rg -xF 'docs/features/agent-auto-update.md' "$DOC_LOG_DIR/public-markdown.list"

require_public_absent() {
  local needle="$1" rc
  if rg -nF -- "$needle" "${PUBLIC_MARKDOWN[@]}"; then
    printf 'forbidden stale public text present: %s\n' "$needle" >&2
    return 1
  else
    rc=$?
  fi
  test "$rc" -eq 1
}

require_public_absent 'Muse is not a tuned `CodingAgentKind`'
require_public_absent 'no AgentsCommander-injected resume token'
require_public_absent '`cursor` intentionally ships no update command (its CLI self-updates with the desktop app).'
require_public_absent 'Cursor ships none, so it is not listed'
require_public_absent 'or the agent is `cursor`, which ships none by design'
require_public_absent '(Claude Code, Codex, Antigravity, and Pi)'
require_public_absent 'Claude Code, Codex, Antigravity, and Pi have first-class tuned integrations.'
require_public_absent 'update commands (none for agy)'
require_public_absent '(Antigravity), the expected behavior is "no update command configured"'
printf 'repository_stale_text_gate=PASS files=%s needles=9\n' "${#PUBLIC_MARKDOWN[@]}"
assert_doc_candidate

printf '%s\n' docs/faq.md docs/features/agent-auto-update.md \
  docs/integrations/coding-agents.md docs/testing/README.md \
  docs/testing/coding-agent-compatibility-muse.md \
  docs/testing/coding-agent-tests-template.md >"$DOC_LOG_DIR/scope.expected"
git diff --name-only "$PHASE_BASE_SHA" "$DOC_CANDIDATE_SHA" -- >"$DOC_LOG_DIR/scope.actual"
cmp -s "$DOC_LOG_DIR/scope.expected" "$DOC_LOG_DIR/scope.actual"
test "$(git diff --diff-filter=A --name-only "$PHASE_BASE_SHA" "$DOC_CANDIDATE_SHA" --)" = \
  docs/testing/coding-agent-compatibility-muse.md
git diff --check "$PHASE_BASE_SHA" "$DOC_CANDIDATE_SHA" --
git diff --exit-code "$PHASE_BASE_SHA" "$DOC_CANDIDATE_SHA" -- package.json package-lock.json \
  Cargo.lock src-tauri/Cargo.toml src-tauri/module-arcs.txt .github/workflows
date -u +%Y-%m-%dT%H:%M:%SZ >"$DOC_LOG_DIR/evidence-date.log"
uname -srm >"$DOC_LOG_DIR/evidence-os.log"
assert_doc_candidate
printf 'docs_executor=PASS matrix_code_sha=%s doc_candidate_sha=%s scope_files=6 added_files=1 head_drift=none\n' "$MATRIX_CODE_SHA" "$DOC_EVIDENCE_SHA"
~~~

Enumerate and resolve every new relative Markdown link. Re-read the complete
matrix against all 14 outcomes and predecessor evidence. A false PASS,
unresolved link, stale contrary claim, fabricated field, or command failure
blocks acceptance.

The lead independently validates the three predecessor attestations, every positive/negative, inventory/link/14-row audit, and exact six-file/one-new-file delta. Persistent movement and checkout/detach H1→H2→H1 during any child remain visible in the complete HEAD reflog even when the branch never moves; all must fail. A failed logging tee, Git inventory, or sort stage blocks attestation even after output; test both inventory failures with complete-executor positives. After configured-required CI passes at `DOC_EVIDENCE_SHA`, the lead sends `ac.docs-evidence.v1` directly to the reviewer. Exact ordered keys are `schema,attester,issue,branch,phase_base_sha,matrix_code_sha,doc_candidate_sha,doc_evidence_sha,ci_head_sha,date_utc,os,plan_sha256,executor_sha256,execution_log_sha256,predecessor_attestations_sha256,public_markdown_sha256,scope_sha256,link_audit_sha256,matrix_audit_sha256,head_drift,result`; duplicates/extras fail. It repeats the guard before/after CI lookup and delivery. One lead `Queued:` proves enqueue only; acceptance requires the exact system-delivered lead notification path and hash.

The frozen adversarial harness must reject `same-six-clean-after-head-drift`, forged/skipped producer execution with plausible hashes and a real receipt, producer-named or copied/renamed/mutated lead attestations, forged predecessor attestations, candidate/CI mismatch, MATRIX_CODE_SHA not equal to the base, DOC_EVIDENCE_SHA not equal to the candidate, schema omission/reorder/extension, and an absent direct lead notification. Any new candidate restarts all evidence.

## Dependency-cycle and layering gate

Planned new/removed module arcs are zero. Markdown adds no executable relation
or role inversion; require no source/module hunk and a byte-identical arc record.
Any source/import/arc drift is out of this phase and blocks immediately. If the
coordinator later authorizes expanded scope, ac-tech-lead-v4—not the writer—must
run `rust-levelization-run` on clean base/candidate and return cyclicSccs, sorted
SCC member sets, added/removed and cross-boundary pairs, regenerated arc byte
comparison, and layering exits. Missing/dirty/mismatched evidence or exit 3
blocks pending architecture approval.

## Commit, CI, scope, and recovery

Commit only the six owned docs; the matrix is the only new file. Against
PHASE_BASE_SHA require exactly those paths, a clean tree, and unchanged plans,
source, tests, manifests, locks, workflows, generated files, arcs, and version.

On `DOC_EVIDENCE_SHA`, exactly the #1862 PR-head and attested CI-head SHA, require every triggered/configured-required
check: test-debt; Windows Rust check, clippy, and full tests; Linux Rust check,
clippy, and its configured test; macOS Rust check and clippy; rust-fmt; all four
portable terminal legs; Windows release CLI smoke; frontend regression; and
validate-branch-name. The PR lockfile-drift detector must pass and skip
regeneration because package inputs do not change. bundle-validation and
version-sync are path-inapplicable. Re-derive after relevant drift; another
SHA, waiver, bypass, or unexplained skip fails.

Recovery is path-scoped and compare-before-restore. Restore only bytes still
matching recorded phase output; preserve external edits and report conflicts.
Never use broad reset, checkout, restore, or clean.

## Acceptance criteria

1. Integration docs state the exact first-class guarded workspace-latest
   contract, atomic wire ownership, live-Root replacement, removed-row versus
   retained-row failure recovery, provider-neutral configured seed/env, exact
   one-attempt/no-fallback semantics, lifecycle, and unsupported scope.
2. The matrix has exactly 14 evidence-backed outcomes with separate code,
   runtime, and live provenance and no fabricated PASS.
3. All six public pages agree; links resolve and the exhaustive nine-needle
   stale-text gate passes even when a stale literal is outside the original four.
4. Exact six-file/one-new-file scope, zero arcs, clean immutable-candidate lead
   execution, canonical attestation, forgery negatives, and exact-candidate CI pass.

Status: READY_FOR_IMPLEMENTATION
