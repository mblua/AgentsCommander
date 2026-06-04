#!/usr/bin/env node
const { spawn } = require('child_process');
const path = require('path');
const os = require('os');
const fs = require('fs');

const binName = os.platform() === 'win32' ? 'agentscommander.exe' : 'agentscommander';
const binPath = path.join(__dirname, 'bin', binName);

if (!fs.existsSync(binPath)) {
  console.error(`Error: Cannot find AgentsCommander executable at ${binPath}`);
  console.error('Please ensure the package was installed correctly.');
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
