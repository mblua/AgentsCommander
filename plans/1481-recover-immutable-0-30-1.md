# Issue #1481: recover the immutable GitHub Release v0.30.1

Status: READY_FOR_IMPLEMENTATION

Impact-HTML: plans/1481-recover-immutable-0-30-1-impact.html

Delivery path: Full, Step 7 consensus complete. This document is the sole implementation specification. Its READY verdict is not authorization to implement, merge, dispatch, publish, clean up, or modify external state; mandatory Step 7.5 human review, freeze, and purge remains next.

## 1. Issue and objective

Issue: [mblua/AgentsCommander#1481](https://github.com/mblua/AgentsCommander/issues/1481)

Branch: `fix/1481-recover-immutable-0301`

Base: `origin/main` at `ce60240a2bfacabe60d27371c0a90e2d114a56d3`

The annotated tag `v0.30.1` exists at its approved release commit, but the tag-triggered Release workflow failed before creating a GitHub Release. Add one temporary, version-specific, manually dispatched recovery workflow that creates and verifies the missing immutable GitHub Release without moving, deleting, or recreating the tag. The workflow must be safe to dispatch when the Release is absent, safe to rerun read-only after exact immutable success, and fail closed for every partial, mutable, conflicting, or ambiguous state.

The implementation changes exactly one new tracked path:

`.github/workflows/recover-immutable-v0.30.1.yml`

After exact recovery success and retained evidence, remove that temporary workflow in a separate reviewed cleanup change. Cleanup is not part of the recovery dispatch or the implementation diff specified by this plan.

## 2. Locked decisions and identities

No value in this table is a workflow input. Put the values into the workflow as literals or fixed environment constants and compare every API response against them.

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
| Release changelog Git blob | `b0095eac4158e3fa575de36b1f4be9ada94b542e` |
| Failed workflow run | `32791566112`, attempt `1` |
| Failed workflow ID | `249288854` |
| Failed job | `prepare-release`, ID `97633984311` |
| Failed step | number `4`, `Validate authority and classify the release` |
| Failed event/ref/SHA | `push`, `refs/tags/v0.30.1`, `da116e0b2463d6ebec6ace5495e2954d6d8ccbee` |
| Failed run timing | created `2026-08-24T23:56:42Z`, completed `2026-08-24T23:56:57Z` |
| Expected original-run artifacts | `total_count: 0` |
| Release title | Exact literal `Agents Commander v0.30.1` |
| Release body | UTF-8 `CHANGELOG.md` slice defined in section 6.6, excluding its heading, trimmed, nonempty, and ending in exactly one LF in both the notes file and API body |
| Release flags | `draft:true`, `prerelease:false`; publish only by changing `draft` to `false` |
| Immutable setting | exact HTTP 200 JSON `{"enabled":true,"enforced_by_owner":false}` |
| Admin-read secret name | `IMMUTABLE_RELEASES_ADMIN_TOKEN` |
| Shared concurrency group | `release-v0.30.1`, `cancel-in-progress:false` |

The recovery workflow must not accept a version, tag, SHA, Release ID, title, body, asset name, repository, failed-run ID, confirmation, or other release-identity input. The only trigger is an empty `workflow_dispatch` mapping. GitHub's branch selector is not trusted input: the first job must reject every ref other than `refs/heads/main`.

## 3. Evidence and identified cause

### 3.1 Repository evidence

- The clean planning branch and `origin/main` both point to `ce60240a2bfacabe60d27371c0a90e2d114a56d3`.
- Release commit `da116e0b2463d6ebec6ace5495e2954d6d8ccbee` is an ancestor of current main.
- `.github/workflows/release.yml` and `CHANGELOG.md` have the same Git blobs at the release commit and current main: `a5e437996ea3a91e0688842f80e6c466f35cff19` and `b0095eac4158e3fa575de36b1f4be9ada94b542e`.
- The current release workflow is a reviewed four-job state machine: `prepare-release`, a four-entry `build-release` matrix, `assemble-release`, and `publish-and-verify-release`.
- It already has exact fresh-versus-immutable-resume classification, build-only matrix jobs, run/attempt-bound artifact transport, serial non-clobbering upload, one publication PATCH, bounded asset reconstruction, Release and per-asset attestation verification, and final evidence retention.
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

### 3.4 Platform contract

GitHub documents that `workflow_dispatch` runs only when its workflow exists on the default branch, Contents write permits Release creation, and the immutable-settings GET requires Administration read. GitHub's immutable sequence is create a draft, attach all assets, then publish the draft. Publication locks the associated tag and assets and generates a signed Release attestation. Official verification is `gh release verify` plus `gh release verify-asset` for every local asset.

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

- Add the one temporary recovery workflow.
- Reuse the frozen Release workflow's exact state-machine logic, matrix, build commands, action pins, toolchains, asset mappings, transport validation, Release creation/upload/publication boundaries, query-first reconciliation, attestation checks, and evidence format.
- Replace tag-push context assumptions with hardcoded `v0.30.1` identities and main-only manual-dispatch validation.
- Add exact failed-run context validation before any mutation.
- Route every immutable-setting GET through the admin-read secret and no other request through that secret.
- Use the ordinary job `GITHUB_TOKEN` with minimum job permissions for Release reads and mutations.
- Preserve v0.30.0 and every pre-existing Release while adding only v0.30.1.
- Specify the later one-file cleanup PR as a separate follow-up boundary.

### 4.2 Out of scope

- Moving, deleting, recreating, force-updating, or otherwise mutating any Git tag.
- Rerunning run `32791566112` or any job from it.
- Editing `.github/workflows/release.yml` or any existing application, package, version, changelog, lock, config, documentation, or test file.
- Publishing, unpublishing, changing dist-tags, or otherwise mutating npm. No `npm publish`, OIDC, npm token, registry credential, or Phase B workflow belongs here.
- Editing, deleting, recreating, republishing, or retagging historical v0.30.0.
- Enabling, disabling, or changing repository immutable-release settings.
- Creating, rotating, revoking, exposing, renaming, copying, or otherwise modifying the PAT or repository secret.
- Generalizing the workflow for another version, repository, tag, run, or asset set.
- Automatically deleting or repairing a conflicting draft or partial Release.
- Removing the temporary workflow in the same implementation PR or from the recovery run.
- Adding a dependency, helper module, reusable workflow, composite action, or generated source file.

## 5. Exact affected surfaces

### 5.1 Proposed modification

| Kind | Exact repo-relative path | Exact surface | Reason |
| --- | --- | --- | --- |
| Add | `.github/workflows/recover-immutable-v0.30.1.yml` | Entire new workflow | Main-branch, no-input, one-shot recovery for the already existing tag and missing Release. |

No other tracked path may change. Planning artifacts are delivery documentation and are not implementation paths.

### 5.2 Evidence-backed related-only files

These paths constrain the new workflow but remain byte-identical.

| Exact repo-relative path | Relationship to the new workflow | Frozen evidence |
| --- | --- | --- |
| `.github/workflows/release.yml` | Copy the reviewed four-job state machine, action pins, permissions pattern, build matrix, asset staging, manifest, mutation boundaries, resume classifier, and evidence checks. | Git blob `a5e437996ea3a91e0688842f80e6c466f35cff19` at release commit and current main. |
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

No Rust module, TypeScript module, IPC command/event/type, Tauri window, configuration schema, persistence format, PTY/ConPTY flow, frontend behavior, package dependency, or module dependency arc changes. The final Step 7 dependency-cycle gate enumerates `0 added / 0 removed` module arcs: the sole implementation path is a new GitHub Actions YAML file outside `src-tauri` and every application module root, so there is no new or removed `file:line -> module` reference to classify, zero cross-boundary arcs, no SCC that can be created, grown, or joined, and no lower layer that can gain a UI-transport, `AppHandle`, or `tauri` dependency. At reviewed head `fbb3e4103eb08502b25f8469b9968c98a6587f36`, the tracked and worktree `src-tauri/module-arcs.txt` bytes are identical at Git object `871fc46e1985728e872bbf1f26743f8bf3518573`. The workgroup `rust-levelization-run` detector is unavailable, so this certification uses the documented explicit per-arc manual fallback and does not claim remeasurement of pre-existing `cyclicSccs` or SCC member sets. That limitation cannot conceal a planned module delta under the exact one-workflow-path contract. The implementation reviewer must require a diff containing only `.github/workflows/recover-immutable-v0.30.1.yml` and a byte-identical arc record. If scope expands into module/import structure, stop, return to planning, run the repository dependency detector on clean base and final trees, and require unchanged `cyclicSccs`, identical SCC member sets, zero cross-boundary arcs, a byte-identical arc record, and green structural layering guards.

## 6. Decided solution

### 6.1 Start from the reviewed workflow, do not redesign it

Create `.github/workflows/recover-immutable-v0.30.1.yml` by copying the current `.github/workflows/release.yml` at Git blob `a5e437996ea3a91e0688842f80e6c466f35cff19`. Keep its four-job build, assembly, publication, exact-immutable resume, and evidence state machines. Apply only the context, authority, failed-run, and temporary-workflow changes specified below. Do not refactor shell/Node blocks, introduce helpers, change the matrix, change filenames, update actions, update toolchains, change retention semantics, or opportunistically simplify the copied logic.

The justified duplication isolates a one-time recovery from the production tag workflow. Making the existing workflow reusable would modify a proven release surface and broaden this recovery beyond one file.

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

Use the copied job graph with recovery-specific first-job name:

```text
prepare-recovery
  -> build-release[linux, windows, mac-arm64, mac-x86_64]   (fresh only)
  -> assemble-release                                      (fresh only)
  -> publish-and-verify-release                            (fresh or exact immutable resume)
```

`publish-and-verify-release` keeps the copied `always()` dependency admission so an exact immutable `resume` can run while build and assembly are intentionally skipped. It must still require successful `prepare-recovery`, and fresh mode must require successful build and assembly.

Use bounded job timeouts: 15 minutes for prepare, 90 minutes for each builder, 45 minutes for assembly, and 30 minutes for final publication/verification. Matrix `fail-fast` is false so independent build failures remain diagnosable, but assembly cannot run unless all four builders succeed.

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
- `github.run_attempt == 1` for the intended first dispatch; a later attempt is admitted only by the state classifier and can mutate only when the Release is still exact 404 and no earlier recovery attempt crossed the draft POST;
- remote default branch is `main` and its head equals the dispatch `github.sha`;
- release commit `da116e0...` is an ancestor of dispatch `github.sha`;
- `.github/workflows/release.yml` and `CHANGELOG.md` at both release commit and dispatch SHA have the fixed Git blobs from section 2;
- the only recovery-workflow run currently in progress is the current run, enforced together with the literal concurrency group;
- the checked-in recovery workflow has no user inputs and no runtime replacement for any fixed identity.

The workflow cannot embed the SHA-256 of its own future merge bytes. Review and Step 7.5 bind those bytes before implementation; implementation review binds the one-file diff; the runtime gate binds the executing file to the current default-branch SHA and rejects a stale ref. This is not an open implementation choice.

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
6. asserts GitHub CLI exactly `2.86.0` before relying on Release-verification JSON;
7. runs `gh release verify v0.30.1 --repo mblua/AgentsCommander --format json` and parses the signed tag, commit, and complete asset subjects;
8. runs `gh release verify-asset v0.30.1 <exact-local-path> --repo mblua/AgentsCommander --format json` once for each of the 16 local files and requires the expected digest/release association;
9. compares the canonical post snapshot of every pre-existing Release and asset to the pre snapshot, excluding only the newly added exact v0.30.1 record. v0.30.0 must also match its explicit ID/commit/state/count guard;
10. emits a canonical final record containing recovery mode, current run/attempt/path/head SHA, failed-run proof, tag object/peel, Release ID/URL/state/body hash, all 16 IDs/names/sizes/digests/URLs, checksum-manifest hash, attestation results, historical snapshot hash, and mutation invocation/outcome flags;
11. uploads a final Actions artifact named `recover-v0.30.1-<run_id>-<run_attempt>-final-evidence` with overwrite false and records its artifact ID/service digest in the job summary.

Read-only polling for immutable state and attestations is allowed for at most 30 attempts, ten seconds apart. Polling never repeats a mutation. Timeout after the publication invocation is an evidence failure over potentially committed immutable state; a later dispatch can enter only exact read-only resume.

### 6.14 Failure and retry behavior

| Boundary/state | Required behavior |
| --- | --- |
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
- Windows path checks must normalize drive/case, remain under the workspace, and reject reparse-point/path traversal surprises. Do not introduce `cmd.exe /C` wrapping; retain the copied workflow's platform-specific shell behavior.
- Existing v0.30.0 and all other Releases remain read-only.

### 7.2 Security

- The least-privilege admin PAT is exposed only to five exact read-only settings steps. It never authorizes a Release mutation.
- The normal `GITHUB_TOKEN` carries Contents write only in jobs that need Release mutation and Actions read only where artifact/run APIs are used. Builders are Contents read only.
- Fixed identities, main-only dispatch, exact failed-run proof, fixed Git blobs, exact tag object/peel, closed filenames, non-clobbering uploads, numeric Release/asset IDs, and query-first classification remove user-controlled release identity and replacement paths.
- No tag command exists. The Release creation API receives an already verified existing tag; no fallback may synthesize one.
- No secret, token, environment value, external JSON, API string, or filename is interpolated into executable shell/JavaScript source. Validate parsed values against fixed schemas and pass them through environment/files.
- Third-party actions remain pinned to reviewed full commit SHAs. No new action or dependency is introduced.
- PAT expiration is safe: inability to prove settings state blocks before the next mutation. This plan does not authorize changing the token or secret.

### 7.3 Residual risks

- GitHub Actions runners, GitHub REST, immutable-release enforcement, Release attestations, and pinned third-party action repositories remain external dependencies.
- A failure after draft creation can leave a mutable draft requiring separate owner reconciliation.
- A failure after PATCH invocation can leave committed immutable state before the run stores final evidence. Exact read-only resume addresses evidence recovery, not rollback.
- The current Windows SignPath placeholder remains accepted release policy; this recovery neither fixes nor worsens it.
- Current main can advance between review and dispatch. The runtime binds to the dispatched main SHA and fixed release/changelog blobs, while normal PR review and human dispatch ownership remain central controls.

## 8. Implementation order

### Phase 1: one-file recovery implementation

1. Reconfirm branch/base and the fixed Git object identities without mutating external state.
2. Copy the exact frozen `.github/workflows/release.yml` blob to `.github/workflows/recover-immutable-v0.30.1.yml`.
3. Change only the workflow envelope, dispatch context, fixed release checkout, failed-run gate, PAT-isolated immutable GETs, and recovery evidence names specified here.
4. Preserve the current state-machine/matrix/action/toolchain/asset/manifest/upload/PATCH/verification blocks byte-for-byte wherever their tag-push context does not require the explicit recovery substitutions.
5. Add executable static fixtures for validation only in an implementer-local or replica-local scratch location. Do not commit a helper/test file.
6. Run the static and executable checks in section 9.
7. Commit exactly the new workflow file. Review the one-file diff against this plan. Do not dispatch from the branch.
8. Merge through the normal reviewed PR to main. Record the merge SHA and exact recovery-workflow SHA-256 externally.

### Phase 2: owner-controlled dispatch and evidence

1. Reconfirm the secret exists without reading its value and that the token remains valid, repository-scoped, and Administration/Metadata read-only.
2. Reconfirm tag object/peel, Release exact 404, failed-run context, immutable setting, current main/workflow bytes, v0.30.0, all pre-existing Releases, and npm 0.30.1 absence.
3. Dispatch the recovery workflow once from `main`, with no inputs.
4. Monitor it to terminal state. After any mutation uncertainty, query exact Release state before considering any further action.
5. Record run/attempt/URL/head SHA, Release ID/URL/state, complete asset IDs/sizes/digests, checksum hash, attestation results, final evidence artifact ID/digest, unchanged historical snapshots, npm still absent, and clean repository state.

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
- `git diff --check` passes and the implementation diff contains exactly `.github/workflows/recover-immutable-v0.30.1.yml`.

### 9.2 Executable state-machine fixtures

Extract the workflow's own classifiers/verifiers into a temporary harness without modifying them. Require:

- exact absent Release plus exact prerequisites selects `fresh`;
- exact immutable 16-asset Release selects `resume` and executes zero mutations;
- compatible empty draft, partial draft, complete draft, mutable published Release, wrong identity, duplicate/extra/missing asset, wrong digest, 403, 5xx, timeout, malformed JSON, and ambiguous command/status combinations select `conflict` and execute zero further mutations;
- fresh mock executes one draft POST, one 16-item upload loop, and one PATCH only;
- wrong tag object/peel, wrong failed run/job/step/artifact count, changed workflow/changelog blob, wrong main ref, expired/empty admin token, or changed historical snapshot fails before the next mutation;
- Release-metadata fixtures cover the next-level-two-heading and EOF boundaries, reject missing or duplicate `0.30.1` headings and an empty trimmed body, prove heading exclusion plus one ECMAScript `.trim()` plus exactly one appended LF, and prove there is no separate CRLF or BOM normalization path;
- exact four transport archives extract safely and reconstruct the 15 payloads; traversal, link, device, duplicate, wrong mode, stale attempt, or wrong action-artifact digest is rejected;
- SHASUMS generation is byte-deterministic, sorted, exactly 15 rows, and validates every asset;
- direct 200 and one safe credential-free HTTPS 302 asset download succeed; other redirect/status cases fail;
- a simulated lost PATCH response followed by exact immutable readback enters verification without a second PATCH;
- exact immutable resume reconstructs a canonical manifest byte-identical to fresh-mode verification and may upload only final Actions evidence.

### 9.3 Live success postconditions

The recovery is successful only when every item is recorded:

1. Every recovery run used for the result is recorded by run ID and attempt and uses `workflow_dispatch`, `refs/heads/main`, the reviewed default-branch SHA, and the exact workflow path. The run that first crosses a Release mutation boundary and the terminal successful verifier run are identified separately when they differ; any later successful attempt is proven to have entered only a classifier-authorized `fresh` retry before POST or exact read-only `resume` after immutable success.
2. Tag ref still names annotated object `5e4da64...`; it still peels to `da116e0...`; no tag mutation command ran.
3. One Release has the captured ID, tag `v0.30.1`, target `da116e0...`, name `Agents Commander v0.30.1`, and exact heading-excluding, trimmed changelog body ending in one LF; `draft:false`, `prerelease:false`, `immutable:true`.
4. The Release has exactly the 16 names in section 6.10, each unique, state uploaded, positive size, and with captured ID/API URL/browser URL/`sha256:` digest matching downloaded local bytes.
5. `SHASUMS256.txt` has exactly the other 15 assets and every digest matches.
6. Windows raw aliases and both macOS archive alias pairs are byte-identical.
7. `gh release verify v0.30.1` passes and identifies the exact tag/commit/assets.
8. All 16 `gh release verify-asset` commands pass against the downloaded bytes.
9. Final canonical Release manifest and evidence artifact ID/digest are retained and reported.
10. v0.30.0 and every pre-existing Release/asset snapshot are unchanged.
11. npm `@mblua/agentscommander@0.30.1` remains absent; no npm mutation occurred.
12. The PAT and repository secret were not printed, copied, rotated, revoked, renamed, or changed.
13. Repository worktree is clean and no file other than the temporary workflow changed in the implementation commit.

### 9.4 Cleanup acceptance

The separate cleanup is complete only when its diff deletes exactly `.github/workflows/recover-immutable-v0.30.1.yml`, no recovery run remains queued/running, immutable v0.30.1 and all evidence still verify, historical Releases remain unchanged, and the removed workflow is no longer dispatchable from main.

## 10. Full-path consensus and readiness

Step 5 developer enrichment completed at commit `e73e302d64ac6456f27aac0bfeaa49c4443eb112`; its UTF-8, no-separate-CRLF/BOM-preprocessing, jq `--rawfile`, same-notes-file comparison, and static/fixture criteria are synchronized between this Markdown and the impact HTML. The user-authorized independent shipper then reviewed head `fbb3e4103eb08502b25f8469b9968c98a6587f36` and issued `FORMAL_STEP_6_PASS`: no blocking release-risk defect, no required plan addition, and no repository change.

The final architect consensus revalidated the clean branch, all frozen related-only blobs, the one-workflow implementation surface, Markdown/HTML contract parity, and the dependency/layering gate in section 5.3. No open decision, unresolved placeholder, competing alternative, or implementer choice remains. The verdict is `READY_FOR_IMPLEMENTATION`.

Step 7.5 must independently freeze the exact plan and impact-HTML bytes, recompute the externally reported digests, obtain explicit human approval, and purge raw planning context. Until that gate succeeds, no implementation, merge, dispatch, publication, cleanup, or external-state mutation is authorized.
