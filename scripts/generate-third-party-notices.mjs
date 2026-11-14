import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TARGETS = [
  'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu',
  'aarch64-apple-darwin', 'x86_64-apple-darwin',
];
// Vite's preload helper ships in dist/ even though Vite is a development dependency.
const FORCE_NPM = new Set(['node_modules/vite']);
const cmp = (a, b) => {
  if (a < b) return -1;
  return a > b ? 1 : 0;
};
const packageOrder = (a, b) => cmp(a.name, b.name) || cmp(a.version, b.version);

// Resolve cargo without PATH: rustup homes first, then fixed system paths (S4036).
const POSIX_CARGO = ['/usr/bin/cargo', '/usr/local/bin/cargo', '/opt/homebrew/bin/cargo'];
export function cargoCandidates(env = process.env, platform = process.platform,
  home = os.homedir()) {
  const p = platform === 'win32' ? path.win32 : path.posix;
  const exe = platform === 'win32' ? 'cargo.exe' : 'cargo';
  const list = [];
  if (env.CARGO_HOME && p.isAbsolute(env.CARGO_HOME)) {
    list.push(p.join(env.CARGO_HOME, 'bin', exe));
  }
  list.push(p.join(home, '.cargo', 'bin', exe));
  if (platform !== 'win32') list.push(...POSIX_CARGO);
  return [...new Set(list)];
}
export function resolveCargo(candidates = cargoCandidates(),
  isFile = file => fs.statSync(file, { throwIfNoEntry: false })?.isFile() ?? false) {
  const found = candidates.find(isFile);
  if (!found) {
    throw new Error(`cargo not found; looked in: ${candidates.join(', ')}. `
      + 'Install Rust via rustup or set CARGO_HOME');
  }
  return found;
}

export function selectCrates(metadataByTarget) {
  const selected = new Map();
  for (const metadata of metadataByTarget) collectMetadataCrates(metadata, selected);
  if (!selected.size) throw new Error('No Rust crates selected');
  return [...selected.values()].sort(packageOrder);
}

function collectMetadataCrates(metadata, selected) {
  const packages = new Map(metadata.packages.map(pkg => [pkg.id, pkg]));
  const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
  const root = metadata.packages.find(pkg => pkg.name === 'agentscommander');
  if (!root) throw new Error('cargo metadata is missing root agentscommander');
  const seen = new Set();
  const pending = [root.id];
  while (pending.length) {
    const id = pending.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    const pkg = packages.get(id);
    if (!pkg) throw new Error(`cargo metadata is missing package ${id}`);
    if (pkg.targets.some(target => target.kind.includes('proc-macro'))) continue;
    if (pkg.source != null) {
      selected.set(`${pkg.name}@${pkg.version}`, {
        name: pkg.name, version: pkg.version, license: pkg.license,
        authors: pkg.authors ?? [], dir: path.dirname(pkg.manifest_path),
      });
    }
    pushNormalDeps(nodes, id, pending);
  }
}

function pushNormalDeps(nodes, id, pending) {
  const node = nodes.get(id);
  if (!node) throw new Error(`cargo metadata is missing resolve node ${id}`);
  for (const dep of node.deps) {
    if (dep.dep_kinds.some(kind => kind.kind === null)) pending.push(dep.pkg);
  }
}

export function selectNpm(lock) {
  const entries = lock.packages ?? {};
  for (const key of FORCE_NPM) {
    if (!entries[key]) throw new Error(`Missing required npm package ${key}`);
  }
  const selected = Object.entries(entries).filter(([key, pkg]) => {
    if (FORCE_NPM.has(key)) return true;
    return key.startsWith('node_modules/') && pkg.dev !== true
      && !key.includes('node_modules/@types/');
  }).map(([key, pkg]) => ({
    name: key.slice(key.lastIndexOf('node_modules/') + 'node_modules/'.length),
    version: pkg.version, license: pkg.license, authors: [], dir: path.join(ROOT, key),
  }));
  if (!selected.length) throw new Error('No npm packages selected');
  return selected.sort(packageOrder);
}

export function licenseFiles(dirEntries) {
  return dirEntries.filter(entry => entry.isFile()
    && /^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)([-_.].*)?$/i.test(entry.name))
    .map(entry => entry.name).sort(cmp);
}

export function normalize(text) {
  const lines = text.replace(/^\uFEFF/, '').replace(/\r\n?/g, '\n').split('\n');
  const body = lines.map(trimSpaceTab).join('\n');
  return `${trimNewlines(body)}\n`;
}

function trimSpaceTab(line) {
  let end = line.length;
  while (end > 0 && (line[end - 1] === ' ' || line[end - 1] === '\t')) end--;
  return line.slice(0, end);
}

function trimNewlines(text) {
  let end = text.length;
  while (end > 0 && text[end - 1] === '\n') end--;
  return text.slice(0, end);
}

function packageTexts(pkg, texts) {
  if (pkg.texts?.length) return pkg.texts.map(normalize);
  if (!pkg.license?.trim()) throw new Error(`${pkg.name}: no license text or expression`);
  const ids = pkg.license.split(/\b(?:OR|AND|WITH)\b|[/()]/).map(id => id.trim()).filter(Boolean);
  if (!ids.length) throw new Error(`${pkg.name}: empty license expression`);
  const authors = pkg.authors?.length ? pkg.authors.join('; ') : `the ${pkg.name} contributors`;
  return ids.map(id => {
    if (!Object.hasOwn(texts, id)) throw new Error(`${pkg.name}: unknown SPDX id ${id}`);
    return normalize(`Copyright: ${authors}\n\n${texts[id]}`);
  });
}

export function render({ crates, npm, staticSections, texts }) {
  const unique = [];
  const identifiers = new Map();
  const lines = [
    '# Third-party notices', '',
    'Generated by scripts/generate-third-party-notices.mjs.',
    'Regenerate: `node scripts/generate-third-party-notices.mjs`.', '',
  ];
  let output = lines.join('\n') + '\n' + staticSections;
  if (!output.endsWith('\n')) output += '\n';
  for (const [heading, packages] of [['Rust crates', crates], ['npm packages', npm]]) {
    output += `\n## ${heading}\n\n`;
    for (const pkg of [...packages].sort(packageOrder)) {
      const refs = packageTexts(pkg, texts).map(text => {
        if (!identifiers.has(text)) {
          identifiers.set(text, `T${unique.length + 1}`);
          unique.push(text);
        }
        return identifiers.get(text);
      });
      output += `- ${pkg.name} ${pkg.version} — ${pkg.license || 'see text'}`
        + ` — texts: ${refs.join(', ')}\n`;
    }
  }
  output += '\n## License texts\n';
  for (const [index, text] of unique.entries()) {
    const runs = text.match(/`+/g) ?? [];
    const longest = runs.reduce((length, run) => Math.max(length, run.length), 0);
    const fence = '`'.repeat(Math.max(3, longest + 1));
    output += `\n### T${index + 1}\n\n${fence}\n${text}${fence}\n`;
  }
  return output;
}

function loadTexts(packages, ecosystem) {
  return packages.map(pkg => {
    if (!fs.existsSync(pkg.dir)) {
      const hint = ecosystem === 'npm' ? '; run npm ci' : '; run cargo metadata --locked';
      throw new Error(`${pkg.name}: missing package directory${hint}`);
    }
    const files = licenseFiles(fs.readdirSync(pkg.dir, { withFileTypes: true }));
    return { ...pkg, texts: files.map(file => fs.readFileSync(path.join(pkg.dir, file), 'utf8')) };
  });
}

async function main(args) {
  try {
    if (args.length && !(args.length === 1 && args[0] === '--check')) {
      throw new Error(`Unknown arguments: ${args.join(' ')}`);
    }
    const cargo = resolveCargo();
    const cargoArgs = ['metadata', '--format-version', '1', '--locked'];
    const options = { cwd: path.join(ROOT, 'src-tauri'), maxBuffer: 128 * 1024 * 1024 };
    // Populate every target's registry packages before reading license files.
    execFileSync(cargo, cargoArgs, { ...options, stdio: ['ignore', 'ignore', 'inherit'] });
    const metadata = TARGETS.map(target => JSON.parse(execFileSync(
      cargo, [...cargoArgs, '--filter-platform', target],
      { ...options, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] },
    )));
    const crates = loadTexts(selectCrates(metadata), 'Rust');
    const lock = JSON.parse(fs.readFileSync(path.join(ROOT, 'package-lock.json'), 'utf8'));
    const npm = loadTexts(selectNpm(lock), 'npm');
    const dataDir = path.join(ROOT, 'scripts', 'third-party-notices');
    const output = render({
      crates, npm,
      staticSections: fs.readFileSync(path.join(dataDir, 'static-sections.md'), 'utf8'),
      texts: JSON.parse(fs.readFileSync(path.join(dataDir, 'spdx-texts.json'), 'utf8')),
    });
    console.log(`crates=${crates.length} npm=${npm.length}`);
    const destination = path.join(ROOT, 'THIRD_PARTY_NOTICES.md');
    if (!args.length) fs.writeFileSync(destination, output, 'utf8');
    else if (!fs.existsSync(destination)
      || !fs.readFileSync(destination).equals(Buffer.from(output))) {
      console.error('THIRD_PARTY_NOTICES.md is out of date; run node '
        + 'scripts/generate-third-party-notices.mjs and commit the result');
      return 1;
    }
    return 0;
  } catch (error) {
    console.error(error.message);
    return 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
