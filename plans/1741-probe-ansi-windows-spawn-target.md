# #1741 - replace the Windows spawn target of the ANSI-skip probe test with cmd.exe

Status: READY_FOR_IMPLEMENTATION

Issue: `#1741` - "ci(rust-regression): the Windows gate fails reproducibly on main, blocking every PR"
Repository: `repo-AgentsCommander`
Target: `main` pinned at `49808be06cb66afae377e6c9e13f3a7472480afa`
Branch: `fix/1741-probe-ansi-windows-spawn-target` (already created off `49808be0`, working tree clean)
Delivery: Lite (one test arm, inside `#[cfg(test)]`, no production byte changes)

## Objective

Make `agent_version::tests::probe_version_skips_an_ansi_only_first_line` measure the
behaviour it names, ANSI-only-first-line skipping, instead of Windows process startup
latency, so the required `rust-regression` check stops failing on `main` and unblocking
every pull request stops depending on a rerun.

## Cause

`src-tauri/src/agent_version.rs:1401-1418` spawns a real process and waits for a version
string against `PROBE_TIMEOUT` (`agent_version.rs:21`, `Duration::from_secs(15)`). On
Windows it spawns `powershell.exe -NoProfile -NonInteractive -Command "[char]27 + '[?25l'; '2.0.1'"`.

The observed CI failure is a timeout, not a behavioural assertion:

```
thread 'agent_version::tests::probe_version_skips_an_ansi_only_first_line' panicked at src-tauri\src\agent_version.rs:1417:9:
assertion `left == right` failed
  left: Failed("timed out after 15s (killed)")
```

The issue records two independent CI failures against two passes on a byte-identical
`src-tauri` tree (tree OID `8fe4ee40313805f1fa40001d2f687a34ae7f1f76` at every commit in
its table), so nothing in the code under test differs between a pass and a fail. The
only job that executes this test is `rust-regression` on `windows-latest`
(`.github/workflows/pr-regression-gates.yml:90-92`, `cargo test --lib --bins --tests`). The
claim is narrow on purpose, because other jobs do run `cargo test`: `rust-regression-linux`
runs one named test at `:154-161`
(`cargo test --lib "$TEST" -- --exact ...` for
`tests::issue_1577_linux_unmarked_adjacent_unwritable_falls_back_to_home`, so `--exact` on a
different name never selects ours), and `terminal-snapshot-portable` runs `cargo test --locked`
at `:292` and `:295` against other packages. What the Linux and macOS legs do to *this* file is
type-check it: `cargo check --all-targets` and `cargo clippy` at `:133-139` (linux) and
`:242-248` (macos).

The "two twins are one observation" reading is not this plan's invention: it is independently
recorded from the 2026-09-03 PR #1740 runs, where both twins failed this test on the 15 second
PowerShell-spawn timeout and only the rerun 30 minutes later counted as a second sample. The
twins are also not the same commit (the push job checks out the branch tip, the pull_request
job a synthetic merge commit), which is why the identity argument has to run on tree OIDs, as
the issue does, and not on commit SHAs.

What the Windows arm charges against the 15 second budget is a PowerShell cold start:
CLR load plus engine initialization plus, on a GitHub runner, Defender scanning both. On
this machine, warm, measured through `std::process::Command` (the same builder the test
uses), 7 runs each:

| target | stdout | elapsed |
| --- | --- | --- |
| `powershell.exe -NoProfile -NonInteractive -Command "[char]27 + '[?25l'; '2.0.1'"` | `[27, 91, 63, 50, 53, 108, 13, 10, 50, 46, 48, 46, 49, 13, 10]` | 433, 483, 505, 536, 551, 627, 691 ms |
| `cmd.exe /C "echo <ESC>[?25l& echo 2.0.1"` | `[27, 91, 63, 50, 53, 108, 13, 10, 50, 46, 48, 46, 49, 13, 10]` | 33, 44, 47, 48, 49, 49, 62 ms |

The two targets emit byte-identical stdout: `ESC [ ? 2 5 l CRLF 2 . 0 . 1 CRLF`, an
ANSI-only first line then the version. The cheaper one costs about a tenth as much warm,
and far less than that cold, because it loads no CLR.

### This is not one of the known `rust-regression` flakes

The recorded flake families in this job are short deadline budgets losing a scheduling
race on a loaded runner: `cli::task_ops` `concurrent_writes_return_correct_post_edit_content`
(#1579), the `session::selection` timing family (#1248, #1234, #1393, #1176, #1582), the
`api::dispatcher` 1-second timeout (#1600), and the Windows loopback WebSocket teardown
(#1710). `probe_version_skips_an_ansi_only_first_line` is in none of them and is not in the
#1241 inventory. Its budget is 15 seconds, not 200 milliseconds, and the thing that eats it
is a fixed startup cost the test never intended to measure. That is why the disposition is
to remove the cost, not to raise the budget or file it as flaky.

## Scope

In scope, exactly one hunk:

- `src-tauri/src/agent_version.rs`, the Windows arm of
  `probe_version_skips_an_ansi_only_first_line` (lines 1404-1412 today).

Out of scope, and not to be touched:

- `PROBE_TIMEOUT` (`agent_version.rs:21`). See "PROBE_TIMEOUT is unchanged" below.
- The Unix arm of the same test (`agent_version.rs:1414`).
- Every other test in the file, and every production path.
- `.github/workflows/**`. No CI configuration change is part of this fix.
- The #1241 flaky inventory. Nothing is added to it.

## Decided solution

Replace the multi-line `powershell.exe` tuple arm with a single-line `cmd.exe` arm, keeping
the surrounding `let (program, args): (&str, Vec<&str>)` shape, the Unix arm, the
`probe_version(...)` call and the assertion byte-identical.

Remove `src-tauri/src/agent_version.rs:1404-1412`:

```rust
            (
                "powershell.exe",
                vec![
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[char]27 + '[?25l'; '2.0.1'",
                ],
            )
```

Insert in its place, one line, at the same indentation:

```rust
            ("cmd.exe", vec!["/C", "echo \x1b[?25l& echo 2.0.1"])
```

The resulting test in full:

```rust
    #[tokio::test]
    async fn probe_version_skips_an_ansi_only_first_line() {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
            ("cmd.exe", vec!["/C", "echo \x1b[?25l& echo 2.0.1"])
        } else {
            ("sh", vec!["-c", "printf '\\033[?25l\\n2.0.1\\n'"])
        };
        let outcome = probe_version(Path::new(program), &args, PROBE_TIMEOUT).await;
        assert_eq!(outcome, ProbeOutcome::Version("2.0.1".to_string()));
    }
```

Net effect on the file: 18 lines become 10, so the file goes from 1839 to 1831 lines and
every line after 1412 shifts up by 8.

Three properties make this the smallest safe change:

1. The `/C` plus `echo A& echo B` argument shape is already used by the neighbouring
   `probe_version_skips_leading_blank_lines` (`agent_version.rs:1390-1399`), which has never
   flaked, so Rust's argv quoting into `cmd.exe` is proven in this same file.
2. The literal tuple arm with a hardcoded `"cmd.exe"` is already used by
   `probe_version_timeout_kills_tree` (`agent_version.rs:1455-1456`), so the shape needs no
   new idiom and `shell_program()` does not have to be pulled into this arm.
3. `\x1b` is an ASCII escape in a Rust string literal, so the source stays plain ASCII and
   the ESC byte reaches `cmd.exe` intact. Verified: the argv byte sequence Rust builds is
   `echo <0x1B>[?25l& echo 2.0.1`, and `cmd.exe` emits the 15 bytes listed above.

### Decision: no per-test `Duration` is added

`PROBE_TIMEOUT` stays at this call site. The belt-and-braces alternative, passing a generous
`Duration` at `agent_version.rs:1416` only, is rejected.

Reason: four sibling tests in this module already spawn `cmd.exe` under `PROBE_TIMEOUT` on
this same Windows runner (`agent_version.rs:1386`, `:1397`, `:1427`, `:1443`) and none of
them has ever flaked. So `cmd.exe` plus `PROBE_TIMEOUT` is not a hope, it is the empirically
proven configuration on the exact machine that fails. Giving this one call site a bespoke
timeout would make it the only asymmetric probe test in the file, would hide a future
regression in `cmd.exe` spawn cost that the four siblings would still catch, and would add a
second variable to a fix whose entire purpose is to take startup latency out of the
measurement. The one call site in the module that does pass an explicit `Duration`
(`agent_version.rs:1461`, `Duration::from_millis(200)`) does so because it is a timeout test,
which is the opposite case.

### The Unix arm is not touched

`("sh", vec!["-c", "printf '\\033[?25l\\n2.0.1\\n'"])` at `agent_version.rs:1414` is left
byte-identical, and so is the `let (program, args)` binding that wraps it. This is deliberate
rather than incidental: no CI leg executes this arm, `cargo check --all-targets` on the Linux
and macOS legs only type-checks it, and this Windows development machine cannot execute it
either. An arm nothing can run is safest when nothing changes it, so the implementation must
not refactor it to `shell_program()` for symmetry.

### `PROBE_TIMEOUT` is unchanged

The constant keeps `Duration::from_secs(15)`. It has nine references in the tree:

| Reference | Kind |
| --- | --- |
| `src-tauri/src/agent_version.rs:21` | definition |
| `src-tauri/src/agent_update.rs:42` | `use` import |
| `src-tauri/src/agent_update.rs:1580` | **production** call site |
| `src-tauri/src/agent_update.rs:1613` | **production** call site |
| `src-tauri/src/agent_version.rs:1386` | test `probe_version_parses_echoed_version` |
| `src-tauri/src/agent_version.rs:1397` | test `probe_version_skips_leading_blank_lines` |
| `src-tauri/src/agent_version.rs:1416` | test `probe_version_skips_an_ansi_only_first_line` |
| `src-tauri/src/agent_version.rs:1427` | test `probe_version_nonzero_exit_fails_with_code` |
| `src-tauri/src/agent_version.rs:1443` | test `probe_version_no_token_fails` |

There is also one dependent outside the source tree, in a plan rather than in code:
`plans/1551-agent-update-status.md:1192` (case T6) builds a `codex.cmd` stub running
`ping -n 11 127.0.0.1`, described there as "about 10 s of delay, always below `PROBE_TIMEOUT` =
15 s", so the fixture's `Pendiente` observation depends on the cap staying above that stall.
Lowering the constant would silently invalidate that manual case. Recorded so nobody concludes
the constant has only in-repo dependents.

Two of those are production, so touching the constant would breach the "no production code
path may change" constraint outright, independently of the test-weakening argument. Issue
acceptance criterion 2 is therefore satisfied vacuously: the constant does not change, so no
test that reads it is re-examined or weakened.

## Required behaviour, edge cases and failure behaviour

- With the change, on Windows, `probe_version` receives stdout
  `[27, 91, 63, 50, 53, 108, 13, 10, 50, 46, 48, 46, 49, 13, 10]` and exit code 0.
  `text_lines` (`agent_version.rs:206-210`) splits on both `\n` and `\r`, sanitizes each
  piece with `strip_ansi_and_controls` (`agent_version.rs:153-202`), and drops the pieces
  that sanitize to empty. The ANSI-only first piece sanitizes to `""` and is dropped, so
  `first_text_line` returns `Some("2.0.1")` and `parse_version_token` yields `"2.0.1"`.
  `parse_probe_completion` (`agent_version.rs:903-949`) returns
  `ProbeOutcome::Version("2.0.1")`, and the assertion holds. **Line numbers after the hunk
  shift up by eight**, so that assertion sits at `agent_version.rs:1409` in the post-fix file,
  not at `:1417` where it sits today; every other citation in this plan is against `49808be0`.
  Verified directly: `first_text_line` over the real `cmd.exe` stdout returns `Some("2.0.1")`.
- Edge case, the ESC never arriving. If the ESC byte were dropped and only `[` survived, the
  first text line would be `[?25l`, which carries no `\d+(\.\d+)+`, so the **unmutated** test
  fails outright rather than passing for the wrong reason, and under the mutation it would
  report `no version in output: [?25l` rather than `<empty>`. Pinning the exact string `<empty>`
  in the control is therefore itself the discriminator proving the ESC arrived and sanitized
  away. Both directions measured out of tree.
- Edge case, trailing whitespace: `echo A& echo B` with no space before the `&` emits `A` with
  no trailing space, which is why the `&` must stay flush against `l`. `strip_ansi_and_controls`
  trims anyway, so a stray space would not change the outcome, but the shape matches the
  sibling test and must not be reformatted.
- Edge case, argv quoting: the argument contains spaces, so Rust's Windows command-line
  builder wraps it in double quotes, producing `cmd.exe /C "echo <ESC>[?25l& echo 2.0.1"`.
  `cmd.exe /C` strips those outer quotes and runs the remainder, which is the same mechanism
  `probe_version_skips_leading_blank_lines` already depends on.
- Failure behaviour is unchanged. If the spawn fails or the process exceeds `PROBE_TIMEOUT`,
  `probe_version` still returns `ProbeOutcome::Failed(...)` and the assertion still fails with
  a readable `left`/`right` diff. Nothing swallows a failure.
- No production code path changes. The edit is entirely inside `mod tests`
  (`agent_version.rs:1273`, `#[cfg(test)]`).

## Positive control (issue acceptance criterion 3)

The test must remain able to fail when the behaviour it asserts is reverted. The behaviour is
"a first line that sanitizes to nothing is skipped", implemented by the ordering in
`text_lines` (`agent_version.rs:206-210`): sanitize first, then drop empties.

**The mutation.** In `src-tauri/src/agent_version.rs`, in `text_lines`, swap the two adapters
so the emptiness test runs on the raw line instead of the sanitized one:

```rust
// SHIPPED
    text.split(['\n', '\r'])
        .map(strip_ansi_and_controls)
        .filter(|line| !line.is_empty())

// MUTATED (positive control only, never committed)
    text.split(['\n', '\r'])
        .filter(|line| !line.is_empty())
        .map(strip_ansi_and_controls)
```

This is a surgical revert of exactly the asserted behaviour: a raw ANSI-only line is non-empty,
so it survives the filter, sanitizes to `""`, and becomes the first text line. Blank-line
skipping is untouched, because a raw blank line is empty and is still dropped. Verified out of
tree against real `cmd.exe` output: under the mutation `first_text_line` returns `Some("")` for
the ANSI fixture and still `Some("1.2.3")` for the sibling blank-line fixture.

**Surgical about the behaviour, not about the blast radius.** While the mutation is applied,
five further assertions across four other test functions also fail, and every one of them is
another ANSI-only-line assertion:

| Panic anchor | Test function | Assertion |
| --- | --- | --- |
| `agent_version.rs:1310` | `parse_version_token_fixtures` | `parse_version_token("\n\x1b[?25l\n2.0.1\n")`, argument on `:1311` |
| `agent_version.rs:1326` | `first_text_line_skips_blank_and_ansi_only_lines` | `first_text_line("\n\x1b[?25l\n2.0.1\n")`, argument on `:1327` |
| `agent_version.rs:1331` | `first_text_line_skips_blank_and_ansi_only_lines` | `first_text_line("\x1b[?25l\n")` |
| `agent_version.rs:1362` | `osc_ends_only_at_bel_or_st` | `first_text_line(unterminated)` |
| `agent_version.rs:1370` | `sanitize_detail_strips_ansi_controls_and_truncates` | `sanitize_detail(b"\n\x1b[?25l\n2.0.1\n")` |

The first two are multi-line `assert_eq!` invocations, and a failing multi-line `assert_eq!`
reports the line the macro starts on, not the line its argument sits on. Measured, not assumed.

That makes the control stronger, not weaker, and V5's `--exact` filter never selects any of
them. But an implementer who runs a module-wide filter while mutated will see five extra reds:
they are expected, and they disappear on the revert.

**Completeness of that list.** Those four functions hold 35 `assert` statements in total
(13 + 4 + 12 + 6 at `49808be0`). Five of the 35 call `strip_ansi_and_controls` directly and are
structurally immune, because the mutation reorders `text_lines` and does not touch the stripper.
The other 30 all route through `text_lines`, and every one of them was evaluated under both
orderings out of tree: the shipped ordering reproduces the expected value on all 35, and exactly
the five above change. Nothing else in the module moves either, including both sibling probe
tests (`probe_version_skips_leading_blank_lines` stays `Some("1.2.3")`,
`probe_version_no_token_fails` stays `"hello"`).

**The command**, run from `src-tauri` in the PowerShell tool, output redirected **outside the
repository** because this workspace collapses `cargo test` output to one summary line:

```
cargo test --lib agent_version::tests::probe_version_skips_an_ansi_only_first_line -- --exact --nocapture *> "$env:TEMP\1741-control.txt"
```

The target is outside the repo on purpose. A bare `*> control.txt` with cwd `src-tauri` writes
`src-tauri/control.txt`, and `git check-ignore -v src-tauri/control.txt` exits 1 with no
matching rule (there is no `src-tauri/.gitignore`), so the file would sit untracked in the tree
holding a deliberately failing test log, invisible to a hash check and to `git diff`, waiting
for someone's `git add -A`. This rule is not specific to the control: **every redirect in this
plan writes under `$env:TEMP`, never into the working tree.**

**The expected failure.** With the mutation applied, `parse_version_token` returns `None` on
both streams, so `parse_probe_completion` takes the `None` branch (`agent_version.rs:921-932`),
`sanitize_detail(stdout)` is empty and is substituted with `<empty>`, and the run reports the
block below. **The anchor is `:1409`, not `:1417`**: the control runs with the fix applied, and
the hunk removes nine lines and adds one, so the assertion moves up by eight. Reading `:1417`
here would be reading the pre-fix file.

```
thread 'agent_version::tests::probe_version_skips_an_ansi_only_first_line' panicked at src-tauri\src\agent_version.rs:1409:9:
assertion `left == right` failed
  left: Failed("no version in output: <empty>")
 right: Version("2.0.1")
test result: FAILED. 0 passed; 1 failed
```

Note that `left` is now a parse failure, not `Failed("timed out after 15s (killed)")`. That
distinction is the whole point: the test fails on behaviour, at cmd.exe speed, not on a clock.

**Apply and revert, so nothing leaks into the commit.**

This is the sequence, in this order. The control runs on top of the applied fix and before any
commit, so a single `git checkout --` discards both and the tree returns to `49808be0`.

1. Apply the fix hunk. Run V1 through V4. Record the SHA-256 of
   `src-tauri/src/agent_version.rs` in this state; call it `H_fix`. `git -C <repo> status
   --porcelain` shows exactly `src-tauri/src/agent_version.rs` at this point (the plan file is
   gitignored and never appears there).
2. Apply the mutation by hand as the two-line swap above. Do not save a patch file inside the
   repository; the working copy is the only mutated artifact.
3. Run the control command, capture the failure, and record it.
4. Revert with `git -C <repo> checkout -- src-tauri/src/agent_version.rs`. This discards the
   mutation and the fix together, which is intended. Never hand-undo the mutation.
5. Re-apply the fix hunk from this plan's "Decided solution" and confirm the file's SHA-256
   equals `H_fix`. That equality is the revert-exactness proof: it is independent of reading
   the diff.
6. Prove cleanliness two more ways, both required. `git -C <repo> diff --stat` shows
   `src-tauri/src/agent_version.rs` with one hunk, and `git -C <repo> diff` contains no
   `text_lines` hunk. Then `git -C <repo> status --porcelain --untracked-files=all` shows
   exactly one line, ` M src-tauri/src/agent_version.rs`, and nothing else. The status check is
   not redundant with the hash and the diff: both of those are blind to untracked files, so
   only status can catch a stray artifact left in the tree. Then run V6 and commit.

## Verification

All cargo commands run from `src-tauri` in the **PowerShell** tool, never the Bash tool: the
MSYS `git` on the Bash PATH cannot resolve the `file:///D:/...` remotes that
`tests/cli_agency_templates.rs` builds, which produces a spurious failure unrelated to this
change. Redirect every run to a file, because a wrapper in this workspace collapses `cargo test`
output to a single summary line and loses panic text. **Every such file goes to
`$env:TEMP\1741-<name>.txt`, never to a path inside the repository**: nothing under `src-tauri`
is gitignored by default, so a log written there is an untracked artifact that survives a file
hash check and a `git diff` and can be swept into a commit by `git add -A`.

| # | Command (cwd `src-tauri`) | Pass condition |
| --- | --- | --- |
| V1 | `cargo fmt --all -- --check` | exit 0, no diff. Already verified against a copy of the post-fix file: `rustfmt --edition 2021 --check` exits 0, and so does the unmodified base (control). |
| V2 | `cargo check --all-targets` | exit 0 |
| V3 | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| V4 | `cargo test --lib agent_version::tests:: -- --nocapture` | `0 failed` in `agent_version::tests`, including all 6 `probe_version_*` process tests and the four `PROBE_TIMEOUT` siblings |
| V5 | positive control (section above) | the target test FAILS with `Failed("no version in output: <empty>")`, panic anchored at `agent_version.rs:1409:9` (post-fix numbering) |
| V6 | `cargo test --lib --bins --tests` | matches CI. Read the **last** `test result:` line per binary, the one with `0 filtered out`; earlier lines are child re-invocations. Pass condition: `0 failed` on that line, and no failure name that was not already failing on `49808be0`. |
| V7 | CI on the pull request | `rust-regression` green |
| V8 | after merge, `rust-regression` on `main` | green on three consecutive runs with no rerun (issue acceptance criterion 1) |

**A green CI run cannot show the margin, only the verdict.** libtest discards stdout for
passing tests, so zero `---- <test> stdout ----` blocks in a green log is structural and not
evidence that the probe finished quickly. V7 and V8 therefore prove pass or fail, never
headroom. If headroom is ever wanted it has to be measured locally with `-- --nocapture`, and
this plan does not make that a gate, because the local host cannot reproduce runner conditions
anyway (see "Environment risk").

V8 is the only criterion this change cannot demonstrate locally, and it is the one the issue
actually asks for. It is satisfied by observation after merge, not by any local run.

## Objective acceptance criteria

1. `src-tauri/src/agent_version.rs:1404-1412` is replaced by the single `cmd.exe` line, and
   `grep -c "powershell.exe" src-tauri/src/agent_version.rs` returns 0.
2. `git diff 49808be0 -- src-tauri/src/agent_version.rs` touches exactly one hunk, entirely
   inside `#[cfg(test)] mod tests`, 9 lines removed and 1 added.
3. `git diff 49808be0 --name-only` lists exactly `plans/1741-probe-ansi-windows-spawn-target.md`
   and `src-tauri/src/agent_version.rs`.
4. `PROBE_TIMEOUT` is byte-identical to `49808be0`, and so are both `agent_update.rs` call
   sites and the Unix arm at `agent_version.rs:1414`.
5. V1 through V4 and V6 pass; V5 fails exactly as specified and the mutation is proven absent
   from the commit.
6. `rust-regression` is green on the pull request, and green on `main` for three consecutive
   runs without a rerun.

## Environment risk

Recorded here because a local green result on this machine is weaker evidence than it looks.

- **This box cannot reproduce the CI condition.** Defender real-time protection is OFF here
  (`(Get-MpComputerStatus).RealTimeProtectionEnabled` is `False`); GitHub's Windows runners have
  it ON, and it is a first-order contributor to PowerShell cold start. The local `--lib` suite
  also runs 5 to 7 times faster than the runner. So "the test passed locally" was already true
  before this fix and proves nothing about CI. What transfers is the *ratio*: `cmd.exe` loads no
  CLR and no PowerShell engine, so its startup is a different cost class on any Windows host,
  loaded or not. The transferable evidence is that ratio plus the four sibling tests that already
  spawn `cmd.exe` under `PROBE_TIMEOUT` on the failing runner and have never flaked.
- **The build directory is global and shared.** `C:\Users\maria\.cargo\config.toml` sets
  `build-dir = "D:/ac_temp_builds/{workspace-path-hash}"` and an sccache cache under the same
  root, both shared by every room on this machine. Concurrent builds from other rooms contend
  for it. Consequence for this plan: never delete anything under `D:\ac_temp_builds`, treat a
  long or stalled build as contention rather than breakage, and if a cargo result looks
  impossible, re-run it rather than reasoning about it. Final artifacts still land in
  `<repo>/target`, not `<repo>/src-tauri/target`.
- **`cargo test` output is collapsed and its counts are not naively comparable.** A wrapper
  reduces the output to one line, so every verification run redirects to a file. The lib binary
  prints more than one `test result:` line because some tests re-exec the binary as a child; the
  whole-run line is the one with `0 filtered out`. The suite total is also `+2` when `dist/` has
  been built, and CI builds `dist/` (`pr-regression-gates.yml:66`) while a fresh local tree may
  not, so a local total and a CI total can differ by 2 with no code difference. Report exit code
  and failure names, not a bare test count.
- **One job executes this test.** `rust-regression` (`windows-latest`,
  `pr-regression-gates.yml:90-92`) is the only job that runs it. Two other jobs do run `cargo
  test`, but neither can select it: `rust-regression-linux` at `:154-161` is `--exact` on a
  single different test name, and `terminal-snapshot-portable` at `:292` and `:295` targets
  other packages. The Linux and macOS legs only type-check this file (`:133-139`, `:242-248`).
  So a change that looked safe on the Unix arm would be validated by nothing, which is the
  reason that arm is left untouched.
- **`rust-fmt` is a separate required gate.** `cargo fmt --all -- --check`
  (`pr-regression-gates.yml:263-265`) will reject a hunk that rustfmt would reformat. The
  proposed line is 64 columns at its indentation and rustfmt keeps it on one line; this was
  verified out of tree with a passing control on the unmodified file.

## File impact

Files ADDED

| Path completo archivo | Qué se modificó? |
| --- | --- |
| `D:\0_repos\AgentsCommander_iac\.ac\room-15-ac-dev-team-v4\repo-AgentsCommander\plans\1741-probe-ansi-windows-spawn-target.md` | This plan. |

`/plans/` is listed in `.gitignore:11` while the directory's contents are tracked by
convention, so this file does not appear in `git status` and a plain `git add` fails on it.
Stage it with `git -C <repo> update-index --add plans/1741-probe-ansi-windows-spawn-target.md`
when the implementation commit is made. Until then the plan exists only in the working tree.

Files REMOVED

| Path completo archivo | Qué se modificó? |
| --- | --- |
| (none) | No file is removed. |

Files MODIFIED

| Path completo archivo | Qué se modificó? |
| --- | --- |
| `D:\0_repos\AgentsCommander_iac\.ac\room-15-ac-dev-team-v4\repo-AgentsCommander\src-tauri\src\agent_version.rs` | Lines 1404-1412, the Windows arm of `probe_version_skips_an_ansi_only_first_line`, replaced by the single line `("cmd.exe", vec!["/C", "echo \x1b[?25l& echo 2.0.1"])`. Inside `#[cfg(test)] mod tests`. Nothing else in the file changes: `PROBE_TIMEOUT`, `text_lines`, the Unix arm, the `probe_version` call and the assertion are byte-identical. |
