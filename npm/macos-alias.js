// Creates the per-user ~/Applications alias for a global npm install on macOS.
// install.js calls it after the bundle is extracted and validated; a failure
// only warns. Issue #2065.
//
// The bundle stays where npm put it, inside the package's bin/ directory. Only a
// symlink is added, so Launchpad and Spotlight can index the app and the user can
// pin it to the Dock. ~/Applications is user-owned: no sudo, no elevation.
// /Applications is deliberately out of scope because it needs elevation.
//
// This module never touches Gatekeeper: no `xattr`, no `spctl`, no quarantine
// attribute is read or cleared. The bundle is unsigned, so the first launch may
// still be blocked; that is a condition to report, never to bypass.
const fs = require('node:fs');
const path = require('node:path');

const ALIAS_NAME = 'AgentsCommander.app';

// Returns the reason the alias is skipped, or null when it should be created.
function skipReason(platform, env) {
  if (platform !== 'darwin') return 'not macOS';
  if (env.npm_config_global !== 'true') return 'not a global install';
  if (env.AGENTSCOMMANDER_NO_SHORTCUT === '1') return 'AGENTSCOMMANDER_NO_SHORTCUT=1';
  if (!env.HOME) return 'HOME is not set';
  return null;
}

function aliasPath(env) {
  return path.join(env.HOME, 'Applications', ALIAS_NAME);
}

// Replaces our own symlink and nothing else: a real directory or file already at
// the alias path belongs to someone else and is left untouched.
function removeExistingAlias(alias) {
  let stat;
  try {
    stat = fs.lstatSync(alias);
  } catch (err) {
    if (err.code === 'ENOENT') return;
    throw err;
  }
  if (!stat.isSymbolicLink()) {
    throw new Error(`${alias} already exists and is not a symlink; leaving it alone`);
  }
  fs.unlinkSync(alias);
}

function createLaunchpadAlias(bundlePath, options = {}) {
  const platform = options.platform || process.platform;
  const env = options.env || process.env;
  const reason = skipReason(platform, env);
  if (reason) return { created: false, reason };

  let bundleStat;
  try {
    bundleStat = fs.statSync(bundlePath);
  } catch (err) {
    throw new Error(`app bundle not found at ${bundlePath} (${err.code})`);
  }
  if (!bundleStat.isDirectory() || !bundlePath.endsWith('.app')) {
    throw new Error(`not an app bundle: ${bundlePath}`);
  }

  const alias = aliasPath(env);
  fs.mkdirSync(path.dirname(alias), { recursive: true });
  removeExistingAlias(alias);
  fs.symlinkSync(bundlePath, alias, 'dir');

  const written = fs.lstatSync(alias);
  if (!written.isSymbolicLink()) throw new Error(`alias was not written as a symlink at ${alias}`);
  if (fs.readlinkSync(alias) !== bundlePath) throw new Error(`alias at ${alias} does not point at ${bundlePath}`);
  return { created: true, path: alias, target: bundlePath };
}

module.exports = { createLaunchpadAlias, skipReason, aliasPath, ALIAS_NAME };
