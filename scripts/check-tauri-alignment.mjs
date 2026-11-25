#!/usr/bin/env node
// Checks that each Tauri npm package and its Rust crate share major.minor.
//
// Tauri requires `@tauri-apps/cli` to match the `tauri` crate, and
// `@tauri-apps/plugin-X` to match `tauri-plugin-X`, on major.minor (patch
// may differ). `@tauri-apps/api` <-> `tauri` is checked by
// src-tauri/tests/tauri_npm_version_coupling.rs, so it is not repeated here.
// Reads the resolved versions from `package-lock.json` and `Cargo.lock`.
//
// Usage:
//   node scripts/check-tauri-alignment.mjs
//
// Exit codes:
//   0 -> every pair is aligned (unpaired plugins are reported as info)
//   1 -> a pair differs on major.minor, or a lockfile cannot be read

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const NPM_CORE = ['@tauri-apps/cli'];
const NPM_PLUGIN = '@tauri-apps/plugin-';
const RUST_PLUGIN = 'tauri-plugin-';

/** Map npm package name -> set of resolved versions in package-lock.json. */
export function npmVersions(lock) {
  const out = new Map();
  for (const [key, entry] of Object.entries(lock.packages ?? {})) {
    const name = key.split('node_modules/').pop();
    if (!name?.startsWith('@tauri-apps/') || !entry?.version) continue;
    if (!out.has(name)) out.set(name, new Set());
    out.get(name).add(entry.version);
  }
  return out;
}

/** Map crate name -> set of resolved versions in Cargo.lock. */
export function cargoVersions(text) {
  const out = new Map();
  for (const block of text.split('[[package]]').slice(1)) {
    const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"$/m.exec(block)?.[1];
    if (!name || !version) continue;
    if (!out.has(name)) out.set(name, new Set());
    out.get(name).add(version);
  }
  return out;
}

const minor = (v) => v.split('.').slice(0, 2).join('.');

/** Compare the pairs; returns { errors, infos, pairs }. */
export function checkAlignment(npm, cargo) {
  const errors = [];
  const infos = [];
  const pairs = [];
  const compare = (js, rust) => {
    const jsV = [...(npm.get(js) ?? [])];
    const rustV = [...(cargo.get(rust) ?? [])];
    if (!jsV.length || !rustV.length) return false;
    const ok = jsV.every((a) => rustV.every((b) => minor(a) === minor(b)));
    const line = `${js} ${jsV.join('/')} <-> ${rust} ${rustV.join('/')}`;
    pairs.push(line);
    if (!ok) errors.push(`major.minor skew: ${line}`);
    return true;
  };
  for (const js of NPM_CORE) {
    if (npm.has(js) && !compare(js, 'tauri')) errors.push(`${js} has no tauri crate in Cargo.lock`);
  }
  for (const js of npm.keys()) {
    if (!js.startsWith(NPM_PLUGIN)) continue;
    const rust = RUST_PLUGIN + js.slice(NPM_PLUGIN.length);
    if (!compare(js, rust)) infos.push(`JS-only plugin: ${js}`);
  }
  for (const rust of cargo.keys()) {
    if (!rust.startsWith(RUST_PLUGIN)) continue;
    if (!npm.has(NPM_PLUGIN + rust.slice(RUST_PLUGIN.length))) {
      infos.push(`Rust-only plugin: ${rust}`);
    }
  }
  return { errors, infos, pairs };
}

function main() {
  const root = fileURLToPath(new URL('..', import.meta.url));
  let npm;
  let cargo;
  try {
    npm = npmVersions(JSON.parse(readFileSync(`${root}package-lock.json`, 'utf8')));
    cargo = cargoVersions(readFileSync(`${root}Cargo.lock`, 'utf8'));
  } catch (err) {
    console.error(`check-tauri-alignment: cannot read lockfiles: ${err.message}`);
    process.exit(1);
  }
  const { errors, infos, pairs } = checkAlignment(npm, cargo);
  for (const p of pairs) console.log(`pair: ${p}`);
  for (const i of infos) console.log(`info: ${i}`);
  if (errors.length) {
    for (const e of errors) console.error(`error: ${e}`);
    console.error('Tauri npm packages and Rust crates must share major.minor.');
    process.exit(1);
  }
  console.log('check-tauri-alignment: all Tauri pairs aligned on major.minor');
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
