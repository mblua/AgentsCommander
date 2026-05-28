#!/usr/bin/env node
// Vendored-snapshot generator for issue #271 - Agent Template Picker.
//
// Builds `src-tauri/src/commands/agency_agents_snapshot.json` from a pinned
// commit of msitarzewski/agency-agents. The generated JSON is a committed
// artifact: `role_templates.rs` `include_str!`s it at crate-compile time.
//
// Run MANUALLY by a maintainer (not CI) in a networked environment with `git`
// on PATH; the placeholder snapshot keeps the branch buildable until then.
//
// Usage:
//   node scripts/generate-agency-snapshot.mjs
//   node scripts/generate-agency-snapshot.mjs --ref main
//   node scripts/generate-agency-snapshot.mjs --ref v1.2.3
//   node scripts/generate-agency-snapshot.mjs --ref 1a2b3c4d...40-hex-chars...
//   node scripts/generate-agency-snapshot.mjs --repo https://github.com/foo/bar
//
// Exit codes:
//   0 → snapshot written
//   1 → bad usage, git failure, duplicate id, or no usable templates

import { execFileSync } from 'node:child_process';
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const ROOT       = resolve(dirname(__filename), '..');
const OUT_PATH   = join(ROOT, 'src-tauri', 'src', 'commands', 'agency_agents_snapshot.json');

const DEFAULT_REPO = 'https://github.com/msitarzewski/agency-agents';
const DEFAULT_REF  = 'main';

// Folders that are upstream tooling/examples, not divisions.
const EXCLUDED_TOP = new Set(['scripts', 'integrations', 'examples', 'docs', 'node_modules']);

const SHA_RE = /^[0-9a-f]{40}$/i;

function parseArgs(argv) {
  const out = { ref: DEFAULT_REF, repo: DEFAULT_REPO };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--ref') {
      out.ref = argv[++i];
    } else if (a === '--repo') {
      out.repo = argv[++i];
    } else if (a === '--help' || a === '-h') {
      printHelp();
      process.exit(0);
    } else {
      console.error(`Unknown argument: ${a}`);
      printHelp();
      process.exit(1);
    }
    if (out.ref === undefined || out.repo === undefined) {
      console.error('--ref and --repo require a value');
      process.exit(1);
    }
  }
  return out;
}

function printHelp() {
  console.log(`Usage:
  node scripts/generate-agency-snapshot.mjs [--ref <branch|tag|sha>] [--repo <url>]

Defaults:
  --ref  ${DEFAULT_REF}
  --repo ${DEFAULT_REPO}

Writes:
  ${OUT_PATH}
`);
}

function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { stdio: ['ignore', 'pipe', 'inherit'], ...opts })
    .toString()
    .trim();
}

function cloneRepo(repo, ref, dest) {
  if (SHA_RE.test(ref)) {
    // G11 - `--depth 1 --branch` cannot accept a raw SHA. Full clone, then checkout.
    console.log(`[clone] full clone ${repo} (SHA pin: ${ref})`);
    run('git', ['clone', repo, dest]);
    run('git', ['-C', dest, 'checkout', ref]);
  } else {
    console.log(`[clone] shallow clone ${repo} @ ${ref}`);
    run('git', ['clone', '--depth', '1', '--branch', ref, repo, dest]);
  }
  return run('git', ['-C', dest, 'rev-parse', 'HEAD']);
}

// Minimal hand-parser - mirrors the Rust side. Supports plain, single-quoted,
// and double-quoted scalar values; surrounding quotes are trimmed. No multi-line
// scalars (the upstream templates do not use any).
function parseFrontmatter(text) {
  if (!text.startsWith('---')) {
    return { meta: {}, body: text };
  }
  const rest = text.slice(3).replace(/^\r?\n/, '');
  const end  = rest.indexOf('\n---');
  if (end < 0) {
    return { meta: {}, body: text };
  }
  const headerBlock = rest.slice(0, end);
  let body = rest.slice(end + 4); // skip "\n---"
  if (body.startsWith('\r')) body = body.slice(1);
  if (body.startsWith('\n')) body = body.slice(1);
  const meta = {};
  for (const raw of headerBlock.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const colon = line.indexOf(':');
    if (colon < 0) continue;
    const key = line.slice(0, colon).trim();
    let val   = line.slice(colon + 1).trim();
    if (
      (val.startsWith('"') && val.endsWith('"')) ||
      (val.startsWith("'") && val.endsWith("'"))
    ) {
      val = val.slice(1, -1);
    }
    meta[key] = val;
  }
  return { meta, body };
}

// "paid-media" → "Paid Media"; "engineering" → "Engineering".
function titleCaseDivision(name) {
  return name
    .split('-')
    .map(seg => (seg.length ? seg[0].toUpperCase() + seg.slice(1) : seg))
    .join(' ');
}

// No-em-dash policy (#332): strip U+2014 from catalog text on write so the
// committed snapshot stays ASCII. 1:1 replacement preserves surrounding spacing.
function stripEmDash(s) {
  return typeof s === 'string' ? s.replace(/\u2014/g, '-') : s;
}

function listDivisions(repoDir) {
  return readdirSync(repoDir, { withFileTypes: true })
    .filter(d => d.isDirectory())
    .map(d => d.name)
    .filter(n => !n.startsWith('.'))
    .filter(n => !EXCLUDED_TOP.has(n))
    .sort();
}

function listMarkdownFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) continue; // divisions are one-level deep
    if (!entry.name.toLowerCase().endsWith('.md')) continue;
    if (entry.name.toLowerCase() === 'readme.md') continue;
    out.push(entry.name);
  }
  return out.sort();
}

function buildTemplates(repoDir) {
  const templates = [];
  const seen = new Set();
  const skipped = [];

  for (const division of listDivisions(repoDir)) {
    const dirPath = join(repoDir, division);
    for (const file of listMarkdownFiles(dirPath)) {
      const fullPath = join(dirPath, file);
      let raw;
      try {
        raw = readFileSync(fullPath, 'utf8');
      } catch (e) {
        skipped.push(`${division}/${file}: read failed (${e.message})`);
        continue;
      }
      // Strip a leading BOM so frontmatter detection works on UTF-8-with-BOM.
      if (raw.charCodeAt(0) === 0xfeff) raw = raw.slice(1);

      const { meta, body } = parseFrontmatter(raw);
      const trimmedBody = body.trim();
      if (!meta.name || !meta.name.trim()) {
        skipped.push(`${division}/${file}: missing frontmatter \`name\``);
        continue;
      }
      if (!trimmedBody) {
        skipped.push(`${division}/${file}: empty body`);
        continue;
      }
      const stem = file.replace(/\.md$/i, '');
      // G12 - derive id from <division>-<stem> so uniqueness is structural,
      // not assumed from upstream filename conventions.
      const id = `agency:${division}-${stem}`;
      if (seen.has(id)) {
        console.error(`duplicate template id: ${id}`);
        process.exit(1);
      }
      seen.add(id);
      templates.push({
        id,
        name: stripEmDash(meta.name.trim()),
        description: stripEmDash((meta.description || '').trim()),
        category: titleCaseDivision(division),
        color: meta.color ? meta.color.trim() : null,
        body: stripEmDash(trimmedBody),
      });
    }
  }

  templates.sort((a, b) => {
    const c = a.category.localeCompare(b.category);
    return c !== 0 ? c : a.name.localeCompare(b.name);
  });

  return { templates, skipped };
}

function main() {
  const { ref, repo } = parseArgs(process.argv.slice(2));
  const work = mkdtempSync(join(tmpdir(), 'agency-agents-'));
  const repoDir = join(work, 'repo');
  let commit;
  try {
    commit = cloneRepo(repo, ref, repoDir);
    const { templates, skipped } = buildTemplates(repoDir);
    if (templates.length === 0) {
      console.error('no usable agency templates found - aborting');
      process.exit(1);
    }
    if (skipped.length) {
      console.warn(`[generate] skipped ${skipped.length} file(s):`);
      for (const s of skipped) console.warn(`  - ${s}`);
    }
    // G14 - no `generatedAt`: provenance lives in the pinned `commit` SHA, and
    // omitting the timestamp makes regenerations byte-stable when inputs are.
    const snapshot = {
      source: repo,
      ref,
      commit,
      templateCount: templates.length,
      templates,
    };
    writeFileSync(OUT_PATH, JSON.stringify(snapshot, null, 2) + '\n', 'utf8');
    console.log(
      `wrote ${OUT_PATH} (${templates.length} templates from ${commit.slice(0, 8)})`
    );
  } finally {
    try {
      rmSync(work, { recursive: true, force: true });
    } catch (e) {
      console.warn(`could not clean temp dir ${work}: ${e.message}`);
    }
  }
}

main();
