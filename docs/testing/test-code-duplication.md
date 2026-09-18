# Test-code duplication

The SonarCloud quality gate scores test-code duplication under `new_duplicated_lines_density`. Nothing used to detect it before a push, so a duplicated arrange-and-assert block in a new test file could pass every GitHub check and then fail the gate after the full regression matrices — exactly what #2108 cost. This page is the convention, the local command, and the CI gate that catch it first.

## The rule

> If the same arrange+assert shape appears twice or more in one test file and each occurrence is 10 lines or longer, hoist the constant part into a local helper in that file taking a named-options object; the varying data stays at the call site. Under 10 lines, leave it duplicated.

The convention is not limited to repeats inside one file. `jscpd` and SonarCloud both detect clones **across** files, not only within one, so:

- When the repeat is within one file, hoist the constant part into a local helper in that file.
- When the same shape repeats across files, hoist it into a shared helper module under the tests' own directory.

## Two guards

A helper must not hide the case matrix:

- It takes **only data** (a named-options object) and does not branch on its options.
- The differing values appear **literally at the call site**.

`expectNoGroupForTeam` in `src/sidebar/stores/sessions.grouped.test.ts` is the reference shape.

## Thresholds and commands

`.jscpd.json` at the repository root is the single source of the thresholds (`minLines` 10, `minTokens` 100); no command line or document restates them independently.

- `npm run dup` — advisory, whole tree. Prints the console report and exits 0 even on findings. Use it to see the shape of the current tree.
- `npm run dup:changed` — the blocking gate. Scans the working tree with the same configuration, scoped to new code against `git merge-base HEAD origin/main` (or the ref in `GATE_BASE_REF`, which CI sets to the PR's base branch). New clones and edits inside baseline clones fail; the recorded baseline clones do not.
- `npm run dup:changed:self` — the gate's self-test. Builds throwaway git repositories under the system temp directory and proves the gate goes red and green for the right reasons. It changes nothing in this repository.

The gate also fails when the scan finds **zero** files (`--fail-on-empty`) and scans without `.gitignore` (`--no-gitignore`), so an ignore entry cannot silently shrink the scan; `.jscpd.json` ignores `**/node_modules/**` to keep dependency test files out of that wider scan. Copies are detected across `.test.ts` and `.test.tsx` as well as within each format.

## The recorded `main` baseline

At the gate's thresholds, `main` contains **148 clones** — 122 `tsx`↔`tsx`, 18 `ts`↔`ts`, and 8 cross-format (`ts`↔`tsx`) pairs — 3202 duplicated lines over 196 scanned files. `npm run dup` prints this baseline. These clones are **not ignored**: nothing was added to any ignore list to hide them. They are simply not new code, so the gate does not fail on them.

The cross-format pairs are the worked example that a block copied between a `.ts` store test and a `.tsx` component test is caught. Seven of the eight have `src/sidebar/components/AgentPickerModal.test.tsx` as one endpoint, the largest being a 223-token pair with `src/sidebar/components/settings-save.test.ts`.

## `jscpd` corroborates the shape; it does not predict Sonar

`jscpd` and SonarCloud use different detectors and do not agree pair-for-pair. #2108 established this on the same bytes: `jscpd` found an 81-token repeat Sonar was not counting, while Sonar's 34 duplicated lines were exactly the pair it flagged. A green `dup:changed` does not guarantee a green Sonar gate; it removes the recurrent cost by catching the shape before the expensive matrices run.

**Scope.** `dup:changed` scans only the files `.jscpd.json`'s `pattern` matches — today the `src/**/*.test.{ts,tsx}` test files. Duplication in `scripts/`, in workflow YAML or in production code is outside it, so a green run is not a statement about those files; SonarCloud is the only detector there. #2155's own self-test is the worked example: Sonar flagged 32 duplicated lines in `scripts/check-test-duplication.mjs` while `dup:changed` was green, which is why this paragraph exists.
