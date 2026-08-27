# Smoke test for the Windows portable zip asset, for issue #1589.
#
# Proves the three things the asset promises and that CI cannot otherwise see:
#   1. The zip carries the expected files, and the binary is named
#      agentscommander.exe (NOT the published raw name, which parses as the
#      instance suffix "64" and silently changes config dir, mutex, and ports).
#   2. The binary in the zip is the build for this version.
#   3. Running it creates its instance directory NEXT TO the executable, under
#      the canonical name, which is the whole portable contract.
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

if (-not (Test-Path -LiteralPath $Zip)) {
    Write-Error "[portable-smoke] zip not found: $Zip"
    exit 1
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
foreach ($name in $expected) {
    Assert-That ($actual -contains $name) "$name is in the zip"
}
$unexpected = $actual | Where-Object { $expected -notcontains $_ }
Assert-That ($unexpected.Count -eq 0) "no unexpected entries (found: $($unexpected -join ', '))"

$exe = Join-Path $work 'agentscommander.exe'

Write-Host ""
Write-Host "Build identity"
if (Test-Path -LiteralPath $exe) {
    $productVersion = (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    Assert-That (-not [string]::IsNullOrWhiteSpace($productVersion)) "the exe carries Windows version info"
    if (-not [string]::IsNullOrWhiteSpace($productVersion)) {
        # Windows pads to four components; compare the SemVer triple only.
        $triple = ($productVersion.Trim().Split('.')[0..2]) -join '.'
        Assert-That ($triple -eq $ExpectedVersion) "exe ProductVersion $productVersion matches $ExpectedVersion"
    }
}

$readme = Join-Path $work 'PORTABLE.txt'
if (Test-Path -LiteralPath $readme) {
    $text = Get-Content -LiteralPath $readme -Raw
    Assert-That ($text -match [regex]::Escape($ExpectedVersion)) "PORTABLE.txt names version $ExpectedVersion"
    Assert-That (-not ($text -match '\{\{')) "PORTABLE.txt has no unresolved placeholder"
}

Write-Host ""
Write-Host "Portable instance contract"
if (Test-Path -LiteralPath $exe) {
    $stdout = Join-Path $work 'smoke-stdout.txt'
    $stderr = Join-Path $work 'smoke-stderr.txt'

    # Start-Process -Wait, never `& $exe`: this is a GUI-subsystem binary, so the
    # call operator returns immediately and leaves $LASTEXITCODE stale.
    # list-sessions needs no token and no running GUI; it reads the instance
    # config directory directly, which is exactly the resolution under test.
    $proc = $null
    try {
        $proc = Start-Process -FilePath $exe -ArgumentList 'list-sessions' -Wait -PassThru -NoNewWindow `
            -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    } catch {
        # A binary that cannot even be launched is a packaging failure, not a
        # script crash: report it as one failed assertion and keep going.
        Assert-That $false "the packed exe could not be launched: $($_.Exception.Message)"
    }
    if ($proc) {
        Assert-That ($proc.ExitCode -eq 0) "list-sessions exited 0 (got $($proc.ExitCode))"
    }

    $instanceDir = Join-Path $work '.agentscommander'
    Assert-That (Test-Path -LiteralPath $instanceDir) "created .agentscommander next to the exe"
    Assert-That (Test-Path -LiteralPath (Join-Path $instanceDir 'app.log')) "wrote app.log inside it"

    # The failure this asset exists to prevent: any other instance directory
    # means the binary resolved a non-canonical identity from its own file name.
    $strayDirs = Get-ChildItem -LiteralPath $work -Directory -Force |
        Where-Object { $_.Name -like '.agentscommander*' -and $_.Name -ne '.agentscommander' }
    Assert-That ($strayDirs.Count -eq 0) "no suffixed instance directory (found: $($strayDirs.Name -join ', '))"

    if (Test-Path -LiteralPath $stderr) {
        # -Raw yields $null for an empty file, so trim only after the null check:
        # calling .Trim() on it would terminate the run before the summary below.
        $errText = Get-Content -LiteralPath $stderr -Raw
        if (-not [string]::IsNullOrWhiteSpace($errText)) {
            Write-Host "  note  stderr: $($errText.Trim())"
        }
    }
}

Write-Host ""
if ($failures.Count -gt 0) {
    Write-Host "[portable-smoke] FAILED with $($failures.Count) assertion(s):" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "[portable-smoke] OK: the portable zip is complete and resolves the canonical instance identity."
exit 0
