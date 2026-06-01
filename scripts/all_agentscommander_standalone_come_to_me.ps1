#requires -Version 5.0
<#
all_agentscommander_standalone_come_to_me.ps1

Assumptions:
- By default, AgentsCommander_ac is checked out at:
  $env:USERPROFILE\0_repos\AgentsCommander_ac
- Override -AcRoot if the checkout moved.
- Workgroup folders are immediate children of the AgentsCommander_ac .ac directory and match ^wg-\d+.
- The script reads from each workgroup repo and writes only to -Destination.

Destination filename convention:
- <full-workgroup-folder>__<source-exe-name>
- Example: wg-1-dev-team__agentscommander_standalone-wg1.exe
#>

[CmdletBinding()]
param(
    [string]$AcRoot = (Join-Path $env:USERPROFILE '0_repos\AgentsCommander_ac\.ac'),
    [string]$Destination = 'C:\Users\maria\0_mmb\0_AC',
    [switch]$Overwrite
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Resolve-AcRoot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        throw 'AcRoot cannot be empty.'
    }

    $candidate = [System.IO.Path]::GetFullPath($Path)

    if ((Split-Path -Leaf $candidate) -ne '.ac') {
        $nested = Join-Path $candidate '.ac'
        if (Test-Path -LiteralPath $nested -PathType Container) {
            return (Resolve-Path -LiteralPath $nested).ProviderPath
        }
    }

    if (Test-Path -LiteralPath $candidate -PathType Container) {
        return (Resolve-Path -LiteralPath $candidate).ProviderPath
    }

    throw "AgentsCommander_ac .ac directory not found: $candidate"
}

function ConvertTo-SafeFileName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $invalidPattern = '[{0}]' -f [Regex]::Escape((-join [System.IO.Path]::GetInvalidFileNameChars()))
    return ($Name -replace $invalidPattern, '_')
}

$acRootPath = Resolve-AcRoot -Path $AcRoot
$destinationPath = [System.IO.Path]::GetFullPath($Destination)

if (-not (Test-Path -LiteralPath $destinationPath -PathType Container)) {
    New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
}

$stats = @{
    WorkgroupsChecked = 0
    MissingReleaseDirs = 0
    NoMatchingExecutables = 0
    Copied = 0
    SkippedIdentical = 0
    SkippedConflict = 0
    Failed = 0
}

$patterns = @('agentscommander*.exe', 'agentscommander.*.exe')
$workgroups = @(
    Get-ChildItem -LiteralPath $acRootPath -Directory -ErrorAction Stop |
        Where-Object { $_.Name -match '^wg-\d+' } |
        Sort-Object Name
)

Write-Host "AgentsCommander .ac root: $acRootPath"
Write-Host "Destination: $destinationPath"
Write-Host "Overwrite conflicts: $($Overwrite.IsPresent)"
Write-Host ''

foreach ($workgroup in $workgroups) {
    $stats.WorkgroupsChecked++

    $releaseDir = Join-Path $workgroup.FullName 'repo-AgentsCommander\src-tauri\target\release'
    if (-not (Test-Path -LiteralPath $releaseDir -PathType Container)) {
        $stats.MissingReleaseDirs++
        Write-Host "SKIP no release dir: $($workgroup.Name)"
        continue
    }

    $byPath = @{}
    foreach ($pattern in $patterns) {
        Get-ChildItem -LiteralPath $releaseDir -Filter $pattern -File -ErrorAction SilentlyContinue |
            ForEach-Object { $byPath[$_.FullName] = $_ }
    }

    $executables = @($byPath.Values | Sort-Object FullName)
    if ($executables.Count -eq 0) {
        $stats.NoMatchingExecutables++
        Write-Host "SKIP no matching executables: $($workgroup.Name)"
        continue
    }

    foreach ($exe in $executables) {
        $safeWorkgroupName = ConvertTo-SafeFileName -Name $workgroup.Name
        $safeExeName = ConvertTo-SafeFileName -Name $exe.Name
        $destName = '{0}__{1}' -f $safeWorkgroupName, $safeExeName
        $destPath = Join-Path $destinationPath $destName

        try {
            if (Test-Path -LiteralPath $destPath) {
                if (Test-Path -LiteralPath $destPath -PathType Container) {
                    $stats.SkippedConflict++
                    Write-Warning "SKIP conflict, destination is a directory: $destPath"
                    continue
                }

                $sourceHash = (Get-FileHash -LiteralPath $exe.FullName -Algorithm SHA256).Hash
                $existingHash = (Get-FileHash -LiteralPath $destPath -Algorithm SHA256).Hash

                if ($sourceHash -eq $existingHash) {
                    $stats.SkippedIdentical++
                    Write-Host "SKIP identical: $destName"
                    continue
                }

                if (-not $Overwrite.IsPresent) {
                    $stats.SkippedConflict++
                    Write-Warning "SKIP conflict, destination exists with different SHA256: $destPath"
                    continue
                }
            }

            if ($Overwrite.IsPresent) {
                Copy-Item -LiteralPath $exe.FullName -Destination $destPath -Force
            } else {
                Copy-Item -LiteralPath $exe.FullName -Destination $destPath
            }

            $destItem = Get-Item -LiteralPath $destPath
            $destHash = (Get-FileHash -LiteralPath $destPath -Algorithm SHA256).Hash
            $stats.Copied++
            Write-Host ("COPY {0} -> {1} ({2} bytes, SHA256 {3})" -f $exe.FullName, $destPath, $destItem.Length, $destHash)
        } catch {
            $stats.Failed++
            Write-Warning ("FAILED {0}: {1}" -f $exe.FullName, $_.Exception.Message)
        }
    }
}

Write-Host ''
Write-Host 'Summary'
Write-Host ("Workgroups checked:       {0}" -f $stats.WorkgroupsChecked)
Write-Host ("Missing release dirs:     {0}" -f $stats.MissingReleaseDirs)
Write-Host ("No matching executables:  {0}" -f $stats.NoMatchingExecutables)
Write-Host ("Copied:                   {0}" -f $stats.Copied)
Write-Host ("Skipped identical:        {0}" -f $stats.SkippedIdentical)
Write-Host ("Skipped conflicts:        {0}" -f $stats.SkippedConflict)
Write-Host ("Failed:                   {0}" -f $stats.Failed)

if ($stats.Failed -gt 0) {
    exit 1
}

exit 0
