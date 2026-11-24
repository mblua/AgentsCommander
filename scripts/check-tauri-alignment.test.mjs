import { test } from 'node:test';
import assert from 'node:assert/strict';
import { npmVersions, cargoVersions, checkAlignment } from './check-tauri-alignment.mjs';

const lock = (pkgs) => ({
  packages: Object.fromEntries(Object.entries(pkgs).map(([n, v]) => [`node_modules/${n}`, { version: v }])),
});
const cargo = (crates) =>
  Object.entries(crates)
    .map(([n, v]) => `[[package]]\nname = "${n}"\nversion = "${v}"\nsource = "x"\n`)
    .join('\n');

test('pairs aligned on major.minor pass, patch may differ', () => {
  const r = checkAlignment(
    npmVersions(lock({ '@tauri-apps/cli': '2.12.4', '@tauri-apps/plugin-dialog': '2.8.4' })),
    cargoVersions(cargo({ tauri: '2.12.5', 'tauri-plugin-dialog': '2.8.4' })),
  );
  assert.deepEqual(r.errors, []);
  assert.equal(r.pairs.length, 2);
});

test('minor skew fails with a clear message', () => {
  const r = checkAlignment(
    npmVersions(lock({ '@tauri-apps/cli': '2.11.0', '@tauri-apps/plugin-dialog': '2.8.4' })),
    cargoVersions(cargo({ tauri: '2.12.5', 'tauri-plugin-dialog': '2.7.0' })),
  );
  assert.equal(r.errors.length, 2);
  assert.match(r.errors[0], /major\.minor skew: @tauri-apps\/cli 2\.11\.0 <-> tauri 2\.12\.5/);
  assert.match(r.errors[1], /plugin-dialog 2\.8\.4 <-> tauri-plugin-dialog 2\.7\.0/);
});

test('major skew fails', () => {
  const r = checkAlignment(npmVersions(lock({ '@tauri-apps/cli': '1.6.0' })), cargoVersions(cargo({ tauri: '2.12.5' })));
  assert.equal(r.errors.length, 1);
});

test('JS-only and Rust-only plugins are info, not errors', () => {
  const r = checkAlignment(
    npmVersions(lock({ '@tauri-apps/cli': '2.12.4', '@tauri-apps/plugin-os': '2.3.0' })),
    cargoVersions(cargo({ tauri: '2.12.5', 'tauri-plugin-fs': '2.7.1' })),
  );
  assert.deepEqual(r.errors, []);
  assert.deepEqual(r.infos, ['JS-only plugin: @tauri-apps/plugin-os', 'Rust-only plugin: tauri-plugin-fs']);
});

test('npm core package without a tauri crate fails', () => {
  const r = checkAlignment(npmVersions(lock({ '@tauri-apps/cli': '2.12.4' })), cargoVersions(cargo({})));
  assert.deepEqual(r.errors, ['@tauri-apps/cli has no tauri crate in Cargo.lock']);
});

test('nested lock entries are read by package name', () => {
  const m = npmVersions({ packages: { '': {}, 'node_modules/x/node_modules/@tauri-apps/api': { version: '2.12.0' } } });
  assert.deepEqual([...m.get('@tauri-apps/api')], ['2.12.0']);
});
