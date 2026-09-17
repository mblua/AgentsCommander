// Creates the per-user application menu entry for a global npm install on Linux.
// install.js calls it after the executable is in place; a failure only warns.
// Issue #2066.
//
// The entry is written under $XDG_DATA_HOME/applications (default
// ~/.local/share/applications). /usr/share/applications needs root and is out of
// scope. The icon is the icon.png shipped inside this package: the Linux release
// asset is a bare binary with no icon of its own.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const ENTRY_NAME = 'agentscommander.desktop';
const ICON_PATH = path.join(__dirname, 'icon.png');

// Fixed, OS-owned locations: never resolved through PATH.
const UPDATE_DB_CANDIDATES = ['/usr/bin/update-desktop-database', '/bin/update-desktop-database'];

// Returns the reason the entry is skipped, or null when it should be created.
function skipReason(platform, env) {
  if (platform !== 'linux') return 'not Linux';
  if (env.npm_config_global !== 'true') return 'not a global install';
  if (env.AGENTSCOMMANDER_NO_SHORTCUT === '1') return 'AGENTSCOMMANDER_NO_SHORTCUT=1';
  if (!env.XDG_DATA_HOME && !env.HOME) return 'neither XDG_DATA_HOME nor HOME is set';
  return null;
}

function applicationsDir(env) {
  const dataHome = env.XDG_DATA_HOME ? env.XDG_DATA_HOME : path.join(env.HOME, '.local', 'share');
  return path.join(dataHome, 'applications');
}

function entryPath(env) {
  return path.join(applicationsDir(env), ENTRY_NAME);
}

// Desktop Entry Specification: a string value escapes backslash, and an Exec
// argument is double-quoted with ", `, $ and \ backslash-escaped, % doubled.
// The quoting layer is applied first, then the string-value layer.
function quoteExecArg(arg) {
  const quoted = '"' + arg.replace(/[\\"`$]/g, (c) => '\\' + c).replace(/%/g, '%%') + '"';
  return quoted.replace(/\\/g, '\\\\');
}

function assertSingleLine(value, label) {
  if (/[\r\n]/.test(value)) throw new Error(`${label} contains a line break: ${JSON.stringify(value)}`);
}

function entryText(targetPath, iconPath = ICON_PATH) {
  assertSingleLine(targetPath, 'executable path');
  assertSingleLine(iconPath, 'icon path');
  return [
    '[Desktop Entry]',
    'Type=Application',
    'Version=1.0',
    'Name=AgentsCommander',
    'Comment=Terminal session manager for AI coding agent teams',
    `Exec=${quoteExecArg(targetPath)}`,
    `TryExec=${targetPath.replace(/\\/g, '\\\\')}`,
    `Icon=${iconPath.replace(/\\/g, '\\\\')}`,
    'Terminal=false',
    'Categories=Development;Utility;',
    '',
  ].join('\n');
}

// Optional: most desktop environments pick the file up without it, so its
// absence or failure is silent.
function refreshDesktopDatabase(dir, candidates = UPDATE_DB_CANDIDATES) {
  const cmd = candidates.find((candidate) => fs.existsSync(candidate));
  if (!cmd) return false;
  const r = spawnSync(cmd, [dir], { stdio: 'ignore', timeout: 15000 });
  return !r.error && r.status === 0;
}

function createDesktopEntry(targetPath, options = {}) {
  const platform = options.platform || process.platform;
  const env = options.env || process.env;
  const reason = skipReason(platform, env);
  if (reason) return { created: false, reason };

  const iconPath = options.iconPath || ICON_PATH;
  if (!fs.existsSync(iconPath)) throw new Error(`icon not found at ${iconPath}`);
  const text = entryText(targetPath, iconPath);
  const file = entryPath(env);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode: 0o644 });
  if (fs.readFileSync(file, 'utf8') !== text) throw new Error(`desktop entry was not written at ${file}`);
  const refreshed = refreshDesktopDatabase(path.dirname(file), options.updateDbCandidates);
  return { created: true, path: file, refreshed };
}

module.exports = {
  createDesktopEntry,
  skipReason,
  applicationsDir,
  entryPath,
  entryText,
  quoteExecArg,
  ENTRY_NAME,
  ICON_PATH,
};
