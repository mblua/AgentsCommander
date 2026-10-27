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
