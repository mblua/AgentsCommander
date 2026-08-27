[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)]
    [string]$BinaryPath,
    [string]$Shell = "powershell.exe",
    [string]$Token = "00000000-0000-0000-0000-000000000000",
    [string]$Root = (New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP "ac-smoke-$([guid]::NewGuid().ToString('N'))")).FullName,
    [string]$LogDir = (Join-Path $env:TEMP "ac-cli-smoke-logs")
)

$ErrorActionPreference = "Continue"
$failed = 0

if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
    Write-Host "FAIL: binary not found: $BinaryPath" -ForegroundColor Red
    exit 1
}

$shellCmd = Get-Command $Shell -ErrorAction SilentlyContinue
if ($null -eq $shellCmd) {
    Write-Host "SKIP: shell not found: $Shell" -ForegroundColor Yellow
    exit 0
}
$ShellPath = $shellCmd.Source

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$summary = New-Object System.Collections.Generic.List[object]

function New-CasePaths {
    param(
        [Parameter(Mandatory=$true)] [string]$CaseName
    )
    $safeCase = $CaseName -replace '[^A-Za-z0-9_.-]', '_'
    [pscustomobject]@{
        StdoutPath = Join-Path $LogDir "$safeCase.stdout.txt"
        StderrPath = Join-Path $LogDir "$safeCase.stderr.txt"
        CommandPath = Join-Path $LogDir "$safeCase.command.txt"
    }
}

function Write-CaseLogs {
    param(
        [Parameter(Mandatory=$true)] [pscustomobject]$Paths,
        [Parameter(Mandatory=$true)] [string]$Command,
        [AllowNull()] [string]$Stdout,
        [AllowNull()] [string]$Stderr
    )
    Set-Content -LiteralPath $Paths.CommandPath -Value $Command -Encoding UTF8
    Set-Content -LiteralPath $Paths.StdoutPath -Value $(if ($null -eq $Stdout) { '' } else { $Stdout }) -Encoding UTF8
    Set-Content -LiteralPath $Paths.StderrPath -Value $(if ($null -eq $Stderr) { '' } else { $Stderr }) -Encoding UTF8
}

# Bug-reproducing harness: spawn a fresh PowerShell process with -NonInteractive -NoProfile
# and run the AC exe via a direct call. This is the mandatory issue #129 shape.
function Invoke-PSNonInteractiveDirect {
    param(
        [Parameter(Mandatory=$true)] [string]$ShellPath,
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [string]$Exe,
        [Parameter(Mandatory=$true)] [string[]]$ExeArgs
    )
    $escapedExe = $Exe -replace "'", "''"
    $quotedArgs = ($ExeArgs | ForEach-Object {
        "'" + ($_ -replace "'", "''") + "'"
    }) -join ' '
    $inner = "& '$escapedExe' $quotedArgs"
    $arguments = "-NonInteractive -NoProfile -Command `"$inner`""
    $paths = New-CasePaths -CaseName $CaseName

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ShellPath
    $psi.Arguments = $arguments
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()

    $stdout = if ($null -eq $stdoutTask.Result) { '' } else { $stdoutTask.Result }
    $stderr = if ($null -eq $stderrTask.Result) { '' } else { $stderrTask.Result }
    $commandText = "$ShellPath $arguments"
    Write-CaseLogs -Paths $paths -Command $commandText -Stdout $stdout -Stderr $stderr

    $case = [pscustomobject]@{
        name = $CaseName
        shellPath = $ShellPath
        binaryPath = $Exe
        commandPath = $paths.CommandPath
        stdoutPath = $paths.StdoutPath
        stderrPath = $paths.StderrPath
        exitCode = $proc.ExitCode
    }
    $summary.Add($case) | Out-Null

    [pscustomobject]@{
        CaseName = $CaseName
        Stdout = $stdout
        Stderr = $stderr
        ExitCode = $proc.ExitCode
        StdoutPath = $paths.StdoutPath
        StderrPath = $paths.StderrPath
        CommandPath = $paths.CommandPath
    }
}

# #1596: Git Bash is the required AC CLI carrier on Windows. bash.exe is
# console-subsystem, so the outer PowerShell waits for it, captures its stdout,
# AND propagates its exit code ($LASTEXITCODE) — which the GUI-subsystem AC
# binary cannot provide through a bare PS `&` (see Invoke-BinaryDirect and the
# Invoke-PSNonInteractiveDirect comment above). Skipped (not a failure) when
# bash.exe is not on PATH.
function Invoke-BashRouted {
    param(
        [Parameter(Mandatory=$true)] [string]$ShellPath,
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [string]$Exe,
        [Parameter(Mandatory=$true)] [string[]]$ExeArgs
    )
    $paths = New-CasePaths -CaseName $CaseName

    if ($null -eq (Get-Command 'bash.exe' -ErrorAction SilentlyContinue)) {
        Write-CaseLogs -Paths $paths -Command 'bash.exe not found on PATH' -Stdout '' -Stderr ''
        $skipped = [pscustomobject]@{
            name = $CaseName
            shellPath = $ShellPath
            binaryPath = $Exe
            commandPath = $paths.CommandPath
            stdoutPath = $paths.StdoutPath
            stderrPath = $paths.StderrPath
            exitCode = $null
            skipped = $true
        }
        $summary.Add($skipped) | Out-Null
        Write-Host "SKIP: bash.exe not found (case $CaseName)" -ForegroundColor Yellow
        return [pscustomobject]@{
            CaseName = $CaseName
            Stdout = ''
            Stderr = ''
            ExitCode = $null
            Skipped = $true
            StdoutPath = $paths.StdoutPath
            StderrPath = $paths.StderrPath
            CommandPath = $paths.CommandPath
        }
    }

    # Bash-level quoting: single quotes around the exe and each arg (backslashes
    # and spaces stay literal inside bash single quotes).
    $bashParts = New-Object System.Collections.Generic.List[string]
    $bashParts.Add("'" + ($Exe -replace "'", "'\''") + "'")
    foreach ($a in $ExeArgs) {
        $bashParts.Add("'" + ($a -replace "'", "'\''") + "'")
    }
    $bashInner = $bashParts -join ' '
    # Embed the whole bash command as ONE PS single-quoted literal: wrap in
    # quotes and double every inner quote for PS, so bash.exe -lc receives the
    # backslashes and spaces verbatim.
    $psLiteral = "'" + ($bashInner -replace "'", "''") + "'"
    $inner = "& bash.exe -lc $psLiteral; exit `$LASTEXITCODE"
    $arguments = "-NonInteractive -NoProfile -Command `"$inner`""

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ShellPath
    $psi.Arguments = $arguments
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()

    $stdout = if ($null -eq $stdoutTask.Result) { '' } else { $stdoutTask.Result }
    $stderr = if ($null -eq $stderrTask.Result) { '' } else { $stderrTask.Result }
    $commandText = "$ShellPath $arguments"
    Write-CaseLogs -Paths $paths -Command $commandText -Stdout $stdout -Stderr $stderr

    $case = [pscustomobject]@{
        name = $CaseName
        shellPath = $ShellPath
        binaryPath = $Exe
        commandPath = $paths.CommandPath
        stdoutPath = $paths.StdoutPath
        stderrPath = $paths.StderrPath
        exitCode = $proc.ExitCode
        skipped = $false
    }
    $summary.Add($case) | Out-Null

    [pscustomobject]@{
        CaseName = $CaseName
        Stdout = $stdout
        Stderr = $stderr
        ExitCode = $proc.ExitCode
        Skipped = $false
        StdoutPath = $paths.StdoutPath
        StderrPath = $paths.StderrPath
        CommandPath = $paths.CommandPath
    }
}

# Start the binary with no shell in between, so the exit code read here is the binary's own.
#
# Why this exists separately from Invoke-PSNonInteractiveDirect: release builds carry
# `windows_subsystem = "windows"` (src-tauri/src/main.rs:1), so they are GUI-subsystem
# executables. PowerShell's `&` call operator does not wait for one; it returns 0 immediately
# while the child keeps writing to the inherited handles. That is why the wrapped case still
# sees correct stdout and stderr but can never observe a non-zero exit code, and why adding
# `exit $LASTEXITCODE` inside the wrapper does not help either: PowerShell never waited, so
# $LASTEXITCODE was never set from the child. Verified against a release build under both
# powershell.exe and pwsh.exe.
#
# Forcing a wait with a pipeline would work, but it routes stdout through PowerShell's
# formatter, which is exactly the raw passthrough the issue #129 cases exist to check. So the
# exit code is asserted here instead, and the shell-wrapped case keeps its stream assertions
# untouched. Do not move this back inside the wrapper.
function Invoke-BinaryDirect {
    param(
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [string]$Exe,
        [Parameter(Mandatory=$true)] [string[]]$ExeArgs
    )
    $paths = New-CasePaths -CaseName $CaseName

    # `Arguments`, not `ArgumentList`: this script also runs under Windows PowerShell 5.1 on
    # .NET Framework, where ProcessStartInfo.ArgumentList does not exist.
    $quotedArgs = ($ExeArgs | ForEach-Object { '"' + ($_ -replace '"', '\"') + '"' }) -join ' '

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = $quotedArgs
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()

    $stdout = if ($null -eq $stdoutTask.Result) { '' } else { $stdoutTask.Result }
    $stderr = if ($null -eq $stderrTask.Result) { '' } else { $stderrTask.Result }
    $commandText = "$Exe $quotedArgs"
    Write-CaseLogs -Paths $paths -Command $commandText -Stdout $stdout -Stderr $stderr

    $case = [pscustomobject]@{
        name = $CaseName
        shellPath = $null
        binaryPath = $Exe
        commandPath = $paths.CommandPath
        stdoutPath = $paths.StdoutPath
        stderrPath = $paths.StderrPath
        exitCode = $proc.ExitCode
    }
    $summary.Add($case) | Out-Null

    [pscustomobject]@{
        CaseName = $CaseName
        Stdout = $stdout
        Stderr = $stderr
        ExitCode = $proc.ExitCode
        StdoutPath = $paths.StdoutPath
        StderrPath = $paths.StderrPath
        CommandPath = $paths.CommandPath
    }
}

function Set-StartupMessageChildEnvironment {
    param(
        [Parameter(Mandatory=$true)] [System.Diagnostics.ProcessStartInfo]$StartInfo
    )
    $StartInfo.EnvironmentVariables['AC_UI_AUTOMATION'] = '0'
    [void]$StartInfo.EnvironmentVariables.Remove('AC_TEST_WINDOW_PLACEMENT')
}

function Stop-TimedOutProcessTree {
    param(
        [Parameter(Mandatory=$true)] [System.Diagnostics.Process]$Process
    )
    $processId = $Process.Id
    & taskkill.exe /PID $processId /T /F 2>&1 | Out-Null
    $taskkillExitCode = $LASTEXITCODE
    [void]$Process.WaitForExit(5000)
    $taskkillExitCode
}

function Invoke-StartupMessagePiped {
    param(
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [string]$Exe
    )
    $paths = New-CasePaths -CaseName $CaseName
    $commandText = "AC_UI_AUTOMATION=0; remove AC_TEST_WINDOW_PLACEMENT; $Exe"

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    Set-StartupMessageChildEnvironment -StartInfo $psi

    $proc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $timedOut = -not $proc.WaitForExit(15000)
    $taskkillExitCode = if ($timedOut) {
        Stop-TimedOutProcessTree -Process $proc
    } else {
        $null
    }
    $hasExited = $proc.HasExited
    if ($hasExited) {
        $proc.WaitForExit()
        $stdout = if ($null -eq $stdoutTask.Result) { '' } else { $stdoutTask.Result }
        $stderr = if ($null -eq $stderrTask.Result) { '' } else { $stderrTask.Result }
    } else {
        $stdout = ''
        $stderr = '<capture unavailable; process tree did not exit after taskkill>'
    }
    Write-CaseLogs -Paths $paths -Command $commandText -Stdout $stdout -Stderr $stderr

    $exitCode = if ($hasExited) { $proc.ExitCode } else { $null }
    $case = [pscustomobject]@{
        name = $CaseName
        shellPath = $null
        binaryPath = $Exe
        commandPath = $paths.CommandPath
        stdoutPath = $paths.StdoutPath
        stderrPath = $paths.StderrPath
        exitCode = $exitCode
        timedOut = $timedOut
    }
    $summary.Add($case) | Out-Null

    [pscustomobject]@{
        CaseName = $CaseName
        Stdout = $stdout
        Stderr = $stderr
        StdoutByteCount = [System.Text.Encoding]::UTF8.GetByteCount($stdout)
        ExitCode = $exitCode
        TimedOut = $timedOut
        TaskkillExitCode = $taskkillExitCode
        StdoutPath = $paths.StdoutPath
        StderrPath = $paths.StderrPath
        CommandPath = $paths.CommandPath
    }
}

function Invoke-StartupMessageRedirectedFiles {
    param(
        [Parameter(Mandatory=$true)] [string]$ShellPath,
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [string]$Exe
    )
    $paths = New-CasePaths -CaseName $CaseName
    [System.IO.File]::WriteAllBytes($paths.StdoutPath, [byte[]]@())
    [System.IO.File]::WriteAllBytes($paths.StderrPath, [byte[]]@())

    $escapedExe = $Exe -replace "'", "''"
    $escapedStdout = $paths.StdoutPath -replace "'", "''"
    $escapedStderr = $paths.StderrPath -replace "'", "''"
    $inner = "`$child = Start-Process -FilePath '$escapedExe' -NoNewWindow -RedirectStandardOutput '$escapedStdout' -RedirectStandardError '$escapedStderr' -PassThru -Wait; exit `$child.ExitCode"
    $arguments = "-NonInteractive -NoProfile -Command `"$inner`""
    $commandText = "$ShellPath $arguments"
    Set-Content -LiteralPath $paths.CommandPath -Value $commandText -Encoding UTF8

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ShellPath
    $psi.Arguments = $arguments
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    Set-StartupMessageChildEnvironment -StartInfo $psi

    $proc = [System.Diagnostics.Process]::Start($psi)
    $shellStdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $shellStderrTask = $proc.StandardError.ReadToEndAsync()
    $timedOut = -not $proc.WaitForExit(15000)
    $taskkillExitCode = if ($timedOut) {
        Stop-TimedOutProcessTree -Process $proc
    } else {
        $null
    }
    $hasExited = $proc.HasExited
    if ($hasExited) {
        $proc.WaitForExit()
        $shellStdout = if ($null -eq $shellStdoutTask.Result) { '' } else { $shellStdoutTask.Result }
        $shellStderr = if ($null -eq $shellStderrTask.Result) { '' } else { $shellStderrTask.Result }
    } else {
        $shellStdout = ''
        $shellStderr = '<capture unavailable; process tree did not exit after taskkill>'
    }
    if (-not [string]::IsNullOrEmpty($shellStdout) -or -not [string]::IsNullOrEmpty($shellStderr)) {
        Add-Content -LiteralPath $paths.CommandPath -Value "`n--- child shell stdout ---`n$shellStdout`n--- child shell stderr ---`n$shellStderr" -Encoding UTF8
    }

    $stdoutBytes = [System.IO.File]::ReadAllBytes($paths.StdoutPath)
    $stderrBytes = [System.IO.File]::ReadAllBytes($paths.StderrPath)
    $stdout = [System.Text.Encoding]::UTF8.GetString($stdoutBytes)
    $stderr = [System.Text.Encoding]::UTF8.GetString($stderrBytes)
    $exitCode = if ($hasExited) { $proc.ExitCode } else { $null }
    $case = [pscustomobject]@{
        name = $CaseName
        shellPath = $ShellPath
        binaryPath = $Exe
        commandPath = $paths.CommandPath
        stdoutPath = $paths.StdoutPath
        stderrPath = $paths.StderrPath
        exitCode = $exitCode
        timedOut = $timedOut
    }
    $summary.Add($case) | Out-Null

    [pscustomobject]@{
        CaseName = $CaseName
        Stdout = $stdout
        Stderr = $stderr
        StdoutByteCount = $stdoutBytes.Length
        ExitCode = $exitCode
        TimedOut = $timedOut
        TaskkillExitCode = $taskkillExitCode
        StdoutPath = $paths.StdoutPath
        StderrPath = $paths.StderrPath
        CommandPath = $paths.CommandPath
    }
}

function Assert-StartupMessageResult {
    param(
        [Parameter(Mandatory=$true)] [object]$Result
    )
    $expectedStderr = "An AgentsCommander instance with this executable identity is already running.`n`nRename this executable to agentscommander_<name>.exe to start an independent instance with its own configuration directory and ports.`n"
    $normalizedStderr = $Result.Stderr.Replace("`r`n", "`n").Replace("`r", "`n")

    Assert-True "$($Result.CaseName) completes within 15 seconds" (-not $Result.TimedOut) "process timed out; taskkill exit code=$($Result.TaskkillExitCode)" $Result.CaseName $Result
    Assert-True "$($Result.CaseName) exits 0" ($Result.ExitCode -eq 0) "exit=$($Result.ExitCode)" $Result.CaseName $Result
    Assert-True "$($Result.CaseName) stdout byte-empty" ($Result.StdoutByteCount -eq 0) "stdout byte count=$($Result.StdoutByteCount)" $Result.CaseName $Result
    Assert-True "$($Result.CaseName) exact normalized stderr" ($normalizedStderr -ceq $expectedStderr) "stderr did not match the exact startup message" $Result.CaseName $Result
}

function New-FailureDetail {
    param(
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [string]$Detail,
        [Parameter(Mandatory=$true)] [object]$Result
    )
    "binary='$BinaryPath'; shell='$ShellPath'; case='$CaseName'; commandLog='$($Result.CommandPath)'; stdoutLog='$($Result.StdoutPath)'; stderrLog='$($Result.StderrPath)'; detail=$Detail"
}

function Assert-True {
    param(
        [Parameter(Mandatory=$true)] [string]$Name,
        [Parameter(Mandatory=$true)] [bool]$Cond,
        [Parameter(Mandatory=$true)] [string]$Detail,
        [Parameter(Mandatory=$true)] [string]$CaseName,
        [Parameter(Mandatory=$true)] [object]$Result
    )
    if ($Cond) {
        Write-Host "PASS: $Name" -ForegroundColor Green
    } else {
        Write-Host "FAIL: $Name -- $(New-FailureDetail -CaseName $CaseName -Detail $Detail -Result $Result)" -ForegroundColor Red
        $script:failed++
    }
}

# Root help: direct, noninteractive, and machine-clean for every wrapper binary/shell pair.
$r0 = Invoke-PSNonInteractiveDirect -ShellPath $ShellPath -CaseName "00-root-help-direct" -Exe $BinaryPath -ExeArgs @('--help')
Assert-True "root --help exits zero" ($r0.ExitCode -eq 0) "exit code was $($r0.ExitCode), expected 0" $r0.CaseName $r0
Assert-True "root --help stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r0.Stdout)) "stdout was empty" $r0.CaseName $r0
Assert-True "root --help lists terminal-snapshot" ($r0.Stdout -match 'terminal-snapshot') "stdout missing terminal-snapshot command" $r0.CaseName $r0
Assert-True "root --help stderr empty" ([string]::IsNullOrWhiteSpace($r0.Stderr)) "stderr leaked content; inspect stderr log" $r0.CaseName $r0

# Terminal snapshot help must exercise only Clap discovery, never a live capture.
$r0Snapshot = Invoke-PSNonInteractiveDirect -ShellPath $ShellPath -CaseName "00-terminal-snapshot-help-direct" -Exe $BinaryPath -ExeArgs @('terminal-snapshot', '--help')
Assert-True "terminal-snapshot --help exits zero" ($r0Snapshot.ExitCode -eq 0) "exit code was $($r0Snapshot.ExitCode), expected 0" $r0Snapshot.CaseName $r0Snapshot
Assert-True "terminal-snapshot --help stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r0Snapshot.Stdout)) "stdout was empty" $r0Snapshot.CaseName $r0Snapshot
Assert-True "terminal-snapshot --help shows required syntax" ($r0Snapshot.Stdout -match '--token' -and $r0Snapshot.Stdout -match '--root' -and $r0Snapshot.Stdout -match '--to') "stdout missing required --token/--root/--to syntax" $r0Snapshot.CaseName $r0Snapshot
Assert-True "terminal-snapshot --help shows format and output syntax" ($r0Snapshot.Stdout -match '--format' -and $r0Snapshot.Stdout -match '--output' -and $r0Snapshot.Stdout -match '--timeout') "stdout missing --format/--output/--timeout syntax" $r0Snapshot.CaseName $r0Snapshot
Assert-True "terminal-snapshot --help shows discovery command" ($r0Snapshot.Stdout -match 'list-peers-lean' -and $r0Snapshot.Stdout -match '--snapshot-targets') "stdout missing snapshot target discovery syntax" $r0Snapshot.CaseName $r0Snapshot
Assert-True "terminal-snapshot --help stderr empty" ([string]::IsNullOrWhiteSpace($r0Snapshot.Stderr)) "stderr leaked content; inspect stderr log" $r0Snapshot.CaseName $r0Snapshot

# Post-parse semantic failures must remain one fixed machine line in either shell.
$snapshotTokenCanary = 'ACSNAP_PS_TOKEN_1173_P5Q1'
$snapshotRootCanary = Join-Path $Root 'ACSNAP_PS_CALLER_PATH_1173_P5Q1'
$snapshotTargetCanary = 'project:wg-1-team/acsnap-ps-target-p5q1'
$r0SnapshotFailure = Invoke-PSNonInteractiveDirect -ShellPath $ShellPath -CaseName "00-terminal-snapshot-fixed-failure-direct" -Exe $BinaryPath -ExeArgs @('terminal-snapshot', '--token', $snapshotTokenCanary, '--root', $snapshotRootCanary, '--to', $snapshotTargetCanary, '--timeout', '4')
$normalizedSnapshotStderr = $r0SnapshotFailure.Stderr -replace "`r`n", "`n"
$expectedSnapshotStderr = "terminal_snapshot_error code=invalid_request detail=The terminal snapshot request is invalid.`n"
Assert-True "terminal-snapshot semantic failure stdout empty" ($r0SnapshotFailure.Stdout.Length -eq 0) "stdout was not byte-empty" $r0SnapshotFailure.CaseName $r0SnapshotFailure
Assert-True "terminal-snapshot semantic failure stderr exact" ($normalizedSnapshotStderr -ceq $expectedSnapshotStderr) "stderr did not match the fixed one-line contract" $r0SnapshotFailure.CaseName $r0SnapshotFailure
Assert-True "terminal-snapshot semantic failure hides token" (-not $r0SnapshotFailure.Stderr.Contains($snapshotTokenCanary)) "stderr reflected the token canary" $r0SnapshotFailure.CaseName $r0SnapshotFailure
Assert-True "terminal-snapshot semantic failure hides path and target" (-not $r0SnapshotFailure.Stderr.Contains($snapshotRootCanary) -and -not $r0SnapshotFailure.Stderr.Contains($snapshotTargetCanary)) "stderr reflected caller input" $r0SnapshotFailure.CaseName $r0SnapshotFailure

# Same invocation, no shell in between, because the exit code is a property of the binary and
# a GUI-subsystem release build cannot report one through PowerShell's `&`. See
# Invoke-BinaryDirect above for why this is not folded back into the wrapped case.
$r0SnapshotExit = Invoke-BinaryDirect -CaseName "00-terminal-snapshot-fixed-failure-exit-code" -Exe $BinaryPath -ExeArgs @('terminal-snapshot', '--token', $snapshotTokenCanary, '--root', $snapshotRootCanary, '--to', $snapshotTargetCanary, '--timeout', '4')
Assert-True "terminal-snapshot semantic failure exits one" ($r0SnapshotExit.ExitCode -eq 1) "exit code was $($r0SnapshotExit.ExitCode), expected 1" $r0SnapshotExit.CaseName $r0SnapshotExit
Assert-True "terminal-snapshot semantic failure stderr exact without a shell" ((($r0SnapshotExit.Stderr -replace "`r`n", "`n")) -ceq $expectedSnapshotStderr) "stderr did not match the fixed one-line contract" $r0SnapshotExit.CaseName $r0SnapshotExit

# Test 1: list-peers stdout must contain JSON, and stderr must be empty.
$r1 = Invoke-PSNonInteractiveDirect -ShellPath $ShellPath -CaseName "01-list-peers-direct" -Exe $BinaryPath -ExeArgs @('list-peers', '--token', $Token, '--root', $Root)
Assert-True "list-peers stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r1.Stdout)) "stdout was empty (issue #129 not fixed)" $r1.CaseName $r1
Assert-True "list-peers stderr empty" ([string]::IsNullOrWhiteSpace($r1.Stderr)) "stderr leaked content; inspect stderr log" $r1.CaseName $r1
if (-not [string]::IsNullOrWhiteSpace($r1.Stdout)) {
    try {
        $parsed = $r1.Stdout | ConvertFrom-Json -ErrorAction Stop
        $trimmedJson = $r1.Stdout.Trim()
        # PowerShell 7 writes no pipeline object for a valid empty JSON array.
        if ($null -eq $parsed -and $trimmedJson -ne '[]') {
            Write-Host "FAIL: list-peers ConvertFrom-Json returned null -- $(New-FailureDetail -CaseName $r1.CaseName -Detail 'non-empty non-array stdout parsed to null' -Result $r1)" -ForegroundColor Red
            $failed++
        } else {
            Write-Host "PASS: list-peers stdout parses as JSON" -ForegroundColor Green
        }
    } catch {
        Write-Host "FAIL: list-peers stdout not valid JSON -- $(New-FailureDetail -CaseName $r1.CaseName -Detail $_.Exception.Message -Result $r1)" -ForegroundColor Red
        $failed++
    }
}

# Test 1b (#1596): same list-peers through the Git Bash carrier. Here the exit
# code IS observable — bash.exe is console-subsystem, so $LASTEXITCODE
# propagates out of the outer PowerShell — which the direct GUI-child case
# above intentionally cannot assert. Missing bash.exe is an accepted SKIP.
$r1b = Invoke-BashRouted -ShellPath $ShellPath -CaseName "01-list-peers-via-git-bash" -Exe $BinaryPath -ExeArgs @('list-peers', '--token', $Token, '--root', $Root)
if (-not $r1b.Skipped) {
    Assert-True "list-peers via Git Bash stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r1b.Stdout)) "stdout was empty via the Git Bash carrier (#1596)" $r1b.CaseName $r1b
    Assert-True "list-peers via Git Bash stderr empty" ([string]::IsNullOrWhiteSpace($r1b.Stderr)) "stderr leaked content; inspect stderr log" $r1b.CaseName $r1b
    if (-not [string]::IsNullOrWhiteSpace($r1b.Stdout)) {
        try {
            $parsed = $r1b.Stdout | ConvertFrom-Json -ErrorAction Stop
            $trimmedJson = $r1b.Stdout.Trim()
            # PowerShell 7 writes no pipeline object for a valid empty JSON array.
            if ($null -eq $parsed -and $trimmedJson -ne '[]') {
                Write-Host "FAIL: list-peers via Git Bash ConvertFrom-Json returned null -- $(New-FailureDetail -CaseName $r1b.CaseName -Detail 'non-empty non-array stdout parsed to null' -Result $r1b)" -ForegroundColor Red
                $failed++
            } else {
                Write-Host "PASS: list-peers via Git Bash stdout parses as JSON" -ForegroundColor Green
            }
        } catch {
            Write-Host "FAIL: list-peers via Git Bash stdout not valid JSON -- $(New-FailureDetail -CaseName $r1b.CaseName -Detail $_.Exception.Message -Result $r1b)" -ForegroundColor Red
            $failed++
        }
    }
    Assert-True "list-peers via Git Bash exits zero" ($r1b.ExitCode -eq 0) "exit code was $($r1b.ExitCode), expected 0 (bash.exe propagates the AC binary's exit code)" $r1b.CaseName $r1b
}

# Test 2: send --help stdout must contain clap-rendered help text.
$r2 = Invoke-PSNonInteractiveDirect -ShellPath $ShellPath -CaseName "02-send-help-direct" -Exe $BinaryPath -ExeArgs @('send', '--help')
Assert-True "send --help stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r2.Stdout)) "stdout was empty (issue #129 not fixed for help path)" $r2.CaseName $r2
Assert-True "send --help mentions --to flag" ($r2.Stdout -match '--to') "stdout missing expected flag mention" $r2.CaseName $r2

# Test 3: send unknown flag stderr must contain clap usage error.
$r3 = Invoke-PSNonInteractiveDirect -ShellPath $ShellPath -CaseName "03-send-unknown-flag-direct" -Exe $BinaryPath -ExeArgs @('send', '--bogus-flag-xyz')
Assert-True "send unknown flag stderr non-empty" (-not [string]::IsNullOrWhiteSpace($r3.Stderr)) "stderr was empty (issue #129 not fixed for clap-error path)" $r3.CaseName $r3

# Test 4: pipeline mode must still produce JSON when stderr is merged.
$escapedBin = $BinaryPath -replace "'", "''"
$escapedRoot = $Root -replace "'", "''"
$escapedToken = $Token -replace "'", "''"
$inner = "& '$escapedBin' list-peers --token '$escapedToken' --root '$escapedRoot' 2>&1 | Out-String"
$arguments = "-NonInteractive -NoProfile -Command `"$inner`""
$paths4 = New-CasePaths -CaseName "04-list-peers-merged-pipeline"
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $ShellPath
$psi.Arguments = $arguments
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$mergedTask = $proc.StandardOutput.ReadToEndAsync()
$stderrTask = $proc.StandardError.ReadToEndAsync()
$proc.WaitForExit()
$mergedOut = if ($null -eq $mergedTask.Result) { '' } else { $mergedTask.Result }
$test4Stderr = if ($null -eq $stderrTask.Result) { '' } else { $stderrTask.Result }
Write-CaseLogs -Paths $paths4 -Command "$ShellPath $arguments" -Stdout $mergedOut -Stderr $test4Stderr
$r4 = [pscustomobject]@{
    CaseName = "04-list-peers-merged-pipeline"
    Stdout = $mergedOut
    Stderr = $test4Stderr
    ExitCode = $proc.ExitCode
    StdoutPath = $paths4.StdoutPath
    StderrPath = $paths4.StderrPath
    CommandPath = $paths4.CommandPath
}
$summary.Add([pscustomobject]@{
    name = $r4.CaseName
    shellPath = $ShellPath
    binaryPath = $BinaryPath
    commandPath = $r4.CommandPath
    stdoutPath = $r4.StdoutPath
    stderrPath = $r4.StderrPath
    exitCode = $r4.ExitCode
}) | Out-Null

if ([string]::IsNullOrWhiteSpace($mergedOut)) {
    Write-Host "FAIL: Test 4 merged output is empty -- $(New-FailureDetail -CaseName $r4.CaseName -Detail 'inner command may have failed or produced no output' -Result $r4)" -ForegroundColor Red
    $failed++
} else {
    try {
        $parsed = $mergedOut | ConvertFrom-Json -ErrorAction Stop
        $trimmedMergedJson = $mergedOut.Trim()
        # PowerShell 7 writes no pipeline object for a valid empty JSON array.
        if ($null -eq $parsed -and $trimmedMergedJson -ne '[]') {
            Write-Host "FAIL: Test 4 ConvertFrom-Json returned null -- $(New-FailureDetail -CaseName $r4.CaseName -Detail 'non-empty non-array merged output parsed to null' -Result $r4)" -ForegroundColor Red
            $failed++
        } else {
            Write-Host "PASS: 2>&1 | ConvertFrom-Json continues to work" -ForegroundColor Green
        }
    } catch {
        Write-Host "FAIL: 2>&1 | ConvertFrom-Json broken -- $(New-FailureDetail -CaseName $r4.CaseName -Detail $_.Exception.Message -Result $r4)" -ForegroundColor Red
        $failed++
    }
}

if ([System.IO.Path]::GetFileName($BinaryPath) -ieq 'agentscommander.exe') {
    $createdNew = $false
    $instanceMutex = New-Object System.Threading.Mutex($true, 'Local\AgentsCommander_SingleInstance', [ref]$createdNew)
    try {
        if (-not $createdNew) {
            throw "Startup-message smoke did not create Local\AgentsCommander_SingleInstance"
        }

        $r5 = Invoke-StartupMessagePiped -CaseName '05-single-instance-piped' -Exe $BinaryPath
        Assert-StartupMessageResult -Result $r5

        $r6 = Invoke-StartupMessageRedirectedFiles -ShellPath $ShellPath -CaseName '06-single-instance-redirected-files' -Exe $BinaryPath
        Assert-StartupMessageResult -Result $r6
    } finally {
        if ($null -ne $instanceMutex) {
            if ($createdNew) {
                $instanceMutex.ReleaseMutex()
            }
            $instanceMutex.Dispose()
        }
    }
}

$summaryPath = Join-Path $LogDir "summary.json"
[pscustomobject]@{
    binaryPath = $BinaryPath
    shell = $Shell
    shellPath = $ShellPath
    root = $Root
    failed = $failed
    cases = @($summary.ToArray())
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding UTF8
Write-Host "Smoke log summary: $summaryPath"

if ($failed -gt 0) {
    Write-Host "`n$failed check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host "`nAll checks passed" -ForegroundColor Green
exit 0
