// Creates the per-user Start Menu shortcut for a global npm install on Windows.
// install.js calls it after the executable is in place; a failure only warns.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const SHORTCUT_NAME = 'AgentsCommander.lnk';

// Paths reach PowerShell through environment variables, never through the
// script text, so a quote or `$` in a user path cannot change the script.
const PS_SCRIPT = [
  '$ErrorActionPreference = "Stop"',
  '$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:AC_SHORTCUT_PATH)',
  '$s.TargetPath = $env:AC_SHORTCUT_TARGET',
  '$s.WorkingDirectory = $env:AC_SHORTCUT_WORKDIR',
  '$s.IconLocation = $env:AC_SHORTCUT_TARGET + ",0"',
  '$s.Description = "AgentsCommander"',
  '$s.Save()',
].join('; ');

// Returns the reason the shortcut is skipped, or null when it should be created.
function skipReason(platform, env) {
  if (platform !== 'win32') return 'not Windows';
  if (env.npm_config_global !== 'true') return 'not a global install';
  if (env.AGENTSCOMMANDER_NO_SHORTCUT === '1') return 'AGENTSCOMMANDER_NO_SHORTCUT=1';
  if (!env.APPDATA) return 'APPDATA is not set';
  return null;
}

function shortcutPath(env) {
  return path.join(env.APPDATA, 'Microsoft', 'Windows', 'Start Menu', 'Programs', SHORTCUT_NAME);
}

// Fixed, OS-owned PowerShell location: never resolved through PATH.
function powershellPath(env) {
  const systemRoot = env.SystemRoot || env.SYSTEMROOT || String.raw`C:\Windows`;
  return path.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
}

function createStartMenuShortcut(targetPath, options = {}) {
  const platform = options.platform || process.platform;
  const env = options.env || process.env;
  const reason = skipReason(platform, env);
  if (reason) return { created: false, reason };

  const lnk = shortcutPath(env);
  fs.mkdirSync(path.dirname(lnk), { recursive: true });
  const r = spawnSync(
    powershellPath(env),
    ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', PS_SCRIPT],
    {
      env: {
        ...env,
        AC_SHORTCUT_PATH: lnk,
        AC_SHORTCUT_TARGET: targetPath,
        AC_SHORTCUT_WORKDIR: env.USERPROFILE || path.dirname(targetPath),
      },
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (r.error) throw new Error(`could not run PowerShell: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`PowerShell exited ${r.status}: ${(r.stderr || '').trim()}`);
  if (!fs.existsSync(lnk)) throw new Error(`shortcut was not written at ${lnk}`);
  return { created: true, path: lnk };
}

module.exports = { createStartMenuShortcut, skipReason, shortcutPath, SHORTCUT_NAME };
