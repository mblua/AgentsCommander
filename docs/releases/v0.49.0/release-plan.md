# Release preparation plan: v0.49.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/5242  
Candidate: `0.49.0` / `v0.49.0`  
Planning base: `619a09c4f98cba34e21fdc96c488e63a5b358327`  
Predecessor: `v0.48.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.49.0/` before tagging.

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

- `docs/releases/v0.49.0/CHANGELOG.release.md`
- `docs/releases/v0.49.0/candidate-assets.v1.json`
- `docs/releases/v0.49.0/input-manifest.v1.json`
- `docs/releases/v0.49.0/predecessor-assets.v1.json`
- `docs/releases/v0.49.0/release-authority-v1.txt`
- `docs/releases/v0.49.0/release-body.md`
- `docs/releases/v0.49.0/release-plan.md`
- `docs/releases/v0.49.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `619a09c4f98cba34e21fdc96c488e63a5b358327`.
- Ordered planning-base parents: [f57e28ad5588a5a48ea714a2313f86984734b939, dd3a4198605ecc6fb36d2c118f1f34980096cb9f].
- Base `.github/workflows/release.yml` blob: `bf5e5605952915b14fb6145c6bb0d4ee54ee1e44`; content SHA-256: `270320a6185e9d2e566f7a95c8cb0766bb56ef56854b5f4d2785bbd2e541373b`.
- Predecessor annotated object: `240b73876760390e5139d0eed3bdc99ab2e2887e`; peeled commit: `8cdd160fe3395421831f63dac7ff4aef61261bd6`.
- Predecessor immutable GitHub Release id: `453713528`.
- npm latest: `0.48.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.105.0`, archive `gh_2.105.0_linux_amd64.tar.gz`, SHA-256 `79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (discovered from `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.48.0` (blob `b28ba94150e7304a644c0eb8a1d07ba849def883`)
- `package-lock.json` `/version`: `0.48.0` (blob `b7fc5ac437781737bff1613984ed7caa7d711597`)
- `package-lock.json` `/packages//version`: `0.48.0` (blob `b7fc5ac437781737bff1613984ed7caa7d711597`)
- `npm/package.json` `/version`: `0.48.0` (blob `9c0dcb7469cdb70f57d3ae6bd77a1406b1badef3`)
- `npm/install.js` `const VERSION`: `0.48.0` (blob `31bbdd21f413e73b877a6ff387a0e63341cb7852`)
- `src-tauri/Cargo.toml` `[package].version`: `0.48.0` (blob `91b116b9d217bff5638dcc372b180d076dbe5770`)
- `Cargo.lock` `agentscommander.version`: `0.48.0` (blob `78a36137cbca9e41201305d6a0035d68f0332c62`)
- `src-tauri/tauri.conf.json` `/version`: `0.48.0` (blob `69f9c153f1b5b6132e9c64c7f5bf18394591528c`)

Approved scope:

- maintenance: Sanitize: drop issue refs from Rust test comments, lote13+14 (#137) ([#137](https://github.com/mblua/AgentsCommander/issues/137), [PR #5047](https://github.com/mblua/AgentsCommander/pull/5047))
- fix: fix(selection): read shutdown deadlines from the Tokio clock (#1234) ([#1234](https://github.com/mblua/AgentsCommander/issues/1234), [PR #5217](https://github.com/mblua/AgentsCommander/pull/5217))
- feature: feat(settings): IPC black box toggle, off by default, applies on restart (refs #1707) ([#1707](https://github.com/mblua/AgentsCommander/issues/1707), [PR #5166](https://github.com/mblua/AgentsCommander/pull/5166))
- fix: fix(idle): team idle clock ignores PTY resize and restore repaint (closes #2158) ([#2158](https://github.com/mblua/AgentsCommander/issues/2158), [PR #4957](https://github.com/mblua/AgentsCommander/pull/4957))
- fix: fix(session): recover a quiescent stuck response-close Closing on explicit discard (refs #3317) ([#3317](https://github.com/mblua/AgentsCommander/issues/3317), [PR #4854](https://github.com/mblua/AgentsCommander/pull/4854))
- feature: feat: warn when Linux sound alerts are silent due to missing GStreamer sink (refs #4843) ([#4843](https://github.com/mblua/AgentsCommander/issues/4843), [PR #4853](https://github.com/mblua/AgentsCommander/pull/4853))
- docs: docs(readme): replace hero screenshot with presentation video (refs #4855) ([#4855](https://github.com/mblua/AgentsCommander/issues/4855), [PR #4856](https://github.com/mblua/AgentsCommander/pull/4856))
- maintenance: test(pty): wait for shutdown worker threads to end before reaping (#4857) ([#4857](https://github.com/mblua/AgentsCommander/issues/4857), [PR #5125](https://github.com/mblua/AgentsCommander/pull/5125))
- maintenance: refactor: split the Linux/macOS-only too_many_lines baseline functions (closes #4859) ([#4859](https://github.com/mblua/AgentsCommander/issues/4859), [PR #4860](https://github.com/mblua/AgentsCommander/pull/4860))
- docs: docs: module docs for the remaining top-level Rust files (closes #4861) ([#4861](https://github.com/mblua/AgentsCommander/issues/4861), [PR #4865](https://github.com/mblua/AgentsCommander/pull/4865))
- maintenance: refactor: add a reason to every allow(clippy::...) (refs #4862) ([#4862](https://github.com/mblua/AgentsCommander/issues/4862), [PR #4868](https://github.com/mblua/AgentsCommander/pull/4868))
- security: fix(deps): update url to 2.5.9 for RUSTSEC-2026-0592 (closes #4863) ([#4863](https://github.com/mblua/AgentsCommander/issues/4863), [PR #4864](https://github.com/mblua/AgentsCommander/pull/4864))
- maintenance: refactor: remove or justify allow(dead_code) in 30 files (closes #4866) ([#4866](https://github.com/mblua/AgentsCommander/issues/4866), [PR #4878](https://github.com/mblua/AgentsCommander/pull/4878))
- maintenance: chore(lint): deny todo, unimplemented and dbg_macro (closes #4867) ([#4867](https://github.com/mblua/AgentsCommander/issues/4867), [PR #4870](https://github.com/mblua/AgentsCommander/pull/4870))
- maintenance: refactor: remove 50 clippy allows that no longer fire (closes #4869) ([#4869](https://github.com/mblua/AgentsCommander/issues/4869), [PR #4900](https://github.com/mblua/AgentsCommander/pull/4900))
- maintenance: ci: pin Node 22 (engines + .nvmrc); confirm --locked and npm ci (closes #4896) ([#4896](https://github.com/mblua/AgentsCommander/issues/4896), [PR #4897](https://github.com/mblua/AgentsCommander/pull/4897))
- fix: fix: resolve 7 Sonar accessibility and nested-ternary findings in sidebar components (refs #4924) ([#4924](https://github.com/mblua/AgentsCommander/issues/4924), [PR #4962](https://github.com/mblua/AgentsCommander/pull/4962))
- fix: fix: bound final session persistence at exit (refs #4929) ([#4929](https://github.com/mblua/AgentsCommander/issues/4929), [PR #4934](https://github.com/mblua/AgentsCommander/pull/4934))
- maintenance: ci: restore the PR APT cache through one local composite action (P01, Refs #4944) ([#4944](https://github.com/mblua/AgentsCommander/issues/4944), [PR #5225](https://github.com/mblua/AgentsCommander/pull/5225))
- fix: fix(sidebar): resolve #4945 batch-2 Sonar accessibility findings ([#4945](https://github.com/mblua/AgentsCommander/issues/4945), [PR #5183](https://github.com/mblua/AgentsCommander/pull/5183))
- maintenance: refactor(pty): break pty::output module cycle (#4963) ([#4963](https://github.com/mblua/AgentsCommander/issues/4963), [PR #4965](https://github.com/mblua/AgentsCommander/pull/4965))
- maintenance: refactor(sanitize): cut session::context_alerts SCC via leaf model.rs (#4976) ([#4976](https://github.com/mblua/AgentsCommander/issues/4976), [PR #4981](https://github.com/mblua/AgentsCommander/pull/4981))
- security: fix(deps): bump cookie to 0.18.3 (RUSTSEC-2026-0594) ([#4987](https://github.com/mblua/AgentsCommander/issues/4987), [PR #4988](https://github.com/mblua/AgentsCommander/pull/4988))
- fix: fix(a11y): S6819 native elements in sidebar, main window and resource monitor (#5061) ([#5061](https://github.com/mblua/AgentsCommander/issues/5061), [PR #5220](https://github.com/mblua/AgentsCommander/pull/5220))
- feature: feat: coding agents export/import buttons (refs #5071) ([#5071](https://github.com/mblua/AgentsCommander/issues/5071), [PR #5129](https://github.com/mblua/AgentsCommander/pull/5129))
- fix: fix: write CLI outbox messages atomically (#5081) ([#5081](https://github.com/mblua/AgentsCommander/issues/5081), [PR #5082](https://github.com/mblua/AgentsCommander/pull/5082))
- fix: fix(sidebar): switch Open Agent from repo list to agent picker (Closes #5103) ([#5103](https://github.com/mblua/AgentsCommander/issues/5103), [PR #5213](https://github.com/mblua/AgentsCommander/pull/5213))
- fix: fix(settings): show export env-values warning toast in red (refs #5130) ([#5130](https://github.com/mblua/AgentsCommander/issues/5130), [PR #5131](https://github.com/mblua/AgentsCommander/pull/5131))
- feature: feat(settings): preflight command tooltip lists supported variables (refs #5139) ([#5139](https://github.com/mblua/AgentsCommander/issues/5139), [PR #5143](https://github.com/mblua/AgentsCommander/pull/5143))
- fix: fix(ui): readable select dropdowns on Linux WebKitGTK (refs #5140) ([#5140](https://github.com/mblua/AgentsCommander/issues/5140), [PR #5200](https://github.com/mblua/AgentsCommander/pull/5200))
- fix: fix(sidebar): name the agent in session warning toasts, refs #5146 ([#5146](https://github.com/mblua/AgentsCommander/issues/5146), [PR #5150](https://github.com/mblua/AgentsCommander/pull/5150))
- maintenance: perf(pty): pass PTY output to the fan-out as a byte slice (#5176) ([#5176](https://github.com/mblua/AgentsCommander/issues/5176), [PR #5181](https://github.com/mblua/AgentsCommander/pull/5181))
- feature: feat(screenshot): copy the captured image to the clipboard by default (refs #5178) ([#5178](https://github.com/mblua/AgentsCommander/issues/5178), [PR #5199](https://github.com/mblua/AgentsCommander/pull/5199))
- maintenance: perf(pty): scan for printable output without building a stripped copy (#5179) ([#5179](https://github.com/mblua/AgentsCommander/issues/5179), [PR #5182](https://github.com/mblua/AgentsCommander/pull/5182))
- maintenance: deps: minimal tokio features instead of full (#5187) ([#5187](https://github.com/mblua/AgentsCommander/issues/5187), [PR #5188](https://github.com/mblua/AgentsCommander/pull/5188))
- maintenance: deps: drop unused reqwest blocking feature (#5194) ([#5194](https://github.com/mblua/AgentsCommander/issues/5194), [PR #5198](https://github.com/mblua/AgentsCommander/pull/5198))
- docs: docs(readme): replace the 60-second presentation video with the 108-second cut (refs #5207) ([#5207](https://github.com/mblua/AgentsCommander/issues/5207), [PR #5208](https://github.com/mblua/AgentsCommander/pull/5208))
- docs: docs(readme): replace the presentation video with the cut that names Linux, Windows and macOS (refs #5222) ([#5222](https://github.com/mblua/AgentsCommander/issues/5222), [PR #5227](https://github.com/mblua/AgentsCommander/pull/5227))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `619a09c4f98cba34e21fdc96c488e63a5b358327`; the GitHub commit API must return the ordered parent list [f57e28ad5588a5a48ea714a2313f86984734b939, dd3a4198605ecc6fb36d2c118f1f34980096cb9f]; the contents API must return workflow blob `bf5e5605952915b14fb6145c6bb0d4ee54ee1e44`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.49.0/CHANGELOG.release.md
docs/releases/v0.49.0/candidate-assets.v1.json
docs/releases/v0.49.0/input-manifest.v1.json
docs/releases/v0.49.0/predecessor-assets.v1.json
docs/releases/v0.49.0/release-authority-v1.txt
docs/releases/v0.49.0/release-body.md
docs/releases/v0.49.0/release-plan.md
docs/releases/v0.49.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[619a09c4f98cba34e21fdc96c488e63a5b358327, HARDENING_HEAD]
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

Only `publish-npm` receives `id-token: write`; it receives `contents: read`, uses `actions/setup-node` at an immutable commit with `node-version: 22` and `registry-url: https://registry.npmjs.org`, installs exact npm `12.2.3`, asserts both versions, and publishes with provenance. It must fail if `NODE_AUTH_TOKEN`, `NPM_TOKEN`, or the legacy package token is present. No other job receives OIDC.

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.105.0_linux_amd64.tar.gz`, verify SHA-256 `79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f`, install from that verified archive, and require `gh version 2.105.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.49.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.49.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.49.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.49.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
