# AgentsCommander 0.30.4 release implementation record

Release issue: https://github.com/mblua/AgentsCommander/issues/1807

The user requested the next patch from remote main and authorized the release changes, PR, merge after all required controls pass, and one correct annotated tag push. This record applies the routine release runbook; it does not claim the historical review phases embedded in the retained V1 format were performed.

Planning base: 9b949f350538cf72763b869e9b503f55eb059739.
Predecessor: v0.30.3.
Candidate evidence: docs/releases/v0.30.4/ (the eight exact files emitted by the canonical public --candidate command after two matching live discovery passes).
SHA256SUMS SHA-256: b28659ec898ee2a9e297aaaf8a32510d2970ec06a4f1141a890d1bc5cc0d4050.

The scope source records 46 merged PRs, including the complete current Unreleased topics. All source at the planning base is included, including the two direct first-parent commits after the predecessor. The root changelog takes the exact generated CHANGELOG.release.md bytes. The candidate asset inventory has the predecessor's 16 names adapted for this version plus agentscommander-0.30.4-windows-x86_64-portable.zip, produced by the existing Windows build.

Run npm run version:bump -- 0.30.4 and npm run version:check to synchronize seven files/eight values. Verify the copied SHA256SUMS, exact deployed evidence guard, packaging dry run and npm pack. Create an issue-backed PR and wait for the current required GitHub checks plus version-sync to pass on its exact head. The documented administrator exception can replace only unavailable self-review and cannot bypass a failed check; no independent human review is claimed.

After merge, verify the resulting remote-main commit, clean checkout, synchronized version files and committed evidence bytes. Independently recheck Git tags, GitHub Releases including drafts, and npm for candidate absence. Extract the committed release-authority-v1.txt as exact LF bytes and use git tag -a --cleanup=verbatim -F to create v0.30.4 on that verified commit. Push the tag once and verify its annotated object and peeled commit remotely.

Publication uses the existing release.yml seven-job chain: guard, release-coordinator, build, checksums, publish-github, verify-release, publish-npm. Preserve the existing npm Trusted Publisher; only publish-npm receives id-token: write, and this deployed workflow has no publishing environment. Completion requires an immutable public GitHub Release with the exact 17 assets and checksums, npm 0.30.4 and latest with matching tarball/provenance, the pipeline's clean installation check, and an independent Windows install/version check. Preserve all historical tags, releases, versions, assets and bundles.
