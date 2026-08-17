# Destructive Filesystem Regression Manual Checks

These checks are fallback evidence for filesystem behavior that may be blocked by CI or workstation policy, especially Windows long paths and directory junction creation.

## Scope And Safety

Run every manual check inside a fresh temp root that you create only for that check. Do not run these commands inside a real project, user profile, repository checkout, or shared working directory.

Recommended temp root pattern:

```powershell
$Root = Join-Path $env:TEMP ("ac-destructive-fs-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $Root | Out-Null
Write-Host $Root
```

Only remove `$Root` after you have verified it is the temp root you created for the check:

```powershell
Resolve-Path $Root
Remove-Item -LiteralPath $Root -Recurse -Force
```

Do not clean up by deleting parent directories or wildcard paths.

## Evidence Requirements

For each manual check, capture:

- The absolute `$Root` path.
- The command line used.
- Stdout and stderr.
- A directory listing before and after the destructive command.
- The outside sentinel path and proof that it survived.
- The expected error code or JSON field when the check is a refusal case.

Use an outside sentinel for every check:

```powershell
$Outside = Join-Path $Root "outside-sentinel\keep.txt"
New-Item -ItemType Directory -Path (Split-Path $Outside) | Out-Null
Set-Content -LiteralPath $Outside -Value "keep"
```

Verify it after the command:

```powershell
Test-Path -LiteralPath $Outside
Get-Content -LiteralPath $Outside
```

## Reset Long Path Check

Automated test:

```powershell
cargo test --test cli_test_reset long_path_target_deletes_only_allowed_directories -- --nocapture
```

Manual fallback if the automated test prints a skip:

1. Create `$Root` and `$Outside` as described above.
2. Build or locate `target\debug\agentscommander-new.exe`.
3. Create a deep binary parent:

```powershell
$Base = $Root
1..10 | ForEach-Object { $Base = Join-Path $Base ("long-segment-{0:D2}-abcdef" -f $_) }
New-Item -ItemType Directory -Path $Base | Out-Null
$Bin = Join-Path $Base "agentscommander_testeable.exe"
Copy-Item -LiteralPath "target\debug\agentscommander-new.exe" -Destination $Bin
```

4. Create only the allowed reset candidates and one sibling that must survive:

```powershell
$Config = Join-Path $Base ".agentscommander_testeable\nested"
$Project = Join-Path $Base "agentscommander_testeable\nested"
$Sibling = Join-Path $Base ".agentscommander_other"
New-Item -ItemType Directory -Path $Config,$Project,$Sibling | Out-Null
```

5. Run reset:

```powershell
& $Bin test-reset --confirm-testeable
```

Expected result:

- Exit code is 0.
- Stdout includes `plannedDelete` with exactly `.agentscommander_testeable` and `agentscommander_testeable`.
- `.agentscommander_testeable` is gone.
- `agentscommander_testeable` is gone.
- `.agentscommander_other` remains.
- `$Outside` remains.

## Reset Junction Reparse Check

Automated test:

```powershell
cargo test --test cli_test_reset junction_target_refuses_and_deletes_nothing -- --nocapture
```

Manual fallback if the automated test prints a skip:

1. Create `$Root` and `$Outside` as described above.
2. Copy the binary into `$Root` as `agentscommander_testeable.exe`:

```powershell
$Bin = Join-Path $Root "agentscommander_testeable.exe"
Copy-Item -LiteralPath "target\debug\agentscommander-new.exe" -Destination $Bin
```

3. Create the target, candidate junction, and second allowed candidate:

```powershell
$Real = Join-Path $Root "real-dir"
$Junction = Join-Path $Root ".agentscommander_testeable"
$Project = Join-Path $Root "agentscommander_testeable"
New-Item -ItemType Directory -Path $Real,$Project | Out-Null
cmd /C mklink /J "$Junction" "$Real"
```

4. Run reset:

```powershell
& $Bin test-reset --confirm-testeable
```

Expected result:

- Exit code is 1.
- Stderr JSON has `"error":"reset_candidate_is_reparse_point"`.
- `$Junction` remains.
- `$Real` remains.
- `$Project` remains.
- `$Outside` remains.

The automated test creates its own temp root and junction. Creating this manual junction does not make a skipped automated test pass; it only provides equivalent manual evidence.

## Workgroup Long Path Check

Automated test:

```powershell
cargo test --test cli_workgroup_team workgroup_remove_deletes_long_path_tree -- --nocapture
```

Manual fallback if the automated test prints a skip:

1. Create `$Root` and `$Outside` as described above.
2. Copy `target\debug\agentscommander-new.exe` into `$Root` and configure the copied binary:

```powershell
$Bin = Join-Path $Root "agentscommander-new.exe"
Copy-Item -LiteralPath "target\debug\agentscommander-new.exe" -Destination $Bin
$ConfigDir = Join-Path $Root ".agentscommander-new"
New-Item -ItemType Directory -Path $ConfigDir | Out-Null
@{
  defaultShell = "powershell.exe"
  defaultShellArgs = @()
  agents = @()
  projectPaths = @($Root)
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $ConfigDir "settings.json")
```

3. Create `ProjectAlpha`, `.ac`, `_agent_architect`, and a workgroup with the CLI:

```powershell
$Project = Join-Path $Root "ProjectAlpha"
$Agent = Join-Path $Project ".ac\_agent_architect"
New-Item -ItemType Directory -Path (Join-Path $Agent "memory") | Out-Null
Set-Content -LiteralPath (Join-Path $Agent "Role.md") -Value "# architect"
& $Bin workgroup add --project ProjectAlpha --team "Dev Team" --title "Build" --coordinator architect
```

4. Inside `.ac\wg-1-dev-team`, create a long nested path and payload file:

```powershell
$Deep = Join-Path $Root "ProjectAlpha\.ac\wg-1-dev-team"
1..10 | ForEach-Object { $Deep = Join-Path $Deep ("long-segment-{0:D2}-abcdef" -f $_) }
New-Item -ItemType Directory -Path $Deep | Out-Null
Set-Content -LiteralPath (Join-Path $Deep "payload.txt") -Value "payload"
```

5. Run:

```powershell
& $Bin workgroup remove --project ProjectAlpha --workgroup wg-1-dev-team --force-dirty
```

Expected result:

- Exit code is 0.
- Machine output, if enabled, reports `"removed":true`.
- `.ac\wg-1-dev-team` is gone.
- `$Outside` remains.
- Any sibling path outside `.ac\wg-1-dev-team` remains.

## Workgroup Reparse Root Check

Automated test:

```powershell
cargo test --test cli_workgroup_team workgroup_remove_refuses_reparse_root -- --nocapture
```

Manual fallback if the automated test prints a skip:

1. Create `$Root` and `$Outside` as described above.
2. Copy `target\debug\agentscommander-new.exe` into `$Root` and configure the copied binary:

```powershell
$Bin = Join-Path $Root "agentscommander-new.exe"
Copy-Item -LiteralPath "target\debug\agentscommander-new.exe" -Destination $Bin
$ConfigDir = Join-Path $Root ".agentscommander-new"
New-Item -ItemType Directory -Path $ConfigDir | Out-Null
@{
  defaultShell = "powershell.exe"
  defaultShellArgs = @()
  agents = @()
  projectPaths = @($Root)
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $ConfigDir "settings.json")
```

3. Set up a throwaway `ProjectAlpha` and create `wg-1-dev-team` with the CLI:

```powershell
$Project = Join-Path $Root "ProjectAlpha"
$Agent = Join-Path $Project ".ac\_agent_architect"
New-Item -ItemType Directory -Path (Join-Path $Agent "memory") | Out-Null
Set-Content -LiteralPath (Join-Path $Agent "Role.md") -Value "# architect"
& $Bin workgroup add --project ProjectAlpha --team "Dev Team" --title "Build" --coordinator architect
```

4. Remove the real workgroup directory and replace it with a junction:

```powershell
$Wg = Join-Path $Root "ProjectAlpha\.ac\wg-1-dev-team"
Remove-Item -LiteralPath $Wg -Recurse -Force
$Real = Join-Path $Root "real-wg-target"
New-Item -ItemType Directory -Path $Real | Out-Null
Set-Content -LiteralPath (Join-Path $Real "sentinel.txt") -Value "target"
cmd /C mklink /J "$Wg" "$Real"
```

5. Run:

```powershell
& $Bin workgroup remove --project ProjectAlpha --workgroup wg-1-dev-team --force-dirty
```

Expected result:

- Exit code is 1.
- Stderr contains `delete_root_is_reparse_point`.
- `$Wg` remains as a junction.
- `$Real\sentinel.txt` remains.
- `$Outside` remains.
- No `workgroupRemoved` project refresh request is written.

The automated test creates its own temp root and junction. Creating this manual junction does not make a skipped automated test pass; it only provides equivalent manual evidence.

## Helper Reparse Root Check

Automated test:

```powershell
cargo test commands::entity_creation::tests::validate_delete_root_rejects_reparse_root -- --nocapture
```

Manual fallback if the automated test prints a skip:

1. Create `$Root` and `$Outside` as described above.
2. Create a real directory and a junction named like a workgroup:

```powershell
$Real = Join-Path $Root "real"
$Junction = Join-Path $Root "wg-1-test"
New-Item -ItemType Directory -Path $Real | Out-Null
cmd /C mklink /J "$Junction" "$Real"
```

3. Run the automated helper test again on a Windows host where junction creation is available.

Expected result:

- The helper rejects the root with `delete_root_is_reparse_point`.
- `$Junction` remains.
- `$Real` remains.
- `$Outside` remains.

The automated helper test creates its own temp root and junction. Creating this manual junction does not make a skipped automated test pass; it documents the same expected filesystem condition for manual evidence.
