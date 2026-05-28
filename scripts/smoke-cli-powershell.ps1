[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)]
    [string]$BinaryPath,
    [string]$Token = "00000000-0000-0000-0000-000000000000",
    [string]$Root = (New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP "ac-smoke-$([guid]::NewGuid().ToString('N'))")).FullName
)

$ErrorActionPreference = "Continue"
$failed = 0

# Bug-reproducing harness: spawn a fresh `powershell.exe -NonInteractive -NoProfile`
# and run the AC exe via `&` direct call (no pipeline operators in the inner command).
# This is the exact shape from verify-prod-binary.ps1 and R2.6's Rust test, and it is
# the ONLY shape that reproduces issue #129's failure mode.
#
# Note on exit codes: PS-NonInteractive bare `&` does NOT propagate $LASTEXITCODE for
# GUI-subsystem children (PE Subsystem=2 - empirically verified in Round 3, R3.G.3).
# The outer process always sees ExitCode=0 regardless of the AC binary's true exit
# code. This is the same bare-`&`-vs-pipeline asymmetry underlying issue #129 itself,
# and it cannot be worked around in this harness shape (the harness shape is mandatory
# to reproduce the bug). Tests 1-3 therefore drop exit-code assertions and rely on
# stdout/stderr presence - those are the bug-relevant signals (Round 4, Option 1).
function Invoke-PSNonInteractiveDirect {
    param(
        [Parameter(Mandatory=$true)] [string]$Exe,
        [Parameter(Mandatory=$true)] [string[]]$ExeArgs
    )
    $escapedExe = $Exe -replace "'", "''"
    $quotedArgs = ($ExeArgs | ForEach-Object {
        "'" + ($_ -replace "'", "''") + "'"
    }) -join ' '
    $inner = "& '$escapedExe' $quotedArgs"

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = 'powershell.exe'
    $psi.Arguments = "-NonInteractive -NoProfile -Command `"$inner`""
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()

    [pscustomobject]@{
        Stdout = if ($null -eq $stdoutTask.Result) { '' } else { $stdoutTask.Result }
        Stderr = if ($null -eq $stderrTask.Result) { '' } else { $stderrTask.Result }
    }
}

function Assert-True([string]$Name, [bool]$Cond, [string]$Detail) {
    if ($Cond) {
        Write-Host "PASS: $Name" -ForegroundColor Green
    } else {
        Write-Host "FAIL: $Name -- $Detail" -ForegroundColor Red
        $script:failed++
    }
}

# Test 1: list-peers -- stdout has JSON, stderr is empty (post-fix contract).
# Failure mode on unfixed binary (per R2.1 / R3.2): AC binary inherits valid PIPE
# stdout from PS-NonInteractive's `&` direct call; the unfixed `attach_parent_console`
# unconditionally calls AttachConsole, which rebinds STD_OUTPUT_HANDLE to PS's hidden
# console buffer (PIPE -> CHAR); PS-NonInteractive does not surface that buffer to
# its captured stdout pipe; captured stdout is empty. Test 1's "stdout non-empty"
# assertion fails. Empirically confirmed in verify-prod-binary.ps1 Test 1.
$r1 = Invoke-PSNonInteractiveDirect -Exe $BinaryPath -ExeArgs @('list-peers', '--token', $Token, '--root', $Root)
Assert-True "list-peers stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r1.Stdout)) "stdout was empty (issue #129 not fixed)"
Assert-True "list-peers stderr empty" ([string]::IsNullOrWhiteSpace($r1.Stderr)) "stderr leaked content: $($r1.Stderr)"
# NEW-4 fix: layered guard mirroring Test 4's NEW-2 fix. Empty/whitespace stdout is
# already covered by the prior `stdout non-empty` assertion; only attempt parse when
# stdout has content, and explicitly fail on `$null -eq $parsed` to avoid the silent
# false-PASS that `'' | ConvertFrom-Json -ErrorAction Stop` would otherwise produce.
if (-not [string]::IsNullOrWhiteSpace($r1.Stdout)) {
    try {
        $parsed = $r1.Stdout | ConvertFrom-Json -ErrorAction Stop
        if ($null -eq $parsed) {
            Write-Host "FAIL: list-peers ConvertFrom-Json returned null on non-empty stdout" -ForegroundColor Red
            $failed++
        } else {
            Write-Host "PASS: list-peers stdout parses as JSON" -ForegroundColor Green
        }
    } catch {
        Write-Host "FAIL: list-peers stdout not valid JSON: $($r1.Stdout)" -ForegroundColor Red
        $failed++
    }
}
# else: empty case is already counted as a fail by the prior `stdout non-empty` assertion above

# Test 2: send --help -- stdout has clap-rendered help text.
# Failure mode on unfixed binary: identical to Test 1 - clap writes the help text
# to stdout; AttachConsole rebinds the inherited PIPE stdout to PS's hidden console
# buffer; captured stdout is empty. Test 2's "stdout non-empty" assertion fails.
# Empirically confirmed in verify-prod-binary.ps1 Test 1 (same `send --help` invocation).
$r2 = Invoke-PSNonInteractiveDirect -Exe $BinaryPath -ExeArgs @('send', '--help')
Assert-True "send --help stdout non-empty" (-not [string]::IsNullOrWhiteSpace($r2.Stdout)) "stdout was empty (issue #129 not fixed for --help path)"
Assert-True "send --help mentions --to flag" ($r2.Stdout -match '--to') "stdout missing expected flag mention"

# Test 3: send unknown flag -- stderr has clap usage error.
# Failure mode on unfixed binary: clap writes the parse-error/usage text to stderr.
# The AC binary's STD_ERROR_HANDLE is also inherited as a valid PIPE; the unfixed
# `attach_parent_console` rebinds it to PS's hidden console buffer the same way as
# stdout; captured stderr is empty. Test 3's "stderr non-empty" assertion fails.
# Per R3.G.3 / R3.G.5 (Round 3 grinch verification, integrated in Round 4),
# exit-code propagation is not usable in this harness: PS-NonInteractive bare `&`
# does NOT update $LASTEXITCODE for GUI-subsystem children, so any `exit non-zero`
# assertion would fail even on a correctly fixed binary. `stderr non-empty` is the
# sole - and bug-relevant - signal for this test.
$r3 = Invoke-PSNonInteractiveDirect -Exe $BinaryPath -ExeArgs @('send', '--bogus-flag-xyz')
Assert-True "send unknown flag stderr non-empty" (-not [string]::IsNullOrWhiteSpace($r3.Stderr)) "stderr was empty (issue #129 not fixed for clap-error path)"

# Test 4 (G3 regression check): `2>&1 | ConvertFrom-Json` on list-peers must still work.
# This test deliberately uses a DIFFERENT inner command shape - pipeline mode with
# `2>&1 | Out-String`. Pipeline mode bypasses issue #129 (R2.1 confirmed: pipeline
# operators give the child a PIPE stdout via STARTUPINFO redirection, no NULL → no
# AttachConsole rebind). So Test 4 PASSES on both fixed and unfixed binary as long
# as stderr is empty (which it is post-Step-A). Test 4 FAILS only if a future change
# reintroduces dual-write to stderr - the merged stream would then contain non-JSON
# stderr content, breaking ConvertFrom-Json. That is the regression Test 4 guards.
#
# NEW-2 fix: replace the broken `-replace [char]39, [char]39 + [char]39` (which is a
# PS parser error -- three args to -replace -- that with $ErrorActionPreference="Continue"
# silently produced an empty inner command) with string-literal `-replace "'", "''"`
# (Option B from R2.G.5). Also tighten the assertion: fail on empty merged output
# regardless of ConvertFrom-Json's silent null-on-empty-input behavior.
# NEW-5 fix (Round 4): escape and single-quote-wrap $Token consistently with
# $BinaryPath / $Root. Default Token is a UUID and therefore safe, but a custom
# -Token containing single quotes would otherwise break the inner command.
$escapedBin = $BinaryPath -replace "'", "''"
$escapedRoot = $Root -replace "'", "''"
$escapedToken = $Token -replace "'", "''"
$inner = "& '$escapedBin' list-peers --token '$escapedToken' --root '$escapedRoot' 2>&1 | Out-String"
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = 'powershell.exe'
$psi.Arguments = "-NonInteractive -NoProfile -Command `"$inner`""
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$mergedTask = $proc.StandardOutput.ReadToEndAsync()
$null = $proc.StandardError.ReadToEndAsync()
$proc.WaitForExit()
$mergedOut = if ($null -eq $mergedTask.Result) { '' } else { $mergedTask.Result }

if ([string]::IsNullOrWhiteSpace($mergedOut)) {
    Write-Host "FAIL: Test 4 merged output is empty (inner command may have failed or produced no output)" -ForegroundColor Red
    $failed++
} else {
    try {
        $parsed = $mergedOut | ConvertFrom-Json -ErrorAction Stop
        if ($null -eq $parsed) {
            Write-Host "FAIL: Test 4 ConvertFrom-Json returned null on non-empty merged output" -ForegroundColor Red
            $failed++
        } else {
            Write-Host "PASS: 2>&1 | ConvertFrom-Json continues to work (no dual-write regression)" -ForegroundColor Green
        }
    } catch {
        Write-Host "FAIL: 2>&1 | ConvertFrom-Json broken -- merged stream is not valid JSON: $mergedOut" -ForegroundColor Red
        $failed++
    }
}

if ($failed -gt 0) {
    Write-Host "`n$failed check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host "`nAll checks passed" -ForegroundColor Green
exit 0
