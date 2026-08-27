#!/usr/bin/env node
// Assembles the Windows portable zip asset, for issue #1589.
//
// The published raw binary is named agentscommander-windows-x86_64.exe so
// npm/install.js can resolve it by URL. That name is NOT safe to run as-is:
// binary_suffix() in src-tauri/src/config/profile.rs splits the file stem on
// the FIRST underscore, so it yields the suffix "64" and the binary comes up
// with a non-default config directory, mutex, and ports. The portable asset
// therefore ships the same binary under its canonical name.
//
// Usage:
//   node scripts/pack-windows-portable.mjs \
//     --binary target/release/agentscommander.exe \
//     --version 0.30.3 \
//     --out dist-portable/agentscommander-0.30.3-windows-x86_64-portable.zip
//
// Exit codes:
//   0 → zip written
//   1 → bad arguments, missing input, or the zip step failed

import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';

const __filename = fileURLToPath(import.meta.url);
const ROOT       = resolve(dirname(__filename), '..');

const TEMPLATE      = join(ROOT, 'packaging', 'windows', 'PORTABLE.txt');
const CANONICAL_EXE = 'agentscommander.exe';
// Files copied verbatim from the repo root into the zip.
const EXTRA_FILES   = ['LICENSE', 'THIRD_PARTY_NOTICES.md'];
const VERSION_RE    = /^\d+\.\d+\.\d+$/;

function die(msg) {
  console.error(`[pack-portable] ${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const out = { binary: null, version: null, out: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--binary')       out.binary  = argv[++i];
    else if (argv[i] === '--version') out.version = argv[++i];
    else if (argv[i] === '--out')     out.out     = argv[++i];
    else die(`Unknown argument: ${argv[i]}`);
  }
  return out;
}

// Compress-Archive rather than a bundled zip library: this only ever runs on a
// Windows runner that already has PowerShell, and it keeps the dependency
// footprint of the release path at zero. Paths travel through the environment,
// never interpolated into the command string, so a quote or a bracket in a
// checkout path cannot break the quoting or inject anything.
function zipDirectory(stageDir, outPath) {
  const command =
    'Compress-Archive -Path (Join-Path $env:AC_PACK_STAGE "*") ' +
    '-DestinationPath $env:AC_PACK_OUT -Force -CompressionLevel Optimal';
  const env = { ...process.env, AC_PACK_STAGE: stageDir, AC_PACK_OUT: outPath };
  for (const shell of ['pwsh', 'powershell']) {
    try {
      execFileSync(shell, ['-NoProfile', '-NonInteractive', '-Command', command], { stdio: 'pipe', env });
      return shell;
    } catch (err) {
      // A missing shell is a resolution failure; anything else is a real zip
      // error and must not be masked by falling through to the next shell.
      if (err?.code !== 'ENOENT') {
        die(`${shell} failed to create the archive: ${err?.stderr?.toString().trim() || err?.message || err}`);
      }
    }
  }
  die('Neither pwsh nor powershell is available to create the archive.');
}

const args = parseArgs(process.argv.slice(2));

if (!args.binary)  die('Missing --binary <path to built agentscommander exe>.');
if (!args.version) die('Missing --version <X.Y.Z>.');
if (!args.out)     die('Missing --out <path to the zip to write>.');
if (!VERSION_RE.test(args.version)) die(`--version must be X.Y.Z, got "${args.version}".`);

const binaryPath = resolve(args.binary);
if (!existsSync(binaryPath) || !statSync(binaryPath).isFile()) {
  die(`Binary not found: ${binaryPath}. Build it before packing.`);
}

const outPath  = resolve(args.out);
const stageDir = join(dirname(outPath), `portable-stage-${args.version}`);

rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });
mkdirSync(dirname(outPath), { recursive: true });

copyFileSync(binaryPath, join(stageDir, CANONICAL_EXE));

for (const name of EXTRA_FILES) {
  const src = join(ROOT, name);
  if (!existsSync(src)) die(`Expected ${name} at the repo root: ${src}`);
  copyFileSync(src, join(stageDir, name));
}

if (!existsSync(TEMPLATE)) die(`Missing template: ${TEMPLATE}`);
const readme = readFileSync(TEMPLATE, 'utf8').replaceAll('{{VERSION}}', args.version);
if (readme.includes('{{')) die('PORTABLE.txt still contains an unresolved placeholder after rendering.');
// CRLF so the file reads correctly in every Windows text editor.
writeFileSync(join(stageDir, 'PORTABLE.txt'), readme.replace(/\r?\n/g, '\r\n'), 'utf8');

rmSync(outPath, { force: true });
const shell = zipDirectory(stageDir, outPath);
rmSync(stageDir, { recursive: true, force: true });

if (!existsSync(outPath)) die(`${shell} reported success but ${outPath} does not exist.`);

const bytes = statSync(outPath).size;
console.log(`[pack-portable] wrote ${outPath} (${bytes} bytes) using ${shell}`);
console.log(`[pack-portable] contents: ${CANONICAL_EXE}, ${EXTRA_FILES.join(', ')}, PORTABLE.txt`);
