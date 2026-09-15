// Shared, platform-aware locator for the AgentsCommander executable inside the
// installed package. run.js (launch) and install.js (post-extraction validation)
// both use it so the two files agree on one on-disk contract. Issue #2016.
const fs = require('fs');
const path = require('path');

// A Tauri Info.plist states CFBundleExecutable as a single <key>/<string> pair.
// A bounded regex reads it without adding a plist parser to the published package.
const CFBUNDLE_EXECUTABLE_RE = /<key>\s*CFBundleExecutable\s*<\/key>\s*<string>([^<]*)<\/string>/;

function resolveBinPath(platform, binDir) {
  if (platform !== 'darwin') {
    return path.join(binDir, platform === 'win32' ? 'agentscommander.exe' : 'agentscommander');
  }

  let entries;
  try {
    entries = fs.readdirSync(binDir, { withFileTypes: true });
  } catch (err) {
    throw new Error(`macOS app bundle not found: cannot read ${binDir} (${err.code})`);
  }

  const bundles = entries.filter((entry) => entry.isDirectory() && entry.name.endsWith('.app'));
  if (bundles.length !== 1) {
    const found = bundles.length === 0 ? 'none' : `${bundles.length}: ${bundles.map((b) => b.name).join(', ')}`;
    throw new Error(`macOS app bundle not found in ${binDir}: expected exactly one *.app directory, found ${found}`);
  }

  const appDir = path.join(binDir, bundles[0].name);
  const plistPath = path.join(appDir, 'Contents', 'Info.plist');
  let plist;
  try {
    plist = fs.readFileSync(plistPath, 'utf8');
  } catch (err) {
    throw new Error(`macOS app bundle is incomplete: cannot read ${plistPath} (${err.code})`);
  }

  const match = CFBUNDLE_EXECUTABLE_RE.exec(plist);
  const executable = match ? match[1].trim() : '';
  if (
    !executable ||
    executable === '.' ||
    executable === '..' ||
    executable.includes('/') ||
    executable.includes('\\')
  ) {
    throw new Error(`macOS app bundle is incomplete: valid CFBundleExecutable not found in ${plistPath}`);
  }

  return path.join(appDir, 'Contents', 'MacOS', executable);
}

function assertExecutable(platform, binDir) {
  const binPath = resolveBinPath(platform, binDir);
  let stat;
  try {
    stat = fs.statSync(binPath);
  } catch (err) {
    throw new Error(`macOS app bundle is incomplete: executable missing at ${binPath} (${err.code})`);
  }
  if (!stat.isFile()) {
    throw new Error(`macOS app bundle is incomplete: not a regular file at ${binPath}`);
  }
  return binPath;
}

module.exports = { resolveBinPath, assertExecutable };
