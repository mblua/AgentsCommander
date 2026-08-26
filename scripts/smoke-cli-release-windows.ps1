[CmdletBinding()]
param(
    [string]$LogDir = "artifacts\cli-release-smoke",
    [string]$Token = "00000000-0000-0000-0000-000000000000",
    [string]$Root = (Join-Path $env:TEMP "ac-release-smoke-$([guid]::NewGuid().ToString('N'))"),
    [string]$OrdinaryTargetDir = "target\1539-release-ordinary",
    [string]$TestableTargetDir = "target\1539-release-testable",
    [string]$StageDir = "target\1539-release-staged",
    [switch]$SkipBuild
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
foreach ($name in @("OrdinaryTargetDir", "TestableTargetDir", "StageDir")) {
    $value = Get-Variable -Name $name -ValueOnly
    if (-not [System.IO.Path]::IsPathRooted($value)) {
        Set-Variable -Name $name -Value (Join-Path $repoRoot $value)
    }
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

function Resolve-TauriBinary {
    param([Parameter(Mandatory=$true)] [string]$TargetDir)
    foreach ($name in @("agentscommander.exe", "agentscommander-new.exe")) {
        $candidate = Join-Path $TargetDir (Join-Path "release" $name)
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw "Tauri release binary was not found below $TargetDir"
}

function Invoke-IsolatedTauriBuild {
    param(
        [Parameter(Mandatory=$true)] [string]$TargetDir,
        [switch]$Testable
    )
    $npm = Get-Command "npm.cmd" -ErrorAction Stop
    $previousTarget = $env:CARGO_TARGET_DIR
    $previousProfile = $env:BUILD_PROFILE
    Push-Location $repoRoot
    try {
        $env:CARGO_TARGET_DIR = $TargetDir
        $env:BUILD_PROFILE = "prod"
        $arguments = @(
            "exec", "tauri", "--", "build", "--no-bundle", "--ci",
            "--config", "src-tauri/tauri.prod.conf.json"
        )
        if ($Testable) {
            $arguments += @("--features", "testable-ui-automation")
        }
        & $npm.Source @arguments
        if ($LASTEXITCODE -ne 0) {
            throw "Tauri build failed with exit code $LASTEXITCODE (testable=$Testable)"
        }
    } finally {
        if ($null -eq $previousTarget) {
            Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        } else {
            $env:CARGO_TARGET_DIR = $previousTarget
        }
        if ($null -eq $previousProfile) {
            Remove-Item Env:\BUILD_PROFILE -ErrorAction SilentlyContinue
        } else {
            $env:BUILD_PROFILE = $previousProfile
        }
        Pop-Location
    }
}

function Invoke-UiRefusalCase {
    param(
        [Parameter(Mandatory=$true)] [string]$BinaryPath,
        [Parameter(Mandatory=$true)] [string]$CaseName
    )
    $caseDir = Join-Path $LogDir $CaseName
    New-Item -ItemType Directory -Force -Path $caseDir | Out-Null
    $stdoutPath = Join-Path $caseDir "stdout.txt"
    $stderrPath = Join-Path $caseDir "stderr.txt"
    $process = Start-Process -FilePath $BinaryPath `
        -ArgumentList @("ui-capabilities") `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -NoNewWindow -Wait -PassThru
    $stdout = [System.IO.File]::ReadAllText($stdoutPath)
    $stderr = [System.IO.File]::ReadAllText($stderrPath)
    $expected = '{"ok":false,"error":"refusing_non_testeable_binary","message":"UI automation is only available from agentscommander_testeable.exe."}' + "`n"
    if ($process.ExitCode -ne 1 -or $stdout -cne $expected -or $stderr.Length -ne 0) {
        throw "$CaseName failed exact refusal framing (exit=$($process.ExitCode), stdoutBytes=$([Text.Encoding]::UTF8.GetByteCount($stdout)), stderrBytes=$([Text.Encoding]::UTF8.GetByteCount($stderr)))"
    }
}

if (-not $SkipBuild) {
    Invoke-IsolatedTauriBuild -TargetDir $OrdinaryTargetDir
    Invoke-IsolatedTauriBuild -TargetDir $TestableTargetDir -Testable
}

New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
$ordinaryStage = Join-Path $StageDir "ordinary"
$testableStage = Join-Path $StageDir "testable"
New-Item -ItemType Directory -Force -Path $ordinaryStage | Out-Null
New-Item -ItemType Directory -Force -Path $testableStage | Out-Null

try {
    $ordinarySource = Resolve-TauriBinary -TargetDir $OrdinaryTargetDir
    $testableSource = Resolve-TauriBinary -TargetDir $TestableTargetDir
} catch {
    Write-Host "FAIL: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

$ordinaryBinary = Join-Path $ordinaryStage "agentscommander.exe"
$renamedOrdinaryBinary = Join-Path $ordinaryStage "agentscommander_testeable.exe"
$testableBinary = Join-Path $testableStage "agentscommander_testeable.exe"
$wrongNameTestableBinary = Join-Path $testableStage "agentscommander-feature-wrong-name.exe"
Copy-Item -LiteralPath $ordinarySource -Destination $ordinaryBinary -Force
Copy-Item -LiteralPath $ordinarySource -Destination $renamedOrdinaryBinary -Force
Copy-Item -LiteralPath $testableSource -Destination $testableBinary -Force
Copy-Item -LiteralPath $testableSource -Destination $wrongNameTestableBinary -Force

try {
    Invoke-UiRefusalCase -BinaryPath $renamedOrdinaryBinary -CaseName "renamed-ordinary-refusal"
    Invoke-UiRefusalCase -BinaryPath $wrongNameTestableBinary -CaseName "feature-wrong-name-refusal"
} catch {
    Write-Host "FAIL: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

$binaries = @(
    $ordinaryBinary,
    $testableBinary
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
