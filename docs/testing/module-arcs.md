# Module arcs

Run `npm run check:arcs` from the repository root. It generates and validates
`src-tauri/module-arcs.txt`, an ignored UTF-8/LF, sorted, unique, non-self arc record.
Missing or stale local output is replaced; zero arcs is valid. Never stage it.
`npm run check:cycles` remains the independent blocking cycle verdict. Arc generation
can succeed for a cyclic tree, so both commands are required.

For explicit comparisons, generate a graph with the unchanged in-repo detector:

```bash
node scripts/01-rust_module-dependency-cycles.mjs src-tauri --emit-graph graph.json --quiet
npm run record:arcs -- --graph graph.json --out artifacts/module-arcs-review.txt
```

Keep graph/output in ignored locations; graph.json contains host paths. Save before/after
records explicitly and compare their arc pairs; there is no automatic PR arc diff.
Self-tests: `npm run check:arcs:self`, `npm run record:arcs:self`, `npm run check:cycles:self`.
Publication copies validated bytes to a unique destination sibling and renames locally,
so OS TEMP and repository may reside on different drives. Failure keeps the previous
final intact. A failed generation never publishes an old record as a new artifact.

The always-on `test-debt` CI job generates and verifies the record and a JSON receipt,
then uploads `module-arcs-<tested-sha>-<run-attempt>` for 14 days. Download it from the
workflow run's Artifacts section. It contains module-arcs.txt and receipt.json, with
SHA256, byte/arc counts, generation time, detector/Node versions and testedSha.
For PR workflows testedSha is the checkout merge tree, not necessarily the PR head;
optional prHeadSha provides that association. Verify the receipt hash/size and the
run's tested tree before using it as evidence. Generation/staging/verification failure
uploads nothing; upload failure fails the job. A valid artifact remains available if
the later cycle gate fails, but does not turn that gate green.

Branches based on this policy merge source edits without tracked-record conflicts.
Branches forked before deletion may still get a generated-file modify/delete conflict:
inspect status/diff and resolve actual source conflicts, then remove only the generated
record from the merge index with `git rm -- src-tauri/module-arcs.txt`. Retain the ignore
rules. Finish the merge, then regenerate with `npm run check:arcs` and run `check:cycles`.
Confirm `git ls-files -- src-tauri/module-arcs.txt` is empty and `git check-ignore
src-tauri/module-arcs.txt` succeeds. Never force-add, reserve, or select ours/theirs as
an authoritative generated record. Unexpected source conflicts need their own review.

## Bounded cycle baseline (#3998)

The detector now records parent-to-child arcs (`use child::T` in a parent). This made
sixteen existing module cycles visible; no application code changed. They are frozen in
`src-tauri/.rust-cycles-baseline.json`, written only by `npm run write:cycles-baseline`.
`functionCycles` is empty. Each entry freezes its exact member set, not just a count.

`npm run check:cycles` fails on any change: a new cycle exits 1, and a frozen cycle that
grows, shrinks or swaps members fails too. A frozen cycle that disappears exits 3 until
the baseline is pruned. This exception covers these sixteen sets only; it never widens automatically.

Known detector limits behind some members (pinned as KNOWN LIMITATION in
`scripts/module-child-paths.test.mjs`):

- D1: a `#[path]` module may be read from the wrong directory and close a false cycle.
- D2: a bare path guessed as a crate-root module may be an import alias.
- D3: `#[cfg]` is not evaluated; arcs from exclusive platforms are unioned.
  `cd361f9ca2d1da52` is such a cross-platform union, not one single-platform cycle.

Imports inside expressions, methods and function bodies remain under-reported.

Pruning contract, for each of the sixteen sets: when a reviewed cut or detector fix
removes that exact cyclic set, measure comparable before/after graphs on clean trees and
review members, pairs, reachability and layers. Every remaining cycle must be a subset of
one earlier set: shrink, split or removal is allowed; growth, new members or merging two
sets is not. Then run `npm run write:cycles-baseline` and `npm run check:cycles` in the
same PR, so the retired id is pruned and a returning cycle fails. There is no automatic
expiry or re-acceptance. An unchanged member set never approves new coupling inside it.
