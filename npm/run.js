#!/usr/bin/env node
const { spawn } = require('child_process');
const path = require('path');
const os = require('os');
const fs = require('fs');
const { resolveBinPath } = require('./resolve-bin');

const binDir = path.join(__dirname, 'bin');

const IGNORE_SCRIPTS_HINT =
  'Hint: if npm install ran with --ignore-scripts, reinstall without that flag, or run: npm rebuild -g @mblua/agentscommander';

let binPath;
try {
  binPath = resolveBinPath(os.platform(), binDir);
} catch (err) {
  console.error(`Error: ${err.message}`);
  console.error('Please ensure the package was installed correctly.');
  console.error(IGNORE_SCRIPTS_HINT);
  process.exit(1);
}

if (!fs.existsSync(binPath)) {
  console.error(`Error: Cannot find AgentsCommander executable at ${binPath}`);
  console.error('Please ensure the package was installed correctly.');
  console.error(IGNORE_SCRIPTS_HINT);
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
