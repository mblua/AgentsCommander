# Smoke test for the Windows portable zip asset, for issue #1589.
#
# Proves the three things the asset promises and that CI cannot otherwise see:
#   1. The zip carries the expected files, and the binary is named
#      agentscommander.exe (NOT the published raw name, which parses as the
#      instance suffix "64" and silently changes config dir, mutex, and ports).
#   2. The binary in the zip is the build for this version.
#   3. Running it creates and uses the unsuffixed HOME storage directory
#      $HOME/.agentscommander and writes no instance directory next to the
#      executable: the unsuffixed binary resolves the default HOME profile, not
#      an adjacent portable one.
#
# This probe writes to a real UserProfile, so it refuses to run anywhere but a
# disposable GitHub-hosted Windows runner with a fresh profile. A rerun in a
# used profile refuses (PROFILE_EXISTS) instead of adopting or cleaning it.
#
# Usage:
#   pwsh -File scripts/smoke-windows-portable.ps1 -Zip <path.zip> -ExpectedVersion 0.30.3
#
# Exit codes: 0 -> every assertion passed, 1 -> an assertion failed.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string]$Zip,
    [Parameter(Mandatory = $true)] [string]$ExpectedVersion
)

$ErrorActionPreference = "Stop"

$failures = @()
function Assert-That {
    param([bool]$Condition, [string]$Message)
    if ($Condition) {
        Write-Host "  PASS  $Message"
    } else {
        Write-Host "  FAIL  $Message" -ForegroundColor Red
        $script:failures += $Message
    }
}

# Admission guards. Script-local, no imports and no runtime flags. They throw
# terminating errors carrying a stable code, read no profile contents, and are
# recorded by the caller through Assert-That so the run keeps its nonzero summary.

function Assert-SmokeHost {
    param([bool]$Windows, [string]$Actions, [string]$Runner)
    if (-not $Windows) {
        throw "[portable-smoke] HOST_REFUSED: this smoke only runs on Windows (IsWindows is not true)"
    }
    if ($Actions -ne 'true') {
        throw "[portable-smoke] HOST_REFUSED: this smoke only runs on GitHub Actions (GITHUB_ACTIONS='$Actions')"
    }
    if ($Runner -ne 'github-hosted') {
        throw "[portable-smoke] HOST_REFUSED: this smoke needs a disposable GitHub-hosted runner (RUNNER_ENVIRONMENT='$Runner')"
    }
}

function Assert-SmokeProfile {
    param([string]$Known, [string]$Environment)
    if ([string]::IsNullOrWhiteSpace($Known) -or [string]::IsNullOrWhiteSpace($Environment)) {
        throw "[portable-smoke] PROFILE_IDENTITY: UserProfile and USERPROFILE must both be non-blank"
    }
    if (-not [IO.Path]::IsPathFullyQualified($Known) -or -not [IO.Path]::IsPathFullyQualified($Environment)) {
        throw "[portable-smoke] PROFILE_IDENTITY: UserProfile and USERPROFILE must both be fully qualified paths"
    }
    if (-not (Test-Path -LiteralPath $Known -PathType Container) -or -not (Test-Path -LiteralPath $Environment -PathType Container)) {
        throw "[portable-smoke] PROFILE_IDENTITY: UserProfile and USERPROFILE must both be existing directories"
    }
    $trailing = [char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    $knownFull = [IO.Path]::GetFullPath($Known).TrimEnd($trailing)
    $envFull = [IO.Path]::GetFullPath($Environment).TrimEnd($trailing)
    if (-not [string]::Equals($knownFull, $envFull, [StringComparison]::OrdinalIgnoreCase)) {
        throw "[portable-smoke] PROFILE_IDENTITY: UserProfile '$knownFull' and USERPROFILE '$envFull' name different directories"
    }
    # Immediate metadata only: names and attributes, never file contents.
    $entries = @(Get-ChildItem -LiteralPath $Known -Force -ErrorAction Stop |
        Where-Object { $_.Name -like '.agentscommander*' })
    if ($entries.Count -ne 0) {
        throw "[portable-smoke] PROFILE_EXISTS: the UserProfile already holds an .agentscommander* entry ($($entries.Name -join ', ')); a fresh profile is required"
    }
}

function Assert-SmokeLayout {
    param([string]$ProfileRoot, [string]$ExeRoot)
    # Metadata only: the canonical HOME container and its app.log leaf exist.
    $homeDir = Join-Path $ProfileRoot '.agentscommander'
    if (-not (Test-Path -LiteralPath $homeDir -PathType Container)) {
        throw "[portable-smoke] HOME_DIR_MISSING: the child did not create $homeDir"
    }
    $homeLog = Join-Path $homeDir 'app.log'
    if (-not (Test-Path -LiteralPath $homeLog -PathType Leaf)) {
        throw "[portable-smoke] HOME_LOG_MISSING: the child did not write $homeLog"
    }
    # Immediate profile entries: the canonical container is expected, anything
    # else starting with .agentscommander is a noncanonical stray.
    $profileStrays = @(Get-ChildItem -LiteralPath $ProfileRoot -Force -ErrorAction Stop |
        Where-Object { $_.Name -like '.agentscommander*' -and $_.Name -ne '.agentscommander' })
    if ($profileStrays.Count -ne 0) {
        throw "[portable-smoke] PROFILE_STRAY: noncanonical .agentscommander* entry in the profile ($($profileStrays.Name -join ', '))"
    }
    # Zero .agentscommander* entries of any type may sit beside the exe.
    $adjacentStrays = @(Get-ChildItem -LiteralPath $ExeRoot -Force -ErrorAction Stop |
        Where-Object { $_.Name -like '.agentscommander*' })
    if ($adjacentStrays.Count -ne 0) {
        throw "[portable-smoke] ADJACENT_STRAY: .agentscommander* entry beside the exe ($($adjacentStrays.Name -join ', '))"
    }
}

if (-not (Test-Path -LiteralPath $Zip)) {
    Write-Error "[portable-smoke] zip not found: $Zip"
    exit 1
}

# Admission first: the host guard runs before the UserProfile is queried, before
# any profile entry is enumerated and before any child is launched. A refusal is
# recorded as a failed assertion and skips all work that depends on it.
$admitted = $false
$profileRoot = $null
try {
    Assert-SmokeHost $IsWindows $env:GITHUB_ACTIONS $env:RUNNER_ENVIRONMENT
    Assert-That $true "host is a Windows GitHub-hosted runner (GITHUB_ACTIONS=true, RUNNER_ENVIRONMENT=github-hosted)"
    $profileRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
    Assert-SmokeProfile $profileRoot $env:USERPROFILE
    Assert-That $true "UserProfile is fresh and consistent with USERPROFILE ($profileRoot)"
    $admitted = $true
} catch {
    Assert-That $false "admission refused: $($_.Exception.Message)"
}

# Deliberately short extraction root: a long path is its own failure mode on
# Windows and would be misread as a packaging defect.
$workRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$work     = Join-Path $workRoot ("acp-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null

Write-Host "[portable-smoke] zip      : $Zip"
Write-Host "[portable-smoke] expected : $ExpectedVersion"
Write-Host "[portable-smoke] extract  : $work"
Write-Host ""

Expand-Archive -LiteralPath $Zip -DestinationPath $work -Force

Write-Host "Contents"
$expected = @('agentscommander.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'PORTABLE.txt')
$actual   = Get-ChildItem -LiteralPath $work | Select-Object -ExpandProperty Name | Sort-Object
$inventoryOk = $true
foreach ($name in $expected) {
    $present = $actual -contains $name
    Assert-That $present "$name is in the zip"
    $inventoryOk = $inventoryOk -and $present
}
$unexpected = $actual | Where-Object { $expected -notcontains $_ }
$noUnexpected = @($unexpected).Count -eq 0
Assert-That $noUnexpected "no unexpected entries (found: $($unexpected -join ', '))"
$inventoryOk = $inventoryOk -and $noUnexpected

$exe = Join-Path $work 'agentscommander.exe'
$exePresent = Test-Path -LiteralPath $exe -PathType Leaf
$inventoryOk = $inventoryOk -and $exePresent

Write-Host ""
Write-Host "Build identity"
$versionOk = $true
if ($exePresent) {
    $productVersion = (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    $hasVersion = -not [string]::IsNullOrWhiteSpace($productVersion)
    Assert-That $hasVersion "the exe carries Windows version info"
    $versionOk = $versionOk -and $hasVersion
    if ($hasVersion) {
        # Windows pads to four components; compare the SemVer triple only.
        $triple = ($productVersion.Trim().Split('.')[0..2]) -join '.'
        $tripleOk = $triple -eq $ExpectedVersion
        Assert-That $tripleOk "exe ProductVersion $productVersion matches $ExpectedVersion"
        $versionOk = $versionOk -and $tripleOk
    }
} else {
    $versionOk = $false
}

$readme = Join-Path $work 'PORTABLE.txt'
if (Test-Path -LiteralPath $readme -PathType Leaf) {
    $text = Get-Content -LiteralPath $readme -Raw
    $namesVersion = $text -match [regex]::Escape($ExpectedVersion)
    Assert-That $namesVersion "PORTABLE.txt names version $ExpectedVersion"
    $noPlaceholder = -not ($text -match '\{\{')
    Assert-That $noPlaceholder "PORTABLE.txt has no unresolved placeholder"
    $versionOk = $versionOk -and $namesVersion -and $noPlaceholder
} else {
    $versionOk = $false
}

Write-Host ""
Write-Host "Unsuffixed HOME storage contract"
if ($admitted -and $inventoryOk -and $versionOk) {
    $stdoutPath = Join-Path $work 'smoke-stdout.txt'
    $stderrPath = Join-Path $work 'smoke-stderr.txt'

    # Bounded child lifecycle mirrors scripts/smoke-cli-release-windows.ps1:
    # start, begin both async reads, wait 15000 ms, then bounded kill/reap and a
    # separate 5000 ms bound per stream. No unbounded Wait/Result on a task that
    # has not completed.
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $exe
    $psi.Arguments = 'list-sessions'
    $psi.WorkingDirectory = $work
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    # Remove only the two test config-dir overrides from the child environment:
    # the parent environment and the real profile stay untouched, so the child
    # resolves the default unsuffixed HOME storage.
    $psi.EnvironmentVariables.Remove('AGENTSCOMMANDER_CONFIG_DIR')
    $psi.EnvironmentVariables.Remove('AGENTSCOMMANDER_TEST_CONFIG_DIR')

    Write-Host "  command : `"$exe`" $($psi.Arguments)"
    Write-Host "  cwd     : $work"

    $child = $null
    $childStarted = $false
    $childPid = $null
    $exitCode = $null
    $timedOut = $false
    $reaped = $true
    $stdoutCaptureOk = $false
    $stderrCaptureOk = $false
    $stdout = ''
    $stderr = ''
    $lifecycleErrors = @()
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()

    try {
        $child = [Diagnostics.Process]::new()
        $child.StartInfo = $psi
        if (-not $child.Start()) {
            throw "Process.Start returned false for $exe"
        }
        $childStarted = $true
        $childPid = $child.Id
        $stdoutTask = $child.StandardOutput.ReadToEndAsync()
        $stderrTask = $child.StandardError.ReadToEndAsync()

        if (-not $child.WaitForExit(15000)) {
            $timedOut = $true
            try { $child.Kill() } catch { $lifecycleErrors += "timeout kill failed: $($_.Exception.Message)" }
            $reaped = $child.WaitForExit(5000)
            if (-not $reaped) { $lifecycleErrors += "timed-out child was not reaped within 5000 ms (pid $childPid)" }
        }

        if ($child.HasExited) {
            $exitCode = $child.ExitCode
        }

        if ($stdoutTask.Wait(5000)) {
            $stdout = $stdoutTask.Result
            $stdoutCaptureOk = $true
        }
        if ($stderrTask.Wait(5000)) {
            $stderr = $stderrTask.Result
            $stderrCaptureOk = $true
        }
    } catch {
        # Preserve the primary error; an exception after start with a live child
        # gets the same bounded kill/reap attempt.
        $lifecycleErrors += $_.Exception.Message
        if ($childStarted -and $null -ne $child -and -not $child.HasExited) {
            try { $child.Kill() } catch { $lifecycleErrors += "kill after failure failed: $($_.Exception.Message)" }
            $reaped = $child.WaitForExit(5000)
            if (-not $reaped) { $lifecycleErrors += "child was not reaped within 5000 ms after failure (pid $childPid)" }
        }
    } finally {
        $stopwatch.Stop()
        if ($null -ne $child) { $child.Dispose() }
    }

    Write-Host "  pid     : $childPid"
    Write-Host "  exit    : $exitCode"
    Write-Host "  elapsed : $($stopwatch.ElapsedMilliseconds) ms"
    Write-Host "  timeout : $timedOut"
    Write-Host "  reaped  : $reaped"
    Write-Host "  capture : stdout=$stdoutCaptureOk stderr=$stderrCaptureOk"

    if ($childStarted) {
        Assert-That $true "list-sessions child started (pid $childPid)"
        Assert-That (-not $timedOut) "list-sessions exited within the 15000 ms bound (elapsed $($stopwatch.ElapsedMilliseconds) ms)"
        Assert-That $reaped "the child process was reaped"
        if ($null -ne $exitCode) {
            Assert-That ($exitCode -eq 0) "list-sessions exited 0 (got $exitCode)"
        } else {
            Assert-That $false "list-sessions exit code was not observed"
        }
        Assert-That ($stdoutCaptureOk -and $stderrCaptureOk) "stdout and stderr were captured within their 5000 ms bound"
    } else {
        Assert-That $false "list-sessions child did not start"
    }
    foreach ($lifecycleError in $lifecycleErrors) {
        Assert-That $false "child lifecycle error: $lifecycleError"
    }

    # Captured text is printed to the CI log and kept in the extraction scratch;
    # a missing capture already failed above.
    Write-Host "  stdout  :"
    if ($stdoutCaptureOk) {
        [IO.File]::WriteAllText($stdoutPath, $stdout)
        if ([string]::IsNullOrWhiteSpace($stdout)) { Write-Host "    (empty)" } else { Write-Host ($stdout.TrimEnd()) }
    } else {
        Write-Host "    (capture failed)"
    }
    Write-Host "  stderr  :"
    if ($stderrCaptureOk) {
        [IO.File]::WriteAllText($stderrPath, $stderr)
        if ([string]::IsNullOrWhiteSpace($stderr)) { Write-Host "    (empty)" } else { Write-Host ($stderr.TrimEnd()) }
    } else {
        Write-Host "    (capture failed)"
    }

    $childOk = $childStarted -and (-not $timedOut) -and $reaped -and ($exitCode -eq 0)
    if ($childOk) {
        try {
            Assert-SmokeLayout $profileRoot $work
            Assert-That $true "the child created the unsuffixed HOME directory $(Join-Path $profileRoot '.agentscommander')"
            Assert-That $true "the child wrote app.log inside the HOME directory"
            Assert-That $true "the profile holds no noncanonical .agentscommander* entry"
            Assert-That $true "no .agentscommander* entry sits beside the exe"
        } catch {
            Assert-That $false "HOME storage contract: $($_.Exception.Message)"
        }
    } else {
        Write-Host "  skip  HOME storage checks: the child did not exit 0 within bounds"
    }
} else {
    $skipReason = if (-not $admitted) { 'admission was refused' } elseif (-not $inventoryOk) { 'the zip inventory failed' } else { 'the build identity checks failed' }
    Write-Host "  skip  child launch and HOME checks: $skipReason"
}

Write-Host ""
if ($failures.Count -gt 0) {
    Write-Host "[portable-smoke] FAILED with $($failures.Count) assertion(s):" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "[portable-smoke] OK: the zip is complete, the unsuffixed binary used HOME storage, and no adjacent instance directory exists."
exit 0
