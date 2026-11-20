#!/usr/bin/env node
const { spawn } = require('node:child_process');
const path = require('node:path');
const os = require('node:os');
const fs = require('node:fs');
const { resolveBinPath } = require('./resolve-bin');

const binDir = path.join(__dirname, 'bin');

const MISSING_EXE_HINT = [
  "Hint: npm did not run this package's install script, which downloads the executable.",
  'npm 12 and newer block it unless allowed. To fix a global install, run:',
  '  npm rebuild -g --allow-scripts=@mblua/agentscommander @mblua/agentscommander',
  'For a local install on npm 12 and newer, run these two commands:',
  '  npm install-scripts approve @mblua/agentscommander',
  '  npm rebuild @mblua/agentscommander',
  'For npx, clear the npx cache, then run npx again with the flag:',
  '  npm cache npx rm --force',
  '  npx --allow-scripts=@mblua/agentscommander @mblua/agentscommander',
  'If ignore-scripts is set (flag or .npmrc), remove it first; it overrides allow-scripts.',
  'On older npm, run: npm rebuild @mblua/agentscommander (add -g for a global install).',
].join('\n');

let binPath;
try {
  binPath = resolveBinPath(os.platform(), binDir);
} catch (err) {
  console.error(`Error: ${err.message}`);
  console.error('Please ensure the package was installed correctly.');
  console.error(MISSING_EXE_HINT);
  process.exit(1);
}

if (!fs.existsSync(binPath)) {
  console.error(`Error: Cannot find AgentsCommander executable at ${binPath}`);
  console.error('Please ensure the package was installed correctly.');
  console.error(MISSING_EXE_HINT);
  process.exit(1);
}

const child = spawn(binPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: true
});

child.on('error', (err) => {
  console.error('Failed to start AgentsCommander:', err.message);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    try {
      process.kill(process.pid, signal);
    } catch (e) {
      process.exit(1);
    }
  } else {
    process.exit(code !== null ? code : 1);
  }
});

const signals = ['SIGINT', 'SIGTERM', 'SIGQUIT'];
signals.forEach(sig => {
  process.on(sig, () => {
    if (child && !child.killed) {
      child.kill(sig);
    }
  });
});
