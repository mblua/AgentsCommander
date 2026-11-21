# Release preparation plan: v0.48.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/4850  
Candidate: `0.48.0` / `v0.48.0`  
Planning base: `2d636ff06c5ede132d5ac652c930edf2b75bb45a`  
Predecessor: `v0.47.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.48.0/` before tagging.

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

- `docs/releases/v0.48.0/CHANGELOG.release.md`
- `docs/releases/v0.48.0/candidate-assets.v1.json`
- `docs/releases/v0.48.0/input-manifest.v1.json`
- `docs/releases/v0.48.0/predecessor-assets.v1.json`
- `docs/releases/v0.48.0/release-authority-v1.txt`
- `docs/releases/v0.48.0/release-body.md`
- `docs/releases/v0.48.0/release-plan.md`
- `docs/releases/v0.48.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `2d636ff06c5ede132d5ac652c930edf2b75bb45a`.
- Ordered planning-base parents: [01880c2b69085bf4a5355e07366e9e45b3789efd, 39f00c5268ceb0d4315137d668156e93af016c53].
- Base `.github/workflows/release.yml` blob: `bf5e5605952915b14fb6145c6bb0d4ee54ee1e44`; content SHA-256: `270320a6185e9d2e566f7a95c8cb0766bb56ef56854b5f4d2785bbd2e541373b`.
- Predecessor annotated object: `281e2d09d668bff3c82518e9dbcbd1774083dc14`; peeled commit: `76eee8875b8f6320970ec7c21db63d28fa24f840`.
- Predecessor immutable GitHub Release id: `443792659`.
- npm latest: `0.47.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
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

- `package.json` `/version`: `0.47.0` (blob `a2efc7f2b21920b64a2b3a2af2c9895398ed824a`)
- `package-lock.json` `/version`: `0.47.0` (blob `430a048c8620a60fe7371486ae066f9b0295c490`)
- `package-lock.json` `/packages//version`: `0.47.0` (blob `430a048c8620a60fe7371486ae066f9b0295c490`)
- `npm/package.json` `/version`: `0.47.0` (blob `7769fc0fae2029a9c955c9dbe6d5b446d6466977`)
- `npm/install.js` `const VERSION`: `0.47.0` (blob `7a4483bc149d7901d4781561d47402b6e604e5e0`)
- `src-tauri/Cargo.toml` `[package].version`: `0.47.0` (blob `db3f8f9460996e45129976c36faf8aedb78effee`)
- `Cargo.lock` `agentscommander.version`: `0.47.0` (blob `9bc50431f7445efd0bb000da98f23aa4d4f42015`)
- `src-tauri/tauri.conf.json` `/version`: `0.47.0` (blob `0a53e7bf77a871f475f9fa9195a0977dc1da2545`)

Approved scope:

- fix: fix(#98): recover poisoned std locks and forbid panicking lock acquisition ([#98](https://github.com/mblua/AgentsCommander/issues/98), [PR #4730](https://github.com/mblua/AgentsCommander/pull/4730))
- fix: fix(mailbox): end stuck wakes instead of blocking delivery forever (refs #3263) ([#3263](https://github.com/mblua/AgentsCommander/issues/3263), [PR #4816](https://github.com/mblua/AgentsCommander/pull/4816))
- feature: feat(#4248): Linux window snapshot via ui-backend window.snapshot ([#4248](https://github.com/mblua/AgentsCommander/issues/4248), [PR #4848](https://github.com/mblua/AgentsCommander/pull/4848))
- security: fix: validate external PTY text before delivery (#4309) ([#4309](https://github.com/mblua/AgentsCommander/issues/4309), [PR #4522](https://github.com/mblua/AgentsCommander/pull/4522))
- maintenance: ci: run cargo deny on every PR (#4315) ([#4315](https://github.com/mblua/AgentsCommander/issues/4315), [PR #4337](https://github.com/mblua/AgentsCommander/pull/4337))
- security: fix(#4330): restrict Tauri capabilities per window ([#4330](https://github.com/mblua/AgentsCommander/issues/4330), [PR #4469](https://github.com/mblua/AgentsCommander/pull/4469))
- maintenance: ci: add ESLint first slice and PR lint job (#4460) ([#4460](https://github.com/mblua/AgentsCommander/issues/4460), [PR #4537](https://github.com/mblua/AgentsCommander/pull/4537))
- fix: fix: recover from window render errors (#4461) ([#4461](https://github.com/mblua/AgentsCommander/issues/4461), [PR #4479](https://github.com/mblua/AgentsCommander/pull/4479))
- maintenance: build: pin Rust 1.98.1 via rust-toolchain.toml (refs #4473) ([#4473](https://github.com/mblua/AgentsCommander/issues/4473), [PR #4530](https://github.com/mblua/AgentsCommander/pull/4530))
- security: fix(telegram): redact bot token in watcher Debug and match redact contexts in any case (#4475) ([#4475](https://github.com/mblua/AgentsCommander/issues/4475), [PR #4487](https://github.com/mblua/AgentsCommander/pull/4487))
- maintenance: ci: npm audit gate with a committed baseline (closes #4490) ([#4490](https://github.com/mblua/AgentsCommander/issues/4490), [PR #4738](https://github.com/mblua/AgentsCommander/pull/4738))
- maintenance: chore: generate and bundle third-party notices (#4491) ([#4491](https://github.com/mblua/AgentsCommander/issues/4491), [PR #4528](https://github.com/mblua/AgentsCommander/pull/4528))
- docs: docs(security): add evidence-backed threat model (#4493) ([#4493](https://github.com/mblua/AgentsCommander/issues/4493), [PR #4509](https://github.com/mblua/AgentsCommander/pull/4509))
- docs: docs(architecture): verify module map and record checked commit (#4526) ([#4526](https://github.com/mblua/AgentsCommander/issues/4526), [PR #4532](https://github.com/mblua/AgentsCommander/pull/4532))
- maintenance: feat(ci): publish per-test flaky rate and advisory flaky-test issues (closes #4527) ([#4527](https://github.com/mblua/AgentsCommander/issues/4527), [PR #4718](https://github.com/mblua/AgentsCommander/pull/4718))
- maintenance: chore(tsconfig): enable noImplicitOverride (#4571 P01) ([#4571](https://github.com/mblua/AgentsCommander/issues/4571), [PR #4585](https://github.com/mblua/AgentsCommander/pull/4585))
- docs: docs(state): map frontend state owners and guard the map (#4580 P01) ([#4580](https://github.com/mblua/AgentsCommander/issues/4580), [PR #4594](https://github.com/mblua/AgentsCommander/pull/4594))
- security: fix(rtk): Claude hooks rewrite without auto-approving (refs #4583) ([#4583](https://github.com/mblua/AgentsCommander/issues/4583), [PR #4586](https://github.com/mblua/AgentsCommander/pull/4586))
- maintenance: ci(codeql): add CodeQL security analysis for TypeScript and Rust (#4591) ([#4591](https://github.com/mblua/AgentsCommander/issues/4591), [PR #4633](https://github.com/mblua/AgentsCommander/pull/4633))
- maintenance: build(frontend): add pinned Prettier config and format scripts (#4596 P01) ([#4596](https://github.com/mblua/AgentsCommander/issues/4596), [PR #4615](https://github.com/mblua/AgentsCommander/pull/4615))
- fix: fix(spec_board): path_is_under uses the key separator on every OS (#4604) ([#4604](https://github.com/mblua/AgentsCommander/issues/4604), [PR #4631](https://github.com/mblua/AgentsCommander/pull/4631))
- security: fix(security): redact tokens and API keys in remaining Debug impls (#4637) ([#4637](https://github.com/mblua/AgentsCommander/issues/4637), [PR #4660](https://github.com/mblua/AgentsCommander/pull/4660))
- security: fix(security): redact secrets in AppSettings and LocalSettingsOverlay Debug (#4638) ([#4638](https://github.com/mblua/AgentsCommander/issues/4638), [PR #4745](https://github.com/mblua/AgentsCommander/pull/4745))
- fix: fix(pty): skip 10s cleanup wait when no shutdown producer exists (#4643) ([#4643](https://github.com/mblua/AgentsCommander/issues/4643), [PR #4688](https://github.com/mblua/AgentsCommander/pull/4688))
- docs: docs: add module docs to top-level Rust modules (refs #4653) ([#4653](https://github.com/mblua/AgentsCommander/issues/4653), [PR #4662](https://github.com/mblua/AgentsCommander/pull/4662))
- maintenance: ci: pin GitHub Actions to commit SHAs and block tag pins (#4690) ([#4690](https://github.com/mblua/AgentsCommander/issues/4690), [PR #4700](https://github.com/mblua/AgentsCommander/pull/4700))
- maintenance: ci: PR net-line budget check with written justification (closes #4715) ([#4715](https://github.com/mblua/AgentsCommander/issues/4715), [PR #4736](https://github.com/mblua/AgentsCommander/pull/4736))
- feature: feat: hide the sidebar WWW button behind webBrowserEnabled (#4751) ([#4751](https://github.com/mblua/AgentsCommander/issues/4751), [PR #4753](https://github.com/mblua/AgentsCommander/pull/4753))
- feature: feat(#4752): P01 move integrity rules into their own context block ([#4754](https://github.com/mblua/AgentsCommander/issues/4754), [PR #4762](https://github.com/mblua/AgentsCommander/pull/4762))
- feature: feat(#4752): P02 persist the Unleash flag and skip Git brakes for unleashed replicas ([#4755](https://github.com/mblua/AgentsCommander/issues/4755), [PR #4761](https://github.com/mblua/AgentsCommander/pull/4761))
- feature: feat(#4752): P03 omit workspace restrictions for unleashed replicas ([#4756](https://github.com/mblua/AgentsCommander/issues/4756), [PR #4764](https://github.com/mblua/AgentsCommander/pull/4764))
- feature: feat(#4752): P04 add the Unleash command and discovery field ([#4757](https://github.com/mblua/AgentsCommander/issues/4757), [PR #4765](https://github.com/mblua/AgentsCommander/pull/4765))
- feature: feat(#4752): P05 add the Unleash/Leash menu item and confirmation ([#4758](https://github.com/mblua/AgentsCommander/issues/4758), [PR #4768](https://github.com/mblua/AgentsCommander/pull/4768))
- feature: feat(#4752): P06 show a breathing red halo on unleashed tiles ([#4759](https://github.com/mblua/AgentsCommander/issues/4759), [PR #4772](https://github.com/mblua/AgentsCommander/pull/4772))
- feature: feat(#4752): P07 drop Unleash flags when a room or replica is deleted ([#4760](https://github.com/mblua/AgentsCommander/issues/4760), [PR #4763](https://github.com/mblua/AgentsCommander/pull/4763))
- maintenance: chore(deps): apply semver-compatible npm and Cargo updates (refs #4769) ([#4769](https://github.com/mblua/AgentsCommander/issues/4769), [PR #4773](https://github.com/mblua/AgentsCommander/pull/4773))
- fix: fix: persist user-input session state only on Enter (#4778) ([#4778](https://github.com/mblua/AgentsCommander/issues/4778), [PR #4781](https://github.com/mblua/AgentsCommander/pull/4781))
- feature: feat(temp-rooms): Temporary Rooms with a bare agent, stage 1 (refs #4779) ([#4779](https://github.com/mblua/AgentsCommander/issues/4779), [PR #4782](https://github.com/mblua/AgentsCommander/pull/4782))
- feature: feat: temporary rooms stage 2 — room-limited context, AC CLI and messaging, Unleash (refs #4780) ([#4780](https://github.com/mblua/AgentsCommander/issues/4780), [PR #4809](https://github.com/mblua/AgentsCommander/pull/4809))
- fix: fix(settings): keep outside-written keys on Settings save (refs #4783) ([#4783](https://github.com/mblua/AgentsCommander/issues/4783), [PR #4789](https://github.com/mblua/AgentsCommander/pull/4789))
- fix: fix(sidebar): one regex filter on the Temporary Rooms header (refs #4785) ([#4785](https://github.com/mblua/AgentsCommander/issues/4785), [PR #4792](https://github.com/mblua/AgentsCommander/pull/4792))
- fix: fix(temp-rooms): resolve %AC_REPLICA_ROOT% and warn on unavailable placeholders (refs #4786) ([#4786](https://github.com/mblua/AgentsCommander/issues/4786), [PR #4793](https://github.com/mblua/AgentsCommander/pull/4793))
- fix: fix: enforce Linux GUI single-instance per executable name (refs #4788) ([#4788](https://github.com/mblua/AgentsCommander/issues/4788), [PR #4791](https://github.com/mblua/AgentsCommander/pull/4791))
- fix: fix: open CTX history card after 2 s hover and translate context history UI to English (refs #4796) ([#4796](https://github.com/mblua/AgentsCommander/issues/4796), [PR #4797](https://github.com/mblua/AgentsCommander/pull/4797))
- feature: feat(agent-update): handle mise-managed coding agents (refs #4804) ([#4804](https://github.com/mblua/AgentsCommander/issues/4804), [PR #4808](https://github.com/mblua/AgentsCommander/pull/4808))
- feature: feat(temp-rooms): groups, Non-stop, favorites and TASK title for temp-room agents (refs #4805) ([#4805](https://github.com/mblua/AgentsCommander/issues/4805), [PR #4822](https://github.com/mblua/AgentsCommander/pull/4822))
- fix: fix(session): free create ticket before finalize response refs #4807 ([#4807](https://github.com/mblua/AgentsCommander/issues/4807), [PR #4812](https://github.com/mblua/AgentsCommander/pull/4812))
- feature: feat(sidebar): filter follows the stuck group header (refs #4810) ([#4810](https://github.com/mblua/AgentsCommander/issues/4810), [PR #4814](https://github.com/mblua/AgentsCommander/pull/4814))
- fix: fix(selection): record selection timing off the async worker (refs #4813) ([#4813](https://github.com/mblua/AgentsCommander/issues/4813), [PR #4815](https://github.com/mblua/AgentsCommander/pull/4815))
- fix: fix(sidebar): keep sticky project header opaque in 4 styles, refs #4821 ([#4821](https://github.com/mblua/AgentsCommander/issues/4821), [PR #4830](https://github.com/mblua/AgentsCommander/pull/4830))
- feature: feat: add data-ac-testid to sidebar menu controls (refs #4825) ([#4825](https://github.com/mblua/AgentsCommander/issues/4825), [PR #4829](https://github.com/mblua/AgentsCommander/pull/4829))
- fix: fix(sidebar): show unleash halo on Temporary Room rows (refs #4826) ([#4826](https://github.com/mblua/AgentsCommander/issues/4826), [PR #4828](https://github.com/mblua/AgentsCommander/pull/4828))
- fix: fix(sidebar): key replica menu and row testids by path (refs #4827) ([#4827](https://github.com/mblua/AgentsCommander/issues/4827), [PR #4836](https://github.com/mblua/AgentsCommander/pull/4836))
- fix: fix(npm): npm 12 allow-scripts opt-in for install, upgrade and npx (refs #4837) ([#4837](https://github.com/mblua/AgentsCommander/issues/4837), [PR #4840](https://github.com/mblua/AgentsCommander/pull/4840))
- fix: fix: show the full TASK title as the sidebar task-title tooltip (refs #4838) ([#4838](https://github.com/mblua/AgentsCommander/issues/4838), [PR #4839](https://github.com/mblua/AgentsCommander/pull/4839))
- fix: fix(sidebar): key row decoration and header blockedMenu testids by path (refs #4841) ([#4841](https://github.com/mblua/AgentsCommander/issues/4841), [PR #4847](https://github.com/mblua/AgentsCommander/pull/4847))
- maintenance: ci(npm): run runtime smoke on npm 11 and 12 with allowScripts opt-in (refs #4842) ([#4842](https://github.com/mblua/AgentsCommander/issues/4842), [PR #4844](https://github.com/mblua/AgentsCommander/pull/4844))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `2d636ff06c5ede132d5ac652c930edf2b75bb45a`; the GitHub commit API must return the ordered parent list [01880c2b69085bf4a5355e07366e9e45b3789efd, 39f00c5268ceb0d4315137d668156e93af016c53]; the contents API must return workflow blob `bf5e5605952915b14fb6145c6bb0d4ee54ee1e44`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.48.0/CHANGELOG.release.md
docs/releases/v0.48.0/candidate-assets.v1.json
docs/releases/v0.48.0/input-manifest.v1.json
docs/releases/v0.48.0/predecessor-assets.v1.json
docs/releases/v0.48.0/release-authority-v1.txt
docs/releases/v0.48.0/release-body.md
docs/releases/v0.48.0/release-plan.md
docs/releases/v0.48.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[2d636ff06c5ede132d5ac652c930edf2b75bb45a, HARDENING_HEAD]
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

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.48.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.48.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.48.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.48.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
