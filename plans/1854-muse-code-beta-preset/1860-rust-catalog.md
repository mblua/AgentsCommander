# Phase #1860 — bootstrap the frozen plan set and add Muse to the Rust catalog
Class: patterned
Owner: AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4
Child issue: [#1860 — Rust: add Muse to the embedded catalog](https://github.com/mblua/AgentsCommander/issues/1860)
Parent epic: [#1854 — Add a beta Muse Code catalog preset](https://github.com/mblua/AgentsCommander/issues/1854)
Repository: /home/mblua/0_repos/AgentsCommander_iac/.ac/room-9-ac-dev-team-v4/repo-AgentsCommander
Branch: feature/1860-muse-embedded-catalog
Branch base: then-current main, recorded when the coordinator creates this branch
Design evidence base: 5c7dd08841a5846f483ba59a0d0dd94ea6410c4d
Depends on: none
Total phase files: 9
## Objective
Track the five frozen plans byte-identically, then separately add Muse as the
eighth embedded preset and update every Rust count consumer. Fresh/default data
gains Muse; valid user data stays byte-identical and Muse-free. #1873 alone owns
runtime kind/resume/lifecycle/persistence/failures. This phase adds no launcher,
IPC/CLI schema, updater, PTY, watcher, credential, dependency, or module change.
## Exact nine-file scope
The complete branch delta contains exactly:
1. plans/1854-muse-code-beta-preset/epic.md
2. plans/1854-muse-code-beta-preset/1860-rust-catalog.md
3. plans/1854-muse-code-beta-preset/1873-muse-auto-resume.md
4. plans/1854-muse-code-beta-preset/1861-frontend-catalog-mirror.md
5. plans/1854-muse-code-beta-preset/1862-documentation-evidence.md
6. src-tauri/resources/coding-agents/agents.default.json
7. src-tauri/src/config/coding_agents_catalog.rs
8. src-tauri/src/web/commands.rs
9. src-tauri/tests/cli_project_registration.rs
The first commit contains only files 1–5. The product commit contains only
files 6–9. No plan byte may change between freeze, transfer, commit, review,
merge, or any later phase.
## Environment-risk acknowledgement
Catalog/unit evidence proves no installation, authentication, launchability, or
resume claim. #1873 owns those tests; no interactive/install/auth/update action
is authorized here.
## Ignored-plan bootstrap
The ignored plans begin intent-to-add. The user freezes five uppercase SHA-256s;
the coordinator requires exact source bytes and no product diff, replaces live
#1854/#1860 bodies byte-for-byte while preserving open/link state, re-extracts
them, and sends an immutable digest sync. Any stale/missing/duplicate byte blocks.
Fetch origin/main, branch `feature/1860-muse-embedded-catalog` from current main,
record `PHASE_BASE_SHA`, transfer/re-hash only the five plans, then stage only:
~~~bash
git add -f -- \
  plans/1854-muse-code-beta-preset/epic.md \
  plans/1854-muse-code-beta-preset/1860-rust-catalog.md \
  plans/1854-muse-code-beta-preset/1873-muse-auto-resume.md \
  plans/1854-muse-code-beta-preset/1861-frontend-catalog-mirror.md \
  plans/1854-muse-code-beta-preset/1862-documentation-evidence.md
~~~

Require exactly that cached set, `git diff --cached --check`, and matching staged
blob hashes. Commit only those plans with parent `PHASE_BASE_SHA`, re-hash committed
blobs, require clean state, and record `PLAN_BOOTSTRAP_COMMIT`; only then start a
separate product commit. Recovery restores only still-matching owned output;
preserve external bytes and forbid broad reset/checkout/restore/clean.

## Catalog contract

Append this exact object after Antigravity, preserving schema version 1 and the
first seven objects:

~~~json
{
  "key": "muse",
  "label": "Muse Code",
  "description": "Meta terminal coding agent (beta; macOS/Linux host only)",
  "color": "#0668E1",
  "command": "muse",
  "envs": [],
  "isolatedHome": false,
  "removable": true,
  "updateCommands": [],
  "autoUpdate": false
}
~~~

instructionsFilename/configSeed are absent; updateCommands is empty. Exact order:

~~~text
claude, codex, hermes, cursor, pi, opencode, antigravity, muse
~~~

No Muse seed, environment, platform/schema, updater, or credential change.

## Exact product changes

### agents.default.json

- Append only the object above.
- Keep every existing object byte-for-byte except the delimiter required before
  the appended object.
- Keep schemaVersion at 1.

### coding_agents_catalog.rs

Production functions and imports remain unchanged. Inside the existing test
module:

- Change EXPECTED_PRESETS from seven to eight entries and its filename member
  to Option<&str>. Wrap existing filenames in Some and give Muse None.
- Pin eight keys in order, with Antigravity immediately before Muse.
- Pin every Muse field and omission, empty env/updater lists, shared home,
  removability, and disabled auto-update.
- Update missing-manifest and corrupt-manifest embedded-default lengths to
  eight and assert Muse is last.
- Preserve corrupt bytes and valid custom-catalog behavior.
- Update seed expectations to three configured seeds and five entries without
  one; Muse must not create or require a seed.

### web/commands.rs

- Update only the embedded catalog route test from seven to eight rows.
- Pin Muse as the last returned row and preserve the existing assertion that
  exactly six rows have update commands.
- Do not change production route behavior, serialization, filtering, or
  updater behavior.

### cli_project_registration.rs

- Update the fresh/default catalog expectation from seven to eight, pin Muse
  last, and prove Muse's exact public catalog fields and omissions.
- Keep the valid custom fixture byte-identical and legacy-only.
- Preserve seed-once behavior and prove Muse creates no config seed.
- Do not add CLI options or production code.

## Required tests and evidence

Run from the repository root with noninteractive stdin, pipefail, retained
stdout/stderr/exit/timing, and 30-minute bounds:

~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
if test -z "${LEAD_CHECKOUT_SHA256:-}"; then LEAD_CANDIDATE_SHA="$(git rev-parse HEAD)" || exit 1; bind_checkout; fi
assert_checkout
command -v timeout rg cmp sha256sum sed cut wc sort awk tar gzip base64 split tr jq find >/dev/null
test -n "${AGENTSCOMMANDER_ROOT:-}"
mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"; test "$(realpath -e "$AGENTSCOMMANDER_ROOT/.evidence")" = "$(realpath -e "$AGENTSCOMMANDER_ROOT")/.evidence"
CATALOG_LOG_DIR="${CATALOG_LOG_DIR:-$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1860.XXXXXX")}"; test -d "$CATALOG_LOG_DIR"
case "$(realpath -e "$CATALOG_LOG_DIR")" in "$(realpath -e "$AGENTSCOMMANDER_ROOT")"/.evidence/*) ;; *) exit 1;; esac
export CATALOG_LOG_DIR PHASE_BASE_SHA
CATALOG_REQUIRED_LABELS=(rustfmt catalog catalog-count web web-count cli-catalog
  cli-catalog-count resolver-default resolver-default-count resolver-explicit
  resolver-explicit-count lib rust-lib-count cli-full cli-full-count cargo-check
  clippy diff-check protected-byte-check phase-scope phase-scope-equal)
CATALOG_LEDGER="$CATALOG_LOG_DIR/catalog-invocations.ledger"
CATALOG_RECIPE_SHA256=1110F3D6660448313F3CA867A9D88B6C06EE936C6B075BBFBE9315767354700D
CATALOG_FROZEN_VERIFIER_SHA256=55C2536BAB01E65455FDCA44F901CB32462F97048DCBCA2CA8152A4EC207B13D
test ! -e "$CATALOG_LEDGER"; : >"$CATALOG_LEDGER"
validate_catalog_meta() {
  local meta="$1" limit="$2" argv="$3" start end; local -a row
  test -f "$meta"; test -n "$argv"; mapfile -t row <"$meta"; test "${#row[@]}" -eq 8
  cmp -s "$meta" <(printf '%s\n' "${row[@]}")
  test "${row[0]}" = "timeout_limit=$limit"; printf '%s\n' "$limit" | rg -x -- '[1-9][0-9]*[smhd]' >/dev/null
  test "${row[1]}" = 'stdin=/dev/null'
  printf '%s\n' "${row[2]}" | rg -x -- 'start_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z' >/dev/null
  printf '%s\n' "${row[3]}" | rg -x -- 'end_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z' >/dev/null
  start="${row[2]#start_utc=}"; end="${row[3]#end_utc=}"; [[ "$end" == "$start" || "$end" > "$start" ]]
  printf '%s\n' "${row[4]}" | rg -x -- 'elapsed_seconds=(0|[1-9][0-9]*)' >/dev/null
  test "${row[5]}" = command_exit=0; test "${row[6]}" = tee_exit=0; test "${row[7]}" = "argv=$argv"
}
run_logged() {
  local label="$1" limit="$2"; shift 2; local start_utc start_s end_utc end_s rc tee_rc meta argv seen
  printf -v argv '%q ' "$@"; test -n "$argv"
  while IFS=$'\t' read -r seen _; do test "$seen" != "$label"; done <"$CATALOG_LEDGER"
  printf '%s\t%s\t%s\n' "$label" "$limit" "$argv" >>"$CATALOG_LEDGER"
  assert_checkout
  start_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; start_s="$(date +%s)"
  set +e; timeout "$limit" "$@" </dev/null 2>&1 | tee "$CATALOG_LOG_DIR/$label.log"
  local -a status=("${PIPESTATUS[@]}"); set -e; rc="${status[0]}"; tee_rc="${status[1]}"
  assert_checkout
  end_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; end_s="$(date +%s)"
  meta="$CATALOG_LOG_DIR/$label.meta"
  { printf 'timeout_limit=%s\nstdin=/dev/null\nstart_utc=%s\nend_utc=%s\nelapsed_seconds=%s\ncommand_exit=%s\ntee_exit=%s\nargv=' "$limit" "$start_utc" "$end_utc" "$((end_s-start_s))" "$rc" "$tee_rc"; printf '%q ' "$@"; printf '\n'; } | tee "$meta"
  local -a meta_status=("${PIPESTATUS[@]}"); for value in "${meta_status[@]}" "$rc" "$tee_rc"; do test "$value" -eq 0 || exit 1; done
  validate_catalog_meta "$meta" "$limit" "$argv"; test "$rc" -eq 0; test "$tee_rc" -eq 0
}
validate_catalog_ledger() {
  test "${#CATALOG_REQUIRED_LABELS[@]}" -eq 21; test "$(wc -l <"$CATALOG_LEDGER")" -eq 21
  test "$(cut -f1 "$CATALOG_LEDGER" | LC_ALL=C sort -u | wc -l)" -eq 21
  test "$(LC_ALL=C sort -u "$CATALOG_LEDGER" | wc -l)" -eq 21
  cmp -s <(printf '%s\n' "${CATALOG_REQUIRED_LABELS[@]}") <(cut -f1 "$CATALOG_LEDGER")
  test "$(sha256sum "$CATALOG_LEDGER" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')" = "$CATALOG_RECIPE_SHA256"
}
prepare_catalog_manifest() {
  local manifest="$CATALOG_LOG_DIR/catalog-evidence.manifest" sidecar="$CATALOG_LOG_DIR/catalog-evidence.payload.sha256"
  local verifier="$CATALOG_LOG_DIR/catalog-evidence.verify.sh" bundle="$CATALOG_LOG_DIR/catalog-evidence.tar.gz"
  local label limit argv log meta log_sha meta_sha
  validate_catalog_ledger; test ! -e "$manifest"; test ! -e "$sidecar"; test ! -e "$verifier"; test ! -e "$bundle"; : >"$manifest"
  while IFS=$'\t' read -r label limit argv; do
    log="$CATALOG_LOG_DIR/$label.log"; meta="$CATALOG_LOG_DIR/$label.meta"
    test -f "$log"; validate_catalog_meta "$meta" "$limit" "$argv"
    log_sha="$(sha256sum "$log" | cut -d' ' -f1)"; meta_sha="$(sha256sum "$meta" | cut -d' ' -f1)"
    { printf 'artifact=%s\nlog_path=%s.log\nlog_sha256=%s\nmeta_path=%s.meta\nmeta_sha256=%s\nmeta_begin\n' "$label" "$label" "$log_sha" "$label" "$meta_sha"; sed -n 'p' "$meta"; printf 'meta_end\n'; } >>"$manifest"
  done <"$CATALOG_LEDGER"
  test "$(wc -l <"$manifest")" -eq 315
  cmp -s <(printf '%s\n' "${CATALOG_REQUIRED_LABELS[@]}") <(sed -n 's/^artifact=//p' "$manifest")
  { printf '#!/usr/bin/env bash\nset -euo pipefail\n'; declare -p CATALOG_REQUIRED_LABELS CATALOG_RECIPE_SHA256; declare -f validate_catalog_meta validate_catalog_ledger; cat <<'VERIFY'
CATALOG_LOG_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
CATALOG_LEDGER="$CATALOG_LOG_DIR/catalog-invocations.ledger"; sidecar="$CATALOG_LOG_DIR/catalog-evidence.payload.sha256"
validate_catalog_ledger; test "$(wc -l <"$sidecar")" -eq 45
cmp -s <({ for label in "${CATALOG_REQUIRED_LABELS[@]}"; do printf '%s.log\n%s.meta\n' "$label" "$label"; done; printf '%s\n' catalog-invocations.ledger catalog-evidence.manifest catalog-evidence.verify.sh; }) <(awk '{print $2}' "$sidecar")
(cd "$CATALOG_LOG_DIR" && sha256sum -c catalog-evidence.payload.sha256)
: "${AGENTSCOMMANDER_ROOT:?}"; evidence_root="$(realpath -e "$AGENTSCOMMANDER_ROOT/.evidence")"; case "$evidence_root" in "$(realpath -e "$AGENTSCOMMANDER_ROOT")"/.evidence) ;; *) exit 1;; esac; rebuilt="$(mktemp "$evidence_root/ac-1860-rebuilt.XXXXXX")"; case "$(realpath -e "$rebuilt")" in "$evidence_root"/*) ;; *) exit 1;; esac
while IFS=$'\t' read -r label limit argv; do log="$CATALOG_LOG_DIR/$label.log"; meta="$CATALOG_LOG_DIR/$label.meta"; validate_catalog_meta "$meta" "$limit" "$argv"; log_sha="$(sha256sum "$log" | cut -d' ' -f1)"; meta_sha="$(sha256sum "$meta" | cut -d' ' -f1)"; { printf 'artifact=%s\nlog_path=%s.log\nlog_sha256=%s\nmeta_path=%s.meta\nmeta_sha256=%s\nmeta_begin\n' "$label" "$label" "$log_sha" "$label" "$meta_sha"; sed -n 'p' "$meta"; printf 'meta_end\n'; } >>"$rebuilt"; done <"$CATALOG_LEDGER"
test "$(wc -l <"$rebuilt")" -eq 315; cmp -s "$CATALOG_LOG_DIR/catalog-evidence.manifest" "$rebuilt"
printf 'receiver_verifier=PASS artifacts=21 sidecar_entries=45 manifest_sha256=%s\n' "$(sha256sum "$rebuilt" | cut -d' ' -f1)"
VERIFY
  } >"$verifier"
  CATALOG_MANIFEST_SHA256="$(sha256sum "$manifest" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')"
  CATALOG_VERIFIER_SHA256="$(sha256sum "$verifier" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')"
  test "$CATALOG_VERIFIER_SHA256" = "$CATALOG_FROZEN_VERIFIER_SHA256"
  assert_checkout; CATALOG_EVIDENCE_SHA="$LEAD_CANDIDATE_SHA"; export CATALOG_MANIFEST_SHA256 CATALOG_VERIFIER_SHA256 CATALOG_EVIDENCE_SHA
}
seal_catalog_bundle() {
  local label sidecar="$CATALOG_LOG_DIR/catalog-evidence.payload.sha256" bundle="$CATALOG_LOG_DIR/catalog-evidence.tar.gz" types="$CATALOG_LOG_DIR/catalog-evidence.types"
  local -a files=(catalog-invocations.ledger catalog-evidence.manifest catalog-evidence.verify.sh catalog-evidence.payload.sha256)
  test ! -e "$sidecar"; test ! -e "$bundle"; : >"$sidecar"
  while IFS=$'\t' read -r label _; do (cd "$CATALOG_LOG_DIR" && sha256sum "$label.log" "$label.meta") >>"$sidecar"; files+=("$label.log" "$label.meta"); done <"$CATALOG_LEDGER"
  (cd "$CATALOG_LOG_DIR" && sha256sum catalog-invocations.ledger catalog-evidence.manifest catalog-evidence.verify.sh) >>"$sidecar"
  test "$(wc -l <"$sidecar")" -eq 45; bash "$CATALOG_LOG_DIR/catalog-evidence.verify.sh"
  tar -C "$CATALOG_LOG_DIR" -czf "$bundle" "${files[@]}"; cmp -s <(printf '%s\n' "${files[@]}") <(tar -tzf "$bundle")
  LC_ALL=C tar --numeric-owner -tvzf "$bundle" >"$types"; test "$(wc -l <"$types")" -eq 46; awk 'substr($1,1,1)!="-"{exit 1}' "$types"
  CATALOG_BUNDLE_SHA256="$(sha256sum "$bundle" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')"; CATALOG_BUNDLE_BYTES="$(wc -c <"$bundle")"
  export CATALOG_BUNDLE_SHA256 CATALOG_BUNDLE_BYTES
}
run_logged rustfmt 30m cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
run_logged catalog 30m cargo test --manifest-path src-tauri/Cargo.toml --locked \
  config::coding_agents_catalog::tests -- --nocapture
run_logged catalog-count 1m bash -c 'rg -n -- "^test result: ok\. [1-9][0-9]* passed; 0 failed" "$CATALOG_LOG_DIR/catalog.log"'
run_logged web 30m cargo test --manifest-path src-tauri/Cargo.toml --locked \
  web::commands::tests::get_coding_agent_catalog_route_returns_backfilled_catalog \
  -- --exact --nocapture
run_logged web-count 1m bash -c 'rg -n -- "^test result: ok\. 1 passed; 0 failed" "$CATALOG_LOG_DIR/web.log"'
run_logged cli-catalog 30m cargo test --manifest-path src-tauri/Cargo.toml --locked \
  --test cli_project_registration catalog -- --nocapture
run_logged cli-catalog-count 1m bash -c 'rg -n -- "^test result: ok\. 2 passed; 0 failed" "$CATALOG_LOG_DIR/cli-catalog.log"'
run_logged resolver-default 30m cargo test --manifest-path src-tauri/Cargo.toml --locked \
  config::agent_command::tests::default_instructions_filename_maps_by_detected_kind \
  -- --exact --nocapture
run_logged resolver-default-count 1m bash -c 'rg -n -- "^test result: ok\. 1 passed; 0 failed" "$CATALOG_LOG_DIR/resolver-default.log"'
run_logged resolver-explicit 30m cargo test --manifest-path src-tauri/Cargo.toml --locked \
  config::agent_command::tests::resolve_instructions_filename_prefers_valid_explicit \
  -- --exact --nocapture
run_logged resolver-explicit-count 1m bash -c 'rg -n -- "^test result: ok\. 1 passed; 0 failed" "$CATALOG_LOG_DIR/resolver-explicit.log"'
run_logged lib 30m cargo test --manifest-path src-tauri/Cargo.toml --locked --lib \
  -- --nocapture
run_logged rust-lib-count 1m bash -c 'rg -n -- "^test result: ok\. [1-9][0-9]* passed; 0 failed" "$CATALOG_LOG_DIR/lib.log"'
run_logged cli-full 30m cargo test --manifest-path src-tauri/Cargo.toml --locked \
  --test cli_project_registration -- --nocapture
run_logged cli-full-count 1m bash -c 'rg -n -- "^test result: ok\. [1-9][0-9]* passed; 0 failed" "$CATALOG_LOG_DIR/cli-full.log"'
run_logged cargo-check 30m cargo check --manifest-path src-tauri/Cargo.toml --locked --all-targets
run_logged clippy 30m cargo clippy --manifest-path src-tauri/Cargo.toml --locked \
  --workspace --all-targets -- -D warnings
run_logged diff-check 1m git diff --check
run_logged protected-byte-check 1m git diff --exit-code -- src-tauri/module-arcs.txt package.json \
  package-lock.json Cargo.lock src-tauri/Cargo.toml .github/workflows
printf '%s\n' \
  plans/1854-muse-code-beta-preset/1860-rust-catalog.md \
  plans/1854-muse-code-beta-preset/1861-frontend-catalog-mirror.md \
  plans/1854-muse-code-beta-preset/1862-documentation-evidence.md \
  plans/1854-muse-code-beta-preset/1873-muse-auto-resume.md \
  plans/1854-muse-code-beta-preset/epic.md \
  src-tauri/resources/coding-agents/agents.default.json \
  src-tauri/src/config/coding_agents_catalog.rs src-tauri/src/web/commands.rs \
  src-tauri/tests/cli_project_registration.rs >"$CATALOG_LOG_DIR/scope.expected"
run_logged phase-scope 1m bash -c 'git diff --name-only "$PHASE_BASE_SHA" --'
run_logged phase-scope-equal 1m bash -c 'cmp -s "$CATALOG_LOG_DIR/scope.expected" "$CATALOG_LOG_DIR/phase-scope.log"'
prepare_catalog_manifest
printf 'catalog_executor=PASS head=%s artifacts=21 recipe_sha256=%s manifest_sha256=%s\n' "$CATALOG_EVIDENCE_SHA" "$CATALOG_RECIPE_SHA256" "$CATALOG_MANIFEST_SHA256"
~~~

`mapfile` plus reconstruction requires exactly eight newline-terminated `.meta`
fields and no tail byte. The frozen full-row digest binds all 21 ordered
label→timeout→`%q argv` relations. Run fresh precommit and at clean product HEAD.

Producer payload/verifier/index/shards and claimed receipts are untrusted. Trust is
the configured CLI, room store, frozen recipe/verifier/executor digests, and exact
tech-lead notifications. Producer `Queued:` output gates each local send but never
crosses the boundary; only lead ACKs and its independent execution enter the capsule. All
messages are canonical and under `256 * 1024`; raw shards are 147456 bytes. Each
`APPLY_PATCH_REQUIRED` is a hard tool pause: add exactly the printed spool as the
new target with `apply_patch`, never shell redirection/copy, then type its exact ACK. Both helpers explicitly reject failed preconditions, reads, hashes, and comparisons even in conditional/assignment callers; test the real helpers through both queues.

~~~bash
CATALOG_PEER='AgentsCommander_iac:room-9-ac-dev-team-v4/ac-tech-lead-v4'
CATALOG_MESSAGE_DIR="$(CDPATH= cd -- "$AGENTSCOMMANDER_ROOT/.." && pwd)/messaging"; test -d "$CATALOG_MESSAGE_DIR"
one() { local k="$1" f="$2"; test "$(rg -c "^${k}=" "$f")" -eq 1; sed -n "s/^${k}=//p" "$f"; }
canonical_queued_id() { local f="$1" count row; count="$(awk '{n+=gsub(/Queued:/,"")} END{print n+0}' "$f")" || return 1; test "$count" -eq 1 || return 1; row="$(rg -- '^Queued:' "$f")" || return 1; [[ "$row" =~ ^Queued:\ [0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] || return 1; printf '%s\n' "${row#Queued: }"; }
materialize_catalog_message() { local b="$1" spool="$2" want ack actual; local target="$CATALOG_MESSAGE_DIR/$b"; test -f "$spool" || return 1; test ! -e "$target" || return 1; want="$(set -o pipefail; sha256sum "$spool"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" || return 1; printf 'APPLY_PATCH_REQUIRED target=%s spool=%s sha256=%s\n' "$target" "$spool" "$want" || return 1; IFS= read -r ack || return 1; test "$ack" = "APPLY_PATCH_DONE $b $want" || return 1; test -f "$target" || return 1; cmp -s "$spool" "$target" || return 1; actual="$(set -o pipefail; sha256sum "$target"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" || return 1; test "$actual" = "$want" || return 1; }
queue_catalog_file() { local b="$1" spool="$2" r="$3" peers rc; materialize_catalog_message "$b" "$spool" || exit 1; peers="$("$AGENTSCOMMANDER_BINARY_PATH" list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT")" || exit 1; test "$(jq --arg p "$CATALOG_PEER" '[.[]|select(.name==$p)]|length' <<<"$peers")" -eq 1 || exit 1; set +e; timeout 2m "$AGENTSCOMMANDER_BINARY_PATH" send --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT" --to "$CATALOG_PEER" --send "$b" --mode wake >"$r" 2>&1; rc=$?; set -e; test "$rc" -eq 0 || exit 1; QUEUED_ID="$(canonical_queued_id "$r")" || exit 1; RECEIPT_SHA256="$(sha256sum "$r"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" || exit 1; RECEIPT_BASE64="$(base64 <"$r"|tr -d '\n')" || exit 1; export QUEUED_ID RECEIPT_SHA256 RECEIPT_BASE64; }
stamp="$(date -u +%Y%m%d-%H%M%S)"; CATALOG_PRESEAL_BASENAME="$stamp-room9-ac-dev-rust-v4-to-room9-ac-tech-lead-v4-catalog-preseal.md"; CATALOG_PRESEAL_MESSAGE="$CATALOG_MESSAGE_DIR/$CATALOG_PRESEAL_BASENAME"; CATALOG_PRESEAL_SPOOL="$CATALOG_LOG_DIR/$CATALOG_PRESEAL_BASENAME.body"; test ! -e "$CATALOG_PRESEAL_MESSAGE"; printf 'CATALOG_EVIDENCE_SHA=%s\nCATALOG_RECIPE_SHA256=%s\nCATALOG_MANIFEST_SHA256=%s\nCATALOG_VERIFIER_SHA256=%s\n' "$CATALOG_EVIDENCE_SHA" "$CATALOG_RECIPE_SHA256" "$CATALOG_MANIFEST_SHA256" "$CATALOG_VERIFIER_SHA256" >"$CATALOG_PRESEAL_SPOOL"; test "$(wc -c <"$CATALOG_PRESEAL_SPOOL")" -lt 262144
queue_catalog_file "$CATALOG_PRESEAL_BASENAME" "$CATALOG_PRESEAL_SPOOL" "$CATALOG_LOG_DIR/catalog-preseal.receipt"; CATALOG_PRESEAL_MESSAGE_SHA256="$(sha256sum "$CATALOG_PRESEAL_MESSAGE"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; export CATALOG_PRESEAL_BASENAME CATALOG_PRESEAL_MESSAGE_SHA256
: "${CATALOG_PRESEAL_TRUST_MESSAGE:?exact tech-lead notification path required}"; test "$(one CATALOG_PRESEAL_MESSAGE_SHA256 "$CATALOG_PRESEAL_TRUST_MESSAGE")" = "$CATALOG_PRESEAL_MESSAGE_SHA256"; test "$(one CATALOG_EVIDENCE_SHA "$CATALOG_PRESEAL_TRUST_MESSAGE")" = "$CATALOG_EVIDENCE_SHA"; test "$(one CATALOG_RECIPE_SHA256 "$CATALOG_PRESEAL_TRUST_MESSAGE")" = "$CATALOG_RECIPE_SHA256"; test "$(one CATALOG_MANIFEST_SHA256 "$CATALOG_PRESEAL_TRUST_MESSAGE")" = "$CATALOG_MANIFEST_SHA256"; test "$(one CATALOG_VERIFIER_SHA256 "$CATALOG_PRESEAL_TRUST_MESSAGE")" = "$CATALOG_VERIFIER_SHA256"
seal_catalog_bundle; split -b 147456 -d -a 4 "$CATALOG_LOG_DIR/catalog-evidence.tar.gz" "$CATALOG_LOG_DIR/catalog-chunk-"; mapfile -t parts < <(find "$CATALOG_LOG_DIR" -maxdepth 1 -type f -name 'catalog-chunk-[0-9][0-9][0-9][0-9]' -print | LC_ALL=C sort); CATALOG_CHUNK_COUNT="${#parts[@]}"; test "$CATALOG_CHUNK_COUNT" -gt 0; rows="$CATALOG_LOG_DIR/catalog-chunks.tsv"; test ! -e "$rows"; : >"$rows"; i=1
for part in "${parts[@]}"; do sequence="$(printf '%04d/%04d' "$i" "$CATALOG_CHUNK_COUNT")"; bytes="$(wc -c <"$part")"; sha="$(sha256sum "$part"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; b="$(date -u +%Y%m%d-%H%M%S)-room9-ac-dev-rust-v4-to-room9-ac-tech-lead-v4-catalog-${sequence%/*}.md"; msg="$CATALOG_MESSAGE_DIR/$b"; spool="$CATALOG_LOG_DIR/$b.body"; test ! -e "$msg"; { printf 'CATALOG_CHUNK_SEQUENCE=%s\nCATALOG_CHUNK_BYTES=%s\nCATALOG_CHUNK_SHA256=%s\nCATALOG_CHUNK_BASE64_BEGIN\n' "$sequence" "$bytes" "$sha"; base64 "$part"; printf 'CATALOG_CHUNK_BASE64_END\n'; } >"$spool"; test "$(wc -c <"$spool")" -lt 262144; queue_catalog_file "$b" "$spool" "$CATALOG_LOG_DIR/receipt-${sequence%/*}.log"; printf '%s\t%s\t%s\t%s\t%s\n' "$sequence" "$b" "$(sha256sum "$msg"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" "$bytes" "$sha" >>"$rows"; i=$((i+1)); done
CATALOG_CHUNK_ROWS_SHA256="$(sha256sum "$rows"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; CATALOG_INDEX_BASENAME="$(date -u +%Y%m%d-%H%M%S)-room9-ac-dev-rust-v4-to-room9-ac-tech-lead-v4-catalog-index.md"; CATALOG_INDEX_MESSAGE="$CATALOG_MESSAGE_DIR/$CATALOG_INDEX_BASENAME"; test ! -e "$CATALOG_INDEX_MESSAGE"
CATALOG_INDEX_SPOOL="$CATALOG_LOG_DIR/$CATALOG_INDEX_BASENAME.body"; { printf 'CATALOG_EVIDENCE_SHA=%s\nCATALOG_BUNDLE_BYTES=%s\nCATALOG_BUNDLE_SHA256=%s\nCATALOG_CHUNK_COUNT=%s\nCATALOG_CHUNK_ROWS_SHA256=%s\nCATALOG_CHUNKS_BEGIN\n' "$CATALOG_EVIDENCE_SHA" "$CATALOG_BUNDLE_BYTES" "$CATALOG_BUNDLE_SHA256" "$CATALOG_CHUNK_COUNT" "$CATALOG_CHUNK_ROWS_SHA256"; cat "$rows"; printf 'CATALOG_CHUNKS_END\n'; } >"$CATALOG_INDEX_SPOOL"; test "$(wc -c <"$CATALOG_INDEX_SPOOL")" -lt 262144; queue_catalog_file "$CATALOG_INDEX_BASENAME" "$CATALOG_INDEX_SPOOL" "$CATALOG_LOG_DIR/catalog-index.receipt"
CATALOG_INDEX_SHA256="$(sha256sum "$CATALOG_INDEX_MESSAGE"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; export CATALOG_INDEX_SHA256 CATALOG_CHUNK_ROWS_SHA256
CATALOG_FINAL_REQUEST_BASENAME="$(date -u +%Y%m%d-%H%M%S)-room9-ac-dev-rust-v4-to-room9-ac-tech-lead-v4-catalog-receipts.md"; CATALOG_FINAL_REQUEST_MESSAGE="$CATALOG_MESSAGE_DIR/$CATALOG_FINAL_REQUEST_BASENAME"; CATALOG_FINAL_REQUEST_SPOOL="$CATALOG_LOG_DIR/$CATALOG_FINAL_REQUEST_BASENAME.body"; test ! -e "$CATALOG_FINAL_REQUEST_MESSAGE"
printf 'CATALOG_PRESEAL_BASENAME=%s\nCATALOG_PRESEAL_MESSAGE_SHA256=%s\nCATALOG_INDEX_BASENAME=%s\nCATALOG_INDEX_SHA256=%s\nCATALOG_BUNDLE_SHA256=%s\nCATALOG_CHUNK_ROWS_SHA256=%s\n' "$CATALOG_PRESEAL_BASENAME" "$CATALOG_PRESEAL_MESSAGE_SHA256" "$CATALOG_INDEX_BASENAME" "$CATALOG_INDEX_SHA256" "$CATALOG_BUNDLE_SHA256" "$CATALOG_CHUNK_ROWS_SHA256" >"$CATALOG_FINAL_REQUEST_SPOOL"; CATALOG_FINAL_REQUEST_SHA256="$(sha256sum "$CATALOG_FINAL_REQUEST_SPOOL"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; queue_catalog_file "$CATALOG_FINAL_REQUEST_BASENAME" "$CATALOG_FINAL_REQUEST_SPOOL" "$CATALOG_LOG_DIR/catalog-final-request.receipt"; export CATALOG_FINAL_REQUEST_BASENAME CATALOG_FINAL_REQUEST_SHA256
~~~

### Receiver-independent lead producer

The tech lead runs this frozen block as `preseal`, then `final`. External inputs are
only the independently known head and exact paths from its received notifications;
the ordered `CATALOG_RECEIVED_NOTIFICATIONS` array is preseal, shards, index, request. Before producer-derived reads, require canonical basename, exact notification membership, and confined non-symlink single-link regular file; immutable room messages are assumed. Receiver paths require the same identity guard under the authenticated lead capsule. Reject before content access; test final callers with safe own-scratch canaries, including traversal, absent membership, symlink, hardlink, and special-file cases.

~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
mode="${1:?preseal|final}"; : "${AGENTSCOMMANDER_ROOT:?}"; CATALOG_MESSAGE_DIR="$(CDPATH= cd -- "$AGENTSCOMMANDER_ROOT/.." && pwd)/messaging"; mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"; test "$(realpath -e "$AGENTSCOMMANDER_ROOT/.evidence")" = "$(realpath -e "$AGENTSCOMMANDER_ROOT")/.evidence"; LEAD_DIR="$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1860-lead.XXXXXX")"; case "$(realpath -e "$LEAD_DIR")" in "$(realpath -e "$AGENTSCOMMANDER_ROOT")"/.evidence/*) ;; *) exit 1;; esac; fixed_recipe=1110F3D6660448313F3CA867A9D88B6C06EE936C6B075BBFBE9315767354700D; fixed_verifier=55C2536BAB01E65455FDCA44F901CB32462F97048DCBCA2CA8152A4EC207B13D; fixed_executor=FE2F12E2D44ABAA07F35FBBE51702B493E4E78AFDF43284D1047B0BFF502B64B; peer='AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4'
one() { local k="$1" f="$2"; test "$(rg -c "^${k}=" "$f")" -eq 1; sed -n "s/^${k}=//p" "$f"; }; catalog_file() { local p="$1" role="$2" b="${1##*/}"; [[ "$b" =~ ^[0-9]{8}-[0-9]{6}-room9-${role}-catalog-[a-z0-9-]+\.md$ ]] || return 1; test "$p" = "$CATALOG_MESSAGE_DIR/$b" || return 1; test ! -L "$CATALOG_MESSAGE_DIR" && test -d "$CATALOG_MESSAGE_DIR" && test ! -L "$p" && test -f "$p" || return 1; test "$(realpath -e -- "$p")" = "$CATALOG_MESSAGE_DIR/$b" || return 1; test "$(stat -c %h -- "$p")" = 1 || return 1; }
canonical_queued_id() { local f="$1" count row; count="$(awk '{n+=gsub(/Queued:/,"")} END{print n+0}' "$f")" || return 1; test "$count" -eq 1 || return 1; row="$(rg -- '^Queued:' "$f")" || return 1; [[ "$row" =~ ^Queued:\ [0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] || return 1; printf '%s\n' "${row#Queued: }"; }
lead_materialize() { local b="$1" spool="$2" want ack actual; local target="$CATALOG_MESSAGE_DIR/$b"; test -f "$spool" || return 1; test ! -e "$target" || return 1; want="$(set -o pipefail; sha256sum "$spool"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" || return 1; printf 'APPLY_PATCH_REQUIRED target=%s spool=%s sha256=%s\n' "$target" "$spool" "$want" || return 1; IFS= read -r ack || return 1; test "$ack" = "APPLY_PATCH_DONE $b $want" || return 1; test -f "$target" || return 1; cmp -s "$spool" "$target" || return 1; actual="$(set -o pipefail; sha256sum "$target"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" || return 1; test "$actual" = "$want" || return 1; }
lead_queue() { local b="$1" spool="$2" out="$3" peers rc; lead_materialize "$b" "$spool" || exit 1; peers="$("$AGENTSCOMMANDER_BINARY_PATH" list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT")" || exit 1; test "$(jq --arg p "$peer" '[.[]|select(.name==$p)]|length' <<<"$peers")" -eq 1 || exit 1; set +e; timeout 2m "$AGENTSCOMMANDER_BINARY_PATH" send --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT" --to "$peer" --send "$b" --mode wake >"$out" 2>&1; rc=$?; set -e; test "$rc" -eq 0 || exit 1; LEAD_QUEUED_ID="$(canonical_queued_id "$out")" || exit 1; LEAD_RECEIPT_SHA256="$(sha256sum "$out"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" || exit 1; LEAD_RECEIPT_BASE64="$(base64 <"$out"|tr -d '\n')" || exit 1; }
if test "$mode" = preseal; then : "${CATALOG_PRESEAL_NOTIFICATION:?}" "${EXPECTED_CATALOG_EVIDENCE_SHA:?}"; p="$CATALOG_PRESEAL_NOTIFICATION"; catalog_file "$p" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; mapfile -t r <"$p"; test "${#r[@]}" -eq 4; cmp -s "$p" <(printf 'CATALOG_EVIDENCE_SHA=%s\nCATALOG_RECIPE_SHA256=%s\nCATALOG_MANIFEST_SHA256=%s\nCATALOG_VERIFIER_SHA256=%s\n' "$(one CATALOG_EVIDENCE_SHA "$p")" "$(one CATALOG_RECIPE_SHA256 "$p")" "$(one CATALOG_MANIFEST_SHA256 "$p")" "$(one CATALOG_VERIFIER_SHA256 "$p")"); test "$(one CATALOG_EVIDENCE_SHA "$p")" = "$EXPECTED_CATALOG_EVIDENCE_SHA"; test "$(one CATALOG_RECIPE_SHA256 "$p")" = "$fixed_recipe"; test "$(one CATALOG_VERIFIER_SHA256 "$p")" = "$fixed_verifier"; b="$(date -u +%Y%m%d-%H%M%S)-room9-ac-tech-lead-v4-to-room9-ac-dev-rust-v4-catalog-preseal-trust.md"; out="$CATALOG_MESSAGE_DIR/$b"; spool="$LEAD_DIR/$b.body"; test ! -e "$out"; printf 'CATALOG_PRESEAL_MESSAGE_SHA256=%s\nCATALOG_EVIDENCE_SHA=%s\nCATALOG_RECIPE_SHA256=%s\nCATALOG_MANIFEST_SHA256=%s\nCATALOG_VERIFIER_SHA256=%s\n' "$(sha256sum "$p"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" "$EXPECTED_CATALOG_EVIDENCE_SHA" "$fixed_recipe" "$(one CATALOG_MANIFEST_SHA256 "$p")" "$fixed_verifier" >"$spool"; lead_queue "$b" "$spool" "$LEAD_DIR/preseal-trust.receipt"; printf 'LEAD_PRESEAL_TRUST_SENT_PATH=%s\n' "$out"; exit 0; fi
test "$mode" = final; : "${CATALOG_FINAL_REQUEST_NOTIFICATION:?}" "${LEAD_PRESEAL_TRUST_SENT_PATH:?}" "${EXPECTED_PHASE_BASE_SHA:?}"; declare -p CATALOG_RECEIVED_NOTIFICATIONS >/dev/null; trust="$LEAD_PRESEAL_TRUST_SENT_PATH"; request="$CATALOG_FINAL_REQUEST_NOTIFICATION"; catalog_file "$request" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; catalog_file "$trust" ac-tech-lead-v4-to-room9-ac-dev-rust-v4 || exit 1; prebase="$(one CATALOG_PRESEAL_BASENAME "$request")"; indexbase="$(one CATALOG_INDEX_BASENAME "$request")"; pre="$CATALOG_MESSAGE_DIR/$prebase"; index="$CATALOG_MESSAGE_DIR/$indexbase";
mapfile -t rr <"$request"; test "${#rr[@]}" -eq 6; cmp -s "$request" <(printf 'CATALOG_PRESEAL_BASENAME=%s\nCATALOG_PRESEAL_MESSAGE_SHA256=%s\nCATALOG_INDEX_BASENAME=%s\nCATALOG_INDEX_SHA256=%s\nCATALOG_BUNDLE_SHA256=%s\nCATALOG_CHUNK_ROWS_SHA256=%s\n' "$prebase" "$(one CATALOG_PRESEAL_MESSAGE_SHA256 "$request")" "$indexbase" "$(one CATALOG_INDEX_SHA256 "$request")" "$(one CATALOG_BUNDLE_SHA256 "$request")" "$(one CATALOG_CHUNK_ROWS_SHA256 "$request")"); catalog_file "$pre" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; catalog_file "$index" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; for required in "$pre" "$index" "$request"; do matches=0; for notified in "${CATALOG_RECEIVED_NOTIFICATIONS[@]}"; do if test "$notified" = "$required"; then matches=$((matches+1)); fi; done; test "$matches" -eq 1 || exit 1; done; prehash="$(sha256sum "$pre"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; indexhash="$(sha256sum "$index"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; requesthash="$(sha256sum "$request"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; test "$(one CATALOG_PRESEAL_MESSAGE_SHA256 "$request")" = "$prehash"; test "$(one CATALOG_INDEX_SHA256 "$request")" = "$indexhash"; test "$(one CATALOG_PRESEAL_MESSAGE_SHA256 "$trust")" = "$prehash"; test "$(one CATALOG_RECIPE_SHA256 "$trust")" = "$fixed_recipe"; test "$(one CATALOG_VERIFIER_SHA256 "$trust")" = "$fixed_verifier"
rows="$LEAD_DIR/chunks.tsv"; sed -n '/^CATALOG_CHUNKS_BEGIN$/,/^CATALOG_CHUNKS_END$/{/^CATALOG_CHUNKS_/d;p}' "$index" >"$rows"; count="$(one CATALOG_CHUNK_COUNT "$index")"; test "$(wc -l <"$rows")" -eq "$count"; awk -F '\t' 'NF!=5{exit 1}' "$rows"; test "$(sha256sum "$rows"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$(one CATALOG_CHUNK_ROWS_SHA256 "$request")"; sources="$LEAD_DIR/sources"; { printf '%s\t%s\n' "$prebase" "$prehash"; cut -f2,3 "$rows"; printf '%s\t%s\n%s\t%s\n' "$indexbase" "$indexhash" "$(basename "$request")" "$requesthash"; } >"$sources"; test "${#CATALOG_RECEIVED_NOTIFICATIONS[@]}" -eq "$(wc -l <"$sources")"
acks="$LEAD_DIR/acks.tsv"; : >"$acks"; i=1; while IFS=$'\t' read -r sourcebase sourcehash; do src="${CATALOG_RECEIVED_NOTIFICATIONS[$((i-1))]}"; catalog_file "$src" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; test "$(basename "$src")" = "$sourcebase"; test "$(sha256sum "$src"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$sourcehash"; ackbase="$(date -u +%Y%m%d-%H%M%S)-room9-ac-tech-lead-v4-to-room9-ac-dev-rust-v4-catalog-ack-$(printf '%04d' "$i").md"; ack="$CATALOG_MESSAGE_DIR/$ackbase"; spool="$LEAD_DIR/$ackbase.body"; test ! -e "$ack"; printf 'ACK_SOURCE_BASENAME=%s\nACK_SOURCE_SHA256=%s\n' "$sourcebase" "$sourcehash" >"$spool"; lead_queue "$ackbase" "$spool" "$LEAD_DIR/ack-$(printf '%04d' "$i").receipt"; printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$sourcebase" "$sourcehash" "$ackbase" "$(sha256sum "$ack"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" "$LEAD_QUEUED_ID" "$LEAD_RECEIPT_SHA256" "$LEAD_RECEIPT_BASE64" >>"$acks"; i=$((i+1)); done <"$sources"
CATALOG_PLAN=plans/1854-muse-code-beta-preset/1860-rust-catalog.md; executor="$LEAD_DIR/catalog-executor.sh"; awk '/^~~~bash$/{b++;next} /^~~~$/{if(b==2) exit} b==2{print}' "$CATALOG_PLAN" >"$executor"; test "$(sha256sum "$executor"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$fixed_executor"; expected_head="$(one CATALOG_EVIDENCE_SHA "$trust")"; LEAD_CANDIDATE_SHA="$expected_head"; bind_checkout; test "$(git rev-parse HEAD)" = "$expected_head"; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; mkdir "$LEAD_DIR/execution"; CATALOG_LOG_DIR="$LEAD_DIR/execution" PHASE_BASE_SHA="$EXPECTED_PHASE_BASE_SHA" env -u BASH_ENV -u ENV bash --noprofile --norc "$executor" </dev/null 2>&1 | tee "$LEAD_DIR/executor.log"; execution_status=("${PIPESTATUS[@]}"); for value in "${execution_status[@]}"; do test "$value" -eq 0 || exit 1; done; assert_checkout; mapfile -t execution_rows < <(rg -- '^catalog_executor=PASS ' "$LEAD_DIR/executor.log"); test "${#execution_rows[@]}" -eq 1; execution_attestation="${execution_rows[0]}"; printf '%s\n' "$execution_attestation" | rg -x -- "catalog_executor=PASS head=$expected_head artifacts=21 recipe_sha256=$fixed_recipe manifest_sha256=[0-9A-F]{64}" >/dev/null; test "$(git rev-parse HEAD)" = "$expected_head"; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; assert_checkout; candidate="$LEAD_DIR/final-trust.candidate"; { printf 'EXPECTED_CATALOG_EVIDENCE_SHA=%s\nEXPECTED_CATALOG_RECIPE_SHA256=%s\nEXPECTED_CATALOG_MANIFEST_SHA256=%s\nEXPECTED_CATALOG_VERIFIER_SHA256=%s\nEXPECTED_CATALOG_EXECUTOR_SHA256=%s\nLEAD_CATALOG_EXECUTION_LOG_SHA256=%s\nLEAD_CATALOG_EXECUTION_ATTESTATION=%s\nEXPECTED_CATALOG_PRESEAL_BASENAME=%s\nEXPECTED_CATALOG_PRESEAL_MESSAGE_SHA256=%s\nEXPECTED_CATALOG_INDEX_BASENAME=%s\nEXPECTED_CATALOG_INDEX_SHA256=%s\nEXPECTED_CATALOG_CHUNK_ROWS_SHA256=%s\nEXPECTED_CATALOG_BUNDLE_SHA256=%s\nEXPECTED_CATALOG_FINAL_REQUEST_BASENAME=%s\nEXPECTED_CATALOG_FINAL_REQUEST_SHA256=%s\nLEAD_ACKS_BEGIN\n' "$expected_head" "$fixed_recipe" "$(one CATALOG_MANIFEST_SHA256 "$trust")" "$fixed_verifier" "$fixed_executor" "$(sha256sum "$LEAD_DIR/executor.log"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" "$execution_attestation" "$prebase" "$prehash" "$indexbase" "$indexhash" "$(one CATALOG_CHUNK_ROWS_SHA256 "$request")" "$(one CATALOG_BUNDLE_SHA256 "$request")" "$(basename "$request")" "$requesthash"; sed -n p "$acks"; printf 'LEAD_ACKS_END\n'; } >"$candidate"
CATALOG_PLAN=plans/1854-muse-code-beta-preset/1860-rust-catalog.md; consumer="$LEAD_DIR/catalog-consumer.sh"; awk '/^~~~bash$/{b++;next} /^~~~$/{if(b==5) exit} b==5{print}' "$CATALOG_PLAN" >"$consumer"; bash -n "$consumer"; CATALOG_FINAL_TRUST_MESSAGE="$candidate" bash "$consumer" | tee "$LEAD_DIR/receiver.log"; rg -x 'receiver_verifier=PASS artifacts=21 sidecar_entries=45 manifest_sha256=[0-9a-f]{64}' "$LEAD_DIR/receiver.log"; finalbase="$(date -u +%Y%m%d-%H%M%S)-room9-ac-tech-lead-v4-to-room9-ac-dev-rust-v4-catalog-final-trust.md"; assert_checkout; lead_queue "$finalbase" "$candidate" "$LEAD_DIR/final-trust.receipt"; assert_checkout
~~~

The final receiver trusts only the exact lead notification and its ACK receipts:

~~~bash
set -euo pipefail; : "${CATALOG_FINAL_TRUST_MESSAGE:?exact tech-lead notification path required}" "${AGENTSCOMMANDER_ROOT:?}"; CATALOG_MESSAGE_DIR="$(CDPATH= cd -- "$AGENTSCOMMANDER_ROOT/.." && pwd)/messaging"; one() { local k="$1" f="$2"; test "$(rg -c "^${k}=" "$f")" -eq 1; sed -n "s/^${k}=//p" "$f"; }
canonical_queued_id() { local f="$1" count row; count="$(awk '{n+=gsub(/Queued:/,"")} END{print n+0}' "$f")" || return 1; test "$count" -eq 1 || return 1; row="$(rg -- '^Queued:' "$f")" || return 1; [[ "$row" =~ ^Queued:\ [0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] || return 1; printf '%s\n' "${row#Queued: }"; }; catalog_file() { local p="$1" role="$2" b="${1##*/}"; [[ "$b" =~ ^[0-9]{8}-[0-9]{6}-room9-${role}-catalog-[a-z0-9-]+\.md$ ]] || return 1; test "$p" = "$CATALOG_MESSAGE_DIR/$b" || return 1; test ! -L "$CATALOG_MESSAGE_DIR" && test -d "$CATALOG_MESSAGE_DIR" && test ! -L "$p" && test -f "$p" || return 1; test "$(realpath -e -- "$p")" = "$CATALOG_MESSAGE_DIR/$b" || return 1; test "$(stat -c %h -- "$p")" = 1 || return 1; }
CATALOG_REQUIRED_LABELS=(rustfmt catalog catalog-count web web-count cli-catalog cli-catalog-count resolver-default resolver-default-count resolver-explicit resolver-explicit-count lib rust-lib-count cli-full cli-full-count cargo-check clippy diff-check protected-byte-check phase-scope phase-scope-equal)
head="$(one EXPECTED_CATALOG_EVIDENCE_SHA "$CATALOG_FINAL_TRUST_MESSAGE")"; recipe="$(one EXPECTED_CATALOG_RECIPE_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; manifest="$(one EXPECTED_CATALOG_MANIFEST_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; verifier="$(one EXPECTED_CATALOG_VERIFIER_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; executor="$(one EXPECTED_CATALOG_EXECUTOR_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; execution="$(one LEAD_CATALOG_EXECUTION_ATTESTATION "$CATALOG_FINAL_TRUST_MESSAGE")"; prebase="$(one EXPECTED_CATALOG_PRESEAL_BASENAME "$CATALOG_FINAL_TRUST_MESSAGE")"; prehash="$(one EXPECTED_CATALOG_PRESEAL_MESSAGE_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; indexbase="$(one EXPECTED_CATALOG_INDEX_BASENAME "$CATALOG_FINAL_TRUST_MESSAGE")"; indexhash="$(one EXPECTED_CATALOG_INDEX_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; chunkrows="$(one EXPECTED_CATALOG_CHUNK_ROWS_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; trusted_bundle="$(one EXPECTED_CATALOG_BUNDLE_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; requestbase="$(one EXPECTED_CATALOG_FINAL_REQUEST_BASENAME "$CATALOG_FINAL_TRUST_MESSAGE")"; requesthash="$(one EXPECTED_CATALOG_FINAL_REQUEST_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")"; test "$recipe" = 1110F3D6660448313F3CA867A9D88B6C06EE936C6B075BBFBE9315767354700D; test "$verifier" = 55C2536BAB01E65455FDCA44F901CB32462F97048DCBCA2CA8152A4EC207B13D; test "$executor" = FE2F12E2D44ABAA07F35FBBE51702B493E4E78AFDF43284D1047B0BFF502B64B; printf '%s\n' "$execution" | rg -x -- "catalog_executor=PASS head=$head artifacts=21 recipe_sha256=$recipe manifest_sha256=[0-9A-F]{64}" >/dev/null; printf '%s\n' "$(one LEAD_CATALOG_EXECUTION_LOG_SHA256 "$CATALOG_FINAL_TRUST_MESSAGE")" | rg -x '[0-9A-F]{64}' >/dev/null
pre="$CATALOG_MESSAGE_DIR/$prebase"; index="$CATALOG_MESSAGE_DIR/$indexbase"; request="$CATALOG_MESSAGE_DIR/$requestbase"; catalog_file "$pre" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; catalog_file "$index" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; catalog_file "$request" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; test "$(sha256sum "$pre"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$prehash"; test "$(sha256sum "$index"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$indexhash"; test "$(sha256sum "$request"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$requesthash"; test "$(one CATALOG_EVIDENCE_SHA "$pre")" = "$head"; test "$(one CATALOG_RECIPE_SHA256 "$pre")" = "$recipe"; test "$(one CATALOG_MANIFEST_SHA256 "$pre")" = "$manifest"; test "$(one CATALOG_VERIFIER_SHA256 "$pre")" = "$verifier"; mapfile -t qr <"$request"; test "${#qr[@]}" -eq 6; cmp -s "$request" <(printf 'CATALOG_PRESEAL_BASENAME=%s\nCATALOG_PRESEAL_MESSAGE_SHA256=%s\nCATALOG_INDEX_BASENAME=%s\nCATALOG_INDEX_SHA256=%s\nCATALOG_BUNDLE_SHA256=%s\nCATALOG_CHUNK_ROWS_SHA256=%s\n' "$prebase" "$prehash" "$indexbase" "$indexhash" "$trusted_bundle" "$chunkrows")
mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"; test "$(realpath -e "$AGENTSCOMMANDER_ROOT/.evidence")" = "$(realpath -e "$AGENTSCOMMANDER_ROOT")/.evidence"; recv="$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1860-receive.XXXXXX")"; case "$(realpath -e "$recv")" in "$(realpath -e "$AGENTSCOMMANDER_ROOT")"/.evidence/*) ;; *) exit 1;; esac; rows="$recv/chunks.tsv"; sed -n '/^CATALOG_CHUNKS_BEGIN$/,/^CATALOG_CHUNKS_END$/{/^CATALOG_CHUNKS_/d;p}' "$index" >"$rows"; count="$(one CATALOG_CHUNK_COUNT "$index")"; test "$(wc -l <"$rows")" -eq "$count"; awk -F '\t' 'NF!=5{exit 1}' "$rows"; test "$(sha256sum "$rows"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$chunkrows"; test "$(cut -f2 "$rows"|sort -u|wc -l)" -eq "$count"; bundle_sha="$(one CATALOG_BUNDLE_SHA256 "$index")"; bundle_bytes="$(one CATALOG_BUNDLE_BYTES "$index")"; test "$bundle_sha" = "$trusted_bundle"; { printf 'CATALOG_EVIDENCE_SHA=%s\nCATALOG_BUNDLE_BYTES=%s\nCATALOG_BUNDLE_SHA256=%s\nCATALOG_CHUNK_COUNT=%s\nCATALOG_CHUNK_ROWS_SHA256=%s\nCATALOG_CHUNKS_BEGIN\n' "$head" "$bundle_bytes" "$bundle_sha" "$count" "$chunkrows"; sed -n p "$rows"; printf 'CATALOG_CHUNKS_END\n'; } >"$recv/index.canonical"; cmp -s "$index" "$recv/index.canonical"
{ printf '%s\t%s\n' "$prebase" "$prehash"; cut -f2,3 "$rows"; printf '%s\t%s\n%s\t%s\n' "$indexbase" "$indexhash" "$requestbase" "$requesthash"; } >"$recv/sources"; test "$(rg -c '^LEAD_ACKS_BEGIN$' "$CATALOG_FINAL_TRUST_MESSAGE")" -eq 1; test "$(rg -c '^LEAD_ACKS_END$' "$CATALOG_FINAL_TRUST_MESSAGE")" -eq 1; sed -n '/^LEAD_ACKS_BEGIN$/,/^LEAD_ACKS_END$/{/^LEAD_ACKS_/d;p}' "$CATALOG_FINAL_TRUST_MESSAGE" >"$recv/acks"; test "$(wc -l <"$recv/acks")" -eq "$(wc -l <"$recv/sources")"; awk -F '\t' 'NF!=7{exit 1}' "$recv/acks"; test "$(cut -f3 "$recv/acks"|sort -u|wc -l)" -eq "$(wc -l <"$recv/acks")"; test "$(cut -f5 "$recv/acks"|sort -u|wc -l)" -eq "$(wc -l <"$recv/acks")"; i=1; while IFS=$'\t' read -r sourcebase sourcehash; do IFS=$'\t' read -r asource ahash ackbase ackhash queued_id receipt_sha receipt_b64 < <(sed -n "${i}p" "$recv/acks"); test "$asource" = "$sourcebase"; test "$ahash" = "$sourcehash"; printf '%s\n' "$ackbase"|rg -x '[0-9]{8}-[0-9]{6}-room9-ac-tech-lead-v4-to-room9-ac-dev-rust-v4-catalog-ack-[0-9]{4}\.md' >/dev/null; ack="$CATALOG_MESSAGE_DIR/$ackbase"; catalog_file "$ack" ac-tech-lead-v4-to-room9-ac-dev-rust-v4 || exit 1; test "$(sha256sum "$ack"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$ackhash"; cmp -s "$ack" <(printf 'ACK_SOURCE_BASENAME=%s\nACK_SOURCE_SHA256=%s\n' "$sourcebase" "$sourcehash"); receipt="$recv/lead.receipt"; printf '%s' "$receipt_b64"|base64 --decode >"$receipt"; test "$(sha256sum "$receipt"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$receipt_sha"; got="$(canonical_queued_id "$receipt")" || exit 1; test "$got" = "$queued_id"; i=$((i+1)); done <"$recv/sources"
i=1; while IFS=$'\t' read -r sequence basename message_sha chunk_bytes chunk_sha; do test "$sequence" = "$(printf '%04d/%04d' "$i" "$count")"; msg="$CATALOG_MESSAGE_DIR/$basename"; catalog_file "$msg" ac-dev-rust-v4-to-room9-ac-tech-lead-v4 || exit 1; test "$(sha256sum "$msg"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$message_sha"; part="$recv/part-${sequence%/*}"; sed -n '/^CATALOG_CHUNK_BASE64_BEGIN$/,/^CATALOG_CHUNK_BASE64_END$/{/^CATALOG_CHUNK_BASE64_/d;p}' "$msg"|base64 --decode >"$part"; test "$(one CATALOG_CHUNK_SEQUENCE "$msg")" = "$sequence"; test "$(wc -c <"$part")" -eq "$chunk_bytes"; test "$(sha256sum "$part"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$chunk_sha"; { printf 'CATALOG_CHUNK_SEQUENCE=%s\nCATALOG_CHUNK_BYTES=%s\nCATALOG_CHUNK_SHA256=%s\nCATALOG_CHUNK_BASE64_BEGIN\n' "$sequence" "$chunk_bytes" "$chunk_sha"; base64 "$part"; printf 'CATALOG_CHUNK_BASE64_END\n'; } >"$recv/canonical-message"; cmp -s "$msg" "$recv/canonical-message"; i=$((i+1)); done <"$rows"; test "$((i-1))" -eq "$count"
bundle="$recv/catalog-evidence.tar.gz"; cat "$recv"/part-* >"$bundle"; test "$(wc -c <"$bundle")" -eq "$bundle_bytes"; test "$(sha256sum "$bundle"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$bundle_sha"
{ printf '%s\n' catalog-invocations.ledger catalog-evidence.manifest catalog-evidence.verify.sh catalog-evidence.payload.sha256; for label in "${CATALOG_REQUIRED_LABELS[@]}"; do printf '%s.log\n%s.meta\n' "$label" "$label"; done; } >"$recv/members.expected"; tar -tzf "$bundle" >"$recv/members.actual"; cmp -s "$recv/members.expected" "$recv/members.actual"; LC_ALL=C tar --numeric-owner -tvzf "$bundle" >"$recv/types"; test "$(wc -l <"$recv/types")" -eq 46; awk 'substr($1,1,1)!="-"{exit 1}' "$recv/types"; rg -v '^[a-z0-9.-]+$' "$recv/members.actual" && exit 1 || true
mkdir "$recv/payload"; tar --no-same-owner --no-same-permissions -xzf "$bundle" -C "$recv/payload"; test "$(sha256sum "$recv/payload/catalog-evidence.manifest"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$manifest"; test "$(sha256sum "$recv/payload/catalog-evidence.verify.sh"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$verifier"; test "$(sha256sum "$recv/payload/catalog-invocations.ledger"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$recipe"; timeout 5m bash "$recv/payload/catalog-evidence.verify.sh" </dev/null >"$recv/verifier.log"; rg -x 'receiver_verifier=PASS artifacts=21 sidecar_entries=45 manifest_sha256=[0-9a-f]{64}' "$recv/verifier.log"; test "$(sha256sum "$bundle"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')" = "$bundle_sha"
~~~

The receiver harness produces one PASS, then clones the fixture and requires
nonzero for eighth-newline tail plus sender rehash; replaced `run_logged`, 21
`FORGED` logs/metas and no commands without the independent lead execution;
attacker verifier/member; changed index/shard; producer receipt; forged lead ACK/capsule;
zero/two `Queued:` occurrences, same-line duplicate, receipt-line suffix; and extra, missing, duplicate, reordered,
renamed, mutated or symlink archive member. Producer bytes cannot mint lead trust.

Zero selection, timeout, unexplained debt, false PASS, private-only evidence, or
changed lock/manifest/workflow/arc bytes blocks #1873.

## Dependency-cycle and layering gate

Planned new module arcs: zero. Planned removed module arcs: zero. The only
production byte change is embedded JSON; Rust changes are tests using existing
imports and module relationships. No role or dependency direction changes.

Require byte-identical module-arcs.txt and no `use`/`mod`/production Rust hunk.
Any reference/arc drift stops and sends clean base/candidate SHAs, diff and output
to the tech lead's `rust-levelization-run`. It must record pre/post cyclicSccs,
sorted SCC sets, all pair deltas/crossings, regenerated arc bytes and guards.
Dirty/missing evidence, exit 3, changed SCCs, crossing or arc drift blocks.

## Commit, CI, and handoff

After the immutable five-plan bootstrap, commit only files 6–9. The final
branch delta against PHASE_BASE_SHA must be exactly all nine listed paths, with
a clean tree and unchanged plan hashes.

Retain the lead candidate/ref/HEAD-and-branch-history anchors through final state, before/after CI lookup and delivery; call assert_checkout at each boundary. Bind CI and delivery to LEAD_CANDIDATE_SHA. A new candidate discards all evidence. Persistent movement and checkout/detach H1→H2→H1 during any child must fail.
At exact #1860 PR head require test-debt; Windows/Linux/macOS Rust check/clippy
and configured tests; rust-fmt; four portable-terminal legs; Windows release CLI
smoke; frontend regression; validate-branch-name; and lockfile-drift passing
without regeneration. bundle-validation/version-sync are path-inapplicable.
Re-derive on relevant drift; another SHA, waiver, bypass or unknown skip fails.

The accepted handoff records PHASE_BASE_SHA, PLAN_BOOTSTRAP_COMMIT, product
commit, exact PR-head SHA, five unchanged plan digests, exact nine-file delta,
producer diagnostics, lead clean-shell executor hash/attestation, exact-head CI, and no arc drift.
#1873 may branch only from main after this accepted merge.

## Acceptance criteria

1. Exactly five frozen plan artifacts are committed first with frozen bytes.
2. Muse is the exact eighth and last embedded catalog entry.
3. Fresh/default catalogs include Muse; valid existing catalogs are preserved
   byte-for-byte and remain Muse-free.
4. Instructions/config-seed fields are omitted, no seed is created, and update
   commands stay empty.
5. All Rust count consumers and exact-field tests pass with nonzero selection.
6. The phase has exactly nine files, zero new module arcs, unchanged dependency
   inputs, exact-head CI, and a green main handoff to #1873.

Status: READY_FOR_IMPLEMENTATION
