# Release preparation plan: v0.47.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/4433  
Candidate: `0.47.0` / `v0.47.0`  
Planning base: `6bc26aed03211c584a06259d4c72025067235aa5`  
Predecessor: `v0.46.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.47.0/` before tagging.

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

- `docs/releases/v0.47.0/CHANGELOG.release.md`
- `docs/releases/v0.47.0/candidate-assets.v1.json`
- `docs/releases/v0.47.0/input-manifest.v1.json`
- `docs/releases/v0.47.0/predecessor-assets.v1.json`
- `docs/releases/v0.47.0/release-authority-v1.txt`
- `docs/releases/v0.47.0/release-body.md`
- `docs/releases/v0.47.0/release-plan.md`
- `docs/releases/v0.47.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `6bc26aed03211c584a06259d4c72025067235aa5`.
- Ordered planning-base parents: [970ebc2ed2d92b7b4c4bd36e05e618ebf6bfa1e7, b4b7eeaa7fc9d6f09539c536290af28d61ddd383].
- Base `.github/workflows/release.yml` blob: `d5056bab9c93e34c98b23b61ce46585bc332988e`; content SHA-256: `1db08753c700d5773aa02dead4748365cca799c12c82863e243ceb5bc10c37ec`.
- Predecessor annotated object: `a9ff63625a899c4d9ec4299c1699ece1e542b4e3`; peeled commit: `656558f0c8ace28c9cfea556d07f194ab7e00873`.
- Predecessor immutable GitHub Release id: `433310996`.
- npm latest: `0.43.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.104.0`, archive `gh_2.104.0_linux_amd64.tar.gz`, SHA-256 `98cb101ad87dbbd3f6b2f2bfcbd43197ccd07643140121d8a9cfaa91eadbc24e`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (discovered from `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.46.0` (blob `52af2e47a5189ffc77158a46ed320d81c0545d01`)
- `package-lock.json` `/version`: `0.46.0` (blob `1be90b6a31948aa2c148ef9479ff7c09ead9fece`)
- `package-lock.json` `/packages//version`: `0.46.0` (blob `1be90b6a31948aa2c148ef9479ff7c09ead9fece`)
- `npm/package.json` `/version`: `0.46.0` (blob `684cce903caeeb905f09610a84d34f75563a230e`)
- `npm/install.js` `const VERSION`: `0.46.0` (blob `05e93ca6f9eddc7d93dfe566370fc479af23cadb`)
- `src-tauri/Cargo.toml` `[package].version`: `0.46.0` (blob `9f7e26b39fbd48849e759e47fe572e5eb58dff41`)
- `Cargo.lock` `agentscommander.version`: `0.46.0` (blob `28ee39eed4b5ffb5823e0746ae0b7f56861c2520`)
- `src-tauri/tauri.conf.json` `/version`: `0.46.0` (blob `06022a6624d39ffe0483bfa555875a5f4739cfee`)

Approved scope:

- maintenance: ci: skip npm install scripts in regression-gate installs, Sonar G8 (closes #2076, #2077) ([#2077](https://github.com/mblua/AgentsCommander/issues/2077), [PR #4180](https://github.com/mblua/AgentsCommander/pull/4180))
- feature: feat(screenshot): Wayland/Hyprland capture and clickable camera chip (refs #2774) ([#2774](https://github.com/mblua/AgentsCommander/issues/2774), [PR #4249](https://github.com/mblua/AgentsCommander/pull/4249))
- feature: feat: local Coding Agent and Profile preflights (refs #3516) ([#3516](https://github.com/mblua/AgentsCommander/issues/3516), [PR #3523](https://github.com/mblua/AgentsCommander/pull/3523))
- feature: feat(context-history): context heatmap (refs #3532) ([#3532](https://github.com/mblua/AgentsCommander/issues/3532), [PR #3921](https://github.com/mblua/AgentsCommander/pull/3921))
- feature: feat(preflight): Test preflights UX (refs #3832) ([#3832](https://github.com/mblua/AgentsCommander/issues/3832), [PR #3909](https://github.com/mblua/AgentsCommander/pull/3909))
- maintenance: chore(build): set static VC runtime explicitly, drop deprecated STATIC_VCRUNTIME (refs #4047) ([#4047](https://github.com/mblua/AgentsCommander/issues/4047), [PR #4057](https://github.com/mblua/AgentsCommander/pull/4057))
- feature: feat(settings): context heat map toggle in Settings, hide CTX when off (refs #4049) ([#4049](https://github.com/mblua/AgentsCommander/issues/4049), [PR #4078](https://github.com/mblua/AgentsCommander/pull/4078))
- feature: feat(context-history): heat map by session count (#4091) ([#4091](https://github.com/mblua/AgentsCommander/issues/4091), [PR #4097](https://github.com/mblua/AgentsCommander/pull/4097))
- maintenance: ci: enforce gate 04 on PR trees with relocation-safe debt (#4098) ([#4098](https://github.com/mblua/AgentsCommander/issues/4098), [PR #4122](https://github.com/mblua/AgentsCommander/pull/4122))
- fix: fix(container): keep last real global-sweep error on deadline-only attempts (#4151) ([#4151](https://github.com/mblua/AgentsCommander/issues/4151), [PR #4156](https://github.com/mblua/AgentsCommander/pull/4156))
- maintenance: ci: fail the complexity gate on unused cognitive-complexity allowlist entries ([#4166](https://github.com/mblua/AgentsCommander/issues/4166), [PR #4167](https://github.com/mblua/AgentsCommander/pull/4167))
- maintenance: fix: resolve Sonar new-code reliability issues, lot L1 (sort compare and floating promises) ([#4170](https://github.com/mblua/AgentsCommander/issues/4170), [PR #4172](https://github.com/mblua/AgentsCommander/pull/4172))
- security: fix: resolve Sonar new-code issues, lot L2 (floating promises and log injection) ([#4171](https://github.com/mblua/AgentsCommander/issues/4171), [PR #4173](https://github.com/mblua/AgentsCommander/pull/4173))
- maintenance: ci: add --ignore-scripts to npm installs in bundle-validation and cache-warm (#4181) ([#4181](https://github.com/mblua/AgentsCommander/issues/4181), [PR #4184](https://github.com/mblua/AgentsCommander/pull/4184))
- security: fix(scripts): sanitize data.state in branch validator log (Sonar S5145) (#4182) ([#4182](https://github.com/mblua/AgentsCommander/issues/4182), [PR #4183](https://github.com/mblua/AgentsCommander/pull/4183))
- maintenance: chore(git): enforce LF for all text files via .gitattributes (refs #4243) ([#4243](https://github.com/mblua/AgentsCommander/issues/4243), [PR #4245](https://github.com/mblua/AgentsCommander/pull/4245))
- fix: fix(terminal): pick a font that really renders monospace on Linux (refs #4244) ([#4244](https://github.com/mblua/AgentsCommander/issues/4244), [PR #4247](https://github.com/mblua/AgentsCommander/pull/4247))
- maintenance: Testable mode on Linux (#4248) ([#4248](https://github.com/mblua/AgentsCommander/issues/4248), [PR #4257](https://github.com/mblua/AgentsCommander/pull/4257))
- fix: fix(ui-automation): ui-wait retries startup preflight errors (refs #4255) ([#4255](https://github.com/mblua/AgentsCommander/issues/4255), [PR #4287](https://github.com/mblua/AgentsCommander/pull/4287))
- security: fix(web-browser): refuse the app origin and prove IPC isolation (#4289) ([#4289](https://github.com/mblua/AgentsCommander/issues/4289), [PR #4322](https://github.com/mblua/AgentsCommander/pull/4322))
- maintenance: ci: run frontend regression on every PR validation (#4291) ([#4291](https://github.com/mblua/AgentsCommander/issues/4291), [PR #4328](https://github.com/mblua/AgentsCommander/pull/4328))
- maintenance: refactor: simplify sidebar nested conditionals (#4297 lot B) ([#4297](https://github.com/mblua/AgentsCommander/issues/4297), [PR #4323](https://github.com/mblua/AgentsCommander/pull/4323))
- security: test(security): guard raw HTML sinks in frontend sources (#4300) ([#4300](https://github.com/mblua/AgentsCommander/issues/4300), [PR #4333](https://github.com/mblua/AgentsCommander/pull/4333))
- maintenance: refactor(tests): group similar tests into parameterized tests (S5976) (#4326) ([#4326](https://github.com/mblua/AgentsCommander/issues/4326), [PR #4327](https://github.com/mblua/AgentsCommander/pull/4327))
- maintenance: refactor(ci): name positional parameters in release.yml shell helpers (#4351) ([#4351](https://github.com/mblua/AgentsCommander/issues/4351), [PR #4352](https://github.com/mblua/AgentsCommander/pull/4352))
- maintenance: refactor: Sonar L1 part A in sanitizing_containers and ipc (#4410) ([#4410](https://github.com/mblua/AgentsCommander/issues/4410), [PR #4411](https://github.com/mblua/AgentsCommander/pull/4411))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `6bc26aed03211c584a06259d4c72025067235aa5`; the GitHub commit API must return the ordered parent list [970ebc2ed2d92b7b4c4bd36e05e618ebf6bfa1e7, b4b7eeaa7fc9d6f09539c536290af28d61ddd383]; the contents API must return workflow blob `d5056bab9c93e34c98b23b61ce46585bc332988e`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.47.0/CHANGELOG.release.md
docs/releases/v0.47.0/candidate-assets.v1.json
docs/releases/v0.47.0/input-manifest.v1.json
docs/releases/v0.47.0/predecessor-assets.v1.json
docs/releases/v0.47.0/release-authority-v1.txt
docs/releases/v0.47.0/release-body.md
docs/releases/v0.47.0/release-plan.md
docs/releases/v0.47.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[6bc26aed03211c584a06259d4c72025067235aa5, HARDENING_HEAD]
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

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.104.0_linux_amd64.tar.gz`, verify SHA-256 `98cb101ad87dbbd3f6b2f2bfcbd43197ccd07643140121d8a9cfaa91eadbc24e`, install from that verified archive, and require `gh version 2.104.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.47.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.47.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.47.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.47.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
