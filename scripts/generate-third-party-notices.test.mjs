import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import {
  selectCrates, selectNpm, licenseFiles, normalize, render, cargoCandidates, resolveCargo,
} from './generate-third-party-notices.mjs';

const pkg = (name, extra = {}) => ({
  id: name, name, version: '1.0.0', source: 'registry+test', license: 'MIT', authors: [],
  manifest_path: `/registry/${name}/Cargo.toml`, targets: [{ kind: ['lib'] }], ...extra,
});
const dep = (name, kind = null) => ({ pkg: name, dep_kinds: [{ kind }] });
function metadata(extra = false) {
  const packages = [pkg('agentscommander', { source: null }), pkg('normal'), pkg('build'),
    pkg('dev'), pkg('macro', { targets: [{ kind: ['proc-macro'] }] }), pkg('macro-child'),
    pkg('workspace', { source: null }), pkg('indirect'), pkg('target-only')];
  const rootDeps = [dep('normal'), dep('build', 'build'), dep('dev', 'dev'),
    dep('macro'), dep('workspace')];
  if (extra) rootDeps.push(dep('target-only'));
  return { packages, resolve: { nodes: packages.map(item => ({
    id: item.id, deps: item.id === 'agentscommander' ? rootDeps
      : item.id === 'macro' ? [dep('macro-child')]
        : item.id === 'workspace' ? [dep('indirect')] : [],
  })) } };
}
const npmLock = (entries = {}) => ({ packages: {
  '': { name: 'root' },
  'node_modules/vite': { dev: true, version: '8.0.2', license: 'MIT' }, ...entries,
} });
const printable = name => ({ name, version: '1', license: 'MIT', texts: ['same\n'] });
const output = (crates = [], npm = [], texts = {}) => render({
  crates, npm, texts, staticSections: '## Static\n',
});

test('normal graph union skips build/dev/proc macros and walks workspace crates', () => {
  assert.deepEqual(selectCrates([metadata(), metadata(true)]).map(p => p.name),
    ['indirect', 'normal', 'target-only']);
});

test('missing Rust root and empty registry selections fail closed', () => {
  assert.throws(() => selectCrates([{ packages: [], resolve: { nodes: [] } }]), /root/);
  assert.throws(() => selectCrates([{ packages: [pkg('agentscommander', { source: null })],
    resolve: { nodes: [{ id: 'agentscommander', deps: [] }] } }]), /No Rust crates/);
  assert.throws(() => selectCrates([]), /No Rust crates/);
});

const crateEntry = name => ({ name, version: '1.0.0', license: 'MIT', authors: [],
  dir: path.dirname(`/registry/${name}/Cargo.toml`) });
const rootedMetadata = (packages, nodes) => ({
  packages: [pkg('agentscommander', { source: null }), ...packages],
  resolve: { nodes },
});

test('selectCrates returns full sorted entries', () => {
  assert.deepEqual(selectCrates([metadata()]), [crateEntry('indirect'), crateEntry('normal')]);
});

test('selectCrates fails closed on a missing package or resolve node', () => {
  assert.throws(() => selectCrates([rootedMetadata([], [
    { id: 'agentscommander', deps: [dep('ghost')] },
  ])]), /cargo metadata is missing package ghost/);
  assert.throws(() => selectCrates([rootedMetadata([pkg('lonely')], [
    { id: 'agentscommander', deps: [dep('lonely')] },
  ])]), /cargo metadata is missing resolve node lonely/);
});

test('selectCrates defaults missing authors to an empty list', () => {
  const noAuthors = pkg('noauth');
  delete noAuthors.authors;
  assert.deepEqual(selectCrates([rootedMetadata([noAuthors], [
    { id: 'agentscommander', deps: [dep('noauth')] }, { id: 'noauth', deps: [] },
  ])]), [crateEntry('noauth')]);
});

test('selectCrates walks a dependency cycle once', () => {
  assert.deepEqual(selectCrates([rootedMetadata([pkg('a'), pkg('b')], [
    { id: 'agentscommander', deps: [dep('a')] },
    { id: 'a', deps: [dep('b')] }, { id: 'b', deps: [dep('a')] },
  ])]), [crateEntry('a'), crateEntry('b')]);
});

test('selectCrates keys the same crate across targets once', () => {
  assert.deepEqual(selectCrates([metadata(), metadata()]),
    [crateEntry('indirect'), crateEntry('normal')]);
});

test('npm filters dev/types, includes nested packages and forced Vite, uses lock version', () => {
  const result = selectNpm(npmLock({
    'node_modules/a': { version: '2-x' },
    'node_modules/a/node_modules/b': { version: '3' },
    'node_modules/dev': { dev: true, version: '1' },
    'node_modules/@types/x': { version: '1' },
    'node_modules/a/node_modules/@types/y': { version: '1' },
  }));
  assert.deepEqual(result.map(p => [p.name, p.version]),
    [['a', '2-x'], ['b', '3'], ['vite', '8.0.2']]);
  assert.throws(() => selectNpm({ packages: {} }), /Missing required npm package/);
});

test('ordering uses code units for package names, versions and license files', () => {
  const names = ['b', 'B', 'a', '_z', 'Z'];
  const lock = npmLock(Object.fromEntries(names.map(name => [
    `node_modules/${name}`, { version: '1' },
  ])));
  assert.deepEqual(selectNpm(lock).map(p => p.name), ['B', 'Z', '_z', 'a', 'b', 'vite']);
  const text = output([printable('a'), { ...printable('a'), version: '10' },
    { ...printable('a'), version: '2' }]);
  assert.ok(text.indexOf('- a 10 ') < text.indexOf('- a 2 '));
});

test('license file names select only top-level regular files and sort', () => {
  const names = ['NOTICE', 'LICENSE.txt', 'LICENSE-MIT', 'COPYING', 'UNLICENSE',
    'READ_LICENSE', 'src', 'LICENSES', 'COPYRIGHT', 'LICENCE'];
  const entries = names.map(name => ({ name,
    isFile: () => !['src', 'LICENSES'].includes(name),
  }));
  assert.deepEqual(licenseFiles(entries),
    ['COPYING', 'COPYRIGHT', 'LICENCE', 'LICENSE-MIT', 'LICENSE.txt', 'NOTICE', 'UNLICENSE']);
});

test('fallback rejects absent expressions and unknown SPDX ids with package name', () => {
  assert.throws(() => output([{ name: 'missing', license: null }]), /missing: no license/);
  assert.throws(() => output([{ name: 'bad', license: 'Mystery' }]), /bad:.*Mystery/);
});

test('fallback handles OR, AND, slash, WITH, parentheses, and copyright authors', () => {
  const text = output([{ name: 'fallback', version: '1',
    license: '(MIT OR Apache-2.0) AND Zlib / BSD-2-Clause WITH Exception',
    authors: ['Alice', 'Bob'] }], [], {
    MIT: 'mit', 'Apache-2.0': 'apache', Zlib: 'zlib', 'BSD-2-Clause': 'bsd', Exception: 'exc',
  });
  assert.match(text, /texts: T1, T2, T3, T4, T5/);
  assert.equal(text.match(/Copyright: Alice; Bob/g).length, 5);
  assert.match(output([{ name: 'fallback', license: 'MIT' }], [], { MIT: 'mit' }),
    /Copyright: the fallback contributors/);
});

test('normalization removes BOM, CRLF, CR, trailing spaces and extra final newlines', () => {
  assert.equal(normalize('\uFEFFa \r\nb\t\rc\n\n'), 'a\nb\nc\n');
  assert.equal(normalize(''), '\n');
});

function oldNormalize(text) {
  return text.replace(/^\uFEFF/, '').replace(/\r\n?/g, '\n')
    .split('\n').map(line => line.replace(/[ \t]+$/g, '')).join('\n')
    .replace(/\n*$/, '\n');
}

test('normalize matches the previous regex implementation', () => {
  const fixed = ['', '\n', '\n\n\n', 'a', 'a \t \n\n', '\uFEFFa\r\nb \r\n\r\n', ' \t',
    'a\n \n\t\n', '\r\r\n\n\r', '\uFEFF\uFEFFa'];
  for (const s of fixed) assert.equal(normalize(s), oldNormalize(s), JSON.stringify(s));
  const alphabet = [' ', '\t', '\n', '\r', '\uFEFF', 'a', '\u00a0'];
  let seed = 12345;
  const next = () => {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    return seed >>> 16;
  };
  for (let i = 0; i < 2000; i++) {
    let s = '';
    const length = next() % 41;
    for (let j = 0; j < length; j++) s += alphabet[next() % alphabet.length];
    assert.equal(normalize(s), oldNormalize(s), JSON.stringify(s));
  }
});

test('normalize stays linear on long space and newline runs', () => {
  for (const input of [' '.repeat(200000) + 'x', '\n'.repeat(200000) + 'x']) {
    const start = performance.now();
    normalize(input);
    assert.ok(performance.now() - start < 200);
  }
});

test('render is stable for shuffled inputs and deduplicates normalized texts', () => {
  const a = { ...printable('a'), texts: ['\uFEFFsame \r\n'] };
  const b = printable('b');
  assert.equal(output([b, a], [printable('c')]), output([a, b], [printable('c')]));
  const text = output([b, a]);
  assert.equal(text.match(/^### T1$/gm).length, 1);
  assert.doesNotMatch(text, /### T2/);
  assert.ok(text.indexOf('- a ') < text.indexOf('- b '));
});

test('fences exceed the longest run of backticks', () => {
  assert.match(output([{ ...printable('a'), texts: ['x````y'] }]),
    /\n`````\nx````y\n`````\n/);
  assert.match(output([printable('a')]), /\n```\nsame\n```\n/);
});

test('entry point returns failure for unknown argument', () => {
  const result = spawnSync(process.execPath,
    ['scripts/generate-third-party-notices.mjs', '--bogus'], {
      encoding: 'utf8', cwd: fileURLToPath(new URL('..', import.meta.url)),
    });
  assert.equal(result.status, 1);
});

test('cargo candidates use Windows paths regardless of host', () => {
  assert.deepEqual(cargoCandidates({ CARGO_HOME: String.raw`C:\ch` }, 'win32',
    String.raw`C:\Users\u`), [String.raw`C:\ch\bin\cargo.exe`,
    String.raw`C:\Users\u\.cargo\bin\cargo.exe`]);
});

test('cargo candidates use POSIX paths regardless of host', () => {
  assert.deepEqual(cargoCandidates({}, 'linux', '/home/u'), [
    '/home/u/.cargo/bin/cargo', '/usr/bin/cargo', '/usr/local/bin/cargo',
    '/opt/homebrew/bin/cargo',
  ]);
});

test('cargo candidates ignore relative CARGO_HOME and deduplicate joined paths', () => {
  const expected = ['/home/u/.cargo/bin/cargo', '/usr/bin/cargo',
    '/usr/local/bin/cargo', '/opt/homebrew/bin/cargo'];
  assert.deepEqual(cargoCandidates({ CARGO_HOME: 'rel' }, 'linux', '/home/u'), expected);
  assert.deepEqual(cargoCandidates({ CARGO_HOME: '/home/u/.cargo' }, 'linux', '/home/u'),
    expected);
});

test('cargo resolution selects the first file or reports all searched paths', () => {
  assert.equal(resolveCargo(['/a', '/b', '/c'], file => file !== '/a'), '/b');
  assert.throws(() => resolveCargo(['/a', '/b'], () => false),
    /cargo not found; looked in: \/a, \/b/);
});

test('selection and render detect new Rust and npm dependencies', () => {
  const withTexts = packages => packages.map(p => ({ ...p, texts: ['same'] }));
  const base = output(withTexts(selectCrates([metadata()])), withTexts(selectNpm(npmLock())));
  const rust = output(withTexts(selectCrates([metadata(true)])), withTexts(selectNpm(npmLock())));
  const npm = output(withTexts(selectCrates([metadata()])), withTexts(selectNpm(npmLock({
    'node_modules/new-npm': { version: '1', license: 'MIT' },
  }))));
  assert.notEqual(rust, base);
  assert.match(rust, /target-only/);
  assert.notEqual(npm, base);
  assert.match(npm, /new-npm/);
});

test('pinned SPDX texts match all seven approved raw UTF-8 hashes', () => {
  const expected = {
    'Apache-2.0': '074E6E32C86A4C0EF8B3ED25B721CA23ACA83DF277CD88106EF7177C354615FF',
    'BSD-2-Clause': 'F32FB3B417A194167CFAD068223FC975BA96C5960513A10F66A3C28720AEC1DF',
    'BSD-3-Clause': '5A93D5831E1297AB10FE643E1A631E83BE392896DA14EE2951285A79012DF69D',
    'BSL-1.0': '84C6EF3EA9E3254A54D0ACF5D3E0C61AE011B8FEF7DD6940591CF060E6380A8F',
    MIT: 'B05785F9F18E6716BAB63424B11454513B9943A222595B70411009202FC592B5',
    'MPL-2.0': '66A3107D5AD6A058AAB753EAAC2047CCB2ED0E39465DD0FE5844DA3E300D5172',
    Zlib: 'BFB1112D49DB5B1DAECDFEF24BD7E2F3EA0BAFB33AA67AA0AB51E2BF8407C03D',
  };
  const texts = JSON.parse(fs.readFileSync(
    new URL('./third-party-notices/spdx-texts.json', import.meta.url), 'utf8'));
  assert.deepEqual(Object.keys(texts), Object.keys(expected));
  for (const [id, digest] of Object.entries(expected)) {
    assert.equal(createHash('sha256').update(texts[id], 'utf8').digest('hex').toUpperCase(),
      digest, id);
  }
});
