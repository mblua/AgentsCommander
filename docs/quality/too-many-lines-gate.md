# Function-length gate

AgentsCommander rejects new functions above **100 Clippy lines** and growth beyond a
committed function's ceiling. Windows and Linux measure every pull request; macOS
measures in the existing full tier. Reduce the reported function or split it into
helpers of at most 100 lines. Do not regenerate the baseline to accept debt.

This page describes the implementation shipped in `8a2fdeb670966e8bf774b3a20ac36a73d4c87cd7`
(#3514, implementation of #3507).

## What runs

Root `clippy.toml` pins `too-many-lines-threshold = 100` alongside cognitive complexity
25. Each measuring leg runs this command from `src-tauri`, using Rust/Clippy **1.98.1**:

```bash
cargo clippy --locked --workspace --all-targets --message-format=json \
  -- -D warnings --force-warn clippy::cognitive_complexity \
  --force-warn clippy::too_many_lines
```

Only those two named lints are force-warned; the whole pedantic group is not enabled.
Other warnings still fail under `-D warnings`. Both gates consume the same complete
head capture. There is no per-PR base checkout, build or Clippy run for this gate.
The baseline ratchet reads Git bytes from the PR base.

`test-debt` runs blocking `tml:self` and `tml:baseline` steps. The length capture/check
steps in `rust-regression-windows`, `clippy-linux` and full-tier
`rust-regression-macos` also block failures. Full tier follows the existing workflow:
manual dispatch, adding `ci:full`, or every tenth workflow run. To request full tier
on a new head, remove and re-add an existing `ci:full` label after the final push.
A nonempty adoption dispatch is a separate exception described below.

The [cognitive gate guide](cognitive-complexity-gate.md) describes that gate separately.
Its Clippy example at line 18 predates the length gate and omits the second
force-warn flag; use the command above for a current shared capture.
`scripts/ci-local.mjs` still omits the length check. Run the explicit commands below.

## Read a failure and fix it

The checker prints source identity, platform, actual count and allowed ceiling:

```text
NEW rust:src/example.rs::heavy (linux): actual 122, allowed 100; shrink to the ceiling or split below 101
GROWN rust:src/example.rs::heavy (linux): actual 181, allowed 180; shrink to the ceiling or split below 101
```

These are illustrative messages. The JSON result also records `sourceCommit`, the
header anchor, and source file/line/column for observed sites. A moved site's result
includes its donor identity, naming the old source location as well as the receiver.
The reported numerator is **Clippy's metric**, not a physical source-line count.

- `NEW`: reduce to 100 or below, or split into helpers each at most 100.
- `GROWN`: reduce to the recorded ceiling or below, or split.
- `AMBIGUOUS`: a move has multiple donors or receivers; reduce the unmatched functions
  to 100 or below rather than assigning debt arbitrarily.
- Invalid/incomplete captures, unresolved identities and unsupported macro identities
  fail as input errors. Fix the capture or ask the gate owner to investigate.

A same-signature reduction, splitting with short helpers, or removal passes without
baseline edits. `EXACT`, `MOVED` and `ABSENT` are nonfailing comparison results.
Below-ceiling and absent sites are reported without changing the baseline; stale
entries do not fail this gate. The separate cognitive gate retains its own rules.

### Identity and bounded moves

Identity combines repository-relative path, enclosing container, function name and
header anchor. Both identity helpers receive the resolved **name-token start**, not
the beginning of Clippy's highlighted header. Visibility and preceding qualifiers
are excluded. Changing `pub` to `pub(crate)` preserves identity, including a bounded
move that needs wider visibility. Body edits and line shifts preserve identity.
Name, parameter, generic, return-type or `where` edits change the site; header
normalization is not a promise of semantic equivalence across reformatting.

A path/container move may inherit one unused ceiling on the same platform only when
exact name plus anchor identifies one donor and one unmatched receiver, and the
original definition is absent from source. Absence from warnings is insufficient:
a retained original still reserves its ceiling even if shortened to 100 or below
or excluded by `cfg`. Exact matches take priority. One donor cannot fund two functions;
duplicate donors/receivers, ambiguous matches and moved growth fail. New long helpers
fail too. There is no fuzzy matching or sanitizing-allowlist exemption.

Macro expansion diagnostics are rejected conservatively. When checking whether a
donor disappeared, macro token trees that may carry its name or synthesize identifiers
also fail closed; ordinary unrelated macro calls do not themselves prevent a move.

### The accepted regrowth gap

Only committed ceilings govern enforcement. With ceiling 180, a function may shrink
from 180 to 150 and later grow to 160 without failing. Removing and reintroducing the
same identity at 180 or below also passes while its baseline entry remains. This is
an accepted limitation. You may lower or remove entries after reductions, but need
not do so. Lowering the ceiling to 150 makes a later count of 160 fail.

Once main contains the baseline, the Git ratchet accepts only entry/platform removal
and ceiling reductions. It rejects new or swapped sites, increased ceilings, changed
production flags and changed metadata, including toolchain/source SHA. A metadata or
toolchain refresh needs separately reviewed scope; there is no automatic refresh
exemption here. Never edit the baseline to conceal a failure.

## Run locally

From the repository root, without building Rust:

```bash
npm run tml:self
npm run tml:baseline -- --base-ref origin/main
```

The second command checks **committed HEAD**, not an unstaged baseline edit.
`--base-ref` defaults to `GATE_BASE_REF` when nonempty, otherwise `origin/main`.
A fresh checkout needs Git history/base refs, not downloaded adoption artifacts.

For a real local measurement, use installed platform prerequisites and pinned Rust
1.98.1. Create an output directory under your project's approved artifact location;
set `out` to its absolute path and `platform` to `windows`, `linux` or `macos`.
Run in Bash from the repository root:

```bash
root="$PWD"
commit=$(git rev-parse HEAD)
(
  cd src-tauri
  set +e
  cargo clippy --locked --workspace --all-targets --message-format=json \
    -- -D warnings --force-warn clippy::cognitive_complexity \
    --force-warn clippy::too_many_lines > "$out/clippy.jsonl"
  printf '%s\n' "$?" > "$out/clippy-exit.txt"
)
node scripts/check-too-many-lines.mjs \
  --capture "$out/clippy.jsonl" --clippy-exit "$(cat "$out/clippy-exit.txt")" \
  --platform "$platform" --rustc-version 1.98.1 --commit "$commit" \
  --workspace-root "$root" --emit "$out/observed.json"
node scripts/check-too-many-lines.mjs --check "$out/observed.json" \
  --baseline too-many-lines.baseline.json --workspace-root "$root"
```

Verify the actual compiler with `rustc -vV`; `--rustc-version` supplies metadata,
it does not select a compiler. Ordinary candidate captures require both explicit
Clippy pins. `CLIPPY_CONF_DIR` must be absent; `RUSTFLAGS` and
`CARGO_ENCODED_RUSTFLAGS` must not override lints. Keep the checkout at the recorded
SHA through capture and check. Input must contain complete Cargo JSONL, exit zero
and exactly one successful terminal `build-finished`; a filtered warning list fails.
Repeated identical spans are deduplicated; conflicting counts or indistinguishable
sites fail. The check writes `<observation-file>.summary.json` and prints JSON.

The supported modes are `--self-test`, `--baseline-diff [--base-ref REF]`,
`--capture FILE` with the arguments above, `--check FILE --baseline FILE`
(with optional `--workspace-root`), and `--merge FILE FILE FILE --out FILE`.
Exactly one mode is required. There is no report mode that bypasses enforcement.
There is no `--help`: it returns input-error exit 2, as do invalid, duplicate,
missing or out-of-mode flags. Comparison/ratchet failures return 1; success returns 0.

## Adoption evidence and coverage

The committed v1 baseline measures source
`af830ff15ccd2990e468e683b14f2a471b655380`. Native re-adoption
[run 41373335712](https://github.com/mblua/AgentsCommander/actions/runs/41373335712)
and the [execution receipt](https://github.com/mblua/AgentsCommander/issues/3507#issuecomment-6567405951)
record these measured **above-100 sites**, not estimates or counts of all functions:

| Platform | Production | Including tests/all targets | Test only |
| --- | ---: | ---: | ---: |
| Linux | 123 | 356 | 233 |
| macOS | 123 | 354 | 231 |
| Windows | 123 | 383 | 260 |
| Union | 124 | 388 | 264 |

Production uses `--workspace --lib --bins`; all targets uses
`--workspace --all-targets`. Test-only means absent from the production capture,
not an assertion that every such function has a test attribute. Union deduplicates
identity plus anchor; production union means observed in production on any platform.

Pinned real controls in P01 measured ordinary methods (122), an ordinary async function
(124), a test function (122), and a Windows `cfg` function (122). The generated-function
macro control emitted a diagnostic but failed identity resolution as an unsupported
expansion. Thus macro-generated debt cannot be assumed measurable and comparable.
The proof does not establish length coverage for standalone async blocks or every
attribute macro, including Tokio entry-point macros. The cognitive guide's measured
blind spots are not length-gate evidence. Only compiled `cfg` branches are observed;
Windows/Linux light tier cannot establish macOS-only coverage, and even full tier
cannot see code excluded on all three platforms.

### Initial-only native capture

The existing PR workflow accepts a manual `tml_adoption_source` SHA on the issue
branch. This historical adoption path is not permission to replace today's baseline:

```bash
gh workflow run pr-regression-gates.yml --repo mblua/AgentsCommander \
  --ref ci/3507-too-many-lines-gate -f tml_adoption_source="$S"
```

A nonempty adoption dispatch runs only the native adoption matrix, with run-specific
concurrency and without cancelling ordinary enforcement. Normal PR/full-tier runs
remain enforced. Each native job uses one clean main source S, Rust 1.98.1,
Node 22/npm 11.6.2, `npm ci --ignore-scripts` then `npm run build` on every platform.
The adoption job's earlier Windows `npm ci` mirror is no longer in that job;
the ordinary `rust-regression-windows` job still uses plain `npm ci`.

Before capture, the owner verifies S descends from the coordinator-supplied actual
#3505 merge SHA with `git merge-base --is-ancestor "$MERGE_3505" "$S"`.
Checker/workflow SHA, measured source S, PR head and CI merge-checkout SHA are distinct:
the checker runs against a separate clean checkout of S; PR CI tests its merge tree.
For the final adoption, checker/workflow SHA was
`0c0ad8818c15a99e704cc00ddbd1ba58e2fcb6b5`. Final P01 CI tested merge checkout
`8fff2d26f424495487e0c9bb934c7cad95061b98` for PR head
`28585703d547127fc0abce3a42540bb438dc4051`.
The default-100 exception requires verified historical configuration and a real native
control diagnostic whose denominator is 100, rather than relying on a default claim.

Adoption capture adds **all five** arguments to the ordinary capture command:

```bash
--production-capture "$out/tml-production.jsonl" \
--production-clippy-exit "$production_exit" --adoption-source "$S" \
--denominator-proof "$out/denominator.jsonl" \
--denominator-clippy-exit "$denominator_exit"
```

The denominator input is raw native Cargo JSONL from the control, with successful
terminal record and a real length diagnostic. All three captures must succeed.
Production sites must belong to the all-target set; resulting observations carry
`productionMeasured=true`. Merge exactly three native observation files:

```bash
node scripts/check-too-many-lines.mjs --merge \
  linux/observed.json macos/observed.json windows/observed.json \
  --out "$out/audited-baseline.json"
```

Merge requires all three distinct platforms, identical source SHA, pinned toolchain
and measured production membership. Baseline v1 contains no denominator-proof fields.
During initial adoption only, main must lack a baseline and candidate Rust/Cargo/build
sources must match S, with bounded gate config/workflow changes. Relevant main drift
requires integration, re-adoption at a new main S, new counts and renewed full-tier
equality proof. This happened before the final baseline above was landed.

CI transports raw captures, control/config copies, tool versions, receipts and hashes;
it does not replace the manual adoption audit. Before acceptance/landing, the developer
and Rust Grinch independently verify all three native artifacts, successful jobs,
run/checker/source/head bindings, proof hashes and byte-identical regenerated baseline.
The coordinator indexes run/artifact/hash/head bindings and both verdicts in external
`phase-commits.json`. Missing or expired proof blocks landing even with green CI.

Blocking `tml:self` retains workflow-contract mutations, invalid-flag and nonzero
case-count controls. First full-tier acceptance additionally requires artifact proof
of platform/site/count equality: final P01 head
`28585703d547127fc0abce3a42540bb438dc4051`, run `41376747500`, matched Windows
383/383, Linux 356/356 and macOS 354/354. That initial equality requirement does not
forbid later legitimate reductions, moves or removals.

## Findings from another pull request and coordinated recovery

If another author's PR introduced a finding, report platform, site and actual/allowed
counts to its owner and ask for an in-scope reduction or split. Do not change their
source or raise the baseline. A macOS-only regression may first surface in another
PR's full-tier run: reproduce on the same platform/configuration against the base
to identify its source, then coordinate the owner's fix and fresh full-tier CI.

For #3507 adoption, room-04 warned room-29 before S capture with an estimated hold;
room-29 and room-37 held their windows, with holds above two hours requiring renewed
discussion. After P01 merged, room-04 sent merge SHA
`8a2fdeb670966e8bf774b3a20ac36a73d4c87cd7` to room-29. As room-29 had instructed,
room-29 then notified room-37 and the other rooms. P02 documentation was authorized
to land separately without extending that hold.

From that merge, landing windows require the tested CI checkout tree to equal the
clean `git merge-tree --write-tree origin/main <head>` tree against current main,
proving Windows/Linux Clippy ran with the baseline. A mismatch requires integration
and fresh CI. The shipper obtains room-29's landing window and exact match-head merge
command under P254; a green older head is insufficient. If main goes red, the offending
PR's owning room or lock holder opens a revert PR and room-29 grants a priority window.
Coordinators neither edit code nor push main; baseline edits never conceal the failure.
These coordination rules were coordinator decisions, not user rulings.
