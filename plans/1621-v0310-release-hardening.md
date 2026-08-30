# Issue #1621: v0.31.0 release hardening, Full delivery

Status: READY_FOR_IMPLEMENTATION

## Purpose and non-authorization

This is the implementation blueprint for GitHub issue #1621, "release: publish
AgentsCommander v0.31.0". It describes the Full delivery path only. It is not
authorization to implement, merge, tag, create a GitHub Release, publish to
npm, deploy an executable, or reuse a failed version.

The implementer must stop at every named human hold. A later consensus pass may
certify the exact bytes of this file, but this draft must not contain a
self-hash, an excluded hash block, or a circular hash protocol.

After all edits to this file stop, a separate out-of-repository certification
record may calculate `Plan-SHA256` over every byte of this complete file,
including its final LF. It must not add a digest or excluded block to this
plan, and any later byte change invalidates the certification. `Plan-SHA256`
is outside the PR 1 evidence, review set, SHA256SUMS, and both archives.

Repository: mblua/AgentsCommander

Issue: #1621, open

Candidate version and tag: 0.31.0 and v0.31.0

Frozen planning base: af7711ad649e99e24baec19321bfb68a553acdd6

Ordered planning-base parents:

~~~text
[6610d25f5657feadf30f1ede86676a2046a45d4a, 496ee8476304cbff28293cce35de1ef6b9c7b4ca]
~~~

Predecessor: v0.30.3, annotated tag object
f8c3b2cba39dbb734b9959352ff592d7c0332539, peeled commit
9ffc24911685906c0222d53eb79898ce9dbdff3e, immutable GitHub Release
377473922.

## 1. Fixed inputs and invariants

### 1.1 Approved WG33 identity

Use only the reviewed WG33 release-preparation delivery with these identities.
Any mismatch is FROZEN_INPUT_CHANGED and requires a new reviewed delivery,
not local repair or a partially reused release.

| Item | SHA-256 |
| --- | --- |
| Skill gzip | 552b1a0a0c6bfba1e54d3f11f0b3a17fe938acff458e62a684865b6066846a3e |
| Skill file manifest | 957b61b5d324f12e81b38b523a3b58586c1099263f42e0908c7a47eff5ae478f |
| Skill generator | 990f97022f0fb1063c88357b232ba105ef6a080e60c854c435c0e679073ed291 |
| Skill test runner | d3e47838c17ff8d9410c6b9ccce55e3ea7b7263d840e7c001bf4b34c72c376d7 |
| Config schema | e79c70cec53f4d21083b0b691475b3d031890bb1c9d6b194894964f10a67bd55 |
| Bundle gzip | 2b9a032ae6635b35161ec7913782d689743866c322e5c9ca07dc0c328a807f95 |
| Bundle file manifest | f93b94c97431af29c28fc149be9f972cd3b50e75fbaa0903882792ee472efa3f |
| Bundle SHA256SUMS | 0fdb079d11fff2bed49960482695965762cdde1263fcc7d5dec65a706558cca5 |
| Review set | 24db335fc7ee35d4b9588fe26692447ae5fe76b41ec84bd3c1b4bf2b5cdbc488 |
| Input manifest | 034d52202c6b81931eb69198f7215bffb579e256216321dffe3c77acf195c8fb |
| Release authority | b81a35fdc0137378b1d93abc1472f8a7aca8f76a073a12aa39af80439464ccac |
| Bundle evidence release plan (`docs/releases/v0.31.0/release-plan.md`) | 04e21b92919a8d44b5ee16978c0d895e9ae7ac7648300f47085eb40f37071338 |

The three source inputs remain pinned as follows:

~~~text
950b7db937194ff70b10bd1dfaa4f4d6648f2be11ffe5ebcd3506c2749768044  config.version.0.31.0.json
e5ef95154150b61884491450719b8da2e3852abf414c289431200e1c2d50f2c7  scope.version.0.31.0.json
66cca8f96002d72fc90a618e2fd211343224b8ada3eddcfbd19cee4f02c3156a  changelog.version.0.31.0.md
~~~

The bundle-evidence row above identifies only the generated
`docs/releases/v0.31.0/release-plan.md` bundle member. It is not a hash of
this canonical plan. No digest input, record, review set, archive, or bundle
may include `plans/1621-v0310-release-hardening.md`.

Verify the gzip digest, gzip integrity, one safe tar root, regular-file
membership, the delivery file manifest, and the internal SHA256SUMS before
using any extracted byte. Stage and extract only in a non-repository temporary
directory. Reject traversal, symlinks, duplicate members, unexpected members,
CRLF-transformed files, or a failed checksum. Do not copy reviewed bytes through
an editor or a newline-converting tool.

### 1.2 The sole public WG33 interface

Promote the verified skill atomically from its verified staging directory into
the approved skill location only after its archive digest, file manifest,
generator digest, test-runner digest, schema digest, and skills-checker result
all match section 1.1. Retain the previous promoted copy until the promoted
copy has passed its checker, then remove only the temporary staging directory.

The only public invocation of the WG33 preparation capability is:

~~~text
node scripts/release.mjs config.version.0.31.0.json
~~~

Run it from the promoted skill root with the reviewed side file. It performs
discover, render, and verify internally. Do not substitute a hand-authored
sequence of git, GitHub CLI, npm, Bash, or PowerShell commands for that
preparation flow. Its output is read-only release evidence and remains
REVIEW_REQUIRED until the independent exact-byte reviews, certification, and
human approval complete.

`scripts/release.mjs` is the public interface of the promoted WG33 skill, not
an existing repository script. It is not a file added by either repository PR.

Run the skill's documented test runner and independent allowlist verifier from
the promoted copy. Require deterministic output for the reviewed inputs and
zero skills-checker errors, warnings, or notes. Never inject fixtures,
credentials, test adapters, or environment-selected offline evidence through
the public command.

### 1.3 Closed hardening boundary

PR 1 is permitted to change exactly these paths, in this order. No wildcard,
path traversal, duplicate, generated extra, test fixture, bundle archive, or
unlisted file is allowed.

~~~text
.github/workflows/release.yml
docs/releases/v0.31.0/CHANGELOG.release.md
docs/releases/v0.31.0/candidate-assets.v1.json
docs/releases/v0.31.0/input-manifest.v1.json
docs/releases/v0.31.0/predecessor-assets.v1.json
docs/releases/v0.31.0/release-authority-v1.txt
docs/releases/v0.31.0/release-body.md
docs/releases/v0.31.0/release-plan.md
docs/releases/v0.31.0/SHA256SUMS
plans/1621-v0310-release-hardening.md
~~~

The final path is derived only from release issue 1621 and candidate 0.31.0.
This plan is outside docs/releases, the review set, SHA256SUMS, and both
archives. The only allowed dependency direction is this plan to the approved
bundle. The bundle must not hash, seal, archive, or otherwise include this
plan.

PR 2 is separate and may change exactly:

~~~text
CHANGELOG.md
package.json
package-lock.json
npm/package.json
npm/install.js
src-tauri/Cargo.toml
Cargo.lock
src-tauri/tauri.conf.json
scripts/smoke-npm-tarball.mjs
~~~

No post-merge mutation of either reviewed evidence tree is allowed. The
candidate asset JSON is a requirements ledger with unknown pre-build digests;
runtime jobs create and verify their separate runtime digest records instead of
rewriting the committed evidence.

### 1.4 Responsibility boundary

| Owner | Required responsibility | Must not do |
| --- | --- | --- |
| WG33 release skill | Read-only discovery, render, and verification of reviewed evidence | Implement, branch, commit, tag, publish, or deploy |
| Hardening PR | Generic tag workflow and exact reviewed evidence bytes | Put candidate-specific logic in the workflow or change unlisted paths |
| Version/evidence PR | Repository version tool, one changelog move, package validation, and the one local npm tarball smoke | Hand-edit version surfaces, add another seam, or modify PR 1 evidence |
| Protected-branch process | Exact-head review, required checks, approved merge | Treat ancestry as an exact topology proof |
| Tag workflow | Guarded build, public immutable GitHub Release, then npm publish | Publish npm before GitHub Release or give OIDC to another job |
| Human operator | Explicit holds and admin exception decision | Bypass a failed, absent, pending, or stale check |

### 1.5 Draft structural note

The planned changes add no Rust or TypeScript module-to-module reference. PR 2
adds one standalone Node harness and a local-source branch inside the existing
npm installer. The harness may use Node standard-library modules only; it must
not import a repository module. The installer seam is co-located and uses only
its existing fs and path dependencies. Thus the planned new project-module arcs
are zero and no lower layer gains a Tauri or UI-transport dependency. The later
certification pass must run the dependency-cycle gate if this boundary changes;
it must compare the base and final src-tauri graph and require unchanged cyclic
SCC member sets, zero cross-boundary new arcs, and a byte-identical module arc
record. This draft does not certify that gate.

## 2. Phase 0: gates before any implementation

1. Start from a clean clone/worktree. Record the tool versions, Git remote URL,
   local HEAD, and authenticated identity without recording credentials. Fail
   if credential material appears in logs, evidence, config, errors, artifacts,
   or issue text.
2. Confirm the issue is still open and retains its identity. Confirm remote
   main with both raw Git and the GitHub ref API equals the frozen base:

   ~~~bash
   git ls-remote origin refs/heads/main
   gh api repos/mblua/AgentsCommander/git/ref/heads/main --jq .object.sha
   ~~~

   Both results must equal af7711ad649e99e24baec19321bfb68a553acdd6.
   Query the GitHub commit API and assert exactly the ordered parent list in
   this plan. Query the contents API and assert the base release workflow blob
   is 1d8a8473b99a2c8e9ee10137daf30aadce7c4d34. A mismatch invalidates the
   delivery and stops work before a branch exists.
3. Verify the whole WG33 delivery using section 1.1. Recompute the
   review-set-v1 recipe from the exact candidate-tree files, including its
   positive vector. The only valid candidate review-set value is
   24db335fc7ee35d4b9588fe26692447ae5fe76b41ec84bd3c1b4bf2b5cdbc488.
   The review-set serialization uses ordered SHA-256 records with two spaces,
   UTF-8 basenames, one LF per record, no prefix, suffix, normalization, or
   length prefix.
4. Confirm the three cold reviews are PASS for this exact SHA256SUMS identity,
   not an earlier delivery identity. Independently compare all listed evidence
   bytes against the approved bundle and require SHA256SUMS verification of all
   seven bundle members it covers.
5. Query the immutable-releases endpoint fail closed before spending the tag:

   ~~~bash
   gh api -H "Accept: application/vnd.github+json" repos/mblua/AgentsCommander/immutable-releases --jq .enabled
   ~~~

   Require an HTTP 200 response whose complete contract has enabled equal to
   true. False, 404, malformed JSON, a missing field, insufficient authority,
   or a transport error is STOP. Repeat this gate in Phase 3 and in the guard
   before useful work on every tag-workflow run and rerun.
6. Obtain an explicit human approval that names v0.31.0, issue #1621, the
   frozen base, review-set digest, and
   0fdb079d11fff2bed49960482695965762cdde1263fcc7d5dec65a706558cca5.
   Until then, do not begin PR 1.

## 3. Phase 1: hardening PR from the frozen base

Create an issue-linked branch from exactly the frozen base. Its PR description
must link #1621 and explain that it is the generic release-hardening PR.

1. Import the eight release evidence files byte-for-byte from the verified
   bundle into docs/releases/v0.31.0. Add this canonical plan as the tenth
   allowed path. Because the existing `.gitignore:11:/plans/` rule ignores it,
   first confirm that exact ignore rule and stage only this file:

   ~~~bash
   git check-ignore -v -- plans/1621-v0310-release-hardening.md
   git add -f -- plans/1621-v0310-release-hardening.md
   ~~~

   Do not force-add a directory, tree, glob, or another path, and do not change
   `.gitignore`.
   Before committing, assert the staged path set is exactly the ten-path
   allowlist in section 1.3. Do not regenerate or format the imported files.
2. Implement .github/workflows/release.yml as a generic normal-SemVer
   annotated-tag workflow. The workflow must not embed v0.31.0, issue #1621,
   base commit, or candidate asset names. It reads candidate-specific authority
   only from docs/releases/vTAG after a tag is guarded.
3. Each workflow job must check out the pushed tag with complete history before
   useful work. Its bootstrap must fail closed unless:

   ~~~bash
   set -euo pipefail
   test "$GITHUB_REF_TYPE" = tag
   TAG="$GITHUB_REF_NAME"
   printf '%s' "$TAG" | grep -E '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
   HEAD_COMMIT="$(git rev-parse 'HEAD^{commit}')"
   PEELED_COMMIT="$(git rev-parse "$TAG^{}")"
   TAG_OBJECT="$(git rev-parse "$TAG^{tag}")"
   test "$GITHUB_SHA" = "$HEAD_COMMIT"
   test "$HEAD_COMMIT" = "$PEELED_COMMIT"
   test "$TAG_OBJECT" != "$PEELED_COMMIT"
   mapfile -t REMOTE_TAG < <(git ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")
   ~~~

   Exactly two matching remote records are required. The guard must assert both
   records equal the locally resolved annotated tag object and peeled commit,
   export that exact pair, and every later job and rerun must re-query and
   assert the same pair before useful work. Printing identities is insufficient.
4. Implement a strict release-authority-v1 parser. It rejects missing,
   reordered, duplicate, unknown, or malformed fields. It recomputes the
   candidate review set from candidate-tree bytes and checks every
   base/predecessor/source/policy value against the manifest. A lightweight
   tag, a missing tag record, a second record, an object change on the same
   peeled commit, or a changed peeled commit fails.
5. Keep workflow order exactly:

   ~~~text
   guard -> platform producers -> checksums -> publish-github -> publish-npm
   ~~~

   Build each macOS architecture only once. Its raw binary, application archive,
   and installer must derive from that one build and its recorded digest
   manifest. Derive the predecessor by strict normal-SemVer comparison, then
   verify its annotated object, peeled commit, public immutable Release, and
   16-asset ledger. Do not use mutable latest as predecessor authority.
6. Enforce the permissions split:

   - build, checksums, and publish-github may receive only job-local
     contents: write where direct asset/release mutation requires it, and no
     id-token;
   - only publish-npm receives id-token: write and contents: read;
   - only the post-publication release-verification job may receive
     attestations: read and an explicit GitHub token;
   - no producer receives attestations: write or runs actions/attest;
   - no other job receives OIDC.

   Before any GitHub CLI attestation check, download only
   gh_2.86.0_linux_amd64.tar.gz, verify SHA-256
   f3b08bd6a28420cc2229b0a1a687fa25f2b838d3f04b297414c1041ca68103c7,
   install it, and require gh version 2.86.0.
7. Require Node 22 and exact npm 11.6.2 in the npm job. Configure only
   https://registry.npmjs.org. Fail if NODE_AUTH_TOKEN, NPM_TOKEN, or any
   legacy package token is set. Publish with provenance only after the GitHub
   Release gate in section 6 passes.
8. Verify the exact changed-path set, line endings, no secret leaks, bundle
   digests, review-set vector, authority parser rejection cases, guard
   semantics, action pins, permission partition, fixed GitHub CLI archive,
   asset-ledger constraints, and GitHub-before-npm ordering. Use the promoted
   WG33 regression suite and independent verifier as the release-contract
   checks, plus all repository checks required by current protected-main
   policy. Do not add an unlisted test file just to make the boundary look
   tested.
9. Freeze HARDENING_HEAD only after the exact PR commit is independently
   reviewed and all required checks are PASS, present, and fresh for that
   commit. A later push, stale check, or changed path invalidates the review.
10. Merge through protected main as a two-parent merge commit only. Set
    HARDENING_MERGE_SHA only after asserting:

    ~~~text
    parents(HARDENING_MERGE_SHA) =
    [af7711ad649e99e24baec19321bfb68a553acdd6, HARDENING_HEAD]
    ~~~

    Reject squash, rebase, reversed parents, extra parents, and intervening
    main commits. If a second eligible human reviewer exists, use normal review.
    If not, mblua may use the documented admin self-review exception only after
    every exact-head check is PASS. Admin authority never overrides a failed,
    absent, pending, or stale check.

## 4. Phase 2: version and evidence PR from HARDENING_MERGE_SHA

Create a new issue-linked branch from exactly HARDENING_MERGE_SHA. Do not carry
unreviewed changes forward from another branch.

1. Move the exact approved Unreleased changelog body once: remove it from
   Unreleased and insert it under 0.31.0. The result must byte-match
   CHANGELOG.release.md. Reject a residual or duplicate copy. Preserve the
   Room and legacy wording, aliases, side effects, exit codes, and output
   described by the approved changelog.
2. Invoke the repository's existing version tool, not hand edits:

   ~~~text
   node scripts/bump-version.mjs 0.31.0
   ~~~

   Its existing two-phase behavior resolves the explicit target and uses the
   root `agentscommander` package entry as the version anchor. It must validate
   every declared patch anchor against its running in-memory buffer before any
   write, then synchronize all targets. A missing package anchor or patch
   anchor fails closed. The tool owns the patch list: do not reproduce it with
   manual edits or an ad hoc script. Parse and assert all eight surfaces are
   exactly 0.31.0: root package version; root lockfile version and root package
   entry; npm package version; npm installer constant; Cargo package version;
   Cargo lock package version; and Tauri configuration version.
3. Verify this PR changes only the nine paths in section 1.3. Require every
   imported PR 1 evidence byte, including its SHA256SUMS, to remain
   byte-identical.
4. In the publish package directory, run npm pack --dry-run, create the exact
   tarball, and inspect its file list. TARBALL is the resulting absolute regular
   .tgz path. CANDIDATE_RELEASE_ASSET is the exact current-platform release
   asset produced by the repository's existing required packaging path. It is
   an absolute regular file, not a raw candidate CLI, a second build, or a
   manually assembled fixture. Its basename must be the platform-derived
   assetName: agentscommander-mac-{aarch64|x86_64}.app.tar.gz on Darwin,
   agentscommander-windows-{aarch64|x86_64}.exe on Windows, or
   agentscommander-linux-{aarch64|x86_64} on Linux. In particular, the Darwin
   input is the real .app.tar.gz release archive, not an executable extracted
   from it.

   The only permitted pre-publication installer seam is the exact environment
   variable AGENTSCOMMANDER_TEST_RELEASE_DIR. The change in npm/install.js is
   limited to this contract:

   - when the variable is absent, retain the two current public HTTPS URLs
     exactly: the vVERSION SHASUMS256.txt URL and the platform-derived release
     asset URL. No production URL, owner, repository, version, asset selection,
     redirect, or checksum behavior changes;
   - when it is present, it must be a nonempty absolute real local directory.
     It must contain direct regular, non-symlink files named SHASUMS256.txt and
     the existing platform-derived assetName. Resolve each real path and reject
     any path that escapes the supplied directory;
   - copy those two local sources into the existing temporary destinations and
     execute the existing verifyChecksum path unchanged. The local source is
     never a fallback to network;
   - an empty, relative, unreadable, non-directory, symlinked, out-of-root,
     missing-asset, missing-checksum-file, absent-record, or wrong-digest
     fixture exits nonzero and leaves no installed final binary. No other
     environment variable may select a test source.

   Add only scripts/smoke-npm-tarball.mjs to build the local fixture and run all
   smoke cases. Its sole invocation is:

   ~~~bash
   node scripts/smoke-npm-tarball.mjs --tarball "$TARBALL" --release-asset "$CANDIDATE_RELEASE_ASSET" --version 0.31.0
   ~~~

   All three flags are required and must resolve to absolute regular inputs
   except the canonical SemVer version; unknown flags, a version mismatch, an
   unsupported platform or architecture, or a release-asset basename that does
   not equal the installer-selected assetName fail. The harness derives that
   assetName with the same current platform/architecture selection as
   npm/install.js. For each case it creates a fresh temporary release directory,
   copies the exact CANDIDATE_RELEASE_ASSET bytes to assetName, computes the
   SHA-256 of those copied bytes, and writes the matching two-space SHA-256
   record for assetName into SHASUMS256.txt. The fixture never creates or
   substitutes an executable.

   For each positive, absent-record, and wrong-digest case, the harness creates
   a fresh temporary npm project, a fresh empty regular npmrc, and an ephemeral
   CommonJS network-deny preload inside that case's temporary tree. It runs
   exactly this lifecycle-enabled command from the temporary npm project:

   ~~~bash
   npm install --offline --no-audit --no-fund --ignore-scripts=false --package-lock=false --userconfig "$EMPTY_TEMP_NPMRC" "$TARBALL"
   ~~~

   The command receives AGENTSCOMMANDER_TEST_RELEASE_DIR only for that child,
   pointing at its local fixture. The harness must set NODE_OPTIONS for that npm
   child and every inherited lifecycle child exactly to
   --require=<TEMP_NETWORK_DENY_PRELOAD>, not append to an inherited value.
   --ignore-scripts is forbidden.

   The ephemeral preload must synchronously append a structured diagnostic with
   at least a network-deny type and the intercepted surface, then synchronously
   throw, for every call to http.request, http.get, https.request, https.get,
   net.connect, net.createConnection, net.Socket.prototype.connect, or
   tls.connect. It must do the same for both the callback and Promise forms of
   every DNS resolver surface: lookup, lookupService, resolve, resolve4,
   resolve6, resolveAny, resolveCaa, resolveCname, resolveMx, resolveNaptr,
   resolveNs, resolvePtr, resolveSoa, resolveSrv, resolveTxt, and reverse. The
   preload and its diagnostic remain only in the harness temporary tree. During
   the three normal smoke cases, any network-deny diagnostic or thrown
   network-deny error is STOP, whether it originates in npm, a lifecycle child,
   or the installer.

   The positive case passes only when the existing verifyChecksum path accepts
   the fixture's assetName record and npm install exits zero. On Darwin, it must
   first establish that the copied .app.tar.gz archive digest equals both the
   fixture digest and the selected SHASUMS256.txt record, let the normal
   installer extract that archive, and then invoke the installed CLI to require
   version 0.31.0. It must never compare an archive digest to an extracted CLI
   digest. On Windows and Linux, it must additionally establish that the raw
   release-asset bytes equal the installed final binary bytes, the selected
   checksum record equals that digest, and the installed CLI version command
   reports 0.31.0.

   In the same harness invocation, create fresh fixtures for two negative
   cases: SHASUMS256.txt without the selected asset record, and SHASUMS256.txt
   with a wrong digest for that record. Each uses the exact offline command and
   network-deny preload above, must exit nonzero through the existing
   verifyChecksum path, and must leave no installed final binary. The harness
   exits zero only after the positive case and both expected checksum failures;
   any unexpected success, failed positive case, network-deny event, or cleanup
   failure is STOP. No agent manually assembles a fixture.

   The later post-publication clean install must exercise the normal installer,
   which downloads the public SHASUMS256.txt and selected asset, verifies the
   checksum, and fails nonzero on invalid input. Its evidence must name the
   selected uploaded asset and record both its observed SHA-256 and matching
   SHASUMS256.txt checksum record. A bare successful install is not proof of
   the installer contract.
5. Snapshot the current required PR checks and review policy. Run all
   applicable repository checks, the eight-version-surface assertion,
   changelog identity assertion, npm pack/tarball/harness gates, and exact
   nine-path changed-path check. Freeze VERSION_HEAD only after independent exact-head
   review and every required check is PASS, present, and fresh.
6. Merge through protected main as a two-parent merge commit only. Assert:

   ~~~text
   parents(VERSION_MERGE_SHA) = [HARDENING_MERGE_SHA, VERSION_HEAD]
   ~~~

   Set FINAL_CANDIDATE_MAIN to VERSION_MERGE_SHA. An ancestry-only proof is not
   sufficient. Apply the same tightly constrained documented admin exception
   only if no second eligible reviewer exists.

## 5. Phase 3: final pre-tag revalidation

Immediately before tag creation, rerun all checks against fresh remote and API
state. Any failure spends no version and stops before tag creation.

1. Repeat the immutable-releases GET from Phase 0 and require its complete
   enabled value to equal true. A false, 404, malformed, missing, or inaccessible
   response is STOP before tag authorization.
2. Require both remote main and the GitHub default-branch ref to equal
   FINAL_CANDIDATE_MAIN, not the frozen planning base.
3. Recompute both exact ordered merge-parent vectors from sections 3 and 4.
   Reject an intervening main commit, different merge method, or changed parent
   order.
4. Read the final candidate tree and reverify:

   - every reviewed bundle file and its hash;
   - SHA256SUMS hash 0fdb079d11fff2bed49960482695965762cdde1263fcc7d5dec65a706558cca5;
   - review-set hash 24db335fc7ee35d4b9588fe26692447ae5fe76b41ec84bd3c1b4bf2b5cdbc488;
   - release-authority hash b81a35fdc0137378b1d93abc1472f8a7aca8f76a073a12aa39af80439464ccac;
   - generic workflow parser, pinned actions, fixed GitHub CLI, permission,
     attestation, Node, npm, package, version, changelog, and asset gates.

5. Before a tag exists only, prove unambiguous candidate absence in all three
   systems: remote Git tag, GitHub Release, and official npm registry. Treat an
   ambiguous API or registry answer as failure, not absence.
6. Confirm the predecessor authority and its 16 unique nonempty immutable
   assets again. Confirm the candidate ledger defines exactly the following 17
   required asset names and no additional asset:

   ~~~text
   Agents.Commander-0.31.0-1.x86_64.rpm
   Agents.Commander_0.31.0_aarch64.dmg
   Agents.Commander_0.31.0_amd64.AppImage
   Agents.Commander_0.31.0_amd64.deb
   Agents.Commander_0.31.0_x64-setup.exe
   Agents.Commander_0.31.0_x64.dmg
   Agents.Commander_aarch64.app.tar.gz
   Agents.Commander_x64.app.tar.gz
   SHASUMS256.txt
   agentscommander-0.31.0-windows-x86_64-portable.zip
   agentscommander-linux-x86_64
   agentscommander-mac-aarch64
   agentscommander-mac-aarch64.app.tar.gz
   agentscommander-mac-x86_64
   agentscommander-mac-x86_64.app.tar.gz
   agentscommander-testeable-windows-x86_64.exe
   agentscommander-windows-x86_64.exe
   ~~~

7. Confirm the protected-main checks from both reviewed PR heads remain
   recorded, the authority still describes the available reviewer policy, and
   no unmodeled bypass was used.

### Human hold A: pre-publication tag window

The immutable-releases endpoint must remain enabled before tag creation and
before every workflow run or rerun. There is a residual window after a SemVer
tag is pushed but before its exact draft is published: no independent SemVer
tag ruleset prevents an administrator from moving that tag in that interval.
Obtain a human decision that explicitly accepts only this pre-publication
window. A public release with immutable equal to true locks its tag and assets,
so it removes that post-publication movement risk.

If the endpoint changes, the object/peeled pair changes, the draft no longer
matches, or publication does not result in immutable true, STOP. After npm
publication, never reuse the version; cut a new version rather than repairing
or replacing the published one.

### Human hold B: unavailable second human review

There is no second eligible human reviewer. Obtain explicit approval for the
documented mblua admin self-review exception only after every exact-head check
is PASS. This approval cannot waive a failed, missing, pending, stale, or
unrelated check.

### Separate human authorization C: one tag authorization

Request a separate explicit approval that names v0.31.0,
FINAL_CANDIDATE_MAIN, the release-authority digest, the review-set digest, and
the fact that GitHub Release publication precedes npm. Without this approval,
do not create or push a tag. This authorization is distinct from human holds A
and B, and it cannot waive either hold or an ordinary failed gate.

## 6. Phase 4: annotated tag and guarded publication

1. Create one annotated tag at FINAL_CANDIDATE_MAIN using the exact bytes of
   docs/releases/v0.31.0/release-authority-v1.txt:

   ~~~bash
   git tag -a v0.31.0 "$FINAL_CANDIDATE_MAIN" \
     -F docs/releases/v0.31.0/release-authority-v1.txt
   ~~~

   Before push, require object type tag, a tag message payload byte-identical
   to the authority file, and a peeled commit equal to FINAL_CANDIDATE_MAIN.
   Reject a lightweight tag, any extra payload byte, or a different target.
2. Push only that tag. Resolve the remote records with:

   ~~~bash
   git ls-remote origin refs/tags/v0.31.0 refs/tags/v0.31.0^{}
   ~~~

   Assert exactly two records. Assert the first is the local annotated object
   and the second is its peeled FINAL_CANDIDATE_MAIN. Repeat this comparison
   after workflow completion and in every workflow rerun. A moved object on
   the same commit is still a failure.
3. The workflow guard first repeats the immutable-releases GET and requires
   enabled equal to true, then exports the resolved object and peeled pair.
   Every job and rerun re-resolves local and remote values, compares both to
   guard outputs, and stops before useful work on any mismatch. After tag
   creation, absence is a failure, never a reason to recreate the tag.
4. Platform producer jobs build the approved artifacts and emit runtime
   SHA-256 records. The checksums job verifies nonempty files, the exact
   17-name set, no duplicates, the SHA256 asset, and a matching runtime ledger.
   It rejects missing, extra, zero-size, path-traversing, name-mismatched, or
   digest-mismatched artifacts.
5. publish-github creates or reuses only the exact guarded draft for this tag.
   Before publishing that draft, repeat the immutable-releases GET, assert the
   exact guarded object/peeled pair and authority, and download every one of
   the 17 draft assets. Verify only their exact names, nonzero sizes, runtime
   SHA-256 records, and draft identity at this point. Do not run release
   attestation verification, gh release verify, or gh release verify-asset
   while the Release is a draft.

   Publish the draft only after those checks pass. Immediately require the
   release API field immutable to equal true:

   ~~~bash
   gh api "repos/mblua/AgentsCommander/releases/tags/$TAG" --jq .immutable
   ~~~

   With the previously verified gh 2.86.0, run:

   ~~~bash
   gh release verify "$TAG" --repo mblua/AgentsCommander
   gh release verify-asset "$TAG" "$LOCAL_ASSET" --repo mblua/AgentsCommander
   ~~~

   Run verify-asset once for each of the 17 local files downloaded from the
   draft and now exposed by the immutable public Release. Revalidate the
   object/peeled pair after every verification set. The release is accepted
   only if draft is false, immutable is true, the pair remains unchanged, all
   17 assets remain present, and every release and asset verification passes.
   Never delete, replace, recreate, or use clobber to hide a mismatch.
6. publish-npm starts only after every post-publication immutable-release,
   release-verification, asset-verification, and object/peeled assertion passes.
   Under Node 22 and npm 11.6.2, use the official registry, publish the
   package with provenance, and verify the returned package name/version,
   dist-tag, integrity, provenance/signature evidence, and registry tarball
   against the publication inputs. Run a clean install from the official
   registry and prove its installed CLI reports 0.31.0. This full install must
   exercise the normal installer and its downloaded asset checksum validation.
   Record the selected public Release asset name, its observed SHA-256, and
   the exact matching `SHASUMS256.txt` checksum record with that clean-install
   evidence. Do not publish again in a rerun if the registry already contains
   0.31.0.

## 7. Phase 5: Windows destination deployment

Only after the tag, workflow, public GitHub Release, npm publication,
provenance, registry clean install, and final object/peeled recheck are all
green:

1. Download the verified release asset agentscommander-windows-x86_64.exe
   through the immutable public Release, not an untrusted cache.
2. Verify its SHA-256 against the release runtime checksum and verify its
   attestation binds it to FINAL_CANDIDATE_MAIN. Execute its version command
   and require 0.31.0.
3. Place it at:

   ~~~text
   C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-23.exe
   ~~~

   Use a same-volume temporary filename followed by an atomic replacement.
   Preserve a recoverable, timestamped prior executable only until the new
   file's SHA-256, version output, and source-commit attestation have been
   independently rechecked. Never copy an unverified file over the destination.
4. Record the release URL, release asset URL, source object and peeled commit,
   asset SHA-256, final destination SHA-256, and version output in the release
   evidence. Remove only temporary download/staging files and credential
   material, never the Release, tag, or published package.

## 8. Completion, failure, and recovery

Completion requires all of the following:

- both PR merge topology assertions;
- public immutable GitHub Release with exactly 17 verified assets and
  attestations;
- verified npm version, dist-tag, integrity, provenance/signature, and clean
  install;
- repeated tag annotated-object and peeled-commit verification;
- deployed executable at the required Windows path with matching digest,
  source-commit attestation, and 0.31.0 version output;
- WG23 independent repetition of GitHub, npm, and executable verification;
- issue #1621 closure only after the preceding evidence is complete.

Failure before public GitHub Release may resume only if the exact tag object,
peeled commit, authority payload, FINAL_CANDIDATE_MAIN, draft id, and asset
digests all match. Failure after a public immutable GitHub Release may resume
only the npm path using that exact verified Release and package tarball.

If npm already contains 0.31.0, require exact repository, tag, provenance, and
tarball identity. Otherwise stop for a new version. Never reuse, move, delete,
replace, or republish a published version.

The implementation reviewer must demonstrate negative failures for at least:
changed or extra allowlist path; canonical plan included in evidence or hashes;
wrong review-set byte/order/separator/terminator; changed frozen base; incorrect
merge topology; lightweight, absent, duplicate, or moved tag; tag absence after
push; immutable-releases false, 404, malformed, or changed-before-publication;
an attempt to verify a draft attestation; an immutable-release or per-asset
verification failure after publication; weak uploader permissions or excess
OIDC; a producer attestation path; public-release-after-npm order; token
presence; mutable GitHub CLI; package/version/changelog drift; an absent,
invalid, out-of-root, missing-record, or wrong-digest local npm fixture;
lifecycle suppression; duplicate, missing, extra, zero-size, or
digest-mismatched assets; candidate collisions; review-authority failure;
credential leakage; any external-network attempt during the local-tarball
smoke, which must produce the temporary preload's structured network-deny
diagnostic and synchronous error; and destination executable
digest/version/source-commit mismatch.
