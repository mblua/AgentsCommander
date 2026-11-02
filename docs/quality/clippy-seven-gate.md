# Seven-lint growth gate

AgentsCommander blocks net growth beyond committed **lint/platform totals** for seven
Clippy lints. Windows and Linux measure every ordinary pull request. Moves, splits and
renames retain capacity without baseline edits. Remove excess findings or contact the
owning room; never raise the baseline to conceal growth.

## What runs

The blocking `Seven-lint gate` follows the shared Clippy capture in
`rust-regression-windows` and `clippy-linux`. `test-debt` also blocks on
`seven:self` and `seven:baseline`. The seven exact codes are:

- `clippy::doc_markdown`
- `clippy::implicit_clone`
- `clippy::map_unwrap_or`
- `clippy::match_same_arms`
- `clippy::semicolon_if_nothing_returned`
- `clippy::unnested_or_patterns`
- `clippy::map_err_ignore`

Rust/Clippy is pinned to **1.98.1**, with locked Cargo dependencies. CI uses Node 22
and npm 11.6.2. Existing length and cognitive consumers share the capture; unrelated
warnings still fail under `-D warnings`. Named force-warn flags expose existing allows.
The pedantic group is not enabled wholesale. macOS checks remain unchanged and do
**not** enforce these seven budgets. Only compiled cfg branches, features and targets
are observed; excluded code and macro coverage are not proved.

The [function-length gate](too-many-lines-gate.md) is the precedent for committed
ceilings and retained slack. Its unique function-anchor donor/receiver matching does
not apply here: seven-lint capacity belongs to a lint and native host, independently
of file, function or production/test label.

## Capacity, moves and retained slack

For each of fourteen separate keys (seven codes times two hosts), current distinct
findings `A` must satisfy `A <= L`, where `L` is the committed native capacity.
`excess = max(0, A - L)`; `slack = max(0, L - A)`. Capacity cannot cross lint or host
boundaries, even when the combined total stays unchanged.

With `L = 5` for one lint on one host:

| Change | Current A | Result |
| --- | ---: | --- |
| Split or rename five findings across files | 5 | Pass |
| Keep five and add or copy one distinct finding | 6 | Fail |
| Copy all five into other files while retaining originals | 10 | Fail |
| Remove two | 3 | Pass; two units remain available |
| Replace or regrow those findings up to five, in any files | 5 | Pass |
| Grow to six | 6 | Fail |
| Lower the committed limit to three, then observe four | 4 | Fail |

Removing or reducing findings requires no baseline edit. A newly written or copied
site can consume unused capacity from the same lint/host, including slack retained
from earlier PRs. Removing five and adding five different findings can pass. This is
net-growth enforcement, with no per-occurrence historical-origin guarantee and no
promise that every syntactically new finding fails.

Each diagnostic counts by exact code plus its sorted, unique primary-span vector
(repository-relative file and UTF-8 byte bounds). Repeated target diagnostics at the
same vector count once per host; distinct locations count again, even with identical
text. Children and suggestions do not add findings. Multiple primaries form one
finding. The first sorted primary owns its reporting label only. Paths containing
`tests`, `benches` or `tests.rs` are labelled nonproduction; inline `cfg(test)` code
in other files retains the production label. Labels do not partition capacity or
establish a freshly measured production subset.

## Read a failure

The checker prints JSON with code, platform, `actual`, `limit`, `excess`, `slack`,
status and current files. `<observation>.summary.json` additionally retains current
primary-span locations. For example, this illustrative excerpt means N+1 findings
against an allowed N=5:

```json
{
  "code": "clippy::implicit_clone",
  "platform": "linux",
  "actual": 6,
  "limit": 5,
  "excess": 1,
  "slack": 0,
  "status": "GROWN_TOTAL",
  "files": ["src-tauri/src/example.rs"]
}
```

Use summary `locations[].spans` byte ranges to find current sites. `GROWN_TOTAL`
reports excess demand, not which individual expression introduced it. A path change
alone is not `NEW`. `WITHIN_TOTAL` passes. Exit **0** means success, **1** means
capacity growth or ratchet regression, and **2** means invalid input or missing proof.
Fix the excess or route platform/code/count/location evidence to the owning room.
Do not edit another room's source or raise debt. Investigate incomplete captures,
source mismatches and unsupported spans instead of treating them as zero findings.

## Run locally

From the repository root, without compiling Rust:

```bash
npm run seven:self
npm run seven:baseline -- --base-ref origin/main
```

The ratchet reads **committed HEAD** against its merge-base with the selected ref.
The default ref is nonempty `GATE_BASE_REF`, otherwise `origin/main`; fetch current
base history first. An unstaged baseline edit is not checked by this command.

For a native measurement, use installed platform prerequisites and verify
`rustc -vV` reports 1.98.1. The metadata flag below does not select a compiler.
Prepare frontend dependencies/assets before Cargo using the platform's established
recipe. Accepted R01 used bounded `npm ci --ignore-scripts` then `npm run build`,
requiring success before Cargo. Its local Windows Node 26.10.0/npm 11.19.1 pair,
selected at `/c/Program Files/nodejs`, satisfied package engines; CI's pinned pair
is different. Those accepted runs did not install tools or change global PATH.

In Bash, set `out` to an existing absolute, approved artifact directory and
`platform` to `windows` or `linux`. For local project evidence use your own ignored
`src-tauri/target/i3528-enforcement` directory after `git check-ignore -q`; keep
Cargo target/build-dir and system TMP/TEMP unchanged. Start from a clean checkout,
keep it at the recorded SHA through capture/check, and run from the repository root:

```bash
root="$PWD"
commit=$(git rev-parse HEAD)
(
  cd src-tauri || exit 2
  set +e
  cargo clippy --locked --workspace --all-targets --message-format=json \
    -- -D warnings --force-warn clippy::cognitive_complexity \
    --force-warn clippy::too_many_lines \
    --force-warn clippy::doc_markdown \
    --force-warn clippy::implicit_clone \
    --force-warn clippy::map_unwrap_or \
    --force-warn clippy::match_same_arms \
    --force-warn clippy::semicolon_if_nothing_returned \
    --force-warn clippy::unnested_or_patterns \
    --force-warn clippy::map_err_ignore \
    > "$out/clippy.jsonl" 2> "$out/clippy.stderr"
  code=$?
  printf '%s\n' "$code" > "$out/clippy-exit.txt"
)
node scripts/check-clippy-seven.mjs \
  --capture "$out/clippy.jsonl" --clippy-exit "$(cat "$out/clippy-exit.txt")" \
  --platform "$platform" --commit "$commit" --workspace-root "$root" \
  --rustc-version 1.98.1 --emit "$out/observed.json"
node scripts/check-clippy-seven.mjs --check "$out/observed.json" \
  --baseline clippy-seven.baseline.json --workspace-root "$root"
```

Preserve full raw JSONL and the original Cargo exit; do not filter warnings. Input
requires exit zero, no error diagnostics and exactly one final successful
`build-finished`. Capture writes `<observation>.controls.json` with environment and
tracked Cargo configuration; check writes `<observation>.summary.json`. Unexplained
lint/compiler/wrapper selectors fail input validation; `CLIPPY_CONF_DIR` must be
absent. Do not override lint policy through `CLIPPY_ARGS`, Rust flags or Cargo aliases.

Source is read from tracked Rust Git blobs at the recorded SHA/tree. Cargo-relative
paths resolve against the repository/Cargo workspace root, **not** the `src-tauri`
invocation directory. Historical absolute paths map only from the recorded original
capture root, with no guessed fallback. Validation uses the existing `normalizeFile`,
UTF-8 byte offsets and Unicode scalar columns. Empty, unresolved, outside, generated,
untracked or expansion-bearing primary spans fail closed. Observation checkout
SHA/tree must match when checking; malformed or incomplete input never proves zero.

Exactly one CLI mode is required. Besides the commands above, historical `--adopt`
and two-host `--merge` exist for audited initial conversion, not routine recovery.
There is no `--help`; unknown, duplicate, missing and out-of-mode flags return 2.

## Baseline provenance and later main changes

`clippy-seven.baseline.json` uses policy `lint-platform-total-v1`: all fourteen
nonnegative limits are explicit, including zero, with fixed metadata and code/host
keys. Zero findings against zero capacity passes; one against zero fails. Missing
or unknown keys/files, invalid metadata or malformed schemas fail. Once adopted,
the Git ratchet permits only lowering limits with metadata unchanged. Raising or
swapping capacities fails even if their combined sum is unchanged. Do not regenerate
the baseline, reset source identities or raise totals to accept a failure.

The accepted native conversion records these distinct historical identities:

| Identity | Commit/tree |
| --- | --- |
| Reference Q | `d2b1089a636e5bce9bbc8e036f595e05f9f4077c` |
| Q referenceTree | `4f385ba16a8b9e4f965c84d51231e87d21edb73a` |
| Actual R01 source R | `51788bee730eb2b7a9b572a20272c0f2e11afb90` |
| R sourceTree | `1ceec88a7c257355ea1b88de150d9dd0d6e580fe` |

R differs from Q by the temporary acquisition workflow; other tracked bytes are
Q-equivalent. Q is not R. Historical #3511 path counts and repeated raw warning
counts are not capacity values. The accepted deduplicated limits are:

| Code (clippy::) | Linux | Windows |
| --- | ---: | ---: |
| doc_markdown | 626 | 661 |
| implicit_clone | 20 | 20 |
| map_unwrap_or | 243 | 244 |
| match_same_arms | 39 | 40 |
| semicolon_if_nothing_returned | 109 | 110 |
| unnested_or_patterns | 19 | 22 |
| map_err_ignore | 865 | 855 |

Historical Q/R equivalence proves initial adoption provenance. It does not require
later candidate source to equal Q. Integrate ordinary main changes through the
issue branch and run existing blocking Windows/Linux jobs on the actual new head
or CI merge checkout against the **same limits**. Unchanged/reduced totals and
same-total inherited splits pass; removals retain slack until limits are optionally
lowered. `--baseline-diff` checks committed baseline/history/schema, `--capture`
binds current source, and `--check` compares current demand with frozen capacities.
Record current main/head/CI-merge identities and proof externally, not in baseline
metadata. Permitted no-growth drift needs no automatic R01 recapture, re-adoption,
new plan/votes or restored temporary workflow. Net growth or invalid proof stops
and routes to the owner. A proposed debt increase or new policy exception requires
explicit room-29 authority before changes.

## Evidence and coverage limits

R01's Windows and Linux streams were independently audited with successful original
exits and source bindings before conversion. That acceptance retains Windows G1:
no saved target-specific environment-absence scan, an unsaved controls writer and
unproved exact inherited Cargo environment. Saved aliases/`CLIPPY_ARGS` controls do
not establish universal absence; wrapper/cache evidence does not prove all possible
configurations. Linux's job-bound timing and tracked-only custody do not repair
Windows gaps or attest external host configuration. Compiled-cfg, macro and macOS
limits remain; historical old-source production membership is not a new production
coverage claim.

Native jobs always attempt to upload `seven-lint-windows` and `seven-lint-linux`
artifacts, retained 14 days, with missing-file errors: raw JSONL/stderr, original
exit, observation, controls, summary and source/run metadata. Green CI alone does
not replace independent proof review. Delivery requires checks on the exact final
PR head, verified native artifacts, applicable integrated-head P2 checks, and a CI
checkout tree equal to the clean current `git merge-tree --write-tree origin/main <head>`
tree. The temporary measurement workflow must be absent. The shipper obtains the
room-29 landing window and exact match-head merge command; older-head success does
not prove the final delivery. This guide does not certify those pending gates.
