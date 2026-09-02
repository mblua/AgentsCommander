# Phase 3 (#1724): Document the switch where the UI automation bridge is documented

Status: READY_FOR_IMPLEMENTATION
Class: `patterned`
Owner: `ac-technical-writer-v4`
Depends on: Phase 1 (`phase-1-rust-gate-and-command`), which fixes the environment-variable name and the gate. Parallel with: Phase 2 (`phase-2-frontend-suppression`), which shares no file.
Branch: `feature/1724-testable-pulse-suppression-switch`, base `1e57aa581de4c4fd18590cdf0652d8bf60b18a4f`.
Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-AgentsCommander`.

## Objective

Satisfy issue #1724 criterion 6: the switch is documented wherever the UI automation bridge is documented. That single place is the "Semantic UI Automation" section of `docs/testing/README.md`.

## Exact files (freeze this set; nothing else may change)

1. `docs/testing/README.md`

## Where the text goes

`docs/testing/README.md` at the pinned base:

- line 79: `## Semantic UI Automation`
- line 81: the `--ui-automation` / `data-ac-testid` paragraph
- line 83: the affordance-matrix paragraph
- line 85: `## Control Coverage Discipline`

Add a new `###` subsection at the end of the "Semantic UI Automation" section, that is after line 83 and before line 85, with one blank line on each side of the heading, matching the file's existing spacing. Do not renumber, reword, or reorder anything already in the file, and do not touch the "Deterministic Testable App" section above it.

## Facts the text must carry, all of them

1. What the pulse is, in one sentence: on every terminal attach that carries history, the app nudges the main splitter by 16 px to force a reflow so the TUI repaints. It is a workaround for the attach-seed fidelity gap tracked in #1656.
2. Why the switch exists: to observe and validate #1656 without the pulse masking the result.
3. How to turn it on: set `AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE=1` before launching, alongside the normal automation launch.
4. The gate, stated as all three conditions: the executable must be `agentscommander_testeable.exe`, UI automation must be enabled with `--ui-automation` or `AC_UI_AUTOMATION=1`, and the variable must be exactly `1`. Any other combination leaves the pulse running. Normal, stage and room binaries ignore the variable entirely and still start normally, exactly as they fail closed on test placement input.
5. How to verify it took effect, and that this is the point of the switch: query the attached terminal and read `target.layoutPulse.reason`. It is `"suppressed"` when the switch did it. The value matters because the pulse also silently declines to move the divider for several unrelated reasons (`clamped`, `busy`, `dragging`, `persistence_owned`), so "the divider did not move" proves nothing on its own, while `"suppressed"` is emitted from exactly one place.
6. That the switch is off by default and changes nothing when off.

## Suggested text (the writer owns the final prose; every fact above must survive)

```markdown
### Suppressing the sidebar layout pulse

On every terminal attach that carries history, the app nudges the main splitter by 16 px to force a reflow so the TUI repaints. It is a workaround for the attach-seed fidelity gap tracked in #1656, and while it runs it masks exactly the rendering that issue needs to measure.

The testable binary can turn it off:

```powershell
$env:AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE='1'
.\agentscommander_testeable.exe --app --ui-automation
```

The switch is off by default and requires all three of: the executable named `agentscommander_testeable.exe`, UI automation enabled through `--ui-automation` or `AC_UI_AUTOMATION=1`, and the variable set to exactly `1`. Any other combination leaves the pulse running. Normal, stage and room binaries ignore the variable and start normally.

Verify it actually took effect rather than assuming it, by reading the pulse trace off the attached terminal:

```powershell
.\agentscommander_testeable.exe ui-terminal --session-id <session-id> query
```

`target.layoutPulse.reason` is `"suppressed"` when the switch suppressed the pulse. Check that value rather than checking that the divider did not move: the pulse also declines silently for unrelated reasons (`clamped`, `busy`, `dragging`, `persistence_owned`), and `"suppressed"` is emitted from exactly one place.
```

Note that the fenced PowerShell blocks are nested inside the block above for transport only; write them as ordinary top-level fences in the file, matching the existing `powershell` fences at lines 62-65 and 71-73.

## Verification commands, from the repository root

```
git diff --stat docs/testing/README.md
git diff docs/testing/README.md
grep -n "AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE" docs/testing/README.md
grep -n "layoutPulse" docs/testing/README.md
grep -c "^## " docs/testing/README.md
```

Expected results:

- `git diff --stat` lists `docs/testing/README.md` and nothing else, with zero deletions: the change is purely additive.
- `AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE` appears at least once, inside the "Semantic UI Automation" section (line number greater than the `## Semantic UI Automation` line and less than the `## Control Coverage Discipline` line).
- `layoutPulse` appears at least once.
- The count of `## ` headings is unchanged from the base SHA, proving no `##` section was added, removed, or promoted; the new heading is a `###`.

No CI check compiles or lints this file. `test-debt`, `frontend-regression`, `rust-regression` and the rest are unaffected by a Markdown-only diff, but they all still run on the pull request and must be green on the exact PR-head SHA.

## Acceptance criteria

1. `git status --porcelain` lists exactly `docs/testing/README.md` and nothing else.
2. `git diff docs/testing/README.md` shows added lines only, zero removed lines.
3. All six facts listed above are present in the added text, checked one by one against that list.
4. The new heading is `###`, sits between the `## Semantic UI Automation` and `## Control Coverage Discipline` headings, and the `## ` heading count is unchanged.
5. Every code fence in the added text is closed, and the rendered section shows no broken fence when viewed on GitHub.

## Preserve list (must not change in this phase)

- Every existing line of `docs/testing/README.md`, including the "Deterministic Testable App", "Control Coverage Discipline", "Test Reset" and "Case IDs" sections and the affordance-matrix link.
- `docs/testing/semantic-ui-automation-affordance-matrix.md` and every other file under `docs/`.
- Everything under `src/`, everything under `src-tauri/`, and every file at the repository root.
