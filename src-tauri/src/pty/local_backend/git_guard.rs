use std::io::Write;
use std::sync::atomic::Ordering;

#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use crate::config::instance_artifacts::GIT_GUARD_DIR_NAME;
use crate::errors::AppError;

use super::GIT_GUARD_TEMP_COUNTER;

#[cfg(windows)]
pub(super) struct GitGuardEnv {
    pub(super) path: String,
    pub(super) pathext: String,
    pub(super) real_git: String,
}

#[cfg(windows)]
pub(super) const GIT_GUARD_PUBLISH_RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_millis(1600),
];

#[cfg(windows)]
pub(super) fn resolve_real_git_path() -> Option<String> {
    crate::process::git::watcher::resolve_real_git_path()
}

#[cfg(windows)]
pub(super) fn ensure_git_guard_wrapper() -> Result<std::path::PathBuf, AppError> {
    let config_dir = crate::config::config_dir()
        .ok_or_else(|| AppError::Other("Could not resolve app config directory".to_string()))?;
    let guard_dir = config_dir.join(GIT_GUARD_DIR_NAME);
    std::fs::create_dir_all(&guard_dir)
        .map_err(|e| AppError::Other(format!("Failed to create git-guard dir: {}", e)))?;

    let cmd_path = guard_dir.join("git.cmd");
    let ps1_path = guard_dir.join("git-guard.ps1");

    let cmd_content = "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \
        \"%~dp0git-guard.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n";
    let ps1_content = concat!(
        r#"$ErrorActionPreference = 'Stop'
$realGit = $env:AC_REAL_GIT
if ([string]::IsNullOrWhiteSpace($realGit)) {
  Write-Error 'AgentsCommander git guard: AC_REAL_GIT is not set.'
  exit 1
}

$originalArgs = @($args)
$target = (Get-Location).Path

for ($i = 0; $i -lt $originalArgs.Count; $i++) {
  $arg = [string]$originalArgs[$i]
  if ($arg -eq '-C') {
    if ($i + 1 -ge $originalArgs.Count) {
      Write-Error 'AgentsCommander git guard: missing path after -C.'
      exit 1
    }

    $next = [string]$originalArgs[$i + 1]
    if ([System.IO.Path]::IsPathRooted($next)) {
      $target = [System.IO.Path]::GetFullPath($next)
    } else {
      $target = [System.IO.Path]::GetFullPath((Join-Path -Path $target -ChildPath $next))
    }
    $i++
    continue
  }

  if ($arg -eq '--git-dir' -or $arg -like '--git-dir=*' -or $arg -eq "#,
        r#"'--work-tree' -or $arg -like '--work-tree=*') {
    Write-Error 'AgentsCommander git guard: --git-dir and --work-tree are not "#,
        r#"allowed in agent sessions.'
    exit 1
  }
}

function Test-AllowedGitTarget([string]$path) {
  try {
    $current = [System.IO.Path]::GetFullPath($path)
  } catch {
    return $false
  }

  while ($true) {
    $name = [System.IO.Path]::GetFileName($current)
    if ($name -like 'repo-*') {
      return $true
    }

    $parent = Split-Path -Path $current -Parent
    if ([string]::IsNullOrWhiteSpace($parent) -or $parent -eq $current) {
      break
    }
    $current = $parent
  }

  return $false
}

if (-not (Test-AllowedGitTarget $target)) {
  Write-Error ('AgentsCommander git guard: git is only allowed inside repo-* "#,
        r#"directories. Target path: ' + $target)
  exit 1
}

& $realGit @originalArgs
exit $LASTEXITCODE
"#,
    );

    write_git_guard_file_if_changed(&cmd_path, cmd_content)
        .map_err(|e| AppError::Other(format!("Failed to write git.cmd guard: {}", e)))?;
    write_git_guard_file_if_changed(&ps1_path, ps1_content)
        .map_err(|e| AppError::Other(format!("Failed to write git-guard.ps1: {}", e)))?;

    Ok(guard_dir)
}

#[cfg(windows)]
pub(super) fn write_git_guard_file_if_changed(path: &Path, content: &str) -> Result<(), String> {
    let desired = content.as_bytes();
    match std::fs::read(path) {
        Ok(existing) if existing == desired => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read existing {}: {}", path.display(), e)),
    }

    write_git_guard_file_atomic(path, desired)
}

#[cfg(windows)]
pub(super) fn write_git_guard_file_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("target {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(GIT_GUARD_DIR_NAME);
    let counter = GIT_GUARD_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.{}.{counter}.tmp", std::process::id()));

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("create temp {}: {}", temp.display(), e))?;

    if let Err(e) = file.write_all(content) {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write temp {}: {}", temp.display(), e));
    }
    if let Err(e) = file.flush() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("flush temp {}: {}", temp.display(), e));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("sync temp {}: {}", temp.display(), e));
    }
    drop(file);

    publish_git_guard_temp_with_retry(&temp, path, content)
}

#[cfg(windows)]
pub(super) fn publish_git_guard_temp_with_retry(
    temp: &Path,
    path: &Path,
    content: &[u8],
) -> Result<(), String> {
    let attempts = GIT_GUARD_PUBLISH_RETRY_DELAYS
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None));

    for (attempt, delay) in attempts.enumerate() {
        if git_guard_file_matches(path, content) {
            let _ = std::fs::remove_file(temp);
            return Ok(());
        }

        match crate::config::root_agent::atomic_replace_existing(temp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if git_guard_file_matches(path, content) {
                    let _ = std::fs::remove_file(temp);
                    return Ok(());
                }

                let Some(delay) = delay else {
                    let _ = std::fs::remove_file(temp);
                    return Err(format!(
                        "publish {} from {} failed after {} attempts: {}",
                        path.display(),
                        temp.display(),
                        attempt + 1,
                        e
                    ));
                };
                std::thread::sleep(delay);
            }
        }
    }

    unreachable!("retry loop includes a final no-delay attempt")
}

#[cfg(windows)]
pub(super) fn git_guard_file_matches(path: &Path, content: &[u8]) -> bool {
    match std::fs::read(path) {
        Ok(existing) => existing == content,
        Err(_) => false,
    }
}
