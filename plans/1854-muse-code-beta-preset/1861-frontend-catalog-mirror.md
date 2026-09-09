# Phase #1861 — mirror Muse in the frontend catalog and pin lifecycle intent
Class: patterned
Owner: AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-webpage-ui-v4
Child issue: [#1861 — Frontend: mirror the Muse catalog preset](https://github.com/mblua/AgentsCommander/issues/1861)
Parent epic: [#1854 — Add a beta Muse Code catalog preset](https://github.com/mblua/AgentsCommander/issues/1854)
Repository: /home/mblua/0_repos/AgentsCommander_iac/.ac/room-9-ac-dev-team-v4/repo-AgentsCommander
Branch: feature/1861-muse-frontend-catalog
Branch base: then-current main, recorded when the coordinator creates this branch
Design evidence base: 5c7dd08841a5846f483ba59a0d0dd94ea6410c4d
Depends on: #1873 landed on main and green, transitively after #1860
Total phase files: 6
## Objective
Mirror the landed Muse catalog row in the frontend fallback and prove existing
ProjectPanel and Root action consumers send the settled fresh/resume intent.
The preceding #1873 phase already landed the Rust serde value and TypeScript
`"muse"` union atomically. No frontend caller invents Muse argv or implements
resume itself.
This phase changes one data constant, one explanatory comment, and four test
files. It changes no type, UI layout, runtime algorithm, backend command, IPC
shape, persistence schema, updater behavior, dependency, or module boundary.
## Exact six-file scope
Only these files may differ from PHASE_BASE_SHA:
1. src/shared/agent-presets.ts
2. src/shared/agent-presets.test.ts
3. src/sidebar/agent-update-status.test.ts
4. src/sidebar/components/ProjectPanel.reopen-resume.test.tsx
5. src/sidebar/components/root-agent-action.ts
6. src/sidebar/components/root-agent-action.test.ts
The five frozen plans, #1860 catalog files, and #1873 runtime files are tracked
base content. Verify their accepted hashes and commits but never edit, stage,
or recommit them.
## Pre-mutation gate
1. After consensus, the coordinator replaces live #1854/#1861 bodies with exact frozen epic/phase bytes, keeps #1861 open and linked, and sends an immutable sync message with both digests. Extracted live bodies must compare byte-identical before branch creation; stale bodies block.
2. After #1873 lands green, branch from updated main and record PHASE_BASE_SHA; never branch from a feature branch.
3. Require exact root, HEAD == base, clean state, issue-sync proof, unchanged plans, and the directly notified tech-lead attestations for exact green #1860/#1873 candidates, including arc and exact-head CI evidence. Producer payloads or receipts do not satisfy this gate.
4. Fetch origin/main; classify drift. Owned-path, Muse-contract, npm/config, dependency-checker, or workflow drift requires focused refresh/review.
5. Require nonempty AGENTSCOMMANDER_ROOT; create its `.evidence` parent and use only `mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1861.XXXXXX"`, proving the result remains below that root. Record six pre-write hashes, OS, and UTC.
6. Provision the Node prerequisite below and run locked npm ci. Producer runs are feedback only. After its product commit the owner sends the candidate SHA/base/branch as an untrusted validation request and stops; no producer summary, hash, private path, or `Queued:` receipt is acceptance evidence.
Wrong root/branch/base, dirty or unknown state, missing predecessor evidence,
plan drift, or relevant unreviewed drift blocks mutation.
### Replica-local Node prerequisite
Host Node 18.19.1/npm 9.2.0 is bootstrap-only and may not run repository validation. Provision Node 22 plus exact npm
11.6.2 below AGENTSCOMMANDER_ROOT. The runner retains combined output and
start/end/elapsed/command-exit/tee-exit metadata for every blocking command:
~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
if test -z "${LEAD_CHECKOUT_SHA256:-}"; then LEAD_CANDIDATE_SHA="$(git rev-parse HEAD)" || exit 1; bind_checkout; fi
assert_checkout
command -v timeout rg npm git cmp sha256sum sort wc >/dev/null
test -n "${AGENTSCOMMANDER_ROOT:-}"
mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"
FRONTEND_LOG_DIR="${FRONTEND_LOG_DIR:-$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1861.XXXXXX")}"
case "$FRONTEND_LOG_DIR" in "$AGENTSCOMMANDER_ROOT"/*) ;; *) exit 1;; esac
assert_frontend_candidate() { assert_checkout; }
run_logged() {
  local label="$1" limit="$2"; shift 2; local start_utc start_s end_utc end_s rc tee_rc
  assert_frontend_candidate
  assert_checkout
  start_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; start_s="$(date +%s)"
  set +e; timeout "$limit" "$@" </dev/null 2>&1 | tee "$FRONTEND_LOG_DIR/$label.log"
  local -a status=("${PIPESTATUS[@]}"); set -e; rc="${status[0]}"; tee_rc="${status[1]}"
  assert_checkout
  end_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; end_s="$(date +%s)"
  { printf 'start_utc=%s\nend_utc=%s\nelapsed_seconds=%s\ncommand_exit=%s\ntee_exit=%s\nargv=' "$start_utc" "$end_utc" "$((end_s-start_s))" "$rc" "$tee_rc"; printf '%q ' "$@"; printf '\n'; } | tee "$FRONTEND_LOG_DIR/$label.meta"
  local -a meta_status=("${PIPESTATUS[@]}"); for value in "${meta_status[@]}" "$rc" "$tee_rc"; do test "$value" -eq 0 || exit 1; done
  test "$rc" -eq 0 || exit 1; test "$tee_rc" -eq 0 || exit 1; assert_frontend_candidate
}
TOOL_ROOT="$AGENTSCOMMANDER_ROOT/.toolchains/ac-1854-node22"
mkdir -p "$TOOL_ROOT"/{cache,tmp,install}
: >"$TOOL_ROOT/npmrc"
BOOTSTRAP_NPM="$(command -v npm)"
NODE_BIN="$TOOL_ROOT/install/node_modules/node/bin"
run_logged node-provision 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" \
  "$BOOTSTRAP_NPM" install --prefix "$TOOL_ROOT/install" --no-save --no-package-lock node@22 npm@11.6.2
NODE22="$NODE_BIN/node"
NPM11="$TOOL_ROOT/install/node_modules/npm/bin/npm-cli.js"
export PATH="$NODE_BIN:$PATH"; hash -r
test "$(command -v node)" = "$NODE22"
run_logged node-direct-version 1m "$NODE22" -p process.versions.node
run_logged node-path-version 1m node -p process.versions.node
cmp -s "$FRONTEND_LOG_DIR/node-direct-version.log" "$FRONTEND_LOG_DIR/node-path-version.log"
rg -x -- '22\.[0-9]+\.[0-9]+' "$FRONTEND_LOG_DIR/node-path-version.log"
run_logged npm-version 1m "$NODE22" "$NPM11" --version
rg -x -- '11\.6\.2' "$FRONTEND_LOG_DIR/npm-version.log"
run_logged npm-ci 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" ci
if test "${FRONTEND_MEASUREMENT_KIND:-candidate}" = base; then test "$LEAD_CANDIDATE_SHA" = "$PHASE_BASE_SHA"; run_logged dependencies-pre 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" run check:frontend-dependencies; fi
~~~
The PATH prefix is active during provisioning and exported before every npm
ci/run child. Direct and PATH-resolved Node must be the same provisioned major
22. Cache/tmp/user config stays below AGENTSCOMMANDER_ROOT; global/HOME writes,
timeout 124, command failure, or tee failure block.
## Catalog contract
Append this exact eighth and last object after Antigravity:
~~~ts
{
  key: "muse",
  label: "Muse Code",
  description: "Meta terminal coding agent (beta; macOS/Linux host only)",
  color: "#0668E1",
  command: "muse",
  envs: [],
  isolatedHome: false,
  removable: true,
  updateCommands: [],
  autoUpdate: false,
}
~~~
instructionsFilename and configSeed remain absent. The exact order is:
~~~text
claude, codex, hermes, cursor, pi, opencode, antigravity, muse
~~~

Verify, but do not edit, the landed `src/shared/types.ts` union containing
`"muse"` and the Rust `CodingAgentKind::Muse` serde value. A mismatch blocks
this phase and returns to #1873; it is never repaired in a catalog commit.

## Lifecycle-consumer contract

The backend alone maps resume intent to eligible effective argv. The frontend
continues to send the existing provider-neutral intent:

| Consumer state | Existing call behavior | Muse runtime result owned by #1873 |
|---|---|---|
| ProjectPanel closed marker requests resume | skipAutoResume: false | muse resume --last when eligible |
| ProjectPanel no closed marker | skipAutoResume: null | fresh configured muse |
| Root has no record | do not call restart | fresh create path |
| Root live record | call restart with skipAutoResume omitted | backend tears down old runtime and spawns one fresh configured muse replacement |
| Root dormant record | call restart with false | muse resume --last when eligible |

No auto-resume token, provider branch, stored UUID, or failure retry belongs in
the frontend. The production ProjectPanel logic already implements its rows and
is test-only scope here. root-agent-action.ts receives only a comment correction
that lists Muse among provider-neutral consumers; its executable logic remains
byte-identical.

The live Root row is deliberately not reuse. RootAgentBanner routes the returned
restart action to SessionAPI.restart; omission serializes as null, the backend
defaults null to fresh, tears down the old runtime, and creates a replacement.

## Exact edits by file

### agent-presets.ts

- Append the exact Muse row.
- Preserve the first seven objects, definitionToSeed, and optional-property
  behavior.
- Update only the adjacent maintenance comment: six agents have verified
  update commands; Cursor and Muse have none; every default auto-update is off.

### agent-presets.test.ts

- Expand expected built-ins and key order from seven to eight.
- Pin every Muse value and prove instructionsFilename/configSeed are absent
  both on the shipped definition and after definitionToSeed.
- Keep existing shared-home, empty-environment, removability, and default
  auto-update checks across all rows.
- Prove the same six update commands remain exact and Cursor/Muse are the only
  empty-update defaults.

### agent-update-status.test.ts

- Append Muse to the default fixture with empty updateCommands.
- Prove exact updateable order remains
  claude, codex, hermes, pi, opencode, antigravity.
- Assert Cursor and Muse are separately absent.
- Preserve the pi-alt view length of seven: it is six updateable defaults plus
  pi-alt, not the built-in catalog count.
- Do not edit production updater code.

### ProjectPanel.reopen-resume.test.tsx

- Convert the configured-agent fixture from Claude to Muse. Retain both
  assertions: a closed marker sends agentId muse, cwd, and
  `skipAutoResume:false`; no marker sends the same identity/cwd with
  `skipAutoResume:null`.
- Do not edit ProjectPanel.tsx or weaken existing close-marker assertions.

### root-agent-action.ts and root-agent-action.test.ts

- Update only the source comment so Muse is covered by the provider-neutral
  state mapping.
- Use Muse fixtures for no prior Root, live Root, and dormant Root. Bind the
  live result, require `toStrictEqual({ kind: "restart", id: "root-id",
  agentId: "muse" })`, and separately require
  `expect(live).not.toHaveProperty("skipAutoResume")`; equality alone is not
  omission proof. Require dormant `toStrictEqual` with own
  `skipAutoResume: false`. This is fresh replacement, never live reuse/no-spawn.
- Preserve the underlying Root action implementation and existing non-Muse
  coverage.

## Failure and unsupported behavior

Missing binaries, authentication failures, process exits, and resume failures
continue through backend error handling. The frontend neither retries fresh nor
parses errors. Muse stays visible on Windows because catalog filtering is not
introduced; #1873 keeps automatic resume fail-closed on unsupported
platforms/transports.

Empty update commands exclude Muse from managed-update rows and actions.
Telegram, context watcher, seed/credential, logical/privileged PTY, version,
installation, authentication, update, and container behavior remain outside
this phase.

## Verification

The producer may run this for feedback. Acceptance requires ac-tech-lead-v4 to
rerun the first three frozen Bash bodies (combined SHA-256
`BC19B0A3BADCC6AF8B2042AB886809B1CDBF56A6AE97F056C590156A38BA8483`) from a fresh noninteractive shell at one clean commit. It treats the producer SHA only as expected input, requires current branch/commit equality, validates the accepted plan digest, and records
`FRONTEND_CANDIDATE_SHA`, symbolic branch ref, and complete HEAD plus branch checkout-history digest. The fourth Bash body owns a separate clean phase-base measurement before the candidate run; producer baseline files or hashes are never inputs. Every logged child is guarded; checkout-away-and-back fails even when the branch never moves.

~~~bash
set -euo pipefail
declare -F run_logged >/dev/null; test -n "${FRONTEND_LOG_DIR:-}"; test -n "${NODE_BIN:-}"; test -n "${PHASE_BASE_SHA:-}"
test "$FRONTEND_CANDIDATE_SHA" = "$LEAD_CANDIDATE_SHA"; test "$LEAD_BASE_SHA" = "$PHASE_BASE_SHA"; test "$LEAD_BASE_SHA" != "$FRONTEND_CANDIDATE_SHA"
test "$(sha256sum "$LEAD_BASE_REPORT" | cut -d' ' -f1)" = "$LEAD_BASE_REPORT_SHA256"; test "$(sed -n '1p' "$LEAD_BASE_BINDING")" = "base_sha=$PHASE_BASE_SHA"; test "$(sed -n '2p' "$LEAD_BASE_BINDING")" = "report_sha256=$LEAD_BASE_REPORT_SHA256"
assert_frontend_candidate; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1
TARGET_TESTS=(src/shared/agent-presets.test.ts src/sidebar/agent-update-status.test.ts \
  src/sidebar/components/ProjectPanel.reopen-resume.test.tsx src/sidebar/components/root-agent-action.test.ts)
for target in "${TARGET_TESTS[@]}"; do test -f "$target" && test -r "$target"; done
OWNED_PATHS=(
  src/shared/agent-presets.test.ts
  src/shared/agent-presets.ts
  src/sidebar/agent-update-status.test.ts
  src/sidebar/components/ProjectPanel.reopen-resume.test.tsx
  src/sidebar/components/root-agent-action.test.ts
  src/sidebar/components/root-agent-action.ts
)
test "${#OWNED_PATHS[@]}" -eq 6
printf '%s\n' "${OWNED_PATHS[@]}" >"$FRONTEND_LOG_DIR/scope.expected"
test "$(wc -l <"$FRONTEND_LOG_DIR/scope.expected")" -eq 6
LC_ALL=C sort -c "$FRONTEND_LOG_DIR/scope.expected"
assert_vitest_four() {
  local label="$1" summary="$FRONTEND_LOG_DIR/$1.summary"
  assert_frontend_candidate
  grep -E '^[[:space:]]*(Test Files|Tests)[[:space:]]' "$FRONTEND_LOG_DIR/$label.log" >"$summary"
  test "$(wc -l <"$summary")" -eq 2
  grep -Eq '^[[:space:]]*Test Files[[:space:]]+4 passed[[:space:]]+\(4\)[[:space:]]*$' "$summary"
  grep -Eq '^[[:space:]]*Tests[[:space:]]+[1-9][0-9]* passed[[:space:]]+\([1-9][0-9]*\)[[:space:]]*$' "$summary"
  assert_frontend_candidate
}
run_logged dependencies-post 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" run check:frontend-dependencies
run_logged dependencies-compare 1m cmp -s "$LEAD_BASE_REPORT" "$FRONTEND_LOG_DIR/dependencies-post.log"
run_logged targeted 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" \
  CI=1 NO_COLOR=1 FORCE_COLOR=0 "$NODE22" "$NPM11" test -- --reporter=default "${TARGET_TESTS[@]}"
assert_vitest_four targeted
for task in typecheck build; do
  run_logged "npm-$task" 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" run "$task"
done
run_logged diff-check 1m git diff --check "$PHASE_BASE_SHA" "$FRONTEND_CANDIDATE_SHA" --
run_logged protected-byte-check 1m git diff --exit-code "$PHASE_BASE_SHA" "$FRONTEND_CANDIDATE_SHA" -- src-tauri/module-arcs.txt package.json package-lock.json src-tauri/Cargo.toml Cargo.lock .github/workflows
run_logged scope-working 1m git diff --name-only "$PHASE_BASE_SHA" "$FRONTEND_CANDIDATE_SHA" --
test "$(wc -l <"$FRONTEND_LOG_DIR/scope-working.log")" -eq 6
run_logged scope-working-compare 1m cmp -s "$FRONTEND_LOG_DIR/scope.expected" "$FRONTEND_LOG_DIR/scope-working.log"
run_logged scope-expected-sha 1m sha256sum "$FRONTEND_LOG_DIR/scope.expected"
run_logged scope-working-sha 1m sha256sum "$FRONTEND_LOG_DIR/scope-working.log"
~~~

The summary contains only anchored reporter result lines, never npm's echoed
argv. It must say exactly Test Files 4 passed (4) and a nonzero all-pass test
count; a missing path fails before execution and three executed files cannot
pass. Independently measured phase-base/candidate dependency output must be byte-identical, including counts and
SCC/rule verdicts. Any timeout, debt, drift, or assertion failure blocks.

The same uninterrupted lead executor then records candidate identity and final
state. No command derives a later symbolic HEAD as evidence:

~~~bash
run_logged status-candidate-pre 1m git status --porcelain --untracked-files=normal
test ! -s "$FRONTEND_LOG_DIR/status-candidate-pre.log"
run_logged targeted-candidate 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" \
  CI=1 NO_COLOR=1 FORCE_COLOR=0 "$NODE22" "$NPM11" test -- --reporter=default "${TARGET_TESTS[@]}"
assert_vitest_four targeted-candidate
run_logged evidence-sha 1m git rev-parse --verify "$FRONTEND_CANDIDATE_SHA^{commit}"
run_logged evidence-date 1m "$NODE22" -p "new Date().toISOString().slice(0, 10)"
run_logged evidence-os 1m "$NODE22" -p "process.platform + '/' + process.arch + ' ' + require('node:os').release()"
run_logged scope-candidate 1m git diff --name-only "$PHASE_BASE_SHA" "$FRONTEND_CANDIDATE_SHA" --
test "$(wc -l <"$FRONTEND_LOG_DIR/scope-candidate.log")" -eq 6
run_logged scope-candidate-compare 1m cmp -s "$FRONTEND_LOG_DIR/scope.expected" "$FRONTEND_LOG_DIR/scope-candidate.log"
run_logged scope-candidate-sha 1m sha256sum "$FRONTEND_LOG_DIR/scope-candidate.log"
run_logged status-final 1m git status --porcelain --untracked-files=normal
test ! -s "$FRONTEND_LOG_DIR/status-final.log"
run_logged status-final-sha 1m sha256sum "$FRONTEND_LOG_DIR/status-final.log"
assert_frontend_candidate
sha256sum "$LEAD_BASE_BINDING" "$LEAD_BASE_REPORT"
printf 'frontend_executor=PASS candidate_sha=%s base_sha=%s test_files=4 scope_files=6 head_drift=none\n' \
  "$FRONTEND_CANDIDATE_SHA" "$PHASE_BASE_SHA"
~~~

`FRONTEND_EVIDENCE_SHA` is exactly the captured candidate, never a later HEAD.
The tech lead independently derives date/OS, nonzero counts, catalog/updater
order, five lifecycle values, scope/status and log hashes from this run. After
every configured-required check passes on that same PR-head SHA, it sends
`ac.frontend-evidence.v1` directly to #1862 and the reviewer through the
configured CLI. The strict ordered attestation binds attester/issue/branch,
base/candidate/CI SHA, accepted plan and extracted-executor SHA-256, execution-
log and six-path/status hashes, counts and lifecycle values, `head_drift=none`,
and `result=pass`. Exact ordered keys are `schema,attester,issue,branch,phase_base_sha,candidate_sha,ci_head_sha,date_utc,os,plan_sha256,executor_sha256,execution_log_sha256,scope_sha256,status_sha256,test_files,tests_passed,catalog_order,updateable_order,project_closed_skip,project_fresh_skip,root_live_skip,root_dormant_skip,head_drift,result`; duplicates/extras fail. The execution log also binds lead base SHA/report/binding hashes. The lead repeats the candidate/ref/reflog guard before and
after CI lookup and attestation delivery. It captures one canonical `Queued:` only as enqueue
proof; acceptance requires the exact system-delivered `[Message from .../ac-tech-lead-v4]`
notification path and byte hash. A producer message, copied path, private log,
plausible fields, hashes, or real producer receipt cannot substitute.

The frozen adversarial harness must return nonzero for `same-six-clean-after-head-drift`, checkout/detach H1→H2→H1 during a child, child exit 42, timeout, log-tee and metadata-tee failure, and fake producer baseline
(H1 is captured/tested, H2 changes only the same six paths and is clean), a
producer that skips every command but fabricates all PASS fields/log hashes plus
a real `Queued:`, a producer-named or copied/renamed/mutated lead attestation,
wrong base/candidate/CI SHA, missing/extra/reordered field, or absent direct lead
notification. A new candidate discards the whole run and starts from zero.

The lead runs this fourth body once per requested candidate after validating the accepted full plan digest. It owns the base checkout, report and hash; the producer supplies only expected candidate/base/branch. The deliberate base→candidate transition precedes candidate anchoring. All later movement revokes the run, including CI/delivery guards; regenerate the baseline for every new candidate.
~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
: "${AGENTSCOMMANDER_ROOT:?}" "${PHASE_BASE_SHA:?}" "${FRONTEND_CANDIDATE_SHA:?}" "${EXPECTED_FRONTEND_BRANCH_REF:?}" "${EXPECTED_PLAN_SHA256:?}"
export PHASE_BASE_SHA
plan=plans/1854-muse-code-beta-preset/1861-frontend-catalog-mirror.md
 test "$(sha256sum "$plan" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')" = "$EXPECTED_PLAN_SHA256"
status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; test "$(git symbolic-ref -q HEAD)" = "$EXPECTED_FRONTEND_BRANCH_REF"; test "$(git rev-parse HEAD)" = "$FRONTEND_CANDIDATE_SHA"; git merge-base --is-ancestor "$PHASE_BASE_SHA" "$FRONTEND_CANDIDATE_SHA"
mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"; LEAD_DIR="$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1861-lead.XXXXXX")"
awk '/^~~~bash$/{b++;inside=(b<=3);next} /^~~~$/{inside=0;next} inside{print}' "$plan" >"$LEAD_DIR/executor.sh"
awk '/^~~~bash$/{b++;next} /^~~~$/{if(b==1)exit} b==1{print}' "$plan" >"$LEAD_DIR/base.sh"
test "$(sha256sum "$LEAD_DIR/executor.sh" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')" = BC19B0A3BADCC6AF8B2042AB886809B1CDBF56A6AE97F056C590156A38BA8483
# The owner has stopped. Only this lead-controlled, clean checkout transition is authorized.
git switch --detach "$PHASE_BASE_SHA"; git switch -c "validation/1861-base-${FRONTEND_CANDIDATE_SHA:0:12}-$(date -u +%Y%m%d%H%M%S)"
LEAD_CANDIDATE_SHA="$PHASE_BASE_SHA"; bind_checkout; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; mkdir "$LEAD_DIR/base"
FRONTEND_LOG_DIR="$LEAD_DIR/base" FRONTEND_MEASUREMENT_KIND=base env -u BASH_ENV -u ENV bash --noprofile --norc "$LEAD_DIR/base.sh" </dev/null >"$LEAD_DIR/base-execution.log" 2>&1
assert_checkout; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1
LEAD_BASE_SHA="$PHASE_BASE_SHA"; LEAD_BASE_REPORT="$LEAD_DIR/base/dependencies-pre.log"; LEAD_BASE_REPORT_SHA256="$(sha256sum "$LEAD_BASE_REPORT" | cut -d' ' -f1)"; LEAD_BASE_BINDING="$LEAD_DIR/base.binding"
printf 'base_sha=%s
report_sha256=%s
checkout_sha256=%s
' "$LEAD_BASE_SHA" "$LEAD_BASE_REPORT_SHA256" "$LEAD_CHECKOUT_SHA256" >"$LEAD_BASE_BINDING"; sha256sum "$LEAD_DIR/base.sh" "$LEAD_DIR/base-execution.log" "$LEAD_DIR/base/node-path-version.log" "$LEAD_DIR/base/npm-version.log" >>"$LEAD_BASE_BINDING"
test "$(git rev-parse "$EXPECTED_FRONTEND_BRANCH_REF")" = "$FRONTEND_CANDIDATE_SHA"; git switch "${EXPECTED_FRONTEND_BRANCH_REF#refs/heads/}"
LEAD_CANDIDATE_SHA="$FRONTEND_CANDIDATE_SHA"; bind_checkout; export PHASE_BASE_SHA FRONTEND_CANDIDATE_SHA LEAD_BASE_SHA LEAD_BASE_REPORT LEAD_BASE_REPORT_SHA256 LEAD_BASE_BINDING
mkdir "$LEAD_DIR/candidate"; set +e
FRONTEND_LOG_DIR="$LEAD_DIR/candidate" FRONTEND_MEASUREMENT_KIND=candidate env -u BASH_ENV -u ENV bash --noprofile --norc "$LEAD_DIR/executor.sh" </dev/null 2>&1 | tee "$LEAD_DIR/execution.log"
outer_status=("${PIPESTATUS[@]}"); set -e; for value in "${outer_status[@]}"; do test "$value" -eq 0 || exit 1; done
assert_checkout; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; cmp -s "$LEAD_DIR/base/node-path-version.log" "$LEAD_DIR/candidate/node-path-version.log"; cmp -s "$LEAD_DIR/base/npm-version.log" "$LEAD_DIR/candidate/npm-version.log"
sha256sum "$LEAD_BASE_BINDING" "$LEAD_BASE_REPORT" | tee "$LEAD_DIR/baseline-attestation.log"
~~~

## Dependency-cycle and layering gate

Planned new/removed module arcs are zero: values, comments, and same-module test
fixtures add no import/production call and create no role inversion. The normal
gate is the exact pre/post `check:frontend-dependencies` comparison above,
including pinned tool version, fixture-matrix verdicts, complete-root
modules/errors/dependencies, rule verdict, and final OK, plus a byte-identical
src-tauri/module-arcs.txt and a diff scan with no module-reference hunk.

Any added/removed import/export/require, changed dependency field/verdict, Rust
source hunk, or arc-record drift is the conditional trigger. Stop and send
PHASE_BASE_SHA, clean candidate SHA, exact diff, and outputs to the coordinator.
Frontend relationship drift is rejected until architecture revises scope. Rust
arc drift is handed to ac-tech-lead-v4, the authorized `rust-levelization-run`
owner, who returns base/candidate cyclicSccs, sorted SCC member sets, added/
removed and cross-boundary pairs, regenerated arc SHA/byte comparison, and
layering exits. Dirty/missing evidence, exit 3, any new cross-boundary pair, or
non-identical SCC/arc results blocks; the frontend owner never improvises it.

## Commit, CI, scope, and recovery

Commit only the six owned files. Against PHASE_BASE_SHA, require exactly
those paths, a clean final tree, and unchanged plan/#1860/#1873 hashes.

On `FRONTEND_EVIDENCE_SHA`, which must be the exact #1861 PR-head and attested
CI-head SHA, require every triggered/configured-required
check: test-debt; Windows Rust check, clippy, and full tests; Linux Rust check,
clippy, and its configured test; macOS Rust check and clippy; rust-fmt; all four
portable terminal legs; Windows release CLI smoke; frontend regression; and
validate-branch-name. The PR lockfile-drift detector must pass and skip
regeneration because package inputs do not change. bundle-validation and
version-sync are path-inapplicable. Re-derive this map after relevant drift;
another SHA, waiver, bypass, or unexplained skip fails.

Recovery is path-scoped and compare-before-restore. Restore only a byte still
matching the recorded phase output. Preserve external edits and report
conflicts. Never use broad reset, checkout, restore, or clean.

## Acceptance criteria

1. Backend and frontend catalogs have the same eight rows and exact Muse data.
2. The already-landed Rust/TypeScript CodingAgentKind contract both admits
   muse and remains unchanged in this phase.
3. Muse optional instruction/config fields stay absent and it stays out of
   managed updates.
4. ProjectPanel and Root tests pin every settled fresh/resume intent, including
   live Root fresh replacement, while runtime argv remains backend-owned.
5. Lead-owned targeted tests, typecheck, build, dependency/arc checks, byte-exact
   six-file candidate scope, empty final porcelain, no ref/reflog drift, canonical
   attestation, and exact-candidate CI pass; every forgery negative fails.
6. No production lifecycle branch, dependency arc, retry, or unsupported
   capability is added.

Status: READY_FOR_IMPLEMENTATION
