# Plan #1742: Native GitHub and npm release controls

Author: `__agent_architect`, wg-23-community, 2026-09-04 UTC.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1742](https://github.com/mblua/AgentsCommander/issues/1742)

Delivery path: Full.

This document supersedes, in place, the prior Ed25519 release-tag-authority
plan at this path. That certification is void. The custom authority, local
vault, signed tag envelope, ledger, and repository verifier are not to be
implemented by this issue.

## 1. Objective, scope, and frozen evidence

Harden the existing tag-to-release pipeline with GitHub and npm controls that
are native or already audited. A release must be initiated by a protected tag,
run its privileged workflow from protected default-branch bytes, keep build
work separate from GitHub Release and npm credentials, publish through npm
Trusted Publishing, and make the exact package tarball attestable and verified
before npm publication.

This plan is based on `main` and `origin/main` at
`302e6c12cad6f1701bbd937c38475ad376fe2afb`, on branch
`feature/1742-release-tag-authority`. The relevant current workflow is
`.github/workflows/release.yml` at blob
`598adb75c6838c1c69ceb435180132167076a44a`.

The current release chain is:

```text
guard -> release-coordinator -> build -> checksums ->
publish-github -> verify-release -> publish-npm
```

It is already a large guarded workflow. Keep its release preparation, version,
checksum, draft, asset, registry, post-publish, and smoke-test logic unless a
step below specifically changes it. This issue does not change a version,
changelog, tag, Release, npm package metadata, lock file, Cargo file, product
source, or application configuration.

### Exact repository change surface

Create:

1. `.github/workflows/release-tag-wake.yml`

Modify:

1. `.github/workflows/release.yml`

Do not create or modify:

- `scripts/publisher-authority.mjs`, `scripts/publish.mjs`, or
  `scripts/verify-release-tag-authorization.mjs`;
- `.github/release-authority/`, an encrypted private-key root, a tag envelope,
  a ledger, claims, intents, lanes, or a custom release database;
- package dependencies, package scripts, a lockfile, Rust or TypeScript code,
  product configuration, or a companion HTML report; and
- a generic release rewrite, a second publisher, or a separate artifact
  transport.

## 2. Security decisions and the trigger seam

### 2.1 The native trust decision

A `push` workflow is evaluated from the pushed ref. A `create` workflow also
has the created tag/ref as its event ref; it is not a reliable default-branch
workflow-source root. Tag protection and workflow-execution protections reduce
who can cause that code to run, but neither changes the bytes GitHub selects
for an allowed tag pusher.

Therefore do not retain a privileged tag `push` release workflow and do not
switch to `create`. Use GitHub's native `workflow_run` privilege-separation
mechanism instead:

```text
protected v* tag push
  -> release-tag-wake.yml, tag-owned, permissions: {}
  -> completed workflow_run event carrying only untrusted wake metadata
  -> release.yml, default-branch source, validate remote tag independently
  -> existing guarded build/release/npm job chain
```

`release-tag-wake.yml` is a deliberately small Module. Its Interface is one
completed workflow run, not an artifact, cache key, output, or assertion. Its
Implementation has no token permission, no secret, no environment, no
checkout, no third-party action, no artifact upload, and no release or npm
operation. Its only role is to wake the protected workflow. If a tag changes
this workflow, it can at most cause a no-credential GitHub-hosted job to run or
fail; it cannot supply trusted data to a privileged job.

`release.yml` is the deep Module that owns all high-impact behavior. Its
Interface is the untrusted `workflow_run` event plus the remote Git reference.
Its guard Implementation independently validates the event, tag, target
commit, release principal, and protected-main reachability before it exposes
validated outputs to callers. No caller or test may use a wake-workflow
artifact, cache, output, workspace, or conclusion as authorization.

### 2.2 The first protected job

Change the `release.yml` trigger from tag `push` to:

```yaml
on:
  workflow_run:
    workflows: [Release tag wake]
    types: [completed]
```

Keep the existing workflow name if it is externally referenced. At the first
job, retain the job id `guard` and make it the only source of release tag and
target outputs. It must have `contents: read` only, no environment,
`id-token`, `attestations: write`, `contents: write`, secret, cache restore,
or writer command.

The guard must fail closed unless all of these are true:

1. The event identifies the same repository, `workflow_run.event` is `push`,
   the triggering workflow path/name is the dedicated wake workflow, and its
   `head_branch` is an exact supported release-tag name. Treat all event fields
   as untrusted strings until validated.
2. `workflow_run.actor.login` exactly equals the administrator-configured
   non-secret Actions variable `RELEASE_TAG_ACTOR`. The variable must name the
   same GitHub App or account allowed by the tag ruleset. Missing, empty, or
   mismatched values fail. Do not use a deploy key as the release actor because
   GitHub can attribute a deploy-key push to an administrator instead.
3. A fresh remote Git query resolves exactly one `refs/tags/<validated-tag>`
   and the peeled target exactly equals `workflow_run.head_sha`. An empty
   successful exact-ref query is absence; a transport, authentication, parse,
   or mismatched-ref result is failure.
4. Fetch `origin/main` from the protected default branch and prove the peeled
   tag target is an ancestor of it. This permits a reviewed historical main
   commit but rejects a tag pointing at an arbitrary branch, fork, or
   tag-only commit. Do not use `GITHUB_SHA`, `GITHUB_REF`, or
   `GITHUB_REF_NAME` as a release source input.
5. Resolve the version and existing preparation evidence from a checkout made
   by exact peeled target SHA only after steps 1 through 4. Preserve the
   current version/evidence checks, but rename or remove any assertion that
   calls `release-authority-v1.txt` tag authorization. That file may remain
   preparation evidence only.

Write only the validated tag name, tag-object identity, peeled target SHA, and
version to `$GITHUB_OUTPUT`. Every downstream job takes these from `guard` and
re-resolves the remote tag immediately before its first mutation. The
re-resolution must require the same tag-object and peeled-target identities;
tag movement, an absent tag, ambiguity, or any query error stops the job.

The upstream workflow's `success` conclusion is only a scheduling convenience.
It is never proof that the tag is trusted. The protected guard is the proof.

### 2.3 Job capabilities and separation

Retain the existing job chain and direct `needs` relationships. Add a direct
or transitive `needs: guard` dependency before every existing job. In
particular, `release-coordinator`, `build`, `checksums`, `publish-github`,
`verify-release`, and `publish-npm` must not execute when `guard` is skipped
or fails.

Set explicit, minimum job permissions instead of inheriting a write-capable
default:

| Job | Required capability | Explicitly forbidden |
|---|---|---|
| `guard` | `contents: read` | all write permissions, environment, OIDC, attestation, cache/artifact intake from wake |
| `release-coordinator` | current minimum `contents: write` only if it creates the draft | npm OIDC, environment, attestation write |
| `build` | `contents: read`, existing artifact-service upload only | `contents: write`, Release asset/upload API, npm OIDC, npm token, environment, attestation write |
| `checksums` | `contents: read`, existing artifact-service upload only | Release write, npm OIDC, npm token, environment, attestation write |
| `publish-github` | `contents: write` only after guard/checksum success | npm OIDC, npm token, environment, attestation write |
| `verify-release` | `contents: read` | Release write, npm OIDC, npm token, environment, attestation write |
| `publish-npm` | `contents: read`, `id-token: write`, `attestations: write`, protected environment | `contents: write`, `NODE_AUTH_TOKEN`, `NPM_TOKEN`, any npm token secret |

`id-token: write` appears in `publish-npm` and nowhere else. The GitHub
artifact attestation is intentionally created in that job after it downloads
and validates the build output, rather than in `build`; this preserves the
requested credential boundary. It records provenance for the exact artifact
the publication job is about to publish, not an unverified artifact passed
from the tag wake workflow.

Do not restore a cache, download an artifact, or consume an output created by
`release-tag-wake.yml`. Keep `swatinem/rust-cache` only in `build`, after the
guard's protected-main/tag validation and target-SHA checkout. This prevents
the standard `workflow_run` cache/artifact privilege-escalation hazard.

## 3. Required workflow edits

### 3.1 Add the zero-credential wake workflow

Create `.github/workflows/release-tag-wake.yml` with these fixed properties:

1. Name it exactly `Release tag wake`.
2. Trigger only on `push` of `v*` tags. It has no `workflow_dispatch`,
   `create`, `schedule`, `repository_dispatch`, reusable-call, or broad branch
   trigger.
3. Set top-level `permissions: {}`. Do not elevate a job permission.
4. Run exactly one no-op wake job. It must not check out a ref, call an action,
   use a secret, create an artifact/cache/output, access a GitHub API, invoke
   npm, or mutate a Release.
5. Use a per-tag concurrency group with cancellation disabled only to reduce
   duplicate wake runs. Concurrency is not an authorization or idempotency
   mechanism.

There are no third-party actions in this workflow. A tag-controlled edit to
this file cannot become a high-privilege action because the privileged work
lives in the default-branch `workflow_run` workflow.

### 3.2 Rewire `release.yml` without rewriting its working jobs

At existing top-level lines 3 through 15, replace the tag `push` trigger with
the `workflow_run` trigger in Section 2.2; retain disabled cancellation and
use a concurrency group based on the validated guard output or the triggering
run id, never an unvalidated shell interpolation.

At the existing `guard` job beginning near line 16:

1. Replace raw tag event assumptions with the validation sequence in Section
   2.2. Use fixed shell arguments and a fresh bare Git directory or existing
   safe Git helper. Never execute or source code from a tag checkout before
   validation.
2. Replace every `GITHUB_SHA`/`GITHUB_REF_NAME` tag-target assumption with
   guard outputs. A `workflow_run` run itself is associated with default
   branch source, not the release tag.
3. Remove the current raw comparison between tag text and
   `docs/releases/<tag>/release-authority-v1.txt` as a security gate. Retain
   the underlying release evidence integrity checks under a preparation-evidence
   name.
4. Preserve current normal-semver, version synchronization, manifest,
   checksum, release-body, and source-tree checks. They are release quality
   checks, not authorization substitutes.

At the existing downstream jobs near lines 777, 878, 1226, 1781, 2124, and
2362:

1. Keep their current work and ordering. Do not move build/release/npm logic
   into a new script or workflow.
2. Feed tag, version, and target SHA solely from `guard` outputs.
3. Reuse and tighten the existing downstream tag-rebinding blocks so each
   writer verifies the live tag object and target against guard outputs before
   receiving a write credential or sending a writer command.
4. Fetch/check out the validated peeled target by SHA for source-dependent
   work. Never check out the wake event ref or use the wake workspace.
5. Keep `build` and `checksums` credentialless with respect to GitHub Releases
   and npm. Artifact upload to the GitHub Actions artifact service remains
   allowed because it is neither a Release mutation nor an npm credential.

### 3.3 Pin every action with a reviewable version comment

The structural scan found the following action references in `release.yml`.
They are already full SHA pins, but each must gain the listed source-version
comment. Do not change their implementation behavior or float a ref.

```yaml
# stable channel snapshot; upstream does not publish a semver action tag
uses: dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c

# v2.9.2
uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6

# v0.6.2
uses: tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5

# v4.6.2
uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02
```

Add the native artifact attestation action in `publish-npm` as this exact pin:

```yaml
# v4.1.0
uses: actions/attest@59d89421af93a897026c735860bf21b6eb4f7b26
```

Before merge, scan both release workflows for every `uses:` key. Green means
each external action has a 40-hex commit SHA and immediately adjacent comment
naming its upstream release tag, channel, or documented untagged snapshot. A
new action is not permitted without an equivalent pin/comment pair and a
reason in the pull request.

### 3.4 Preserve immutable Release behavior and make absence checks fail closed

At `release-coordinator`, `publish-github`, and `verify-release`, retain the
existing draft and asset flow. Tighten it to this closed state machine:

```text
exactly absent Release -> create one draft -> attach only verified missing assets
verified complete draft -> publish once -> immutable public Release
```

The only public Release state transition is draft to public. Never amend a
published Release, replace/delete a published asset, move/delete the release
tag, or re-upload a same-named asset. Before an asset upload, query the draft
assets by exact name. If it exists, require all of name, uploaded state,
byte-length, and `sha256:` digest to match the local expected file and skip
the upload. If it is absent, upload once. A mismatch, duplicate name response,
ambiguous asset, unexpected mutable/public state, or failed query is terminal.

Do not use a generic nonzero exit to infer absence. Update the existing shell
helpers at their current call sites so every absence decision has this
contract:

| Subject | Only accepted absence | Any other result |
|---|---|---|
| Git tag | successful exact-ref query with no exact ref | transport/auth/parse failure or a different ref: stop |
| GitHub Release | exact API 404 with expected parseable GitHub not-found response | 401, 403, 429, TLS/network, redirect, parse failure, 5xx, or unexpected body: stop |
| Draft asset | successful parsed asset list with no exact name | non-200, parse failure, duplicate/malformed entries, or wrong metadata: stop |
| npm version | exact official-registry version endpoint 404 with expected parseable not-found response | auth, rate limit, TLS/network, registry mismatch, parse failure, or 5xx: stop |
| Attestation | never absence-tolerant | no matching verified attestation: stop |

Retain the existing official-registry and token-clearing checks in
`publish-npm` rather than replacing them with a looser `npm view` exit-code
test. Only a successful request with a fully parsed expected response may
establish presence or absence.

### 3.5 Create and verify the package provenance before npm publish

At existing `publish-npm` lines 2464 through 2646, preserve the current
tokenless configuration checks and `npm pack` step. Insert these steps after
the final package tarball is assembled and SHA-256-checked against the
checksums artifact, immediately before the existing `npm publish` command:

1. Require the tarball's name, byte length, SHA-256, package name, and version
   to match the guarded release/checksum facts. Do not attest a glob or a
   workspace directory.
2. Run the SHA-pinned `actions/attest` action with `subject-path` set to that
   one tarball. The job-level `id-token: write` and `attestations: write`
   permissions are the only capabilities it needs to create GitHub build
   provenance.
3. In the next shell step, run `gh attestation verify` on that exact tarball
   with all of: `--repo "$GITHUB_REPOSITORY"`, the exact signer workflow
   `"$GITHUB_REPOSITORY/.github/workflows/release.yml"`, the expected
   default-branch workflow digest from `GITHUB_WORKFLOW_SHA`, the GitHub Actions
   OIDC issuer, and the default SLSA provenance predicate. Give `gh` only the
   job's ephemeral `github.token` as `GH_TOKEN`. A nonzero result, a different
   signer/repository/digest, a wrong predicate, a wrong subject, or a malformed
   result stops before npm publication.
4. Retain the existing tokenless command shape for the final publication:
   clear `NODE_AUTH_TOKEN`, `NPM_TOKEN`, `NPM_CONFIG_USERCONFIG`,
   `NPM_ID_TOKEN`, and `SIGSTORE_ID_TOKEN`, use the already validated official
   registry, then run `npm publish <exact-tarball> --access public --provenance`.
   Do not add an npm token secret or a `NODE_AUTH_TOKEN` fallback.
5. Preserve the current post-publication npm provenance and installed-package
   smoke checks. They remain useful confirmation but cannot compensate for a
   missing pre-publish GitHub attestation.

The pre-publish verification prevents publication of a tarball whose required
GitHub provenance is absent, for a different tarball, or signed by a different
repository/workflow/digest. It does not claim an offline human authorized the
release; that is an explicitly accepted residual risk in Section 6.

## 4. Administrator-owned deployment prerequisites

These are production gates, not implementation-agent mutations. An
administrator must configure and record them before a real release. If a gate
is unavailable, incorrectly scoped, or only in evaluate mode, do not release.

1. **Tag protection rule or ruleset.** Target `v*`; restrict creation, update,
   deletion, and force movement to the one `RELEASE_TAG_ACTOR` GitHub App or
   account. Do not allow deploy-key release pushes. Test that an ordinary
   writer cannot create, update, delete, or force a release tag.
2. **Workflow execution protection.** Enable an active Actions policy, not
   evaluate mode, that permits the intended release-principal `push` path and
   blocks unapproved manual/dispatch execution paths. Current GitHub
   documentation describes actor/event allow rules, not a tag-ref glob. The
   administrator must validate its actual repository-wide effect against normal
   CI and record the resulting actor/event policy. Do not claim a nonexistent
   `refs/tags/v*` policy scope. This is defense in depth; the default-branch
   `workflow_run` split is the high-privilege trust root.
3. **Protected default branch and workflow paths.** Require reviewed PRs for
   `main`; protect `.github/workflows/release.yml` and
   `.github/workflows/release-tag-wake.yml` with code-owner review; do not let
   the release actor bypass that protection. The guard's reachable-from-main
   check relies on this governance.
4. **Immutable Releases.** Enable repository or organization immutable
   releases before production use. Evidence is the administration setting or
   authenticated `GET /repos/{owner}/{repo}/immutable-releases` returning
   enabled, not a workflow guess. The setting applies to future Releases, so
   prove it using a disposable non-production release.
5. **npm Trusted Publishing.** Configure the `@mblua/agentscommander` trusted
   publisher with repository `mblua/AgentsCommander`, workflow filename
   `release.yml`, environment `release-production`, and allowed action `npm
   publish` only, not stage publish. Create `release-production` with required
   reviewers and deployment branches restricted to `main`. Assign that
   environment only to `publish-npm`.
6. **Release principal proof.** Store the exact allowed actor in
   `RELEASE_TAG_ACTOR`, verify it matches the tag rule, and produce a sandbox
   trace showing a permitted tag actor starts the zero-credential wake but a
   nonpermitted actor cannot create the tag or reach the privileged flow.

## 5. Kept controls and the failure each prevents

| Kept item | Concrete failure prevented | Proportional implementation |
|---|---|---|
| npm Trusted Publishing via OIDC | leaked, rotated, over-broad, or mistakenly injected npm token can publish a package | keep current tokenless npm checks; configure npm OIDC and grant `id-token: write` only to `publish-npm` |
| Full-SHA action pins plus comments | a moving tag or compromised action release silently changes CI code | retain existing pins, add version/channel comments, and pin `actions/attest` to v4.1.0 SHA |
| Build/publish separation | build or checksum code can use a Release or npm credential | explicit job permissions, no OIDC/Release write in build/checksum jobs |
| GitHub provenance plus pre-publish verification | an un-attested, substituted, or differently signed tarball reaches npm | attest one checksum-validated tarball in `publish-npm`, then verify its subject, repo, workflow, and workflow digest before `npm publish` |
| Immutable Releases and query-first asset upload | a public Release asset or tag is replaced, deleted, or silently clobbered | administrator enables immutable Releases; preserve draft -> asset -> publish and require exact name/size/digest checks |
| Exact absence checks | outage, auth denial, rate limit, proxy response, or parser failure is mistaken for an absent tag/Release/asset/npm version | explicit result classification; only exact successful not-found responses permit creation |
| Default-branch `workflow_run` split | tag-owned YAML gets writer credentials merely because a tag was pushed | permissionless tag wake plus protected `release.yml` that independently resolves tag/target before every writer |
| Tag rule and workflow execution policy | ordinary repository writers cause release wakes or use an unintended trigger | restrict tag creation to one actor and use active external actor/event policy as defense in depth |
| Protected npm environment | an automated or compromised release run publishes before human deployment review | required reviewer gates the only OIDC-enabled job |

No item above is ceremonial: each blocks a distinct credential, source, asset,
or availability failure. The plan deliberately avoids a new cryptographic
format, database, CLI, or package dependency.

## 6. Explicitly deferred scope and accepted residual risks

### 6.1 Deferred items

| Deferred item | Why it is dropped now | Consequence |
|---|---|---|
| `publisher-authority.mjs`, encrypted vault, Ed25519 key, signed tag envelope, verifier | bespoke cryptography and local state are disproportionate to this release mechanism after the user decision | GitHub authorization and repository policy, not an offline signature, authorize the release path |
| ledger, permanent claims, mutation intents, lanes, reconciliation | requires a durable custom state machine and recovery protocol | no independent cryptographic once-only intent record exists |
| `publish.mjs` inspect/execute graph | duplicates a guarded workflow with a custom publisher | existing workflow remains the one release implementation |
| default-branch `create` trigger | `create` does not turn tag workflow bytes into default-branch bytes | use `workflow_run`, which is documented to load the downstream workflow from default branch |
| Node ownership of Release/assets/checksum/npm | rewriting a large existing, guarded shell workflow would enlarge the risk and scope | retain and tighten the current workflow jobs instead |
| Phase-A installed-version proof | it needs a separately versioned producer and claims beyond a release mechanism | current checksum, post-publish provenance, and install smoke checks remain the evidence |

### 6.2 Residual risks accepted by deferring signing authority

1. A compromise or malicious use of `RELEASE_TAG_ACTOR`, a repository
   administrator, or an approved environment reviewer can initiate a release
   of a protected-main commit. There is no separate offline signing key or
   second cryptographic authorization factor.
2. Native tag restrictions prove platform authorization, not a human's
   version-specific intent. They do not produce a durable signed request,
   nonce, approval payload, or append-only operation ledger.
3. npm Trusted Publishing binds the repository, workflow filename,
   environment, and selected npm action, but not a specific workflow commit.
   Branch protection and the default-branch `workflow_run` source are the
   workflow-byte trust root.
4. Immutable Releases protect tags and assets after public release. They do
   not prevent an authorized actor from creating an unwanted draft or from
   releasing a reviewed but undesired historical main commit before publication.
5. GitHub provenance proves what the GitHub workflow attested to. It cannot
   protect against a compromised GitHub-hosted runner, a malicious reviewed
   main commit, or an approver who deliberately approves the wrong deployment.

These risks are explicit user acceptance for this issue, not gaps to be filled
implicitly by an implementer.

## 7. Tests and acceptance evidence

Do not publish a real version as a test. Use the smallest meaningful checks for
a workflow-only change:

1. On the implementation PR, GitHub must parse both workflow files and all
   existing required PR regression gates must succeed. Run `git diff --check`.
2. Inspect every `uses:` in the two changed workflow files. Assert the exact
   full-SHA/comment rule in Section 3.3, with no floating ref.
3. Review job permissions from rendered YAML. Assert `id-token: write` occurs
   once, in `publish-npm`; `attestations: write` occurs only there; `build` and
   `checksums` have no GitHub Release writer or npm publish capability; and
   the wake workflow is `permissions: {}` with no checkout/action/cache/output.
4. In a non-production sandbox repository with no npm trusted publisher,
   simulate a tag pointing at a non-main commit that changes the wake workflow.
   The default-branch release workflow must fail its guard before source
   checkout, cache, environment, or writer operation.
5. In the same sandbox, simulate a permitted release actor and a tag pointing
   at a protected-main commit. The wake may complete, and the default-branch
   guard must emit the expected tag/target facts; stop the sandbox flow before
   draft creation. Simulate a mismatched target, moved tag, missing tag,
   missing actor variable, unexpected API status, malformed response, and
   registry 401/429/5xx. Every case must stop with no Release or npm mutation.
6. After an administrator enables immutable Releases, use a disposable
   non-production Release to prove draft asset upload followed by one publish
   locks asset replacement/deletion and tag movement. Do not reuse the test tag.
7. In the sandbox publish job, attest a checksum-validated dummy tarball,
   verify it with the exact repository/signer workflow/signer digest command,
   then demonstrate that a different tarball or signer digest fails before a
   simulated npm publish step.
8. Before the first real release, have the administrator record the evidence
   for every prerequisite in Section 4. An absent evidence item is a release
   blocker, not a warning.

## 8. Dependency-cycle and layering gate

The plan adds zero Rust, TypeScript, JavaScript, or product-source module
imports. It removes no module import. The new relation is a GitHub event
scheduling relation only:

```text
.github/workflows/release-tag-wake.yml --workflow_run event-->
.github/workflows/release.yml
```

It is not an ESM, Rust, TypeScript, Tauri, `AppHandle`, or UI transport arc.
No lower layer gains a UI transport dependency. The manual per-arc SCC result
is therefore zero new source-module arcs, zero reverse paths, and no role
inversion.

Implementation acceptance: keep `src-tauri/module-arcs.txt` byte-identical.
If implementation expands scope to add, remove, or move any Rust/TypeScript/
JavaScript module reference, stop and run the Step-N `rust-levelization-run`
pre/post comparison on clean trees. Green then requires unchanged
`cyclicSccs`, identical cyclic-SCC member sets, zero new arcs across a
previously clean SCC boundary, a byte-identical regenerated arc record, and
green structural layering guards. Without that evidence, the change is
`NEEDS_ANOTHER_ROUND`.

## 9. Staged implementation and ownership estimate

One developer can implement this scope, provided a reviewer checks the
workflow security semantics. Use two code phases plus an administrator
activation phase:

1. **Privilege split and permissions.** Add `release-tag-wake.yml`, make
   `release.yml` a default-branch `workflow_run` workflow, implement the
   independent guard/rebinding checks, set explicit permissions, and annotate
   all existing action pins. Land only after PR and sandbox no-writer tests.
2. **Provenance and immutable-release behavior.** Add the pinned native
   attestation/verification steps in `publish-npm`, tighten query-first asset
   and exact-absence behavior while preserving existing guards, and prove the
   negative cases in the sandbox.
3. **Administrator activation.** Configure and independently prove the tag
   ruleset, active workflow-execution policy, protected workflow paths,
   immutable Releases, release actor variable, npm trusted publisher, and
   `release-production` reviewer gate. This is not code implementation and
   must occur before any production tag.

The old custom-authority plan required distinct crypto, Windows local-state,
GitHub, and npm expertise. This native-controls plan is realistic for one
developer in the two code phases above, with mandatory security review and
administrator proof before production use.

## 10. Certification summary

- Scope: one new permissionless wake workflow and a narrow hardening of the
  existing release workflow. No product source, dependency, package metadata,
  or custom authority code changes.
- Trust root: `workflow_run` default-branch source plus protected-main/tag
  validation. Native tag restrictions alone do not make pushed-ref YAML
  trusted, so `create` is not used as a false solution.
- Credentials: only `publish-npm` receives OIDC, attestation write, and the
  protected environment; no npm token is introduced. Build and checksum jobs
  cannot create/update/upload Releases or publish npm.
- Release integrity: enable immutable Releases, retain draft -> assets ->
  public behavior, enforce query-first exact asset identity, and fail closed on
  every ambiguous absence decision.
- Provenance: attest and then verify the exact package tarball before the
  existing tokenless npm publication command.
- Module-cycle gate: zero planned source-module arcs, manual zero-arc SCC
  analysis passed, and no layer or UI transport inversion is introduced.
- Verdict: READY_FOR_IMPLEMENTATION, conditional on the fail-closed
  administrator prerequisites in Section 4 before a production release.

Authoritative references for implementation:

- [GitHub workflow event semantics and `workflow_run`](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows)
- [GitHub workflow execution protections](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/actions-policies/workflow-execution-protections)
- [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers/)
- [GitHub immutable Releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
- [GitHub artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations)
- [`gh attestation verify`](https://cli.github.com/manual/gh_attestation_verify)

Any byte change to this plan after its reported SHA-256 invalidates this
certification and requires a new review round.
