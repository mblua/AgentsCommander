[CmdletBinding()]
param(
    [string]$LogDir = "artifacts\cli-release-smoke",
    [string]$Token = "00000000-0000-0000-0000-000000000000",
    [string]$Root = (Join-Path $env:TEMP "ac-release-smoke-$([guid]::NewGuid().ToString('N'))")
)

$ErrorActionPreference = "Continue"

$scriptDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptDir
if (-not [System.IO.Path]::IsPathRooted($LogDir)) {
    $LogDir = Join-Path $repoRoot $LogDir
}
if (-not [System.IO.Path]::IsPathRooted($Root)) {
    $Root = Join-Path $repoRoot $Root
}

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

if (-not $IsWindows -and $env:OS -ne "Windows_NT") {
    Write-Host "SKIP: Windows release CLI smoke only runs on Windows" -ForegroundColor Yellow
    [pscustomobject]@{
        status = "skipped"
        reason = "not-windows"
        logDir = $LogDir
        invocations = @()
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $LogDir "summary.json") -Encoding UTF8
    exit 0
}

function Convert-ToSafeName {
    param([Parameter(Mandatory=$true)] [string]$Name)
    $Name -replace '[^A-Za-z0-9_.-]', '_'
}

$releaseDir = Join-Path $repoRoot "src-tauri\target\release"
$binaries = @(
    (Join-Path $releaseDir "agentscommander.exe"),
    (Join-Path $releaseDir "agentscommander_testeable.exe")
)
$shells = @("powershell.exe", "pwsh.exe")
$results = New-Object System.Collections.Generic.List[object]
$failed = 0
$passed = 0
$skipped = 0

$missingBinaries = @($binaries | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) })
if ($missingBinaries.Count -gt 0) {
    foreach ($binary in $missingBinaries) {
        Write-Host "FAIL: expected release smoke binary missing: $binary" -ForegroundColor Red
        Write-Host "Run npm run build:prod before smoke:cli-release-windows." -ForegroundColor Red
        $results.Add([pscustomobject]@{
            binaryPath = $binary
            shell = $null
            status = "failed"
            exitCode = $null
            root = $null
            logDir = $null
            reason = "missing-binary"
        }) | Out-Null
    }
    [pscustomobject]@{
        passed = 0
        skipped = 0
        failed = $missingBinaries.Count
        logDir = $LogDir
        invocations = @($results.ToArray())
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $LogDir "summary.json") -Encoding UTF8
    exit 1
}

foreach ($binary in $binaries) {
    $binaryStem = Convert-ToSafeName -Name ([System.IO.Path]::GetFileNameWithoutExtension($binary))
    foreach ($shell in $shells) {
        $shellCmd = Get-Command $shell -ErrorAction SilentlyContinue
        $shellStem = Convert-ToSafeName -Name ([System.IO.Path]::GetFileNameWithoutExtension($shell))
        $caseLogDir = Join-Path $LogDir (Join-Path $binaryStem $shellStem)
        $caseRoot = Join-Path $Root (Join-Path $binaryStem $shellStem)
        New-Item -ItemType Directory -Force -Path $caseLogDir | Out-Null
        New-Item -ItemType Directory -Force -Path $caseRoot | Out-Null

        if ($null -eq $shellCmd) {
            if ($shell -ieq "pwsh.exe") {
                Write-Host "SKIP: optional shell not found: $shell" -ForegroundColor Yellow
                $skipped++
                $results.Add([pscustomobject]@{
                    binaryPath = $binary
                    shell = $shell
                    status = "skipped"
                    exitCode = $null
                    root = $caseRoot
                    logDir = $caseLogDir
                    reason = "optional-shell-missing"
                }) | Out-Null
                continue
            }

            Write-Host "FAIL: required shell not found: $shell" -ForegroundColor Red
            $failed++
            $results.Add([pscustomobject]@{
                binaryPath = $binary
                shell = $shell
                status = "failed"
                exitCode = $null
                root = $caseRoot
                logDir = $caseLogDir
                reason = "required-shell-missing"
            }) | Out-Null
            continue
        }

        & (Join-Path $scriptDir "smoke-cli-powershell.ps1") `
            -BinaryPath $binary `
            -Shell $shell `
            -Token $Token `
            -Root $caseRoot `
            -LogDir $caseLogDir
        $exitCode = $LASTEXITCODE

        if ($exitCode -eq 0) {
            Write-Host "PASS: $binary under $shell" -ForegroundColor Green
            $passed++
            $status = "passed"
        } else {
            Write-Host "FAIL: $binary under $shell exited $exitCode" -ForegroundColor Red
            $failed++
            $status = "failed"
        }

        $results.Add([pscustomobject]@{
            binaryPath = $binary
            shell = $shell
            shellPath = $shellCmd.Source
            status = $status
            exitCode = $exitCode
            root = $caseRoot
            logDir = $caseLogDir
            reason = $null
        }) | Out-Null
    }
}

$summaryPath = Join-Path $LogDir "summary.json"
[pscustomobject]@{
    passed = $passed
    skipped = $skipped
    failed = $failed
    logDir = $LogDir
    invocations = @($results.ToArray())
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Host "Windows release CLI smoke summary: passed=$passed skipped=$skipped failed=$failed logs=$LogDir"
Write-Host "Wrapper summary: $summaryPath"

if ($failed -gt 0) {
    exit 1
}
exit 0
