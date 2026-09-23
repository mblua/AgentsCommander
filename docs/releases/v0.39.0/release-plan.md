# Release preparation plan: v0.39.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/2459  
Candidate: `0.39.0` / `v0.39.0`  
Planning base: `46a16241be0046fa52b0675daa09d0cb415af2d1`  
Predecessor: `v0.38.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.39.0/` before tagging.

### review-set-v1 byte construction

The authoritative machine-readable recipe is `input-manifest.v1.json.contracts.reviewSet` with schema `prepare-agentscommander-release/review-set/v1`. Recompute it as follows; do not hardcode the candidate digest:

1. Read these basenames in this exact order, with no directory prefix:

```text
CHANGELOG.release.md
candidate-assets.v1.json
input-manifest.v1.json
predecessor-assets.v1.json
release-body.md
release-plan.md
```

2. Each file input is its exact emitted UTF-8 byte sequence. Perform no parsing, reserialization, Unicode normalization, or line-ending conversion. Text permits LF byte `0a` only, forbids a BOM and CR, and ends in exactly one LF. Generated JSON is already canonical: keys recursively sorted by ECMAScript UTF-16 string order, `JSON.stringify(value, null, 2)`, then one LF.
3. For each file, compute SHA-256 over those exact bytes and encode the digest as 64 lowercase hexadecimal ASCII bytes.
4. Serialize one record as `digest || 0x20 0x20 || UTF-8 basename || 0x0a`. There is no preamble, postamble, NUL, inter-record data, or length prefix. Concatenate records directly in the stated order; the last record retains its LF.
5. SHA-256 the complete concatenated record bytes and encode the result as 64 lowercase hexadecimal characters. That value must equal the sole `review-set-sha256` field in `release-authority-v1.txt`.

Positive conformance vector:

- `alpha.txt`: content base64 `YWxwaGEK`; SHA-256 `b6a98d9ce9a2d9149288fa3df42d377c3e42737afdcdaf714e33c0a100b51060`
- `beta.json`: content base64 `e30K`; SHA-256 `ca3d163bab055381827226140568f3bef7eaac187cebd76878e0b63e9e442356`

- exact order: `alpha.txt -> beta.json`
- serialized record bytes, base64: `YjZhOThkOWNlOWEyZDkxNDkyODhmYTNkZjQyZDM3N2MzZTQyNzM3YWZkY2RhZjcxNGUzM2MwYTEwMGI1MTA2MCAgYWxwaGEudHh0CmNhM2QxNjNiYWIwNTUzODE4MjcyMjYxNDA1NjhmM2JlZjdlYWFjMTg3Y2ViZDc2ODc4ZTBiNjNlOWU0NDIzNTYgIGJldGEuanNvbgo=`
- expected review-set SHA-256: `9d3945ec3d10e79abc60f28fd95e94cfb9d7f6b95ade412ea7bac4c138956ff8`

An implementation must first reproduce every vector byte and digest, then compute the candidate digest from candidate-tree files. A vector mismatch, different order, changed file byte, one-space separator, CRLF terminator, prefix, suffix, or normalization is fatal.

Generated evidence paths allowed in the hardening PR:

- `docs/releases/v0.39.0/CHANGELOG.release.md`
- `docs/releases/v0.39.0/candidate-assets.v1.json`
- `docs/releases/v0.39.0/input-manifest.v1.json`
- `docs/releases/v0.39.0/predecessor-assets.v1.json`
- `docs/releases/v0.39.0/release-authority-v1.txt`
- `docs/releases/v0.39.0/release-body.md`
- `docs/releases/v0.39.0/release-plan.md`
- `docs/releases/v0.39.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `46a16241be0046fa52b0675daa09d0cb415af2d1`.
- Ordered planning-base parents: [67bd069ffb1a37d6150a859d8a8d36cf6064b1fa, 03219efaad1cf348a20d383728a418284861b3e3].
- Base `.github/workflows/release.yml` blob: `1813662691aea512b4c8c087a388b0e9e87e66fa`; content SHA-256: `8c032478e665a51f17de2c061e793dacafe3351312fa2c0eadc7a8103a6e04db`.
- Predecessor annotated object: `c4ebf5f8b2fe805e4c28dd689a7297d4da6ce365`; peeled commit: `7fc7a4226eacf631cb75277799774c0fd15f2c70`.
- Predecessor immutable GitHub Release id: `394029251`.
- npm latest: `0.38.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.101.0`, archive `gh_2.101.0_linux_amd64.tar.gz`, SHA-256 `9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (discovered from `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.38.0` (blob `551a2cc6e6b7a66029111edd7b7f4dd148a3008c`)
- `package-lock.json` `/version`: `0.38.0` (blob `8fbe3368f5dcba7ce24b155dad70a220e4795f0e`)
- `package-lock.json` `/packages//version`: `0.38.0` (blob `8fbe3368f5dcba7ce24b155dad70a220e4795f0e`)
- `npm/package.json` `/version`: `0.38.0` (blob `8192f9a66f4be5acdd15b8464a312f16f2a3e1f6`)
- `npm/install.js` `const VERSION`: `0.38.0` (blob `2c022244441635c17e22f34faf7e40b5b0e635d1`)
- `src-tauri/Cargo.toml` `[package].version`: `0.38.0` (blob `40032fddab2f8906f0eb5836bccf46bcb889e8cb`)
- `Cargo.lock` `agentscommander.version`: `0.38.0` (blob `822eed771d7ef805df57d66efb58a0f8a80e0cdd`)
- `src-tauri/tauri.conf.json` `/version`: `0.38.0` (blob `91b0409c3f5b4b398f0286480d04f51e4bfef664`)

Approved scope:

- maintenance: ci(2234): live proofs of the cognitive-complexity gate, and its documentation ([#2260](https://github.com/mblua/AgentsCommander/issues/2260), [PR #2392](https://github.com/mblua/AgentsCommander/pull/2392))
- docs: docs: Co-managed phase 10 documentation (#2273) ([#2273](https://github.com/mblua/AgentsCommander/issues/2273), [PR #2403](https://github.com/mblua/AgentsCommander/pull/2403))
- feature: feat(compact): compact sidebar CSS — overlay, compact row, content removal (#2283) ([#2283](https://github.com/mblua/AgentsCommander/issues/2283), [PR #2401](https://github.com/mblua/AgentsCommander/pull/2401))
- feature: feat(compact): banner toggle, activation button, hotkey hydration (#2284) ([#2284](https://github.com/mblua/AgentsCommander/issues/2284), [PR #2416](https://github.com/mblua/AgentsCommander/pull/2416))
- feature: feat(compact): configurable toggle hotkey, terminal veto and capture control (#2285) ([#2285](https://github.com/mblua/AgentsCommander/issues/2285), [PR #2427](https://github.com/mblua/AgentsCommander/pull/2427))
- feature: feat(loops): notice when saving rebaselines the schedule (#2288) ([#2288](https://github.com/mblua/AgentsCommander/issues/2288), [PR #2407](https://github.com/mblua/AgentsCommander/pull/2407))
- feature: feat(2314): reorder coding agents from Settings rows and Step 1 cards (#2306 P3) ([#2314](https://github.com/mblua/AgentsCommander/issues/2314), [PR #2390](https://github.com/mblua/AgentsCommander/pull/2390))
- fix: fix(codex): accept turn_complete alias and never re-emit a superseded turn (#2356) ([#2356](https://github.com/mblua/AgentsCommander/issues/2356), [PR #2446](https://github.com/mblua/AgentsCommander/pull/2446))
- feature: feat(#2374): room activity status CLI ([#2374](https://github.com/mblua/AgentsCommander/issues/2374), [PR #2398](https://github.com/mblua/AgentsCommander/pull/2398))
- fix: fix(config): use verbatim paths for ReplaceFileW in publish_temp_config (#2378) ([#2378](https://github.com/mblua/AgentsCommander/issues/2378), [PR #2441](https://github.com/mblua/AgentsCommander/pull/2441))
- fix: fix(terminal): typing-hold corrections - no #, gray open / colored closed padlock, manual hold auto-release (#2379) ([#2379](https://github.com/mblua/AgentsCommander/issues/2379), [PR #2400](https://github.com/mblua/AgentsCommander/pull/2400))
- fix: fix(#2382): real Unix process liveness in pid_is_alive ([#2382](https://github.com/mblua/AgentsCommander/issues/2382), [PR #2397](https://github.com/mblua/AgentsCommander/pull/2397))
- maintenance: test(capture): de-flake a_baseline_is_consumed_but_never_routed (#2389) ([#2389](https://github.com/mblua/AgentsCommander/issues/2389), [PR #2438](https://github.com/mblua/AgentsCommander/pull/2438))
- fix: fix(#2393): persist first-run maximize placement seed ([#2393](https://github.com/mblua/AgentsCommander/issues/2393), [PR #2404](https://github.com/mblua/AgentsCommander/pull/2404))
- fix: fix(#2394): real Unix liveness probe for daemon.pid ([#2394](https://github.com/mblua/AgentsCommander/issues/2394), [PR #2415](https://github.com/mblua/AgentsCommander/pull/2415))
- fix: fix(ui): size TelegramIcon at 14x14 so the sidebar Telegram icon renders in WebKit (#2399) ([#2399](https://github.com/mblua/AgentsCommander/issues/2399), [PR #2402](https://github.com/mblua/AgentsCommander/pull/2402))
- feature: feat(sidebar): Co-managed from context menu + ring around activity dot (#2408) ([#2408](https://github.com/mblua/AgentsCommander/issues/2408), [PR #2428](https://github.com/mblua/AgentsCommander/pull/2428))
- maintenance: test(styles): guard that every markup class has a CSS rule (#2410) ([#2410](https://github.com/mblua/AgentsCommander/issues/2410), [PR #2439](https://github.com/mblua/AgentsCommander/pull/2439))
- fix: fix(#2411): internal-system wake resumes an auto-closed orchestrator ([#2411](https://github.com/mblua/AgentsCommander/issues/2411), [PR #2414](https://github.com/mblua/AgentsCommander/pull/2414))
- feature: feat: remove the Guide window (Hints and Tutorial) and its ActionBar button ([#2412](https://github.com/mblua/AgentsCommander/issues/2412), [PR #2426](https://github.com/mblua/AgentsCommander/pull/2426))
- fix: fix(session): keep close markers when a coordinator create fails (refs #2413) ([#2413](https://github.com/mblua/AgentsCommander/issues/2413), [PR #2437](https://github.com/mblua/AgentsCommander/pull/2437))
- maintenance: ci: light CI on every run, full CI every 10th run, refs #2417 ([#2417](https://github.com/mblua/AgentsCommander/issues/2417), [PR #2418](https://github.com/mblua/AgentsCommander/pull/2418))
- maintenance: ci: drop push trigger from PR regression gates, refs #2419 ([#2419](https://github.com/mblua/AgentsCommander/issues/2419), [PR #2420](https://github.com/mblua/AgentsCommander/pull/2420))
- maintenance: ci: Linux clippy and cognitive gate on every PR, refs #2423 ([#2423](https://github.com/mblua/AgentsCommander/issues/2423), [PR #2424](https://github.com/mblua/AgentsCommander/pull/2424))
- maintenance: chore(scripts): fix stale header comment in check-cognitive-complexity (#2425) ([#2425](https://github.com/mblua/AgentsCommander/issues/2425), [PR #2440](https://github.com/mblua/AgentsCommander/pull/2440))
- fix: fix(resource_monitor): stop process-tree walk looping on a PID-reuse cycle (refs #2443) ([#2443](https://github.com/mblua/AgentsCommander/issues/2443), [PR #2445](https://github.com/mblua/AgentsCommander/pull/2445))
- docs: docs: file naming convention target state (refs #2448) ([#2448](https://github.com/mblua/AgentsCommander/issues/2448), [PR #2449](https://github.com/mblua/AgentsCommander/pull/2449))

## 3. One mandatory order

Perform these stages exactly once and in this order:

1. Freeze the exact evidence and generated bundle.
2. Obtain WG33 cold-plan `PASS` on the exact `SHA256SUMS` identity.
3. Certify the exact bytes and record the `SHA256SUMS` SHA-256.
4. Obtain explicit human approval naming that exact hash.
5. Purge the planning context required by the implementation workflow.
6. Cold-implement the generic release hardening and exact evidence bundle; independently review the exact PR head; merge it through the protected branch.
7. Create the version/evidence PR from that exact hardening merge, move the approved Unreleased changelog bytes, run the repository version tool, independently review the exact PR head, and merge it through the protected branch.
8. Run final authority, topology, package, workflow, collision, and remote-main gates.
9. Create and push one annotated tag with the exact `release-authority-v1.txt` payload.
10. Verify the GitHub workflow, immutable public Release, npm package/provenance/install, and destination executable.

No implementation, PR, merge, tag, or publication occurs before steps 1-5. There is no second hardening or version merge later in the sequence.

## 4. Planning-base gate

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `46a16241be0046fa52b0675daa09d0cb415af2d1`; the GitHub commit API must return the ordered parent list [67bd069ffb1a37d6150a859d8a8d36cf6064b1fa, 03219efaad1cf348a20d383728a418284861b3e3]; the contents API must return workflow blob `1813662691aea512b4c8c087a388b0e9e87e66fa`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.39.0/CHANGELOG.release.md
docs/releases/v0.39.0/candidate-assets.v1.json
docs/releases/v0.39.0/input-manifest.v1.json
docs/releases/v0.39.0/predecessor-assets.v1.json
docs/releases/v0.39.0/release-authority-v1.txt
docs/releases/v0.39.0/release-body.md
docs/releases/v0.39.0/release-plan.md
docs/releases/v0.39.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[46a16241be0046fa52b0675daa09d0cb415af2d1, HARDENING_HEAD]
```

Set `HARDENING_MERGE_SHA` to that merge commit. A squash, rebase, reversed parent order, extra parent, or intervening main commit invalidates this bundle.

### Generic workflow bootstrap in every job

Every job checks out the pushed tag with full history before useful work. Its first executable step must implement assertions equivalent to:

```bash
set -euo pipefail
test "$GITHUB_REF_TYPE" = "tag"
test "$GITHUB_REF_NAME" = "${GITHUB_REF#refs/tags/}"
TAG="$GITHUB_REF_NAME"
[[ "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]
HEAD_COMMIT="$(git rev-parse 'HEAD^{commit}')"
PEELED_COMMIT="$(git rev-parse "$TAG^{}")"
TAG_OBJECT="$(git rev-parse "$TAG^{tag}")"
test "$GITHUB_SHA" = "${{ github.sha }}"
test "$GITHUB_SHA" = "$HEAD_COMMIT"
test "$HEAD_COMMIT" = "$PEELED_COMMIT"
test "$TAG_OBJECT" != "$PEELED_COMMIT"
mapfile -t REMOTE_TAG < <(git ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")
test "${#REMOTE_TAG[@]}" -eq 2
printf '%s\n' "${REMOTE_TAG[@]}" | grep -Fx "$TAG_OBJECT"$'\t'"refs/tags/$TAG"
printf '%s\n' "${REMOTE_TAG[@]}" | grep -Fx "$PEELED_COMMIT"$'\t'"refs/tags/$TAG^{}"
```

`GITHUB_SHA`, `${{ github.sha }}`, `HEAD`, and the peeled commit are one identity. The annotated object is a second identity. `guard` exports both; every later job compares its local and remote pair to those immutable outputs. Printing the pair without assertions is forbidden.

The strict `release-authority-v1` parser must reject reordered, missing, extra, duplicate, unknown, or malformed fields; recompute the review-set digest from candidate-tree files; and compare every base/predecessor/source/policy value with the manifest.

### Tag-state semantics

- Before tag creation only: candidate absence is required across remote Git, GitHub Releases, and npm.
- In the tag workflow and every rerun: absence is failure. Exactly one existing annotated-object record and one peeled record must equal the guard pair and canonical payload.
- A different object on the same peeled commit is a moved tag and fails.

No claim of “no bypass” or immutable ref is allowed. Bind the tag equivalently by exporting the exact annotated-object/peeled pair from guard, re-querying and asserting it before useful work in every job/rerun, and repeating it after workflow completion. Any movement invalidates the run and requires a new version if publication occurred.

### Release construction and permissions

Keep this order:

```text
guard -> platform producers -> checksums -> publish-github -> publish-npm
```

`publish-github` makes the Release public only after the exact 17-entry candidate ledger, SHA-256 file, downloadable required assets, and attestations verify. `publish-npm` runs only after the GitHub Release is public and immutable. Use string-valued `make_latest: 'true'`.

`build`, `checksums`, and `publish-github` each receive job-local `contents: write` because they directly upload or mutate Release assets/state; none receives `id-token`.

Only `publish-npm` receives `id-token: write`; it receives `contents: read`, uses `actions/setup-node` at an immutable commit with `node-version: 22` and `registry-url: https://registry.npmjs.org`, installs exact npm `11.6.2`, asserts both versions, and publishes with provenance. It must fail if `NODE_AUTH_TOKEN`, `NPM_TOKEN`, or the legacy package token is present. No other job receives OIDC.

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.101.0_linux_amd64.tar.gz`, verify SHA-256 `9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8`, install from that verified archive, and require `gh version 2.101.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.39.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.39.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

Predecessor ledger: exactly 17 unique uploaded nonempty assets. Candidate ledger: exactly 17 unique names derived from predecessor version substitution plus exactly these declared candidate-only assets:

- No candidate-only assets are declared.

Any missing, extra, duplicate, zero-size, digest mismatch, producer mismatch, or name mismatch fails.

## 6. Version/evidence PR and exact topology

Branch from exactly `HARDENING_MERGE_SHA`. The exact changed-path allowlist is:

```text
CHANGELOG.md
package.json
package-lock.json
npm/package.json
npm/install.js
src-tauri/Cargo.toml
Cargo.lock
src-tauri/tauri.conf.json
```

The root `CHANGELOG.md` must equal generated `CHANGELOG.release.md`. All generated evidence already committed by the hardening PR remains byte-identical. Record `VERSION_HEAD` only after exact-head review and every required check/version-sync/package gate passes.

After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[HARDENING_MERGE_SHA, VERSION_HEAD]
```

Set `VERSION_MERGE_SHA` and `FINAL_CANDIDATE_MAIN` to that commit. Ancestry alone is insufficient.

The repository currently has no second eligible identity. After every required check and version-sync passes on the exact head, `mblua` may use the documented admin merge path solely to satisfy unavailable self-review. Admin authority must never override a failed, missing, pending, or stale check.

## 7. Final pre-tag gates

Immediately before tagging, all of these must pass against fresh remote/API state:

- remote main and GitHub default-branch ref both equal `FINAL_CANDIDATE_MAIN`, not the old planning base;
- exact hardening and version ordered parent vectors match section 5 and 6;
- the workflow and generated bundle exist at the final candidate with exact reviewed hashes;
- workflow parser/permissions/action/CLI/npm/attestation fixtures pass;
- every version/package/changelog/asset contract passes;
- candidate tag, Release, and npm version remain unambiguously absent;
- required status checks passed on each exact reviewed PR head;
- review authority matches the documented policy, with no unmodeled bypass.

Any failure spends no version: stop without creating a tag.

## 8. Annotated tag and executable post-push proof

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.39.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.39.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
