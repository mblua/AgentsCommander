# #1929 — FINAL CORRECTION: execute the compiled release harness

Status: READY_FOR_IMPLEMENTATION (Architect authorship, round 4, 2026-09-11).
This certifies the specification, not permission to implement. Dev and Grinch must
review these exact bytes; coordinator dispatch follows their digest-bound consensus.
Diagnostic completion and green diagnostic CI do not satisfy correction acceptance.

## 1. Identity, outcome and evidence

Repo: D:/0_repos/AgentsCommander_iac/.ac/room-17-ac-dev-team-v4/repo-AgentsCommander
Issue: https://github.com/mblua/AgentsCommander/issues/1929 — OPEN verified this round.
Branch: ci/1929-split-focused-release-build.
Immutable planning HEAD: c95d4cbe4eb9fc8f57aa0d31fa01441af37ecbde.
Recorded merged-main baseline: d8816883676ed2ebf9e700152944598b1de47b52.
Entry: empty index; sole working edit was the completed diagnostic plan.
The actual implementation base must be captured after bounded synchronization.

Outcome on Windows/Linux/macOS: compilation finishes below 40 minutes; focused
release assertions finish below 20 minutes with no application compilation in that
step. Preserve all five-name, positive-count/zero-failure, exit and Windows-negative
guards and every other regression job. Remove temporary Linux fingerprint tracing.
This bypasses Cargo freshness assessment during assertions; it does not repair the
underlying macro/Cargo freshness interaction.

Authoritative retained evidence: room-shared/1929-implementation/diagnostic-round2/.
Read architect-correction-proposal.md and dev-correction-feasibility.md for provenance;
all implementer decisions are inlined below. Proposal SHA256:
0427667B3D1E66595C0018AC167F9F5A09BC5251C705FE0E3FF2F11F0C40F35C.
Linux causal proof: OUT_DIR input newer than dep-info by 2.748976061 seconds,
FsStatusOutdated/StaleItem/ChangedFile; second application compile took 5m46.
Decoded committed icon bytes match that input's BLAKE3 name. Cached bytes can remain
unchanged; neither unconditional rewriting nor a syscall writer nor cross-OS cause
equivalence was proved. Historical Cargo version was not printed and stays unknown.
Diagnostic runs 34627262899/34627263172 passed all 15 observed checks/14 regression
jobs. Prior Windows retry/deadline failure is NOT REPRODUCED at diagnostic HEAD,
not a fixed or proven flaky test. Preserve inherited-windows-comparison.md.

Graph root verified: this room's repo, ready, 23242 nodes/154274 edges at planning
HEAD. Workflow/configuration coverage was metadata_changed or excluded; inspected
those files directly. Package agentscommander-new has lib agentscommander_lib with
lib/cdylib/staticlib kinds. Real outputs use workspace target/release/deps.
The three jobs already install Node 22; keep Rust stable action, npm 11.6.2,
npm ci, lockfiles, frontend build, system packages and cache settings unchanged.

Threat model: routine trusted GitHub CI correction with a small custom artifact
selector/launcher. Its plausible risks require protocol/failure fixtures and real
native CI. Host binary attestation, signing, hostile PATH, transactional filesystem
and bespoke process-tree controls are non-applicable. No release/install authority.

## 2. Full scope and partition

Score: remaining correction 33/100, raw 71; Full because of the new job-local
producer/consumer protocol and cross-platform launch behavior.
PARTITION: 1 phase. Class: design-bearing. Owner: ac-dev-rust-v4.
Issue/branch remain #1929 and the existing branch above, as explicitly directed.
No child issue, new branch, shared epic branch, or independent phase PR is created.
One owner, two tracked files, no IPC/CLI/persistence/schema contract, and no useful
green intermediate cut between publishing and consuming the harness. Splitting
would duplicate protocol work; the >10-file trigger is absent. No parallel phase.

Complete tracked inventory:
1. .github/workflows/pr-regression-gates.yml:
   - Three existing build steps named "IS #1850 focused release build", in
     rust-regression, rust-regression-linux, rust-regression-macos.
   - Their three existing focused release consumer steps; rename each to
     "IS #1850 focused release assertions (prebuilt harness)".
   - Three preceding #1929 comments: explain direct artifact launch.
   - Both Linux CARGO_LOG env mappings: remove the four diagnostic lines.
   - Three NEW upload steps, one immediately after each consumer, specified below.
2. plans/1929-split-focused-release-build.md: this final specification, committed
   alongside the correction, unlike the excluded diagnostic-plan working edit.

No committed helper/fixture, other workflow, product/test/module, dependency,
lockfile, icon, build-script or cache-policy change. Inline Python/Node belongs
only in the six listed steps; validation fixtures live in room-shared evidence.
The coordinator's explicit repository-canonical instruction overrides the default
shared-plan location and phase-branch rules for this single continuing correction.
This file is the sole working plan. Unchanged historical bytes were copied to
D:/0_repos/AgentsCommander_iac/.ac/plans/issue-1929/round2-diagnostic-historical.md,
SHA256 516B4827551AA78E221FBD21B68DA58E0C899D1A407B84F9FF147241C6A74FAD.
round1-historical.md remains historical. No competing shared phase set exists.

## 3. Build producer: exact contract

Keep shell bash, src-tauri cwd, set -euo pipefail, timeout-minutes: 40.
Give each build step id: issue1850_release_build (job-local reuse is intentional).
Use Node 22 built-ins and inline Python standard library only; no install/download.
Python is used solely to parse Cargo configuration correctly, not to launch tests:
invoke python on Windows and python3 elsewhere, require >=3.11 and tomllib;
record resolved Python/Node/Cargo/rustc versions. Missing capability fails preflight.

First use Node fs.mkdtempSync below native process.env.RUNNER_TEMP with prefix
issue1850-release-<GITHUB_RUN_ID>-<GITHUB_RUN_ATTEMPT>-<GITHUB_JOB>-.
Use native Node paths, never infer Windows paths from Bash pwd.
Publish only diagnostics-dir=<absolute directory> immediately to GITHUB_OUTPUT.
Pass paths between inline commands through quoted environment variables, not
expression interpolation into shell/JavaScript source. Reject newline paths.
Create context.json and cargo-messages.jsonl there; manifest.json must not exist
until selection succeeds. No reuse, fixed old path, cache restore or shared manifest.

Native preflight runs IN CI only, before compilation. Inline Python tomllib reads
the active config/config.toml at src-tauri and each ancestor .cargo directory plus
resolved CARGO_HOME (platform home/.cargo when absent). If both exist, config wins.
Deduplicate resolved filenames. Record paths and relevant settings, not full configs
or secrets. Fail on unreadable/malformed active config. Reject any include key,
any nonempty [env] table, any build.target, build.rustc, or any target.*.runner
entry (including cfg tables). These are explicitly unsupported native-job settings;
report the exact key/path and stop, never silently bypass a runner or override it.
Reject nonempty CARGO_BUILD_TARGET, CARGO_BUILD_RUSTC, RUSTC and any
CARGO_TARGET_*_RUNNER environment value. No extra --config, --target or aliases
are introduced. Record RUSTUP_TOOLCHAIN, target-dir/build-dir controls and
compiler-wrapper/flag presence without clearing them or dumping arbitrary env.
This bounded check avoids implementing Cargo's merge/cfg/include evaluator.
Even an inactive runner entry is rejected intentionally and reported as unsupported;
supporting such configuration needs a revised plan, not an implementer workaround.

From the same cwd/environment run cargo --version, rustc -vV,
rustc --print target-libdir, and cargo metadata --locked --no-deps --format-version=1.
Require all exits zero. Retain metadata JSON; resolve exactly one package by canonical
manifest_path == src-tauri/Cargo.toml, then its exact package id and lib target.
Require rustc host OS/architecture matches native Node process.platform/process.arch
(win32/x64 -> x86_64-pc-windows-msvc; linux/x64 -> x86_64-unknown-linux-gnu;
darwin/arm64 -> aarch64-apple-darwin; darwin/x64 -> x86_64-apple-darwin).
Other pairs fail with observed values. Derive target_directory from metadata,
not cache labels. Supported output is target_directory/release/deps, with no
explicit target triple. Require a real rustc target-libdir directory.

Then run exactly:
cargo test --locked --release --lib issue_1850 --no-run --message-format=json-render-diagnostics
Pipe stdout through tee to the fresh cargo-messages.jsonl; leave stderr inherited
and visible in the CI log. Preserve Cargo AND tee failure through pipefail.
Do not parse merged stderr as Cargo JSON. A failed build cannot publish a manifest.
Parse stdout linewise: ignore blank/non-JSON diagnostic lines for selection while
retaining them; malformed JSON-looking lines (trimmed prefix { or [) fail.
Require exactly one build-finished record with success:true and Cargo exit zero.

Select exactly ONE compiler-artifact record satisfying all of:
- package_id equals the metadata-selected package id;
- canonical manifest_path and target.src_path equal this checkout's
  src-tauri/Cargo.toml and src-tauri/src/lib.rs;
- target.name is agentscommander_lib, target.kind includes lib
  (not equality to a singleton array), profile.test === true;
- executable is a nonempty string resolving to an existing regular file directly
  in the resolved release/deps directory, with .exe on Windows.
Fresh true or false is valid. Count records, not distinct filenames: duplicate
matching records fail even when they identify the same file. Never glob artifacts.
Use realpath/native path.resolve; case-fold comparisons only on Windows.
Containment uses path.relative component checks, not a string-prefix test.

Loader list order is fixed: build-script-executed linked_paths in message/list order
after stripping recognized native=, dependency=, crate=, framework= or all= prefix;
then release/deps; then release; then rustc target-libdir. Resolve emitted native
absolute paths; fail ambiguous relative linked paths. Exclude linked paths outside
canonical target_directory; fail missing target-contained directories. Deduplicate
canonical entries keeping their first position, case-insensitively only on Windows.
Unknown KIND= syntax fails rather than accidentally treating it as a directory.

Write schema:1 manifest.json exclusively only after every check passes. Fields:
runId, runAttempt, job, checkoutSha, prHeadSha, cwd, packageId, manifestPath,
sourcePath, targetName, host, targetDirectory, executable, executableSha256,
executableBytes, loaderDirs, versions, createdAt. checkoutSha is git rev-parse HEAD
and must equal GITHUB_SHA; set ISSUE1850_PR_HEAD in both build/consumer step env
to ${{ github.event.pull_request.head.sha || github.sha }}. PR checkout can be
a merge SHA: retain both identities, with prHeadSha from ISSUE1850_PR_HEAD.
Hashing the artifact binds producer/consumer bytes; it is not host attestation.
After closing the file, publish manifest=<absolute path> to GITHUB_OUTPUT.
Log selection identity, digest, byte size and timing. Never publish success early.

## 4. Assertion consumer and upload

Keep shell bash, src-tauri cwd, set -euo pipefail, timeout-minutes: 20.
Set ISSUE1850_MANIFEST via env to steps.issue1850_release_build.outputs.manifest.
Inline Node reads only that path and requires schema 1, same run/attempt/job,
same git checkout HEAD/GITHUB_SHA and PR head env, canonical cwd/paths, native host,
existing executable and unchanged size/SHA256. Missing/invalid/mismatched input
fails; never discover another artifact or invoke Cargo/rustc/metadata as fallback.

Preserve inherited environment. Prepend loaderDirs to the existing platform variable:
PATH on Windows, LD_LIBRARY_PATH on Linux, DYLD_FALLBACK_LIBRARY_PATH on macOS.
Use path.delimiter. Preserve inherited value/order, including empty elements.
Windows: consolidate all case-insensitive PATH keys into one PATH; use the first
lexicographically sorted existing key's value, matching Node's environment lookup,
then remove other spellings. macOS: only when the variable is absent, append
HOME/lib, /usr/local/lib, /usr/lib; require existing HOME for this fallback.
An explicitly empty macOS variable is not absent. Never set HOME/USERPROFILE,
the real-profile authorization marker, Cargo config env or other test state.

Use child_process.spawn(executable,
['issue_1850', '--test-threads=1', '--nocapture'],
{shell:false, cwd:manifest.cwd, env, stdio:['ignore','inherit','inherit']}).
No Cargo flags or '--' separator reach the harness. Log exact path, argv,
identity/digest, loader variable name and launch/completion timestamps.
Handle error event as failure; on close propagate integer nonzero exit unchanged;
signal or null status fails with exit 1 and explicit signal/error detail.
Do not let a subsequent close event erase an earlier error. No buffered exec,
maxBuffer, retries or application compilation. CI timeout owns cancellation.

Pipe Node stdout/stderr through existing tee test-1850-release.log with pipefail.
Keep every byte from the existing following "for name in" through that release
step's end unchanged, including five names, grep patterns/messages, positive count
and Windows ISSUE1850_WINDOWS_PROFILE_PROOF_OK rejection. Keep set -euo pipefail.
No count relaxation or deletion of debug/default-root/real-profile coverage.

In each of the three jobs, immediately after this consumer add:
- name: Upload IS #1850 release diagnostics
- uses: actions/upload-artifact@v4 (existing workflow precedent)
- if: always()
- timeout-minutes: 5
- with.name: issue1850-release-${{ github.job }}-${{ github.run_id }}-${{ github.run_attempt }}
- with.path: two lines: ${{ steps.issue1850_release_build.outputs.diagnostics-dir || format('{0}/issue1850-release-not-started', runner.temp) }} and src-tauri/test-1850-release.log
- with.if-no-files-found: warn
- with.retention-days: 14
- with.compression-level: 6
- with.include-hidden-files: false

No continue-on-error. Fresh directory includes metadata, context, Cargo stdout
and manifest if reached; full Actions logs retain stderr. Upload never makes a failed producer successful.
Missing files before producer start are explained warnings. Once producer started,
missing expected diagnostics blocks evidence acceptance. Cancellation can prevent
uploads; retain available full Actions logs and report missing evidence, never pass.

## 5. Entry, local proof and recovery

Dev records evidence under room-shared/1929-implementation/correction-round3/.
Architect writes no workflow code and starts no builds/CI. Dev is the single writer.
At initial pre-implementation entry and again before PR creation/update, run
the common identity/drift checks below from the exact repo; apply the distinct
state gates that follow, rather than repeating the initial working-tree condition:
```bash
pwd
git rev-parse --show-toplevel
git branch --show-current
git rev-parse HEAD
git status --porcelain=v1 --untracked-files=all
git diff --cached --name-only
gh issue view 1929 --repo mblua/AgentsCommander --json state,url
git fetch origin main
git diff --name-status d8816883676ed2ebf9e700152944598b1de47b52 origin/main
git merge-base --is-ancestor d8816883676ed2ebf9e700152944598b1de47b52 HEAD
```
At both gates require authorized root/branch and open issue. At initial entry,
require an empty index and only this approved plan as a working edit; verify its
raw SHA256 against the approved digest. Capture actual CORRECTION_BASE after
bounded synchronization and recheck the initial state before product mutation.

At the later pre-PR gate, retain that recorded CORRECTION_BASE and permit either
the reviewed workflow+plan correction as working/staged changes, or its clean
committed equivalent. Compare the complete correction against CORRECTION_BASE,
including committed, staged and unstaged changes; the tracked path set must be
exactly the two inventoried paths. No unrelated tracked, staged or ordinary
untracked drift is allowed. A clean committed correction requires an empty index
and clean working tree; the plan need not remain a working edit. For an uncommitted
correction, any staged paths must be within the reviewed two-path set, and verify
plan bytes in each applicable working/staged artifact against the approved digest.
Before PR creation/update, commit the reviewed correction, require its clean
committed state, and verify the committed plan digest as specified below.
Recheck the applicable state after synchronization; unexpected drift blocks.
Classify target drift by relevance: workflow/toolchain/lock/build-input changes
refresh only affected proof; unrelated main movement does not reopen design.
No reset, force rewrite, direct main push or branch replacement. Save workflow and
plan bytes/hashes before writes; recheck HEAD/index/file hashes immediately before
mutation. On failure restore only owned paths still equal to recorded attempt bytes;
preserve external changes and report conflict. No broad restore/clean.

Before expensive CI, dev extracts candidate YAML and inline scripts into evidence
scratch using installed validation tools; records tool/version and exact invocation.
Parse YAML with duplicate-key rejection, bash -n each changed shell block,
node --check each extracted Node snippet, and Python compile() for the preflight.
No dependency installation or product build for these local checks.
Execute fixtures against the EXTRACTED candidate functions, not rewritten copies:
- Selector: valid cold/warm records with multiple lib kinds; zero, duplicate,
  wrong package/source/manifest, null executable, debug/outside-root path,
  malformed JSON, unsuccessful/missing/duplicate build-finished must fail.
- Config: absent config, valid jobs-only config; quoted/dotted runner, cfg runner,
  build.target, env target, include, malformed TOML and unsupported native host fail.
- Paths/loader: spaces, Windows mixed case/separators/PATH spellings, sibling-prefix
  exclusion, linked KIND prefixes, deterministic order/dedup, all three delimiters,
  absent versus empty macOS fallback and missing library directories.
- Launch: copied native Node executable in a space-containing path as fixture child,
  exit 0 and 7, missing file, identity/hash/run mismatch, malformed/stale manifest;
  signal/null-status fixture must fail. Validate tee+pipefail preserves child failure.
Use the actual harness argv in the real CI proof, not the fixture's argv.

Compare all three guard suffixes byte-for-byte against planning HEAD; compare
unchanged step definitions and top-level workflow settings against CORRECTION_BASE.
Only the nine inventoried steps, three comments and removed Linux env lines may differ.
Assert 40/20 bounds and original guards remain; assertion snippets contain no
Cargo/rustc/build invocation. Prove no other workflow/test/source/cache change.
Run git diff --check, git diff --name-status "$CORRECTION_BASE",
git diff --cached --name-only, and git status --porcelain=v1 --untracked-files=all.
Expected tracked set is exactly workflow + plan; no unexpected untracked/index drift.
Hash every extracted snippet, fixture transcript and guard comparison for Grinch.
Freeze this plan as UTF-8/LF bytes. Before commit, require the staged plan's
git show :plans/1929-split-focused-release-build.md | sha256sum to equal the
approved raw-file digest; EOL conversion must not silently change reviewed bytes.
At the clean committed pre-PR gate, require:
```bash
git show HEAD:plans/1929-split-focused-release-build.md | sha256sum
git diff --name-status "$CORRECTION_BASE" HEAD
git diff --cached --name-only
git status --porcelain=v1 --untracked-files=all
```
The committed plan digest must equal the approved digest; the full base-to-HEAD
path set must be exactly workflow+plan, and the last two outputs must be empty.
Retain these outputs with the reviewed correction HEAD; a digest mismatch or
unrelated drift stops delivery and requires resolution before proceeding.

## 6. CI, review and delivery gates

Dev owns authorized correction push/PR execution and durable collection; Grinch
independently checks full proof. On each native OS require one successful producer,
one selected release harness with matching consumer digest/run identity, direct
launch with all five passing tests, original guards green, build <40m/assertions
<20m, and no compile invocation/output during assertions. Inspect complete logs
and extracted invocation: short runtime or unchanged hash alone is insufficient.
Real native loading is a remote-only gate, not claimed proved by local fixtures.

Current applicability: push triggers branch validator plus all 14 regression jobs.
Regression includes four terminal-snapshot-portable matrix legs, test-debt,
rust-regression Windows/Linux/macOS, rust-linux-release-parity, rust-fmt,
windows-release-cli-smoke, issue-1850-windows-profile and frontend-regression.
PR also triggers lockfile-drift (its unchanged-input branch should pass).
Bundle/version workflows' paths do not match these two files; cache warm/release
are not correction triggers. Re-derive against actual full diff, not this expectation.
Coordinator reconciles configured-required checks via branch protection/rulesets:
```bash
gh api repos/mblua/AgentsCommander/branches/main/protection/required_status_checks
gh api repos/mblua/AgentsCommander/rules/branches/main
gh pr view PR --repo mblua/AgentsCommander --json url,baseRefName,headRefName,headRefOid,closingIssuesReferences,statusCheckRollup
gh api --paginate repos/mblua/AgentsCommander/commits/HEAD_SHA/check-runs
gh api repos/mblua/AgentsCommander/commits/HEAD_SHA/status
gh run view RUN --repo mblua/AgentsCommander --json headSha,event,status,conclusion,jobs,url
gh run view RUN --repo mblua/AgentsCommander --log
gh run download RUN --repo mblua/AgentsCommander --dir EVIDENCE_DIR
```
Replace uppercase placeholders with recorded values; save stdout/stderr and exits
to evidence outside the repo. Collect every run/attempt, complete logs/artifacts,
step timings and checks, then SHA256SUMS. Reconcile missing/truncated output.
403/404 from policy APIs is not proof of zero requirements; coordinator resolves
access/policy evidence. All triggered and configured-required checks must succeed
on exact PR head; record synthetic PR checkout SHA separately. Unexplained skips,
cancellation, timeout, failure or missing evidence block delivery without waiver.

| Gate | Owner/time; executable evidence and failure behavior |
| --- | --- |
| CI parity | Dev local scope/guard/script proof before push; coordinator complete exact-head checks before merge; mismatch blocks. |
| Determinism/config/cwd | Producer versions, TOML preflight, metadata/native/artifact checks each OS; unsupported input fails before launch. |
| Git/scope/recovery | Dev entry/final commands, recorded base, two-path diff and preserved backups each write gate; conflict stops without clobber. |
| Bounded diagnostics | Dev 40/20 steps, pipefail, uploads/full logs/hashes after success or failure; missing evidence is inconclusive. |
| Protocol proof | Dev fixtures and written environment-risk assessment; Grinch independently reviews exact plan and implementation bytes before acceptance. |
| Cycles/layering | Added/removed application module arcs: zero. Two-path scope excludes all module/arc-record edits. No lower layer gains UI/transport dependency. SCC measurement/arc regeneration not applicable; none claimed. |

No accepted test debt or failure waiver is introduced. A repeat Windows failure
is a blocker with retained evidence, not permission to weaken tests.
Any helper-file/dependency/config/cache/product expansion requires new inventory,
digest and review. Do not edit this READY specification silently after peer approval.

One correction PR into main includes this plan and references #1929 without an
automatic closing keyword; closingIssuesReferences must be empty. Coordinator
verifies head/base/closing set, merge policy, tested head, merge SHA and ancestry.
After merge coordinator records current main and validates relevant integration
drift. ac-shipper-v4, separately from coordinator, owns the final build on that
recorded current-main SHA and supplies command, tool versions, build-time timestamp,
artifact path/SHA256/byte size/source-SHA receipt in room-shared/1929-delivery/.
Coordinator owns build dispatch/output contract, final branch main tracking
origin/main, clean tree and HEAD == fetched origin/main checks, and issue closure.
Non-destructive synchronization only. No version bump, installation or release
follows from this plan. Final closure requires integrated build success, not merely
a merged PR. Author certification does not perform or claim any of these executions.

Sources: [Cargo JSON](https://doc.rust-lang.org/cargo/reference/external-tools.html),
[Cargo config](https://doc.rust-lang.org/cargo/reference/config.html),
[Cargo loader environment](https://doc.rust-lang.org/cargo/reference/environment-variables.html#dynamic-library-paths),
[Node child processes](https://nodejs.org/download/release/v22.22.0/docs/api/child_process.html).
