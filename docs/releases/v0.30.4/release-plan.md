# Release preparation plan: v0.30.4

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/1807  
Candidate: `0.30.4` / `v0.30.4`  
Planning base: `9b949f350538cf72763b869e9b503f55eb059739`  
Predecessor: `v0.30.3`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.30.4/` before tagging.

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

- `docs/releases/v0.30.4/CHANGELOG.release.md`
- `docs/releases/v0.30.4/candidate-assets.v1.json`
- `docs/releases/v0.30.4/input-manifest.v1.json`
- `docs/releases/v0.30.4/predecessor-assets.v1.json`
- `docs/releases/v0.30.4/release-authority-v1.txt`
- `docs/releases/v0.30.4/release-body.md`
- `docs/releases/v0.30.4/release-plan.md`
- `docs/releases/v0.30.4/SHA256SUMS`

The final canonical release-hardening plan is not generated or sealed by this bundle. Only its deterministic repository path is present in the hardening allowlist. It remains outside `docs/releases/`, `SHA256SUMS`, `review-set-v1`, and the bundle archive. After exact-bundle approval, the architect authors that separate plan with `READY_FOR_IMPLEMENTATION`; the plan may reference the approved bundle (`P -> B`), but this bundle never hashes or incorporates the future plan.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `9b949f350538cf72763b869e9b503f55eb059739`.
- Ordered planning-base parents: [f190b989bb1e2c5137f8d1bc0059ed01c77511e1, e0113874b0cd15c8c313f084f63e7fcac26d24a3].
- Base `.github/workflows/release.yml` blob: `a6fa2631e0f66d801cc9d8ee4b07cc01738b5122`; content SHA-256: `c35d24cb96a393f8ea0c7f8663c493fc63613aed920f0499502df054c3235a4c`.
- Predecessor annotated object: `f8c3b2cba39dbb734b9959352ff592d7c0332539`; peeled commit: `9ffc24911685906c0222d53eb79898ce9dbdff3e`.
- Predecessor immutable GitHub Release id: `377473922`.
- npm latest: `0.30.3`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.100.0`, archive `gh_2.100.0_linux_amd64.tar.gz`, SHA-256 `e4d4bb4498e8d007abe545b6568926793ace1b6447da598294a610018cb164be`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02` (discovered from `ea165f8d65b6e75b540449e92b4886f43607fa02`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.30.3` (blob `500c8d71e61227547a8efe589e03ba474cfc6f21`)
- `package-lock.json` `/version`: `0.30.3` (blob `57871c0ee6ee84f29f3f6ae7cc232060dbb7b4c2`)
- `package-lock.json` `/packages//version`: `0.30.3` (blob `57871c0ee6ee84f29f3f6ae7cc232060dbb7b4c2`)
- `npm/package.json` `/version`: `0.30.3` (blob `60d4d5a520a0983a4f41318c7e318ea06fecaf9f`)
- `npm/install.js` `const VERSION`: `0.30.3` (blob `743dc4340d1bad212bcd9291c6561fca78a859cf`)
- `src-tauri/Cargo.toml` `[package].version`: `0.30.3` (blob `dcf4505b60fcd1410a4d23de98d2866501580f22`)
- `Cargo.lock` `agentscommander-new.version`: `0.30.3` (blob `390ca303bac12aea3949250fd699e7f78459fb46`)
- `src-tauri/tauri.conf.json` `/version`: `0.30.3` (blob `48e40156e2f605e25a100d149f0cc643658bc687`)

Approved scope:

- feature: refactor(1614): create rooms as room-N-<team> and rename Workgroup to Room in GUI, CLI and docs ([#1614](https://github.com/mblua/AgentsCommander/issues/1614), [PR #1633](https://github.com/mblua/AgentsCommander/pull/1633))
- feature: feat(cli): add self-handoff-and-restart, refs #1632 ([#1632](https://github.com/mblua/AgentsCommander/issues/1632), [PR #1655](https://github.com/mblua/AgentsCommander/pull/1655))
- feature: feat(wake): honor dispatch agent+profile at spawn with delivered receipt (#1639) ([#1639](https://github.com/mblua/AgentsCommander/issues/1639), [PR #1645](https://github.com/mblua/AgentsCommander/pull/1645))
- docs: docs: document --agent per wake, --profile, dispatch ranking (#1640) ([#1640](https://github.com/mblua/AgentsCommander/issues/1640), [PR #1660](https://github.com/mblua/AgentsCommander/pull/1660))
- feature: feat(list-peers-lean): expose effective agent and profile (#1641) ([#1641](https://github.com/mblua/AgentsCommander/issues/1641), [PR #1657](https://github.com/mblua/AgentsCommander/pull/1657))
- feature: feat(pty): menu guard backend core (refs #1646) ([#1647](https://github.com/mblua/AgentsCommander/issues/1647), [PR #1653](https://github.com/mblua/AgentsCommander/pull/1653))
- feature: feat(pty): menu guard queue, settle holds and injection freeze (refs #1646) ([#1648](https://github.com/mblua/AgentsCommander/issues/1648), [PR #1659](https://github.com/mblua/AgentsCommander/pull/1659))
- feature: feat(ui): menu guard notification and universal sidebar indicator (refs #1646) ([#1649](https://github.com/mblua/AgentsCommander/issues/1649), [PR #1664](https://github.com/mblua/AgentsCommander/pull/1664))
- fix: fix(pty): use current-thread watcher runtimes (refs #1651) ([#1651](https://github.com/mblua/AgentsCommander/issues/1651), [PR #1683](https://github.com/mblua/AgentsCommander/pull/1683))
- fix: fix(#1652): renderer IPC black box and in-flight registry (phase 2 of 2) ([#1652](https://github.com/mblua/AgentsCommander/issues/1652), [PR #1701](https://github.com/mblua/AgentsCommander/pull/1701))
- fix: fix(sidebar): remove duplicate tile controls (refs #1654) ([#1654](https://github.com/mblua/AgentsCommander/issues/1654), [PR #1670](https://github.com/mblua/AgentsCommander/pull/1670))
- docs: docs: install AgentsCommander through a trusted coding agent ([#1662](https://github.com/mblua/AgentsCommander/issues/1662), [PR #1675](https://github.com/mblua/AgentsCommander/pull/1675))
- docs: docs: replace invented Fusion team copy with parallel Rooms positioning (refs #1667) ([#1667](https://github.com/mblua/AgentsCommander/issues/1667), [PR #1725](https://github.com/mblua/AgentsCommander/pull/1725))
- maintenance: chore: regenerate module-arcs record after #1646 (refs #1668) ([#1668](https://github.com/mblua/AgentsCommander/issues/1668), [PR #1671](https://github.com/mblua/AgentsCommander/pull/1671))
- feature: feat(sidebar): move session actions into context menu (#1673) ([#1673](https://github.com/mblua/AgentsCommander/issues/1673), [PR #1684](https://github.com/mblua/AgentsCommander/pull/1684))
- feature: feat(settings): add a Use button to each Coding Agent row ([#1674](https://github.com/mblua/AgentsCommander/issues/1674), [PR #1688](https://github.com/mblua/AgentsCommander/pull/1688))
- feature: feat(#1682): show the last coding-agent message timestamp in the terminal status strip ([#1682](https://github.com/mblua/AgentsCommander/issues/1682), [PR #1716](https://github.com/mblua/AgentsCommander/pull/1716))
- feature: feat(#1690): authoritative Rust cancellation and transport parity ([#1690](https://github.com/mblua/AgentsCommander/issues/1690), [PR #1712](https://github.com/mblua/AgentsCommander/pull/1712))
- feature: feat(#1691): monotonic frontend cancellation state and truthful English outcomes ([#1691](https://github.com/mblua/AgentsCommander/issues/1691), [PR #1718](https://github.com/mblua/AgentsCommander/pull/1718))
- docs: docs(#1692): synchronize the agent auto-update guide and changelog ([#1692](https://github.com/mblua/AgentsCommander/issues/1692), [PR #1728](https://github.com/mblua/AgentsCommander/pull/1728))
- maintenance: ui(settings): clarify configuration rail tooltip ([#1698](https://github.com/mblua/AgentsCommander/issues/1698), [PR #1699](https://github.com/mblua/AgentsCommander/pull/1699))
- fix: fix(#1702): discover room-* clones in reclaim-build-artifacts ([#1702](https://github.com/mblua/AgentsCommander/issues/1702), [PR #1703](https://github.com/mblua/AgentsCommander/pull/1703))
- maintenance: ui(settings): label rail action See profiles ([#1705](https://github.com/mblua/AgentsCommander/issues/1705), [PR #1706](https://github.com/mblua/AgentsCommander/pull/1706))
- maintenance: #1708 - sidebar context menu icons and the Detach session rename ([#1708](https://github.com/mblua/AgentsCommander/issues/1708), [PR #1711](https://github.com/mblua/AgentsCommander/pull/1711))
- maintenance: Show the Ctrl+Shift+W shortcut on the close-session controls (#1723) ([#1723](https://github.com/mblua/AgentsCommander/issues/1723), [PR #1736](https://github.com/mblua/AgentsCommander/pull/1736))
- feature: feat(#1730): Telegram glyph, agent-name chip and the target chip order on the sidebar rows ([#1730](https://github.com/mblua/AgentsCommander/issues/1730), [PR #1739](https://github.com/mblua/AgentsCommander/pull/1739))
- maintenance: ui(sidebar): swap the Add to Group and Create new group icons (refs #1731) ([#1731](https://github.com/mblua/AgentsCommander/issues/1731), [PR #1740](https://github.com/mblua/AgentsCommander/pull/1740))
- feature: feat(config): .local alter-ego override layer for context templates and instance settings (refs #1737) ([#1737](https://github.com/mblua/AgentsCommander/issues/1737), [PR #1744](https://github.com/mblua/AgentsCommander/pull/1744))
- fix: fix(agent-version): spawn cmd.exe instead of powershell.exe in the ANSI-skip probe test, refs #1741 ([#1741](https://github.com/mblua/AgentsCommander/issues/1741), [PR #1747](https://github.com/mblua/AgentsCommander/pull/1747))
- maintenance: ci(1742): connect release preparation to the deployed pipeline ([#1742](https://github.com/mblua/AgentsCommander/issues/1742), [PR #1806](https://github.com/mblua/AgentsCommander/pull/1806))
- fix: fix(sidebar): render RUNNING chips after the repo chips (#1745) ([#1745](https://github.com/mblua/AgentsCommander/issues/1745), [PR #1746](https://github.com/mblua/AgentsCommander/pull/1746))
- feature: feat(#1752): add See terminal action to the blocked-menu toast ([#1752](https://github.com/mblua/AgentsCommander/issues/1752), [PR #1766](https://github.com/mblua/AgentsCommander/pull/1766))
- maintenance: #1753: persist the blocked-menu communication on real transitions ([#1753](https://github.com/mblua/AgentsCommander/issues/1753), [PR #1759](https://github.com/mblua/AgentsCommander/pull/1759))
- feature: feat(#1754): surface blocked terminals in list-peers and list-peers-lean ([#1754](https://github.com/mblua/AgentsCommander/issues/1754), [PR #1772](https://github.com/mblua/AgentsCommander/pull/1772))
- feature: feat(sidebar): permanent working tint on rows and rooms, refs #1755 ([#1755](https://github.com/mblua/AgentsCommander/issues/1755), [PR #1781](https://github.com/mblua/AgentsCommander/pull/1781))
- feature: feat(#1757): detect the Codex "Hooks need review" blocking menu ([#1757](https://github.com/mblua/AgentsCommander/issues/1757), [PR #1782](https://github.com/mblua/AgentsCommander/pull/1782))
- docs: docs(#1758): document the menu guard (blockingMenus / menuGuardEnabled) ([#1758](https://github.com/mblua/AgentsCommander/issues/1758), [PR #1800](https://github.com/mblua/AgentsCommander/pull/1800))
- fix: fix(#1760): make the global context template distribution-owned (epic #1748 phase 01) ([#1760](https://github.com/mblua/AgentsCommander/issues/1760), [PR #1789](https://github.com/mblua/AgentsCommander/pull/1789))
- fix: fix(#1761): the global state entry stops recording a version ([#1761](https://github.com/mblua/AgentsCommander/issues/1761), [PR #1798](https://github.com/mblua/AgentsCommander/pull/1798))
- maintenance: test(#1767): pin the blocked-menu CLEAR path against a reintroduced rollback ([#1767](https://github.com/mblua/AgentsCommander/issues/1767), [PR #1787](https://github.com/mblua/AgentsCommander/pull/1787))
- docs: docs(glossary): name the two repo kinds and add 13 missing core terms, refs #1769 ([#1769](https://github.com/mblua/AgentsCommander/issues/1769), [PR #1771](https://github.com/mblua/AgentsCommander/pull/1771))
- docs: docs: adopt the work repo / Agents config repo vocabulary across docs, refs #1770 ([#1770](https://github.com/mblua/AgentsCommander/issues/1770), [PR #1776](https://github.com/mblua/AgentsCommander/pull/1776))
- docs: docs: retire the stale "container agents cannot reach their repos" claims (refs #1775) ([#1775](https://github.com/mblua/AgentsCommander/issues/1775), [PR #1792](https://github.com/mblua/AgentsCommander/pull/1792))
- feature: feat(sidebar): tint a quick-access orchestrator row when its room is working, refs #1783 ([#1783](https://github.com/mblua/AgentsCommander/issues/1783), [PR #1788](https://github.com/mblua/AgentsCommander/pull/1788))
- fix: fix(#1784): complete the room rename in the live session context ([#1784](https://github.com/mblua/AgentsCommander/issues/1784), [PR #1794](https://github.com/mblua/AgentsCommander/pull/1794))
- fix: fix(sidebar): drop the literal "RUNNING" word from the running-peer chip (#1790) ([#1790](https://github.com/mblua/AgentsCommander/issues/1790), [PR #1799](https://github.com/mblua/AgentsCommander/pull/1799))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `9b949f350538cf72763b869e9b503f55eb059739`; the GitHub commit API must return the ordered parent list [f190b989bb1e2c5137f8d1bc0059ed01c77511e1, e0113874b0cd15c8c313f084f63e7fcac26d24a3]; the contents API must return workflow blob `a6fa2631e0f66d801cc9d8ee4b07cc01738b5122`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.30.4/CHANGELOG.release.md
docs/releases/v0.30.4/candidate-assets.v1.json
docs/releases/v0.30.4/input-manifest.v1.json
docs/releases/v0.30.4/predecessor-assets.v1.json
docs/releases/v0.30.4/release-authority-v1.txt
docs/releases/v0.30.4/release-body.md
docs/releases/v0.30.4/release-plan.md
docs/releases/v0.30.4/SHA256SUMS
plans/1807-v0304-release-hardening.md
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[9b949f350538cf72763b869e9b503f55eb059739, HARDENING_HEAD]
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

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.100.0_linux_amd64.tar.gz`, verify SHA-256 `e4d4bb4498e8d007abe545b6568926793ace1b6447da598294a610018cb164be`, install from that verified archive, and require `gh version 2.100.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.30.4`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- Scope issue #1614: summary requires every case-insensitive substring ["room"]; changelog requires every case-insensitive substring ["Workgroups are now Rooms","every existing"] and at least one whole-word match from each alternative group [["unchanged"]].
- Scope issue #1632: summary requires every case-insensitive substring ["self-handoff-and-restart"]; changelog requires every case-insensitive substring ["self-handoff-and-restart","same profile letter"] and at least one whole-word match from each alternative group [].
- Scope issue #1691: summary requires every case-insensitive substring ["cancellation"]; changelog requires every case-insensitive substring ["cancellable","Cancel all","Nothing to update"] and at least one whole-word match from each alternative group [].

### Package and asset gates

Run the repository version tool for `0.30.4`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

Predecessor ledger: exactly 16 unique uploaded nonempty assets. Candidate ledger: exactly 17 unique names derived from predecessor version substitution plus exactly these declared candidate-only assets:

- "agentscommander-0.30.4-windows-x86_64-portable.zip" produced by "windows"

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.30.4/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.30.4`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
