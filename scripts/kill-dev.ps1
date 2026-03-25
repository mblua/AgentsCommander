# kill-dev.ps1 - Kill ONLY dev instances of agentscommander.exe
# NEVER touches production (Program Files) or release builds.

$procs = Get-WmiObject Win32_Process -Filter "Name='agentscommander.exe'" 2>$null

if (-not $procs) {
    Write-Host "[kill-dev] No agentscommander.exe instances running." -ForegroundColor Green
    exit 0
}

$killed = 0

foreach ($p in $procs) {
    $path = $p.ExecutablePath
    $procId = $p.ProcessId

    if ($path -like "*Program Files*") {
        Write-Host "[kill-dev] SKIPPING PID $procId - PROD instance ($path)" -ForegroundColor Yellow
        continue
    }

    if ($path -like "*target\release*") {
        Write-Host "[kill-dev] SKIPPING PID $procId - release build ($path)" -ForegroundColor Yellow
        continue
    }

    if ($path -like "*target\debug*") {
        Write-Host "[kill-dev] Killing PID $procId - dev instance ($path)" -ForegroundColor Cyan
        Stop-Process -Id $procId -Force
        $killed++
        continue
    }

    # Unknown path - do not touch
    Write-Host "[kill-dev] SKIPPING PID $procId - unknown path ($path)" -ForegroundColor Red
}

if ($killed -eq 0) {
    Write-Host "[kill-dev] No dev instances to kill." -ForegroundColor Green
} else {
    Write-Host "[kill-dev] Killed $killed dev instance(s)." -ForegroundColor Cyan
}
