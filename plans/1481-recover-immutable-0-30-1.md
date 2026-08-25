# Issue #1481: make the immutable v0.30.1 recovery deterministic

Status: READY_FOR_IMPLEMENTATION

Impact-HTML: plans/1481-recover-immutable-0-30-1-impact.html

Delivery path: Full revision reopened after recovery run `32883517350`. Developer round-2 enrichment passed, the formal Step 6 rereview passed, independent release/shipper validation passed, and final architect consensus at reviewed head `a8910732da43f715efafaabf7acbd5f71b5cc1fa` produced the fresh dependency-cycle and layering certificate in section 5.3. This document is the sole implementation specification and is technically `READY_FOR_IMPLEMENTATION`. Step 7.5 human impact approval and workgroup purge remain mandatory blockers before implementation handoff. Nothing here authorizes implementation, merge, rerun, dispatch, Release mutation, npm publication, tag or secret changes, cleanup, deployment, or other external-state mutation.

## 1. Issue and objective

Issue: [mblua/AgentsCommander#1481](https://github.com/mblua/AgentsCommander/issues/1481)

Branch: `fix/1481-pin-gh-2-86-recovery`

Base: synchronized `main == origin/main` at `5c7c1ce3d1c35352a93c185e94498dd113641107`

The annotated tag `v0.30.1` exists at its approved release commit, but the tag-triggered Release workflow failed before creating a GitHub Release. The temporary, version-specific recovery workflow subsequently landed, then recovery run `32883517350` failed before mutation because the moving `ubuntu-22.04` image supplied GitHub CLI `2.97.0` while the reviewed workflow correctly required exact `2.86.0`.

Modify the existing recovery workflow so each of its three `gh`-using jobs independently downloads, verifies, selects, and proves the reviewed GitHub CLI `2.86.0` before its first `gh` use. Preserve the landed recovery state machine and all of its fail-closed Release, tag, credential, prior-run, build, asset, mutation, verification, and evidence behavior. The workflow must remain safe to dispatch when the Release is absent, safe to rerun read-only after exact immutable success, and fail closed for every partial, mutable, conflicting, ambiguous, or bootstrap-failure state.

The later implementation changes exactly one existing tracked path:

`.github/workflows/recover-immutable-v0.30.1.yml`

After exact recovery success and retained evidence, remove that temporary workflow in a separate reviewed cleanup change. Cleanup is not part of the recovery dispatch or the implementation diff specified by this plan.

## 2. Locked decisions and identities

No value in this table is a workflow input. Runtime identities already governed by the workflow remain literals or fixed environment constants and every relevant API response is compared against them. Rows explicitly marked review evidence bind planning and diff review only; they must not become new runtime constants.

| Identity | Required value |
| --- | --- |
| Repository | `mblua/AgentsCommander` |
| Default dispatch ref | `refs/heads/main` |
| Release version | `0.30.1` |
| Existing tag | `v0.30.1` |
| Annotated tag object | `5e4da64d42acf341580ff5de623d18539e267c8e` |
| Peeled release commit | `da116e0b2463d6ebec6ace5495e2954d6d8ccbee` |
| Frozen tag workflow path | `.github/workflows/release.yml` |
| Frozen tag workflow Git blob | `a5e437996ea3a91e0688842f80e6c466f35cff19` |
| Landed recovery workflow Git blob | Review evidence only: `a49b2ab1765fa1714002b1a5ccc4db6c6d4d437d` |
| Landed recovery workflow SHA-256 | Review evidence only: `34B7F64C25582C8D805F4E4616BC0A45DFE670DF62788D7EB42E322C896EFF83` over committed LF bytes |
| Release changelog Git blob | `b0095eac4158e3fa575de36b1f4be9ada94b542e` |
| Failed workflow run | `32791566112`, attempt `1` |
| Failed workflow ID | `249288854` |
| Failed job | `prepare-release`, ID `97633984311` |
| Failed step | number `4`, `Validate authority and classify the release` |
| Failed event/ref/SHA | `push`, `refs/tags/v0.30.1`, `da116e0b2463d6ebec6ace5495e2954d6d8ccbee` |
| Failed run timing | created `2026-08-24T23:56:42Z`, completed `2026-08-24T23:56:57Z` |
| Expected original-run artifacts | `total_count: 0` |
| Failed recovery run | Review evidence only, not a runtime constant: `32883517350`, terminal attempt `1`, workflow ID `342322795`, head `5c7c1ce3d1c35352a93c185e94498dd113641107` |
| Recovery draft-create step | Review evidence and generic classifier contract: exact name `Create the sole fresh draft and reconcile its result`; `status:completed`, `conclusion:skipped` |
| Expected failed-recovery artifacts | Review evidence only: `total_count: 0` |
| Reviewed GitHub CLI | Exact version `2.86.0` in `prepare-recovery`, `assemble-release`, and `publish-and-verify-release`; no install in `build-release` |
| GitHub CLI archive URL | `https://github.com/cli/cli/releases/download/v2.86.0/gh_2.86.0_linux_amd64.tar.gz` |
| GitHub CLI archive size | Exact decimal bytes `13627589` |
| GitHub CLI archive SHA-256 | `f3b08bd6a28420cc2229b0a1a687fa25f2b838d3f04b297414c1041ca68103c7` |
| GitHub CLI archive member | Exact `gh_2.86.0_linux_amd64/bin/gh` |
| Release title | Exact literal `Agents Commander v0.30.1` |
| Release body | UTF-8 `CHANGELOG.md` slice defined in section 6.6, excluding its heading, trimmed, nonempty, and ending in exactly one LF in both the notes file and API body |
| Release flags | `draft:true`, `prerelease:false`; publish only by changing `draft` to `false` |
| Immutable setting | exact HTTP 200 JSON `{"enabled":true,"enforced_by_owner":false}` |
| Admin-read secret name | `IMMUTABLE_RELEASES_ADMIN_TOKEN` |
| Shared concurrency group | `release-v0.30.1`, `cancel-in-progress:false` |

The recovery workflow must not accept a version, tag, SHA, Release ID, title, body, asset name, repository, failed-run ID, confirmation, or other release-identity input. The only trigger is an empty `workflow_dispatch` mapping. GitHub's branch selector is not trusted input: the first job must reject every ref other than `refs/heads/main`.

## 3. Evidence and identified cause

### 3.1 Repository evidence

- The clean correction branch was created from synchronized `main == origin/main` at `5c7c1ce3d1c35352a93c185e94498dd113641107`.
- The historical approved plan is Git blob `62895f04770b62a5838704b11bb61cabba291f7e`, SHA-256 `95A54C0FB9E9F242980F20F4FCAF0D57015A3A2FF21AA31DEC7E0B933936B094`; its synchronized impact report is blob `14236921d56b90a9c5796461ff8e0740d433c34a`, SHA-256 `38B621FDB471891CCD48C3DEFDE58933E799F6BF04C6066E6904F3935483CD07`. These are historical identities only and do not authorize the correction.
- The recovery workflow has landed at Git blob `a49b2ab1765fa1714002b1a5ccc4db6c6d4d437d`, SHA-256 `34B7F64C25582C8D805F4E4616BC0A45DFE670DF62788D7EB42E322C896EFF83` over committed LF bytes.
- Release commit `da116e0b2463d6ebec6ace5495e2954d6d8ccbee` is an ancestor of current main.
- `.github/workflows/release.yml` and `CHANGELOG.md` have the same Git blobs at the release commit and current main: `a5e437996ea3a91e0688842f80e6c466f35cff19` and `b0095eac4158e3fa575de36b1f4be9ada94b542e`.
- The landed recovery workflow is a reviewed four-job state machine: `prepare-recovery`, a four-entry `build-release` matrix, `assemble-release`, and `publish-and-verify-release`.
- It already has exact fresh-versus-immutable-resume classification, recovery-run and attempt proof, build-only matrix jobs, run/attempt-bound artifact transport, serial non-clobbering upload, one publication PATCH, bounded asset reconstruction, Release and per-asset attestation verification, and final evidence retention. The correction must not redesign any of those surfaces.
- Codebase Memory snippets show that `npm/install.js` downloads `SHASUMS256.txt`, downloads one platform asset, verifies its SHA-256 before installation, and removes partial files on failure. The closed asset and checksum contract is therefore externally consumed behavior, not presentation metadata.

### 3.2 Failed-run evidence

[Run 32791566112](https://github.com/mblua/AgentsCommander/actions/runs/32791566112) failed in `prepare-release` before any Release mutation. The first immutable-setting request used `${{ github.token }}`. Its effective permissions were Contents write and Metadata read. GitHub requires repository Administration read for `GET /repos/{owner}/{repo}/immutable-releases`, so the request returned HTTP 403. `build-release`, `assemble-release`, and `publish-and-verify-release` were skipped and the run produced no artifacts.

This is a deterministic credential-capability defect. A rerun keeps the original tag commit and frozen workflow bytes, so it repeats the same unauthorized call. Merging a correction on main cannot change that original run. The tag must not move or be recreated.

### 3.3 Current external-state evidence

The verified recovery baseline is:

- remote annotated tag object is `5e4da64d42acf341580ff5de623d18539e267c8e`;
- it peels to `da116e0b2463d6ebec6ace5495e2954d6d8ccbee`;
- immutable Releases are enabled and not owner-enforced;
- GitHub Release `v0.30.1` is exact HTTP 404;
- npm `@mblua/agentscommander@0.30.1` is absent;
- historical `v0.30.0` is public, `immutable:false`, Release ID `374749117`, peels to `45530b46b0a3d4bfe8715ac8b19916df98b6f8cd`, and retains its 16 assets;
- repository secret `IMMUTABLE_RELEASES_ADMIN_TOKEN` exists and refers to a fine-grained token scoped only to this repository with Administration read-only and Metadata read-only, expiring `2026-09-01`.

Every live value is a precondition, not an assumption. Re-read it immediately before the first Release mutation. An expired, missing, inaccessible, differently scoped, or otherwise unusable secret causes a pre-mutation stop.

### 3.4 Failed recovery-run evidence and identified correction

[Recovery run 32883517350](https://github.com/mblua/AgentsCommander/actions/runs/32883517350) is terminal attempt 1 on the landed main SHA `5c7c1ce3d1c35352a93c185e94498dd113641107`, event `workflow_dispatch`, workflow ID `342322795`, and the exact recovery path. Its `prepare-recovery` job `97918449982` failed at step 5 before any Release mutation. The sole draft boundary, step 8 named exactly `Create the sole fresh draft and reconcile its result`, is conclusively `status:completed`, `conclusion:skipped`; all downstream jobs were skipped; the run produced zero Actions artifacts; Release-by-tag remained exact HTTP 404; npm 0.30.1 remained absent; and the annotated tag object and peel remained unchanged.

The job used `ubuntu-22.04` image version `20260817.266.1`, manifest `ubuntu22/20260817.266`, whose preinstalled GitHub CLI was `2.97.0`. The landed workflow asserted the reviewed exact `2.86.0` before its first `gh api`, so it failed closed. The defect is not in the assertion or Release classifier; it is the unreviewed assumption that a moving hosted image would continue to supply the frozen CLI version.

The owner approved planning, but not implementation or dispatch, of the smallest correction: install the official Linux x64 GitHub CLI `2.86.0` archive independently in exactly the three jobs that invoke `gh`, verify the literal size and SHA-256 before extraction, select only the exact binary member, and prove PATH/version before the first job-local `gh` use. Never rerun run `32883517350`.

### 3.5 Platform contract

GitHub documents that `workflow_dispatch` runs only when its workflow exists on the default branch, Contents write permits Release creation, and the immutable-settings GET requires Administration read. GitHub's immutable sequence is create a draft, attach all assets, then publish the draft. Publication locks the associated tag and assets and generates a signed Release attestation. Official verification is `gh release verify` plus `gh release verify-asset` for every local asset.

Hosted runner images are moving dependencies and do not provide a stable `gh` identity. The correction therefore treats the official `cli/cli` v2.86.0 archive as an explicit credential-free bootstrap input, with the literal reviewed digest as the trust anchor. The normal release workflow remains frozen and byte-identical; its separate moving-runner exposure is outside this recovery correction.

Sources:

- [workflow_dispatch event](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch)
- [workflow permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions)
- [immutable-settings REST endpoint](https://docs.github.com/en/rest/repos/repos?apiVersion=2026-03-10#check-if-immutable-releases-are-enabled-for-a-repository)
- [immutable Release sequence and guarantees](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
- [Release REST endpoints](https://docs.github.com/en/rest/releases/releases?apiVersion=2026-03-10)
- [Release asset REST endpoints](https://docs.github.com/en/rest/releases/assets?apiVersion=2026-03-10)
- [Release verification](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/secure-your-dependencies/verify-release-integrity)

## 4. Scope

### 4.1 In scope

- Modify the existing temporary recovery workflow and no other implementation path.
- Add one identical, credential-free, SHA-256-pinned GitHub CLI `2.86.0` bootstrap immediately after `Set up Node.js` in each of `prepare-recovery`, `assemble-release`, and `publish-and-verify-release`, before that job's first `gh` use.
- Preserve the landed workflow's exact envelope, state-machine logic, matrix, build commands, action pins, toolchains other than deterministic `gh` acquisition, asset mappings, transport validation, Release creation/upload/publication boundaries, query-first reconciliation, credential routing, prior-run proof, attestation checks, and evidence format.
- Preserve the exact create-step name `Create the sole fresh draft and reconcile its result` and its existing prior-run classification logic byte-for-byte.
- Preserve v0.30.0 and every pre-existing Release while adding only v0.30.1.
- Specify the later one-file cleanup PR as a separate follow-up boundary.

### 4.2 Out of scope

- Moving, deleting, recreating, force-updating, or otherwise mutating any Git tag.
- Rerunning run `32791566112` or any job from it.
- Rerunning recovery run `32883517350`, rerunning any job from it, or treating its prior owner authorization as authorization for another dispatch.
- Editing `.github/workflows/release.yml` or any existing application, package, version, changelog, lock, config, documentation, or test file.
- Installing `gh` in `build-release`, changing the required CLI version, relaxing/removing its equality assertion, accepting a version range, or trusting the hosted runner's preinstalled CLI.
- Using `sudo`, `apt`, `dpkg`, a `.deb`, a third-party setup action, a container, a custom/self-hosted runner, or an Actions artifact to transport the CLI between jobs.
- Publishing, unpublishing, changing dist-tags, or otherwise mutating npm. No `npm publish`, OIDC, npm token, registry credential, or Phase B workflow belongs here.
- Editing, deleting, recreating, republishing, or retagging historical v0.30.0.
- Enabling, disabling, or changing repository immutable-release settings.
- Creating, rotating, revoking, exposing, renaming, copying, or otherwise modifying the PAT or repository secret.
- Generalizing the workflow for another version, repository, tag, run, or asset set.
- Automatically deleting or repairing a conflicting draft or partial Release.
- Removing the temporary workflow in the same implementation PR or from the recovery run.
- Adding any dependency beyond the one explicit official CLI archive, or adding a helper module, reusable workflow, composite action, generated source file, crate, npm package, or committed test file.

## 5. Exact affected surfaces

### 5.1 Proposed modification

| Kind | Exact repo-relative path | Exact surface | Reason |
| --- | --- | --- | --- |
| Modify | `.github/workflows/recover-immutable-v0.30.1.yml` | Exactly three job-local CLI bootstrap steps plus strictly necessary same-file PATH/version assertions | Select the reviewed `gh 2.86.0` deterministically before the first `gh` use in each `gh`-using job. |

No other tracked path may change. Planning artifacts are delivery documentation and are not implementation paths.

### 5.2 Evidence-backed related-only files

These paths constrain the corrected recovery workflow but remain byte-identical.

| Exact repo-relative path | Relationship to the corrected recovery workflow | Frozen evidence |
| --- | --- | --- |
| `.github/workflows/release.yml` | Frozen source of the landed four-job state machine, action pins, permissions pattern, build matrix, asset staging, manifest, mutation boundaries, resume classifier, and evidence checks. Preserve byte-identically. | Git blob `a5e437996ea3a91e0688842f80e6c466f35cff19` at release commit and current main. |
| `CHANGELOG.md` | Sole source of the exact v0.30.1 Release body. | Git blob `b0095eac4158e3fa575de36b1f4be9ada94b542e`. No Release-body hash is frozen: the earlier `4c04c2...` evidence hashes a normalized full section including the heading and must not be used for the heading-excluding API body. |
| `scripts/check-version-sync.mjs` | Existing `npm run version:check` oracle for all managed 0.30.1 anchors. | Git blob `0ef5d4507f43873d3bd1062fcb78bd2d6a59d83f`. |
| `package.json` | Root version and the existing version-check/build commands. | Git blob `9c821a40c7e74c8c7e1e8dacb0694f18ac5b717b`. |
| `package-lock.json` | Exact locked Node dependency graph consumed by `npm ci`. | Git blob `5212baf5c045b665990a3734e1a0628fa82f1960`. |
| `npm/package.json` | Managed wrapper version checked as 0.30.1; no publication occurs. | Git blob `40c9d0c706a609254eff0c053ea3edddfdf1393d`. |
| `npm/install.js` | Defines the Release URL, installer-selected asset names, checksum download, and checksum enforcement that the 16-asset contract must satisfy. | Git blob `b99725505ffc6937b9a85bee796e0668d6dd20a6`; Codebase Memory symbols `fetchUrl`, `downloadFile`, `verifyChecksum`, and `main`. |
| `Cargo.lock` | Locked Rust dependency graph used by the release builds and managed internal version anchor. | Git blob `f3fe3d2c9acc2c5acf531ffa9208768500223b7e`. |
| `src-tauri/Cargo.toml` | Rust package version checked as 0.30.1. | Git blob `4c254ebb849346ee678824562a2d4d2b642636cd`. |
| `src-tauri/tauri.conf.json` | Tauri application version checked as 0.30.1. | Git blob `36a0915839395ea228705ffd6e788ddbc1472a44`. |
| `src-tauri/tauri.prod.conf.json` | Existing production bundle configuration used unchanged by all four builds. | Git blob `794c9f54191b8e73aa113d0e707e58201e16f0ba`. |
| `scripts/copy-testable-binary.mjs` | Existing Windows testable/raw binary staging contract. | Git blob `6a58510b4b351846bdd33af494494b867cdf6983`. |

If implementation changes any related-only path, adds another file, or changes an existing file, stop. The plan is invalid and must return to Full planning.

### 5.3 No application or module surface

No Rust module, TypeScript module, IPC command/event/type, Tauri window, configuration schema, persistence format, PTY/ConPTY flow, frontend behavior, package dependency, or application module dependency arc changes. The current correction design enumerates `0 added / 0 removed` application module arcs: its sole later implementation path is an existing GitHub Actions YAML file outside `src-tauri` and every application module root. New arcs: none. Removed arcs: none. Cross-boundary arcs: none. No SCC can be created, grown, or joined, and no lower layer can gain a UI-transport, `AppHandle`, or `tauri` dependency.

The fresh Step 7 certificate compared clean base `5e94bef8f92b5ec50dfbdafaaa95cd857155d149` with clean reviewed head `a8910732da43f715efafaabf7acbd5f71b5cc1fa`. Both commits have the exact `src-tauri` tree object `2dbcd4115333dad5adaca30a4725385b7c442a95`. The byte-identical detector copies used for the run have SHA-256 `3B7D838DBC36C459414691D1C29019CB129099E9D913DA95E604474D071016F7`; the levelizer has SHA-256 `518505A6D0A9B3A7417DEC789B6A5A11BF824AC45D8970230C0A4F29A0263312`. Detector exit `1` was the documented normal result because the repository retains one existing cycle, and both graphs were written. Base and head each contain 190 modules, 1,030 unique arcs, 3,696 reference sites, 4,278 functions, 6,756 function edges, 106 SCCs, and `cyclicSccs: 1`. The sole cyclic SCC contains the identical 85-module member set in both graphs; its canonical sorted member-set SHA-256 is `F11E08FD541290172E7058C7DAEA511B4B6DF48A41D4A6F9BB2D4927ECB1A86B`. The measured delta is `0 added / 0 removed` arcs and zero cross-boundary arcs.

Base, head, and committed `src-tauri/module-arcs.txt` are byte-identical at 1,030 arcs and 81,643 LF bytes, SHA-256 `E9160F4E537CD19A400394F58339F373346997621B9DB25EAFC5A8EA36E182FA`, committed Git blob `10b04d9f4d14f3c128a97b5896e5e3aed568de63`. The arc-record self-test passed all 42 cases. `loops_layering`, `instance_gitignore_layering`, and `project_settings_layering` passed all 8 tests across the three suites. The workflow-only design adds no UI transport, `AppHandle`, or `tauri` dependency and changes no lower-layer role.

The detector's unchanged base/head coverage lower bounds are 64 duplicate function nodes, 26,655 unresolved method calls, 14,435 unresolved bare calls, 4,126 skipped macro invocations, 1 skipped macro definition, 129 calls outside function bodies, 7 glob imports, 1,385 external references, 30 bare-path resolutions, zero ambiguous bare paths, and zero unresolved internal paths; `build.rs` is the sole reported unreachable file. It also cannot see unanchored paths. These limitations do not weaken this delta certificate because the exact `src-tauri` Git tree is identical, the only planned implementation path is workflow YAML outside module roots, the arc record is byte-identical, and all three independent layering guards pass.

The implementation reviewer must rerun this clean-tree detector, SCC-member, arc-record, arc-delta, and structural-guard gate from implementation base to final head. It must again prove unchanged `cyclicSccs`, identical SCC member sets, zero cross-boundary arcs, a byte-identical `src-tauri/module-arcs.txt`, and green layering guards. Any module delta or changed application path is a scope breach: stop and return to Full planning.

## 6. Decided solution

### 6.1 Correct the landed recovery workflow, do not redesign it

Modify `.github/workflows/recover-immutable-v0.30.1.yml` from landed Git blob `a49b2ab1765fa1714002b1a5ccc4db6c6d4d437d`, SHA-256 `34B7F64C25582C8D805F4E4616BC0A45DFE670DF62788D7EB42E322C896EFF83`. Keep its four-job build, assembly, publication, exact-immutable resume, prior-run proof, and evidence state machines. Apply only the three job-local CLI bootstraps and strictly necessary same-file PATH/version assertions specified below. Do not refactor shell/Node blocks, introduce helpers, change the matrix, change filenames, update action pins, change any other toolchain, change permissions or secret boundaries, change mutation sources/counts, rename the create step, change retention semantics, or opportunistically simplify the landed logic.

The existing justified duplication continues to isolate the one-time recovery from the production tag workflow. `.github/workflows/release.yml` stays byte-identical at Git blob `a5e437996ea3a91e0688842f80e6c466f35cff19`; fixing its independent moving-runner risk would broaden this issue.

### 6.2 Workflow envelope

Use these exact top-level semantics:

- `name` and fixed `run-name`: `Recover immutable Release v0.30.1`;
- trigger: only `workflow_dispatch`, with no `inputs` child;
- top-level `permissions: {}`;
- concurrency group exactly `release-v0.30.1` and `cancel-in-progress: false`;
- no `push`, `pull_request`, `schedule`, `workflow_call`, `repository_dispatch`, or Release trigger;
- no environment or deployment gate;
- no workflow-level secret or token environment variable;
- all shell blocks use fail-fast mode and receive GitHub expressions only through step `env`, never by interpolation into executable script source;
- use explicit `mblua/AgentsCommander` selection for every `gh` or REST operation.

Use the landed job graph with recovery-specific first-job name:

```text
prepare-recovery
  -> build-release[linux, windows, mac-arm64, mac-x86_64]   (fresh only)
  -> assemble-release                                      (fresh only)
  -> publish-and-verify-release                            (fresh or exact immutable resume)
```

`publish-and-verify-release` keeps the landed `always()` dependency admission so an exact immutable `resume` can run while build and assembly are intentionally skipped. It must still require successful `prepare-recovery`, and fresh mode must require successful build and assembly.

Use bounded job timeouts: 15 minutes for prepare, 90 minutes for each builder, 45 minutes for assembly, and 30 minutes for final publication/verification. Matrix `fail-fast` is false so independent build failures remain diagnosable, but assembly cannot run unless all four builders succeed.

### 6.2.1 Deterministic GitHub CLI 2.86.0 bootstrap

Add one step named exactly `Install exact GitHub CLI 2.86.0` immediately after the existing `Set up Node.js` step in each of `prepare-recovery`, `assemble-release`, and `publish-and-verify-release`. The three step bodies must be byte-identical. Each step must prove the selected binary first and make PATH registration its final operation. The existing job-local assertion blocks identified below must then prove the runner-provided PATH before that job's first other `gh` invocation. Do not add a separate assertion step. Do not add the bootstrap to `build-release`; that job does not invoke `gh`. GitHub Actions jobs share neither filesystem nor PATH, so one download cannot safely cover another job.

Each bootstrap uses these fixed literals and no runtime replacement:

| Bootstrap identity | Exact required value |
| --- | --- |
| Version | `2.86.0` |
| Archive URL | `https://github.com/cli/cli/releases/download/v2.86.0/gh_2.86.0_linux_amd64.tar.gz` |
| Archive byte size | `13627589` |
| Archive SHA-256 | `f3b08bd6a28420cc2229b0a1a687fa25f2b838d3f04b297414c1041ca68103c7` |
| Selected member | `gh_2.86.0_linux_amd64/bin/gh` |
| Install root | `${RUNNER_TEMP}/agentscommander-gh-2.86.0` |
| Archive path | `${RUNNER_TEMP}/agentscommander-gh-2.86.0/gh_2.86.0_linux_amd64.tar.gz` |
| Curl metadata path | `${RUNNER_TEMP}/agentscommander-gh-2.86.0/curl-metadata.json` |
| Selected binary path | `${RUNNER_TEMP}/agentscommander-gh-2.86.0/gh_2.86.0_linux_amd64/bin/gh` |
| Selected bin directory | `${RUNNER_TEMP}/agentscommander-gh-2.86.0/gh_2.86.0_linux_amd64/bin` |
| Selected archive-entry mode | Exact `0755` regular file |
| Extracted binary mode | Exact `0755` regular file |
| Connect timeout | `20` seconds |
| Total transfer timeout | `180` seconds |
| Redirect limit | At most `1`, HTTPS to HTTPS only |

The identical Bash step must implement this exact fail-closed order:

1. Require nonempty absolute `RUNNER_TEMP` and `GITHUB_PATH`, require `RUNNER_TEMP` to be an existing non-symlink directory, and canonicalize it before deriving the fixed paths. Reject a pre-existing install root of any type, including a symlink, then create exactly that root with mode `0700` and require its canonical path to remain directly beneath the canonical runner-temp directory. Archive, response metadata, and extracted-content writes stay beneath that fresh root. The sole allowed path-registration write is the fixed bin-directory line appended to the runner-owned `GITHUB_PATH` file.
2. Download from the fixed public HTTPS URL without `GH_TOKEN`, `GITHUB_TOKEN`, a secret expression, cookie, credential, authorization header, action input, `.netrc`, or `gh`. Invoke curl with `--disable` as its first option, no retry option, and the fixed options `--location`, `--max-redirs 1`, `--proto '=https'`, `--proto-redir '=https'`, `--disallow-username-in-url`, `--connect-timeout 20`, `--max-time 180`, `--max-filesize 13627589`, `--fail`, `--silent`, `--show-error`, `--output` to the fixed archive path, and `--write-out '%{json}'`. Capture write-out stdout, separately from the response body, at the fixed metadata path. Do not use write-out-to-file syntax or an effective-URL component field added after curl 7.81.0.
3. Require the curl metadata file to be nonempty and at most `8192` bytes, then parse it with the already installed Node.js runtime. Require a top-level JSON object whose `http_code`, `num_redirects`, and `size_download` fields are integers equal to `200`, either `0` or `1`, and `13627589`, respectively. Require `url_effective` to be a string accepted by the Node.js `URL` parser with protocol exactly `https:` and empty `username` and `password`. Also require the fixed archive's filesystem size to equal `13627589`. Curl 7.81.0 does not guarantee streaming enforcement of `--max-filesize` when the remote size is unknown, so the curl-reported and filesystem postconditions remain mandatory. Malformed, oversized, noninteger, or ambiguous metadata fails.
4. Use the installed `sha256sum` tool to compute the fixed archive's SHA-256 and compare it to the literal lowercase digest before the first invocation of `tar`, whether for listing or extraction. Do not download or trust a checksum as the run-time trust anchor.
5. After checksum success, use GNU tar with the fixed listing flags `--gzip --list --verbose --file`, `--no-wildcards`, and `--anchored`, followed by `--` and only the literal selected member. Require exactly one selected-name record and require its type-and-mode field to be exactly `-rwxr-xr-x`, meaning a regular file with mode `0755`. A missing, renamed, duplicate, linked, nonregular, `0644`, `0775`, `4755`, or otherwise wrong selected entry fails before extraction.
6. Extract only that literal member beneath the fixed install root with GNU tar using `--gzip --extract --file`, `--directory` set to the fixed install root, `--no-wildcards`, `--anchored`, `--no-same-owner`, `--same-permissions`, and `--keep-old-files`, followed by `--` and only the literal selected member. The extraction vector must not contain `--no-overwrite-dir`, which GNU tar 1.34 rejects when paired with `--keep-old-files`. The required absent install root, fresh mode-`0700` root, literal-member selection, and `--keep-old-files` retain a hard error for every selected-path collision. Do not extract another member, use a wildcard, restore an owner, overwrite an existing path, or apply a post-extraction `chmod` repair.
7. Require the extracted fixed path to be a regular, non-symlink, executable file with mode exactly `0755` and link count exactly one. Its canonical path must equal the fixed selected binary path and remain beneath the fresh canonical install root. Temporarily prepend only the fixed bin directory to PATH within the bootstrap, require `command -v gh` to return an absolute path canonically equal to that file, and require the binary to report exact version `2.86.0`. Do not fall back to the preinstalled CLI.
8. Only after every current-step proof succeeds, append exactly one LF-terminated fixed bin-directory line to `GITHUB_PATH` as the bootstrap's final operation. Preserve any prior file content and perform no fallible command after the append. Every earlier failure must leave `GITHUB_PATH` byte-identical.

Extend the three existing assertion blocks, without adding or moving a step:

- In `prepare-recovery`, extend `Validate recovery authority, evidence, and classify the release` before its current version check and first ordinary-token `gh` use. Leave the intervening admin-only immutable-settings read in its current position.
- In `assemble-release`, extend `Assert Node.js and npm` before `Resolve the current-run transport artifacts`.
- In `publish-and-verify-release`, extend `Assert reviewed publication tools` before its current `gh` version and help calls.

Use one byte-identical assertion fragment at all three integration points. It must derive the fixed expected binary path, require `command -v gh` to return an absolute path, require canonical equality to the fixed selected binary, repeat the regular, non-symlink, executable, exact-mode-`0755`, and link-count-one checks, and then require the binary to report exact version `2.86.0`.

Any bootstrap or assertion failure exits nonzero before the next job-local GitHub CLI read or mutation. Prepare therefore cannot reach classification or draft creation; assembly cannot upload an asset; publication cannot PATCH a Release. The existing post-draft and post-PATCH ambiguity behavior is unchanged because the correction introduces no new Release mutation.

### 6.3 Minimum permissions and credential separation

| Job | Job permissions | Credential behavior |
| --- | --- | --- |
| `prepare-recovery` | `actions: read`, `contents: write` | Normal token reads run/tag/Release state and performs the sole draft POST in fresh mode. Admin secret appears only in dedicated immutable-setting GET steps. |
| `build-release` | `contents: read` | No Release ID, Release token, admin secret, attestation permission, or Actions write permission. Checkout does not persist credentials. |
| `assemble-release` | `actions: read`, `contents: write` | Normal token resolves/downloads current-attempt artifacts and performs the sole serial Release upload loop. Admin secret appears only in the dedicated pre-upload immutable-setting GET. |
| `publish-and-verify-release` | `actions: read`, `contents: write`, `attestations: read` | Normal token reads/reconciles/publishes/verifies the Release and uploads final Actions evidence. Admin secret appears only in dedicated pre-PATCH and final immutable-setting GET steps. |

Set `GH_TOKEN: ${{ secrets.IMMUTABLE_RELEASES_ADMIN_TOKEN }}` only on five isolated steps whose only network request is a GET of `/repos/mblua/AgentsCommander/immutable-releases` with API version `2026-03-10`. The steps must:

1. reject an empty secret without printing its value or length;
2. capture status and body separately without command tracing;
3. require HTTP 200 and exact parsed booleans `enabled:true`, `enforced_by_owner:false`;
4. write only the nonsecret JSON response to a bounded temporary file;
5. expose no secret-derived output;
6. make no other API, git, action, or shell-tool call while the admin token is in scope.

The five settings reads occur at the initial authority gate, immediately before the draft POST, immediately before the first asset upload, immediately before the publication PATCH, and during final verification. There is no PUT, POST, PATCH, or DELETE to the immutable-settings endpoint.

Set `GH_TOKEN: ${{ github.token }}` only on steps that need ordinary GitHub API or Release access. Never pass either token to a third-party action input, checkout credential store, build command, process argument, job output, workflow artifact, Release asset, or log. GitHub's automatic masking is defense in depth, not the control.

### 6.4 Dispatch and current-main gate

Before any external mutation, require all of the following:

- `github.event_name == workflow_dispatch`;
- `github.repository == mblua/AgentsCommander`;
- `github.ref == refs/heads/main` and `github.ref_name == main`;
- the recovery workflow path is `.github/workflows/recover-immutable-v0.30.1.yml`;
- `github.run_attempt == 1` for a separately authorized new dispatch from the corrected main bytes; a later attempt is admitted only by the state classifier and can mutate only when the Release is still exact 404 and every earlier recovery run/attempt is proven terminal and safe before the draft POST;
- remote default branch is `main` and its head equals the dispatch `github.sha`;
- release commit `da116e0...` is an ancestor of dispatch `github.sha`;
- `.github/workflows/release.yml` and `CHANGELOG.md` at both release commit and dispatch SHA have the fixed Git blobs from section 2;
- the only recovery-workflow run currently in progress is the current run, enforced together with the literal concurrency group;
- the checked-in recovery workflow has no user inputs and no runtime replacement for any fixed identity.

The workflow cannot embed the SHA-256 of its own future correction bytes. The repeated Full review and Step 7.5 bind the revised specification before implementation; implementation review binds the one-file diff; the runtime gate binds the executing file to the current default-branch SHA and rejects a stale ref. This is not an open implementation choice.

### 6.4.1 Existing recovery-run proof

Preserve the landed recovery-run proof and the exact step name `Create the sole fresh draft and reconcile its result` byte-for-byte. Do not add a run-specific constant for `32883517350` or special-case that run. At the initial classifier and again before draft POST, first upload, publication PATCH, and final verification, the existing proof must continue to query every recovery run and every prior attempt:

1. Every non-current recovery run and attempt must be terminal.
2. Each prior attempt must contain exactly one terminal `prepare-recovery` job and exactly one step with the exact create-step name.
3. Only `status:completed` plus `conclusion:skipped` is safe-before-POST evidence.
4. A missing or renamed create step, missing or duplicate prepare job, nonterminal state, conclusion `success`, `failure`, `cancelled`, `null`, another value, additional attempt with incomplete evidence, ambiguous jobs, or query ambiguity is `crossed-or-ambiguous` and blocks `fresh`.
5. Safe prior-run proof is necessary but not sufficient: Release-by-tag must also be exact HTTP 404 and every other gate must pass before `fresh` is selected.

Run `32883517350` currently satisfies the generic proof because it is terminal and its exact create step is completed/skipped. That fact permits the classifier to evaluate a future new dispatch; it does not authorize that dispatch. Do not rerun `32883517350`. After the corrected workflow is merged and a fresh live preflight passes, a separate owner decision is required for exactly one new no-input dispatch from the reviewed main SHA.

### 6.5 Frozen failed-run gate

Using only `${{ github.token }}` and Actions read permission, query and validate before any Release mutation:

1. `GET /repos/mblua/AgentsCommander/actions/runs/32791566112` is completed failure, run attempt 1, workflow ID `249288854`, event `push`, head branch `v0.30.1`, head SHA `da116e0...`, workflow path `.github/workflows/release.yml`, repository `mblua/AgentsCommander`, and the fixed creation/completion timestamps.
2. `GET /repos/mblua/AgentsCommander/actions/runs/32791566112/attempts/1/jobs` contains job ID `97633984311`, named `prepare-release`, completed failure. Its step 4 is exactly `Validate authority and classify the release` with conclusion failure. Every later Release job is skipped and no build matrix entry completed.
3. `GET /repos/mblua/AgentsCommander/actions/runs/32791566112/artifacts` returns exact HTTP 200, `total_count:0`, and an empty artifact array.
4. The workflow ID has no second tag-push run or later attempt for this tag/commit. A surprise rerun or additional tag workflow run changes the evidence and blocks recovery pending reconciliation.
5. The failed commit's release workflow blob is exactly `a5e437996ea3a91e0688842f80e6c466f35cff19`.

Do not fetch or parse logs with the admin secret. Do not rerun or cancel the historical run.

### 6.6 Tag, version, body, and preservation gate

Check out only release commit `da116e0...` with checkout pin `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`, `fetch-depth:0`, and `persist-credentials:false`. Never check out a user-selected ref for build inputs.

Read the checked-out `CHANGELOG.md` as UTF-8 and derive the Release metadata deterministically:

1. Set the Release name to the exact literal `Agents Commander v0.30.1`. The annotated-tag message `Release v0.30.1` is not the Release name.
2. Require exactly one line matching `/^## 0\.30\.1[ \t]*$/gm`.
3. Begin the body slice immediately after that heading and end immediately before the next line matching `/^## [^\r\n]+$/m`, or at EOF when there is no later heading.
4. Apply JavaScript `.trim()` to the slice and reject an empty result.
5. Create the notes file once with exclusive-create mode `wx` and write exactly the trimmed body followed by one LF. Never append, overwrite, reuse a pre-existing file, normalize the body again, or include the heading.
6. Do not separately normalize CRLF to LF or validate, strip, or reject a BOM. The UTF-8 decode, exact regexes, slice, single ECMAScript `.trim()`, and one appended LF above are the complete transformation.
7. Construct the Release JSON with jq's `--rawfile body` argument pointed at that exact exclusive-created notes file, so the API `body` does not come from a second derivation or a shell variable. On every classifier and postcondition read, materialize the parsed `release.body` as UTF-8 without transformation and compare it byte-for-byte to that same notes file; compare `release.name` separately to the exact literal.

No expected Release-body hash exists in the frozen workflow, prior plan history, issue #1481, or PR #1543. Do not use the normalized-section SHA-256 `4c04c234e2286479c78f701ae7029d998351047da394481a7a597039f40ce7dd`: it includes the heading, whereas the Release body excludes it. The earlier changelog-order conflict was corrected by commit `eec945cdb8a4d601d22e280ccb9cb74306348f3a` before the tagged merge and is not an open recovery condition.

Require:

- GitHub ref `refs/tags/v0.30.1` exists exactly once and points to object `5e4da64...` of type `tag`;
- the annotated tag object recursively peels to commit `da116e0...` and no other object;
- local Git object checks agree with remote Git database API results;
- `npm run version:check` passes and all managed surfaces report exactly 0.30.1;
- the fixed Release title and exact extracted body bytes match section 2;
- release-workflow and changelog blobs match their fixed identities;
- no workflow command contains `git tag`, `git push`, a ref update, tag deletion, or tag creation;
- v0.30.0 still has Release ID `374749117`, the expected peeled commit, public non-draft/non-prerelease state, and 16 unique uploaded assets;
- capture a canonical pre-mutation snapshot of v0.30.0 plus every other existing Release and asset record. Exclude only the absent v0.30.1 identity from the later set comparison.

Canonical preservation snapshots sort releases by numeric ID and assets by numeric ID and name. Record Release ID, tag, target, name, SHA-256 of body bytes, draft/prerelease/immutable flags, timestamps, and for each asset its ID, name, size, state, `sha256:` digest, API URL, and browser URL. Pagination must continue to exhaustion at 100 records per page. Duplicate IDs/names, missing digest, malformed URL, or incomplete pagination blocks before mutation.

### 6.7 Query-first Release classifier

Query `GET /repos/mblua/AgentsCommander/releases/tags/v0.30.1` with the normal token, preserving HTTP status, parsed body, and command status independently.

Classify into exactly one mode:

| Mode | Exact predicate | Allowed continuation |
| --- | --- | --- |
| `fresh` | HTTP 404, expected nonzero CLI status, no parseable Release object, and every earlier gate passes. | Requery all mutation-adjacent guards, then use the single draft POST and fresh build/assembly path. |
| `resume` | HTTP 200 and one Release whose ID is stable; tag/target/title/body are exact; `draft:false`, `prerelease:false`, `immutable:true`; all 16 exact asset records are unique, uploaded, positive-size, and have valid `sha256:` digests/URLs. | Skip draft POST, all builds, all Release uploads, and PATCH. Reconstruct local evidence by asset ID and run read-only final verification. |
| `conflict` | Any draft; any mutable published Release; wrong ID/identity/flags; partial, duplicate, extra, missing, zero-size, pending, or digestless asset; any other HTTP/status combination; timeout; malformed JSON; more than one candidate; or any ambiguity. | Stop with zero further mutation and an exact state report. No automatic delete, repair, clobber, or retry. |

The classifier runs before mutation and again at each mutation boundary. It never treats transport, authentication, rate-limit, 403, 5xx, or malformed output as absence.

### 6.8 Sole draft creation

Fresh mode alone may invoke one Release creation POST after a final tag/run/setting/Release-404 race check. Use the existing Release workflow's reviewed REST creation program. The body must contain only the fixed:

- `tag_name: v0.30.1`;
- `target_commitish: da116e0b2463d6ebec6ace5495e2954d6d8ccbee`;
- exact name/title from section 2;
- exact body bytes from section 2;
- `draft:true`;
- `prerelease:false`.

The already existing tag is verified immediately before the POST. No call may create a tag, and no fallback may change `target_commitish`. Require exact HTTP 201 and response identity, capture the numeric Release ID once, and use that ID for all later mutation checks. A lost or non-201 response is queried once by tag: exact immutable success may transition to read-only resume; every draft, partial, or ambiguous result stops. Never issue a second POST.

Immediately after creation, require exact tag/target/title/body, `draft:true`, `prerelease:false`, `immutable:false`, zero assets, and unchanged historical snapshot.

### 6.9 Frozen build matrix

Run only in fresh mode. Preserve these exact four entries and build arguments:

| Key | Runner | Build argument | Payload count |
| --- | --- | --- | ---: |
| `linux-x86_64` | `ubuntu-22.04` | `--config src-tauri/tauri.prod.conf.json` | 4 |
| `windows-x86_64` | `windows-latest` | `--config src-tauri/tauri.prod.conf.json --bundles nsis` | 3 |
| `mac-aarch64` | `macos-latest` | `--config src-tauri/tauri.prod.conf.json --target aarch64-apple-darwin` | 4 |
| `mac-x86_64` | `macos-latest` | `--config src-tauri/tauri.prod.conf.json --target x86_64-apple-darwin` | 4 |

Every builder:

1. checks out fixed `da116e0...`, not `github.sha` or a tag name;
2. verifies the fixed Git blobs relevant to its build;
3. installs Node `22.23.2`, asserts npm `10.9.8`, installs Rust `1.98.0` and the exact Apple target when applicable;
4. restores the existing Rust cache without changing its key design;
5. runs `npm ci` and the existing version synchronization gate;
6. invokes `tauri-apps/tauri-action` with no Release/tag/title/body/draft input and uses only its parsed `artifactPaths` output to locate bundles;
7. retains the existing Windows SignPath placeholder and does not claim signing;
8. canonicalizes every source/destination under the checked-out workspace, rejects traversal and reparse/symlink surprises, and requires exact basenames;
9. stages only its closed payload set and creates one mode-preserving outer tar;
10. uploads one current-run/current-attempt/matrix-unique workflow artifact with overwrite false, hidden files excluded, and missing files fatal;
11. resolves and records the exact Actions artifact ID and service digest for later ID-based download.

Use these exact action pins, with no tags or branches:

| Action | Commit pin |
| --- | --- |
| `actions/checkout` v5.1.0 | `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` |
| `actions/setup-node` v5.0.0 | `a0853c24544627f65ddf259abe73b1d18a591444` |
| `dtolnay/rust-toolchain` stable snapshot | `4360b52568e2003a75bf9bc1d59f33a8e3fc893c` |
| `Swatinem/rust-cache` v2.9.2 | `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` |
| `tauri-apps/tauri-action` v0.6.2 | `84b9d35b5fc46c1e45415bdb6144030364f7ebc5` |
| `actions/upload-artifact` v7.0.1 | `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` |
| `actions/download-artifact` v8.0.1 | `3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c` |

### 6.10 Exact Release assets and SHASUMS256.txt

The 15 build payloads and sole checksum asset are:

| Order | Exact Release asset | Source/matrix contract |
| ---: | --- | --- |
| 1 | `Agents.Commander-0.30.1-1.x86_64.rpm` | Linux Tauri RPM, renamed from the exact `Agents Commander` basename. |
| 2 | `Agents.Commander_0.30.1_aarch64.dmg` | macOS ARM64 Tauri DMG. |
| 3 | `Agents.Commander_0.30.1_amd64.AppImage` | Linux Tauri AppImage. |
| 4 | `Agents.Commander_0.30.1_amd64.deb` | Linux Tauri DEB. |
| 5 | `Agents.Commander_0.30.1_x64-setup.exe` | Windows Tauri NSIS installer. |
| 6 | `Agents.Commander_0.30.1_x64.dmg` | macOS Intel Tauri DMG. |
| 7 | `Agents.Commander_aarch64.app.tar.gz` | macOS ARM64 Tauri app archive. |
| 8 | `Agents.Commander_x64.app.tar.gz` | macOS Intel Tauri app archive. |
| 9 | `agentscommander-linux-x86_64` | `src-tauri/target/release/agentscommander`. |
| 10 | `agentscommander-mac-aarch64` | `src-tauri/target/aarch64-apple-darwin/release/agentscommander`. |
| 11 | `agentscommander-mac-aarch64.app.tar.gz` | Byte-identical copy of item 7. |
| 12 | `agentscommander-mac-x86_64` | `src-tauri/target/x86_64-apple-darwin/release/agentscommander`. |
| 13 | `agentscommander-mac-x86_64.app.tar.gz` | Byte-identical copy of item 8. |
| 14 | `agentscommander-testeable-windows-x86_64.exe` | Preserve the repository spelling and the current testable/raw copy contract. |
| 15 | `agentscommander-windows-x86_64.exe` | Same Windows raw executable bytes as item 14. |
| 16 | `SHASUMS256.txt` | Generated by the Ubuntu assembler from items 1-15. |

The four matrix payload sets are exactly 4/3/4/4 and their union is the first 15 unique names. `SHASUMS256.txt` contains exactly 15 lines sorted by bytewise filename. Each line is lowercase 64-hex SHA-256, two ASCII spaces, exact filename, and LF. It contains no self-entry, path prefix, blank line between records, duplicate, extra, or missing record.

Require all filenames to be plain basenames with no slash, backslash, control character, leading dot, or Unicode normalization ambiguity. Require executable modes on the four raw Unix binaries/archives exactly as in the frozen workflow transport. Require byte equality for the two Windows raw names and for each macOS archive alias pair.

### 6.11 Serial assembly and upload

Fresh mode assembly has no source checkout. Resolve the four transport artifacts through the Actions API by exact current `run_id`, `run_attempt`, expected matrix-qualified name, ID, and service digest. Download each by ID with the pinned action. Reject a prior-attempt/name-only artifact, duplicate match, expired record, digest mismatch, unexpected zip member, unsafe tar entry, traversal, absolute path, symlink, hardlink, device, or unexpected mode.

After extracting the exact 15 files:

1. verify uniqueness, positive size, SHA-256, alias equalities, and the four installer substring selections;
2. generate exact `SHASUMS256.txt`;
3. build a canonical manifest sorted by asset name with name, byte size, lowercase SHA-256 and expected `sha256:` REST digest;
4. requery tag object/peel, current main, failed-run identity, immutable setting, Release ID/identity/draft state, zero Release assets, and preservation snapshot;
5. upload all 16 assets serially in table order with one `gh release upload` command source inside one bounded loop, explicit repository, and no `--clobber`;
6. after each upload, require the exact prefix inventory and reject any extra or duplicate asset;
7. after all uploads, require exact `draft:true`, `immutable:false`, 16 unique assets, state `uploaded`, positive sizes, and API digests equal local bytes;
8. download all 16 by captured REST asset ID into an empty directory and reverify name, size, digest, manifest, checksum rows, and aliases before publication;
9. upload one run/attempt-qualified verified-draft Actions artifact containing exactly the 16 local assets plus canonical `release-identity.json` and `release-manifest-draft.json`. Record its Actions artifact ID and digest.

Release asset downloads accept only direct HTTP 200 or one HTTP 302. A redirect must be an absolute HTTPS URL with no embedded credentials and is fetched without forwarding `Authorization`. Any second redirect, other scheme, unexpected host class, or other status blocks.

### 6.12 Sole publication boundary

Immediately before publication, use fresh local/downloaded bytes and revalidate every condition in sections 6.3 through 6.11, including the dedicated fourth immutable-setting GET. Requery the Release by captured numeric ID and require the exact complete mutable draft.

The sole publication request is one REST PATCH to `/repos/mblua/AgentsCommander/releases/<captured-id>` with exact JSON body:

```json
{"draft":false}
```

Do not include title, body, tag, target, prerelease, discussion, latest, or another field. Do not wrap the PATCH in a retry helper or loop. Capture request-invoked, status, stdout, and stderr separately. Once invocation begins, a cancellation, timeout, lost response, non-200, or parser failure is committed-or-ambiguous, never absence.

Immediately query by both captured Release ID and tag:

- exact published immutable state continues to final verification;
- exact complete draft state stops and requires separate owner review before any later PATCH attempt;
- any other state stops as conflict;
- no path deletes, recreates, edits, reuploads, retags, or rebuilds.

### 6.13 Read-only immutable resume and final verification

Resume mode and post-PATCH reconciliation converge on one common verifier. Resume performs zero Release, asset, tag, setting, workflow, or npm mutation.

The verifier:

1. reads the immutable setting with the fifth dedicated admin-token GET;
2. requires the remote annotated tag object and peeled commit to remain exact;
3. requires Release ID/tag/target/title/body, `draft:false`, `prerelease:false`, `immutable:true`, nonnull publication time, and the exact 16-asset metadata contract;
4. downloads all 16 assets by captured REST IDs under the bounded 200/302 rule and reconstructs the same canonical manifest used by fresh mode;
5. requires local/API size and digest equality, exact SHASUMS rows, four installer selections, and all three alias equalities;
6. requires PATH to resolve the fixed binary installed under `RUNNER_TEMP` by section 6.2.1 and asserts GitHub CLI exactly `2.86.0` before relying on Release-verification JSON;
7. runs `gh release verify v0.30.1 --repo mblua/AgentsCommander --format json` and parses the signed tag, commit, and complete asset subjects;
8. runs `gh release verify-asset v0.30.1 <exact-local-path> --repo mblua/AgentsCommander --format json` once for each of the 16 local files and requires the expected digest/release association;
9. compares the canonical post snapshot of every pre-existing Release and asset to the pre snapshot, excluding only the newly added exact v0.30.1 record. v0.30.0 must also match its explicit ID/commit/state/count guard;
10. emits a canonical final record containing recovery mode, current run/attempt/path/head SHA, failed-run proof, tag object/peel, Release ID/URL/state/body hash, all 16 IDs/names/sizes/digests/URLs, checksum-manifest hash, attestation results, historical snapshot hash, and mutation invocation/outcome flags;
11. uploads a final Actions artifact named `recover-v0.30.1-<run_id>-<run_attempt>-final-evidence` with overwrite false and records its artifact ID/service digest in the job summary.

Read-only polling for immutable state and attestations is allowed for at most 30 attempts, ten seconds apart. Polling never repeats a mutation. Timeout after the publication invocation is an evidence failure over potentially committed immutable state; a later dispatch can enter only exact read-only resume.

### 6.14 Failure and retry behavior

| Boundary/state | Required behavior |
| --- | --- |
| CLI bootstrap, canonical PATH, or exact-version proof fails | Stop before the first subsequent job-local `gh` call. Prepare reaches no classifier or draft POST; assembly performs zero uploads; publication performs zero PATCH calls. Never fall back to the hosted CLI. |
| Before draft POST; Release remains exact 404 | No Release mutation exists. Fix/review the cause, confirm the prior run is terminal and no other recovery run crossed POST, then a new main dispatch may re-enter `fresh`. |
| Admin secret missing/expired or immutable GET is not exact 200 JSON | Stop before the next mutation. Do not fall back to `github.token`, another credential, cached state, or an assumption. |
| Failed-run/tag/blob/main/body/version/preservation evidence differs | Stop before mutation and reconcile externally. Do not adapt constants in a running workflow. |
| Draft POST invoked and response is lost or non-201 | Query once. Exact immutable success may enter read-only resume; every draft/partial/ambiguous state blocks. Never POST again. |
| Build or assembly fails after draft creation | Leave the confirmed mutable draft untouched. Do not delete, clobber, repair, upload a subset, or rerun blindly. A separate owner-reviewed recovery decision is required. |
| Any duplicate, partial, extra, zero-byte, wrong-digest, or unexpected draft asset | Stop before publication. Do not use `--clobber` or repair an individual asset in place. |
| Publication PATCH not invoked | The draft is mutable but conflicting for this workflow. Separate owner review is required before any later mutation. |
| Publication PATCH invoked or response uncertain | Query exact state first. Exact immutable state continues read-only; exact draft or any mismatch blocks. Never blindly repeat PATCH. |
| Exact immutable Release exists but verification/evidence upload fails | A later dispatch enters read-only `resume`, reconstructs from asset IDs, and retries only verification/evidence retention. |
| Immutable Release identity or asset differs | The version cannot be repaired in place. Stop and plan a new version. Never delete/recreate/reupload/retag. |
| Historical Release snapshot changes during the run | Stop and report both snapshots. Do not attempt rollback or touch the other Release. |

Every failure summary after any mutation states: query exact external state first; do not rerun, rebuild, reupload, retag, delete, recreate, or republish from an uncertain result.

## 7. Compatibility and security impact

### 7.1 Compatibility

- No application runtime, UI, CLI, PTY, ConPTY, IPC, config, persistence, Rust API, TypeScript type, or dependency behavior changes.
- Release artifacts keep the exact v0.30.1 names and bytes contract already expected by `npm/install.js`.
- Linux, Windows, macOS ARM64, and macOS Intel retain the reviewed runners, build arguments, toolchains, target paths, bundle selection, and raw-binary aliases.
- Windows path checks must normalize drive/case, remain under the workspace, and reject reparse-point/path traversal surprises. Do not introduce `cmd.exe /C` wrapping; retain the landed workflow's platform-specific shell behavior.
- Existing v0.30.0 and all other Releases remain read-only.

### 7.2 Security

- The least-privilege admin PAT is exposed only to five exact read-only settings steps. It never authorizes a Release mutation.
- The normal `GITHUB_TOKEN` carries Contents write only in jobs that need Release mutation and Actions read only where artifact/run APIs are used. Builders are Contents read only.
- Fixed identities, main-only dispatch, exact failed-run proof, fixed Git blobs, exact tag object/peel, closed filenames, non-clobbering uploads, numeric Release/asset IDs, and query-first classification remove user-controlled release identity and replacement paths.
- No tag command exists. The Release creation API receives an already verified existing tag; no fallback may synthesize one.
- No secret, token, environment value, external JSON, API string, or filename is interpolated into executable shell/JavaScript source. Validate parsed values against fixed schemas and pass them through environment/files.
- Third-party actions remain pinned to reviewed full commit SHAs. No new action, crate, npm package, package-manager install, or committed helper is introduced; the official CLI archive is the one newly explicit bootstrap dependency and is governed by the literal identity and validation boundary below.
- The new CLI bootstrap is public and credential-free. Its literal official URL, exact byte size, literal SHA-256, exact selected member, one-HTTPS-redirect limit, checksum-before-extract order, fixed `RUNNER_TEMP` boundary, regular non-symlink executable check, PATH precedence proof, and exact version assertion form one fail-closed supply-chain boundary. It receives no token or authorization header and executes no downloaded byte before the literal digest matches.
- PAT expiration is safe: inability to prove settings state blocks before the next mutation. This plan does not authorize changing the token or secret.

### 7.3 Residual risks

- Availability of the official GitHub CLI asset and GitHub/release-assets network remains external. Outage, removal, status drift, or digest drift blocks safely.
- The upstream `cli/cli` v2.86.0 Release currently reports mutable, so the literal reviewed archive digest is essential. Remote replacement yields a checksum failure, never execution.
- The hosted runner OS and its basic `curl`, `tar`, and SHA-256 tools remain external. The archive hash removes `gh` version drift but not host compromise.
- Three independent 13.6 MB downloads add latency and an availability dependency, accepted to avoid cross-job artifact state and permissions.
- GitHub REST, immutable-release enforcement, Release attestations, and pinned third-party action repositories remain external dependencies.
- A failure after draft creation can leave a mutable draft requiring separate owner reconciliation.
- A failure after PATCH invocation can leave committed immutable state before the run stores final evidence. Exact read-only resume addresses evidence recovery, not rollback.
- The current Windows SignPath placeholder remains accepted release policy; this recovery neither fixes nor worsens it.
- The admin token expires on `2026-09-01` and must be revalidated immediately before any later dispatch. This plan does not authorize changing it.
- `.github/workflows/release.yml` separately asserts `gh 2.86.0` on a moving hosted runner. It remains byte-identical for this recovery; track that future-release risk outside issue #1481.
- Current main can advance between review and dispatch. The runtime binds to the dispatched main SHA and fixed release/changelog blobs, while normal PR review and human dispatch ownership remain central controls.

## 8. Implementation order

### Phase 1: one-file recovery correction

1. Reconfirm branch/base and the fixed Git object identities without mutating external state.
2. Require the existing recovery workflow to match landed Git blob `a49b2ab1765fa1714002b1a5ccc4db6c6d4d437d` before editing and require `.github/workflows/release.yml` to remain Git blob `a5e437996ea3a91e0688842f80e6c466f35cff19`.
3. Add the identical CLI bootstrap immediately after `Set up Node.js` in exactly `prepare-recovery`, `assemble-release`, and `publish-and-verify-release`; add no bootstrap to `build-release`.
4. Make only strictly necessary same-file changes to prove the runner PATH resolves the pinned binary and exact version before each job's first `gh` call. Preserve every other state-machine, prior-run, matrix, action-pin, permission, secret, asset, manifest, upload, POST, PATCH, verifier, evidence, and failure-guidance byte unless a directly adjacent assertion must reference the fixed install path.
5. Add executable static fixtures for validation only in an implementer-local or replica-local scratch location. Do not commit a helper/test file.
6. Run the static and executable checks in section 9.
7. Commit exactly the modified recovery workflow file. Review the one-file diff against the landed blob and this plan. Do not dispatch or rerun from the branch.
8. Merge through the normal reviewed PR to main. Record the merge SHA and new exact recovery-workflow Git blob/SHA-256 externally.

### Phase 2: owner-controlled dispatch and evidence

1. After implementation merge, reconfirm the secret exists without reading its value and that the token remains valid, repository-scoped, and Administration/Metadata read-only.
2. Reconfirm tag object/peel, Release exact 404, original failed-run context, recovery run `32883517350` terminal/create-skipped proof, immutable setting, corrected current main/workflow bytes, v0.30.0, all pre-existing Releases, and npm 0.30.1 absence.
3. Obtain a fresh explicit owner authorization for exactly one no-input dispatch from that reviewed main SHA. The authorization consumed by run `32883517350` does not carry forward.
4. Dispatch the recovery workflow once from `main`, with no inputs. Never rerun `32883517350`.
5. Monitor it to terminal state. After any mutation uncertainty, query exact Release state before considering any further action.
6. Record run/attempt/URL/head SHA, Release ID/URL/state, complete asset IDs/sizes/digests, checksum hash, attestation results, final evidence artifact ID/digest, unchanged historical snapshots, npm still absent, and clean repository state.

### Phase 3: separate cleanup change

Only after Phase 2 exact success and retained evidence:

1. verify there is no queued or running recovery run and the successful run is terminal;
2. reverify exact immutable v0.30.1, tag object/peel, all 16 assets/attestations, evidence artifact, and unchanged historical Releases;
3. open a separate PR whose sole implementation change deletes `.github/workflows/recover-immutable-v0.30.1.yml`;
4. merge and verify the workflow is no longer dispatchable from default-branch bytes;
5. do not alter the PAT or `IMMUTABLE_RELEASES_ADMIN_TOKEN` as part of cleanup.

Cleanup is not performed by a step in the recovery workflow and is not part of the Phase 1 commit.

## 9. Tests and objective acceptance criteria

### 9.1 Static workflow checks before merge

Require all of the following:

- YAML parses and has exactly the four intended jobs and four matrix entries.
- Only `workflow_dispatch` exists and it has no inputs.
- Top-level permissions are empty; job permissions exactly match section 6.3.
- Concurrency is literal `release-v0.30.1` with cancellation disabled.
- Every action use is one of the seven 40-hex pins in section 6.9; no tag/branch action reference remains.
- Exactly three steps are named `Install exact GitHub CLI 2.86.0`, with byte-identical Bash `run:` scalars: one immediately after `Set up Node.js` in each of `prepare-recovery`, `assemble-release`, and `publish-and-verify-release`, all before that job's first `gh`; `build-release` has no bootstrap and still has zero `gh` invocations.
- Each bootstrap contains the exact URL, byte size `13627589`, literal digest, selected member, install root, archive path, curl-metadata path, selected binary path, and bin directory from section 6.2.1. It rejects an empty, relative, or symlinked `RUNNER_TEMP` and every pre-existing install root.
- The sole curl invocation uses `--disable` as its first option, has no retry, and contains the exact curl 7.81-compatible redirect, HTTPS-protocol, credential, timeout, maximum-size, failure, logging, output, and `%{json}` write-out flags from section 6.2.1. The response body and metadata have the fixed separate paths; metadata is bounded to `8192` bytes and its Node.js parser requires integer `http_code`, `num_redirects`, and `size_download` plus a credential-free HTTPS `url_effective`. Both curl-reported and filesystem sizes must equal `13627589`.
- The literal SHA-256 comparison precedes the first `tar` invocation. Listing selects exactly one literal member whose regular-file mode is exactly `0755`. The extraction option sequence is exactly `--gzip --extract --file`, the fixed archive path, `--directory`, the fixed install root, `--no-wildcards`, `--anchored`, `--no-same-owner`, `--same-permissions`, `--keep-old-files`, `--`, and the literal selected member. It must not contain `--no-overwrite-dir`. Extraction selects only that member with no wildcard matching, owner restoration, overwrite, or chmod repair; a selected-path collision is a hard error. The extracted binary is regular, non-symlink, executable, mode `0755`, link count one, and canonically equal to the fixed selected path beneath the fresh canonical install root.
- Each bootstrap proves the fixed binary with a temporary PATH before its sole `GITHUB_PATH` write. That write is the final operation and appends exactly one LF-terminated fixed bin-directory line. Static ordering proves every prior failure leaves `GITHUB_PATH` unchanged.
- No assertion step is added. The same byte-identical PATH fragment appears only in `Validate recovery authority, evidence, and classify the release`, `Assert Node.js and npm`, and `Assert reviewed publication tools`, at the exact pre-`gh` integration points in section 6.2.1. Each fragment requires absolute resolution, canonical equality, the complete file/mode/link checks, and exact version `2.86.0` before the existing first ordinary `gh` use.
- Bootstrap steps contain no `GH_TOKEN`, `GITHUB_TOKEN`, secret expression, authorization header, credential, cookie, `.netrc`, `gh` installer call, `sudo`, `apt`, `dpkg`, `.deb`, third-party action, container, custom runner, or workflow-artifact transport. All bootstrap payload writes remain under `RUNNER_TEMP`, apart from the fixed line appended to `GITHUB_PATH`.
- The existing exact create-step name and prior-run classifier logic are byte-identical. No `32883517350` run-specific constant or special case is added.
- Exactly five secret references exist, all name `IMMUTABLE_RELEASES_ADMIN_TOKEN`, and each belongs to an isolated immutable-settings GET step. No other secret expression exists.
- The admin-token steps contain no mutation method and no endpoint other than `/immutable-releases`.
- Release mutation sources are exactly one draft POST, one serial upload command source over the exact 16 names, and one `draft:false` PATCH.
- No `--clobber`, `gh release create/edit/delete`, Release/asset delete, tag mutation, settings mutation, npm publish, retry wrapper around a mutation, or user-controlled release identity exists.
- Every checkout uses fixed release commit `da116e0...`, fetch depth and credential behavior from section 6.6; assembler has no checkout.
- Static inspection confirms one UTF-8 changelog read, the exact two heading regexes, one ECMAScript `.trim()`, one exclusive `wx` notes-file creation with exactly one appended LF, no separate CRLF/BOM handling, `jq --rawfile` from that notes file, and byte-for-byte comparison of every parsed Release body to the same file.
- Builders receive no Release ID or admin/Release credential.
- Artifact names include recovery run ID, run attempt, and matrix/final role; overwrite is false; downloads are by recorded ID.
- Every Bash block passes `bash -n`, every inline Node block passes `node --check`, and any retained inline Python block compiles.
- `npm run version:check`, `node --check npm/install.js`, and `node --check npm/run.js` pass at the fixed release tree.
- `git diff --check` passes; `.github/workflows/release.yml` remains byte-identical; the implementation diff contains exactly `.github/workflows/recover-immutable-v0.30.1.yml`; and no committed helper/test file exists.
- From clean implementation-base and final-head trees, rerun the same dependency detector, SCC-member comparison, `scripts/02-module-arc-record.mjs` record check, and the `loops_layering`, `instance_gitignore_layering`, and `project_settings_layering` guards. Green requires unchanged `cyclicSccs`, identical cyclic-SCC member sets, `0 added / 0 removed` application arcs, zero cross-boundary arcs, byte-identical `src-tauri/module-arcs.txt`, and all structural guards passing. Detector exit `1` is normal when it writes the graph for the existing cycle; exit `3`, any module delta, or any changed application path is a scope breach and returns the work to Full planning.

### 9.2 Executable state-machine fixtures

Use a deterministic temporary harness that parses the workflow as YAML, extracts the three bootstrap `run:` scalars, and first proves their parsed UTF-8 bytes identical. Execute one exact, unmodified bootstrap scalar in an isolated fresh temporary environment. Environment variables and PATH-injected spies or test doubles are the only fixture controls; do not rewrite the scalar or replace its shell logic. Spies record external-command order and every subsequent ordinary `gh` or Release-mutation call. The one-byte-corruption case uses the real checksum tool and proves `tar` was never invoked. Because a synthetic malicious tar cannot match the reviewed literal SHA-256, post-checksum malformed-archive cases may inject a checksum double that returns the reviewed literal while real GNU tar and real filesystem checks exercise every later guard. The positive compatibility fixture must run one complete exact unmodified bootstrap scalar on Ubuntu 22.04 with GNU tar exactly 1.34, use the real fixed official archive and production tools, prove the exact curl metadata, filesystem size, literal digest, member type/mode, compatible extraction vector, and canonical selected path, then execute the selected binary and prove its `gh version` output reports exact version `2.86.0` before the final `GITHUB_PATH` write.

Extract the workflow's existing classifiers/verifiers into the same temporary harness without modifying them. Require:

- exact absent Release plus exact prerequisites selects `fresh`;
- exact immutable 16-asset Release selects `resume` and executes zero mutations;
- with a hosted/preinstalled `gh 2.97.0`, the verified pinned `2.86.0` wins PATH at each of the three exact existing assertion locations and the mocked classifier reaches its first ordinary read;
- empty, relative, or symlinked `RUNNER_TEMP`, a pre-existing install root of any type, or a selected path escaping the canonical fresh root fails before download execution or any subsequent `gh` call;
- bad status, connection timeout, total timeout, second redirect, HTTP downgrade, credential-bearing effective URL, wrong curl-reported or filesystem size, empty/malformed/over-8192-byte curl JSON, or noninteger status/redirect/size metadata fails before checksum, tar, any subsequent `gh` call, or Release mutation as applicable;
- one-byte archive corruption fails under the real checksum tool before the first `tar`; missing, renamed, duplicate, traversing, symlink, or hardlink output, nonregular selected member, selected or extracted mode `0644`, `0775`, or `4755`, a pre-existing selected-path collision, non-executable output, link count other than one, wrong canonical PATH, or a selected binary reporting `2.97.0` each fails before any subsequent `gh` call or Release mutation;
- command-order evidence proves checksum precedes the first tar call, every bootstrap failure leaves an initially empty `GITHUB_PATH` byte-empty, and success leaves it containing exactly the fixed bin-directory line terminated by one LF;
- bootstrap failure in `assemble-release` proves zero asset-upload calls; bootstrap failure in `publish-and-verify-release` proves zero PATCH calls;
- compatible empty draft, partial draft, complete draft, mutable published Release, wrong identity, duplicate/extra/missing asset, wrong digest, 403, 5xx, timeout, malformed JSON, and ambiguous command/status combinations select `conflict` and execute zero further mutations;
- fresh mock executes one draft POST, one 16-item upload loop, and one PATCH only;
- wrong tag object/peel, wrong failed run/job/step/artifact count, changed workflow/changelog blob, wrong main ref, expired/empty admin token, or changed historical snapshot fails before the next mutation;
- a terminal prior recovery attempt containing exactly one terminal prepare job and the exact create step at `status:completed`, `conclusion:skipped`, together with exact Release 404, may select `fresh`; a nonterminal run, missing/duplicate prepare job, missing/renamed create step, create conclusion `success`, `failure`, `cancelled`, `null`, another value, an extra incomplete attempt, or ambiguous jobs reject `fresh`;
- Release-metadata fixtures cover the next-level-two-heading and EOF boundaries, reject missing or duplicate `0.30.1` headings and an empty trimmed body, prove heading exclusion plus one ECMAScript `.trim()` plus exactly one appended LF, and prove there is no separate CRLF or BOM normalization path;
- exact four transport archives extract safely and reconstruct the 15 payloads; traversal, link, device, duplicate, wrong mode, stale attempt, or wrong action-artifact digest is rejected;
- SHASUMS generation is byte-deterministic, sorted, exactly 15 rows, and validates every asset;
- direct 200 and one safe credential-free HTTPS 302 asset download succeed; other redirect/status cases fail;
- a simulated lost PATCH response followed by exact immutable readback enters verification without a second PATCH;
- exact immutable resume reconstructs a canonical manifest byte-identical to fresh-mode verification and may upload only final Actions evidence.

### 9.3 Live success postconditions

The recovery is successful only when every item is recorded:

1. Run `32883517350` remains terminal and its exact create step remains completed/skipped. Every recovery run used for the result is recorded by run ID and attempt and uses `workflow_dispatch`, `refs/heads/main`, the reviewed default-branch SHA, and the exact workflow path. The run that first crosses a Release mutation boundary and the terminal successful verifier run are identified separately when they differ; any later successful attempt is proven to have entered only classifier-authorized `fresh` before POST or exact read-only `resume` after immutable success.
2. Tag ref still names annotated object `5e4da64...`; it still peels to `da116e0...`; no tag mutation command ran.
3. One Release has the captured ID, tag `v0.30.1`, target `da116e0...`, name `Agents Commander v0.30.1`, and exact heading-excluding, trimmed changelog body ending in one LF; `draft:false`, `prerelease:false`, `immutable:true`.
4. The Release has exactly the 16 names in section 6.10, each unique, state uploaded, positive size, and with captured ID/API URL/browser URL/`sha256:` digest matching downloaded local bytes.
5. `SHASUMS256.txt` has exactly the other 15 assets and every digest matches.
6. Windows raw aliases and both macOS archive alias pairs are byte-identical.
7. `gh release verify v0.30.1` passes and identifies the exact tag/commit/assets.
8. All 16 `gh release verify-asset` commands pass against the downloaded bytes.
9. In every `gh`-using job, PATH resolved the reviewed `RUNNER_TEMP` binary and it reported exact GitHub CLI `2.86.0`; the hosted preinstalled CLI was not used.
10. Final canonical Release manifest and evidence artifact ID/digest are retained and reported.
11. v0.30.0 and every pre-existing Release/asset snapshot are unchanged.
12. npm `@mblua/agentscommander@0.30.1` remains absent; no npm mutation occurred.
13. The PAT and repository secret were not printed, copied, rotated, revoked, renamed, or changed.
14. Repository worktree is clean and no file other than the temporary workflow changed in the implementation commit.

### 9.4 Cleanup acceptance

The separate cleanup is complete only when its diff deletes exactly `.github/workflows/recover-immutable-v0.30.1.yml`, no recovery run remains queued/running, immutable v0.30.1 and all evidence still verify, historical Releases remain unchanged, and the removed workflow is no longer dispatchable from main.

## 10. Step 7 certification and remaining gates

The former Step 5 enrichment, shipper `FORMAL_STEP_6_PASS`, architect consensus, and Step 7.5 approval apply only to the historical plan blob `62895f04770b62a5838704b11bb61cabba291f7e` and impact blob `14236921d56b90a9c5796461ff8e0740d433c34a`. Their committed-byte SHA-256 values are `95A54C0FB9E9F242980F20F4FCAF0D57015A3A2FF21AA31DEC7E0B933936B094` and `38B621FDB471891CCD48C3DEFDE58933E799F6BF04C6066E6904F3935483CD07`. Those approvals remain historical audit evidence, but recovery run `32883517350` proved the moving-runner CLI assumption incomplete. They are not current authorization for this correction.

The owner approved planning and review of the exact GitHub CLI `2.86.0` correction in section 6.2.1. That approval does not authorize workflow implementation, rerun, dispatch, Release or npm mutation, tag or secret changes, cleanup, or deployment.

Developer round-2 enrichment reviewed all 690 pre-certification plan lines at exact head `a8910732da43f715efafaabf7acbd5f71b5cc1fa` and returned PASS with no implementation-critical gap. The formal Step 6 rereview returned `FORMAL_STEP_6_PASS` at the same head with blocker count `0` and optional finding count `0`. Independent release/shipper validation passed the complete positive Ubuntu 22.04 and GNU tar 1.34 fixture, the real corrected collision case, the unchanged-scalar collision fixture, retained negative fixtures, exact `gh 2.86.0` execution, and every zero-mutation boundary. The Markdown and impact HTML are synchronized across all locked identities, the CLI archive contract, failed-run evidence, create-step name, secret, and SHASUMS contract. The Plan Contract is complete and the unresolved-choice scan found no open implementation choice.

Final architect consensus freshly certifies the exact dependency-cycle and layering evidence in section 5.3: `cyclicSccs` remains `1 -> 1`, the sole 85-member SCC set is identical, the arc delta is `0 added / 0 removed`, zero arcs cross a previously clean SCC boundary, `src-tauri/module-arcs.txt` is byte-identical, all 42 arc-record self-tests pass, all 8 tests across the three layering suites pass, and no lower layer gains a UI transport, `AppHandle`, or `tauri` dependency. The sole later implementation path remains the existing recovery workflow YAML.

The remaining delivery gate is Step 7.5 human impact review/approval followed by the required workgroup purge. Only after those gates may the coordinator authorize an implementation handoff. After a later reviewed implementation merge, the complete live preflight and a separate explicit owner authorization are still required before exactly one new no-input dispatch. Never rerun `32883517350`.

Verdict: `READY_FOR_IMPLEMENTATION` as a technical plan consensus only. It authorizes no implementation or external mutation.
