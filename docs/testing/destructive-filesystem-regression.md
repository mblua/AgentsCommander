# Destructive Filesystem Regression Checks

These checks cover Windows filesystem behaviors that can be unavailable in some CI runners because long paths or junction creation may be disabled by host policy.

## Reset Long Path Check

Run `cargo test --test cli_test_reset long_path_target_deletes_only_allowed_directories -- --nocapture`.

If the test prints a skip message, run the same command on a Windows host with long paths enabled. Confirm the JSON plan contains exactly the two testable reset directories and that unrelated sibling directories remain.

## Reset Junction Reparse Check

Run `cargo test --test cli_test_reset junction_target_refuses_and_deletes_nothing -- --nocapture`.

If the test prints a skip message, create a directory junction with `mklink /J`, then rerun the test on a Windows host where developer mode or elevated junction creation is available. Confirm the reset command reports `reset_candidate_is_reparse_point` and leaves both the junction and target intact.

## Workgroup Long Path Check

Run `cargo test --test cli_workgroup_team workgroup_remove_deletes_long_path_tree -- --nocapture`.

If the test prints a skip message, rerun it on a Windows host with long paths enabled. Confirm `workgroup remove --force-dirty` deletes the workgroup root and does not delete unrelated sibling paths.

## Workgroup Reparse Root Check

Run `cargo test --test cli_workgroup_team workgroup_remove_refuses_reparse_root -- --nocapture`.

If the test prints a skip message, create a workgroup root as a junction with `mklink /J`, then rerun the test on a Windows host where junction creation is available. Confirm deletion is refused before canonicalization and the junction target remains intact.
