#!/usr/bin/env node

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CATEGORIES = new Set([
  'ignored-rust-test',
  'placeholder-rust-test',
  'skipped-frontend-test',
  'placeholder-frontend-test',
]);

const EXCLUDED_DIRS = new Set([
  '.ac',
  '.git',
  '_logbooks',
  '_plans',
  'dist',
  'node_modules',
  'target',
]);

function normalizePath(filePath) {
  return filePath.replaceAll(path.sep, '/');
}

function relPath(root, filePath) {
  return normalizePath(path.relative(root, filePath));
}

function lineOf(source, index) {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

function blankPreserveNewlines(text) {
  return text.replace(/[^\n\r]/g, ' ');
}

function isIdentifierChar(ch) {
  return ch !== undefined && /[A-Za-z0-9_]/.test(ch);
}

function isTokenBoundary(source, index) {
  return index === 0 || !isIdentifierChar(source[index - 1]);
}

function rustRawStringStop(source, index) {
  if (!isTokenBoundary(source, index)) return -1;
  let markerStart;
  if (source[index] === 'r') {
    markerStart = index + 1;
  } else if (source[index] === 'b' && source[index + 1] === 'r') {
    markerStart = index + 2;
  } else {
    return -1;
  }

  let quote = markerStart;
  while (source[quote] === '#') quote += 1;
  if (source[quote] !== '"') return -1;

  const hashes = source.slice(markerStart, quote);
  const terminator = `"${hashes}`;
  const end = source.indexOf(terminator, quote + 1);
  return end === -1 ? source.length : end + terminator.length;
}

function quotedStringStop(source, index, quote) {
  let i = index + 1;
  while (i < source.length) {
    const ch = source[i];
    if (ch === '\\') {
      i += 2;
      continue;
    }
    i += 1;
    if (ch === quote) return i;
  }
  return source.length;
}

function rustCharLiteralStop(source, index) {
  if (source[index] !== '\'') return -1;
  let i = index + 1;
  if (i >= source.length || source[i] === '\n' || source[i] === '\r') return -1;

  if (source[i] === '\\') {
    i += 1;
    if (i >= source.length || source[i] === '\n' || source[i] === '\r') return -1;
    if (source[i] === 'x') {
      i += 1;
      for (let count = 0; count < 2; count += 1) {
        if (!/[0-9A-Fa-f]/.test(source[i] ?? '')) return -1;
        i += 1;
      }
    } else if (source[i] === 'u' && source[i + 1] === '{') {
      i += 2;
      while (i < source.length && source[i] !== '}') {
        if (source[i] === '\n' || source[i] === '\r') return -1;
        i += 1;
      }
      if (source[i] !== '}') return -1;
      i += 1;
    } else {
      i += 1;
    }
  } else {
    i += 1;
  }

  return source[i] === '\'' ? i + 1 : -1;
}

function appendLiteral(out, source, start, stop, maskStrings) {
  const slice = source.slice(start, stop);
  return out + (maskStrings ? blankPreserveNewlines(slice) : slice);
}

function maskSource(source, options = {}) {
  const maskStrings = options.maskStrings ?? false;
  const singleQuote = options.singleQuote ?? true;
  let out = '';
  for (let i = 0; i < source.length;) {
    const rawStop = rustRawStringStop(source, i);
    if (rawStop !== -1) {
      out = appendLiteral(out, source, i, rawStop, maskStrings);
      i = rawStop;
      continue;
    }

    if (isTokenBoundary(source, i) && source[i] === 'b' && source[i + 1] === '"') {
      const stop = quotedStringStop(source, i + 1, '"');
      out = appendLiteral(out, source, i, stop, maskStrings);
      i = stop;
      continue;
    }

    if (isTokenBoundary(source, i) && source[i] === 'b' && source[i + 1] === '\'') {
      const stop = rustCharLiteralStop(source, i + 1);
      if (stop !== -1) {
        out = appendLiteral(out, source, i, stop, maskStrings);
        i = stop;
        continue;
      }
    }

    const ch = source[i];
    if (ch === '"' || ch === '`' || (singleQuote && ch === '\'')) {
      const stop = quotedStringStop(source, i, ch);
      out = appendLiteral(out, source, i, stop, maskStrings);
      i = stop;
      continue;
    }

    if (!singleQuote && ch === '\'') {
      const stop = rustCharLiteralStop(source, i);
      if (stop !== -1) {
        out = appendLiteral(out, source, i, stop, maskStrings);
        i = stop;
        continue;
      }
    }

    if (source.startsWith('//', i)) {
      const end = source.indexOf('\n', i);
      if (end === -1) {
        out += blankPreserveNewlines(source.slice(i));
        break;
      }
      out += blankPreserveNewlines(source.slice(i, end)) + '\n';
      i = end + 1;
      continue;
    }
    if (source.startsWith('/*', i)) {
      const end = source.indexOf('*/', i + 2);
      const stop = end === -1 ? source.length : end + 2;
      out += blankPreserveNewlines(source.slice(i, stop));
      i = stop;
      continue;
    }
    out += source[i];
    i += 1;
  }
  return out;
}

function maskComments(source) {
  return maskSource(source, { maskStrings: false });
}

function maskCommentsAndStrings(source, options = {}) {
  return maskSource(source, { ...options, maskStrings: true });
}

function findMatchingBrace(masked, openIndex) {
  let depth = 0;
  for (let i = openIndex; i < masked.length; i += 1) {
    if (masked[i] === '{') depth += 1;
    if (masked[i] === '}') {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

function findMatchingParen(masked, openIndex) {
  let depth = 0;
  for (let i = openIndex; i < masked.length; i += 1) {
    if (masked[i] === '(') depth += 1;
    if (masked[i] === ')') {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

function discoverFiles(root) {
  const files = [];
  const roots = [
    path.join(root, 'src-tauri', 'src'),
    path.join(root, 'src-tauri', 'tests'),
    path.join(root, 'src'),
  ];

  function walk(dir) {
    if (!fs.existsSync(dir)) return;
    const base = path.basename(dir);
    if (EXCLUDED_DIRS.has(base)) return;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile()) {
        const rel = relPath(root, full);
        if (
          (rel.startsWith('src-tauri/src/') && rel.endsWith('.rs')) ||
          (rel.startsWith('src-tauri/tests/') && rel.endsWith('.rs')) ||
          (rel.startsWith('src/') && (rel.endsWith('.test.ts') || rel.endsWith('.test.tsx')))
        ) {
          files.push(full);
        }
      }
    }
  }

  for (const scanRoot of roots) walk(scanRoot);
  files.sort((a, b) => relPath(root, a).localeCompare(relPath(root, b)));
  return files;
}

function moduleRanges(masked) {
  const ranges = [];
  const modRe = /\bmod\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/g;
  let match;
  while ((match = modRe.exec(masked)) !== null) {
    const open = masked.indexOf('{', match.index);
    const close = findMatchingBrace(masked, open);
    if (close !== -1) {
      ranges.push({ name: match[1], start: open, end: close });
    }
  }
  return ranges;
}

function rustTestId(rel, modules, fnName) {
  const parts = [rel];
  if (!rel.startsWith('src-tauri/tests/')) {
    parts.push(...modules);
  } else if (modules.length > 0) {
    parts.push(...modules);
  }
  parts.push(fnName);
  return `rust:${parts.join('::')}`;
}

function hasExecutableRustBody(originalBody, strippedBody) {
  const withoutComments = strippedBody.trim().replace(/;/g, '').trim();
  if (withoutComments.length === 0) return false;
  const compact = maskComments(originalBody).trim();
  if (/^(todo!\s*\(\s*\)|unimplemented!\s*\(\s*\)|panic!\s*\(\s*["'`]TODO[\s\S]*["'`]\s*\))\s*;?$/.test(compact)) {
    return false;
  }
  if (/\b(?:assert|assert_eq|assert_ne|debug_assert|debug_assert_eq|debug_assert_ne)!\s*\(/.test(withoutComments)) return true;
  if (/\?/.test(withoutComments)) return true;
  if (/\.(?:unwrap|expect)\s*\(/.test(withoutComments)) return true;
  if (/(^|[^A-Za-z0-9_])(?:[A-Za-z_][A-Za-z0-9_]*::)*[A-Za-z_][A-Za-z0-9_]*!\s*[\(\[]/.test(withoutComments)) return true;
  if (/(^|[^A-Za-z0-9_])(?:[A-Za-z_][A-Za-z0-9_]*::)*[A-Za-z_][A-Za-z0-9_]*\s*\(/.test(withoutComments)) return true;
  if (/\.[A-Za-z_][A-Za-z0-9_]*\s*\(/.test(withoutComments)) return true;
  return false;
}

function scanRustFile(root, filePath) {
  const source = fs.readFileSync(filePath, 'utf8');
  const rel = relPath(root, filePath);
  const maskedComments = maskComments(source);
  const masked = maskCommentsAndStrings(source, { singleQuote: false });
  const modules = moduleRanges(masked);
  const findings = [];
  const warnings = [];
  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*)*)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>)?\s*\(/g;
  let match;

  while ((match = fnRe.exec(masked)) !== null) {
    const attrs = match[1] || '';
    if (!/#\s*\[\s*test\b/.test(attrs)) continue;
    const fnName = match[2];
    const fnStart = match.index + match[0].lastIndexOf('fn ');
    const bodyOpen = masked.indexOf('{', fnRe.lastIndex);
    if (bodyOpen === -1) {
      warnings.push({
        category: 'parse-warning',
        id: `parse:${rel}::${fnName}`,
        file: rel,
        line: lineOf(source, match.index),
        message: 'Rust test body open brace not found',
      });
      continue;
    }
    let bodyClose = findMatchingBrace(masked, bodyOpen);
    let usedFallbackClose = false;
    if (bodyClose === -1) {
      const nextTest = maskedComments.slice(bodyOpen + 1).search(/#\s*\[\s*test\b/);
      if (nextTest !== -1) {
        bodyClose = bodyOpen + 1 + nextTest;
        usedFallbackClose = true;
      } else {
        bodyClose = source.length;
        usedFallbackClose = true;
      }
    }

    const modulePath = modules
      .filter((mod) => mod.start < fnStart && fnStart < mod.end)
      .sort((a, b) => a.start - b.start)
      .map((mod) => mod.name);
    const id = rustTestId(rel, modulePath, fnName);
    const line = lineOf(source, match.index);
    const hasIgnore = /#\s*\[\s*ignore(?:\s|\]|=)/.test(attrs);
    const originalBody = source.slice(bodyOpen + 1, bodyClose);
    const strippedBody = maskCommentsAndStrings(originalBody, { singleQuote: false });
    const executable = hasExecutableRustBody(originalBody, strippedBody);

    if (hasIgnore) {
      findings.push({ category: 'ignored-rust-test', id, file: rel, line });
    }
    if (!executable) {
      if (usedFallbackClose) {
        warnings.push({
          category: 'parse-warning',
          id: `parse:${rel}::${fnName}`,
          file: rel,
          line,
          message: 'Rust test body close brace not found for a possible placeholder',
        });
      }
      findings.push({ category: 'placeholder-rust-test', id, file: rel, line });
    }
  }

  return { findings, warnings };
}

function describeRanges(source) {
  const masked = maskCommentsAndStrings(source);
  const ranges = [];
  const re = /\bdescribe(?:\s*\.\s*(?:only|skip|concurrent|each))*\s*\(\s*(['"`])([^'"`]+)\1[\s\S]*?\{/g;
  let match;
  while ((match = re.exec(source)) !== null) {
    const open = masked.indexOf('{', match.index);
    if (open === -1) continue;
    const close = findMatchingBrace(masked, open);
    if (close !== -1) ranges.push({ name: match[2], start: open, end: close });
  }
  return ranges;
}

function frontendId(rel, suite, name) {
  const parts = [rel, ...suite, name].filter(Boolean);
  return `frontend:${parts.join('::')}`;
}

function suiteAt(ranges, index) {
  return ranges
    .filter((range) => range.start < index && index < range.end)
    .sort((a, b) => a.start - b.start)
    .map((range) => range.name);
}

function skipWhitespace(source, index) {
  let i = index;
  while (i < source.length && /\s/.test(source[i])) i += 1;
  return i;
}

function readStringLiteral(source, index) {
  let i = skipWhitespace(source, index);
  const quote = source[i];
  if (quote !== '"' && quote !== '\'' && quote !== '`') return null;
  i += 1;
  let value = '';
  while (i < source.length) {
    const ch = source[i];
    if (ch === '\\') {
      if (i + 1 < source.length) value += source[i + 1];
      i += 2;
      continue;
    }
    if (ch === quote) {
      return { value, end: i + 1 };
    }
    value += ch;
    i += 1;
  }
  return null;
}

function addFrontendFinding(findings, category, source, rel, ranges, matchIndex, nameStart) {
  const literal = readStringLiteral(source, nameStart);
  if (!literal) return null;
  const finding = {
    category,
    id: frontendId(rel, suiteAt(ranges, matchIndex), literal.value),
    file: rel,
    line: lineOf(source, matchIndex),
  };
  if (!findings.some((item) => item.category === finding.category && item.id === finding.id)) {
    findings.push(finding);
  }
  return { finding, literal };
}

function addFrontendSkippedFindings(source, rel, ranges, findings) {
  const masked = maskCommentsAndStrings(source);

  let direct;
  const directRe = /\b(describe|it|test)(?:\s*\.\s*(?:only|concurrent))*\s*\.\s*(?:skip|todo)\s*\(/g;
  while ((direct = directRe.exec(masked)) !== null) {
    addFrontendFinding(findings, 'skipped-frontend-test', source, rel, ranges, direct.index, directRe.lastIndex);
  }

  let skipEach;
  const skipEachRe = /\b(describe|it|test)(?:\s*\.\s*(?:only|concurrent))*\s*\.\s*(?:skip|todo)\s*\.\s*each\s*\(/g;
  while ((skipEach = skipEachRe.exec(masked)) !== null) {
    const eachClose = findMatchingParen(masked, skipEachRe.lastIndex - 1);
    if (eachClose === -1) continue;
    const nameOpen = masked.indexOf('(', eachClose + 1);
    if (nameOpen !== -1) {
      addFrontendFinding(findings, 'skipped-frontend-test', source, rel, ranges, skipEach.index, nameOpen + 1);
    }
  }

  let eachSkip;
  const eachSkipRe = /\b(describe|it|test)(?:\s*\.\s*(?:only|concurrent))*\s*\.\s*each\s*\(/g;
  while ((eachSkip = eachSkipRe.exec(masked)) !== null) {
    const eachClose = findMatchingParen(masked, eachSkipRe.lastIndex - 1);
    if (eachClose === -1) continue;
    const suffix = masked.slice(eachClose + 1).match(/^\s*\.\s*(?:skip|todo)\s*\(/);
    if (suffix) {
      addFrontendFinding(
        findings,
        'skipped-frontend-test',
        source,
        rel,
        ranges,
        eachSkip.index,
        eachClose + 1 + suffix[0].length,
      );
    }
  }
}

function callbackBody(source, masked, fromIndex) {
  const open = masked.indexOf('{', fromIndex);
  if (open === -1) return null;
  const close = findMatchingBrace(masked, open);
  if (close === -1) return null;
  return {
    start: open,
    end: close,
    body: source.slice(open + 1, close),
    maskedBody: maskComments(source.slice(open + 1, close)),
  };
}

function isFrontendPlaceholder(body) {
  const trimmed = maskComments(body).trim();
  if (trimmed === '') return true;
  if (/^return\s*;?$/.test(trimmed)) return true;
  if (/^expect\s*\(\s*true\s*\)\s*\.\s*toBe\s*\(\s*true\s*\)\s*;?$/.test(trimmed)) return true;
  if (/^assert\s*\.\s*ok\s*\(\s*true\s*\)\s*;?$/.test(trimmed)) return true;
  return false;
}

function addFrontendPlaceholderFindings(source, rel, ranges, findings, warnings) {
  const masked = maskCommentsAndStrings(source);
  const directRe = /\b(it|test)(?:\s*\.\s*(?:only|concurrent))*\s*\(/g;
  let match;
  while ((match = directRe.exec(masked)) !== null) {
    const added = addFrontendFinding([], 'placeholder-frontend-test', source, rel, ranges, match.index, directRe.lastIndex);
    if (!added) continue;
    const comma = masked.indexOf(',', added.literal.end);
    if (comma === -1) continue;
    const body = callbackBody(source, masked, comma + 1);
    if (!body) {
      continue;
    }
    if (isFrontendPlaceholder(body.body)) {
      findings.push({
        category: 'placeholder-frontend-test',
        id: added.finding.id,
        file: rel,
        line: lineOf(source, match.index),
      });
    }
  }

  const eachRe = /\b(it|test)(?:\s*\.\s*(?:only|concurrent))*\s*\.\s*each\s*\(/g;
  while ((match = eachRe.exec(masked)) !== null) {
    const eachClose = findMatchingParen(masked, eachRe.lastIndex - 1);
    if (eachClose === -1) continue;
    if (/^\s*\.\s*(?:skip|todo)\s*\(/.test(masked.slice(eachClose + 1))) continue;
    const nameOpen = masked.indexOf('(', eachClose + 1);
    if (nameOpen === -1) continue;
    const added = addFrontendFinding([], 'placeholder-frontend-test', source, rel, ranges, match.index, nameOpen + 1);
    if (!added) continue;
    const comma = masked.indexOf(',', added.literal.end);
    if (comma === -1) continue;
    const body = callbackBody(source, masked, comma + 1);
    if (!body) continue;
    if (isFrontendPlaceholder(body.body)) {
      findings.push({
        category: 'placeholder-frontend-test',
        id: added.finding.id,
        file: rel,
        line: lineOf(source, match.index),
      });
    }
  }
}

function scanFrontendFile(root, filePath) {
  const source = fs.readFileSync(filePath, 'utf8');
  const rel = relPath(root, filePath);
  const ranges = describeRanges(source);
  const findings = [];
  const warnings = [];
  addFrontendSkippedFindings(source, rel, ranges, findings);
  addFrontendPlaceholderFindings(source, rel, ranges, findings, warnings);
  return { findings, warnings };
}

function scan(root) {
  const findings = [];
  const warnings = [];
  for (const file of discoverFiles(root)) {
    const rel = relPath(root, file);
    const result = rel.endsWith('.rs') ? scanRustFile(root, file) : scanFrontendFile(root, file);
    findings.push(...result.findings);
    warnings.push(...result.warnings);
  }
  findings.sort((a, b) => `${a.category}:${a.id}`.localeCompare(`${b.category}:${b.id}`));
  warnings.sort((a, b) => a.id.localeCompare(b.id));
  return { findings, warnings };
}

function keyOf(item) {
  return `${item.category}\u0000${item.id}`;
}

function loadAllowlist(filePath) {
  const raw = fs.readFileSync(filePath, 'utf8');
  const parsed = JSON.parse(raw);
  const errors = [];
  if (!parsed || parsed.version !== 1 || !Array.isArray(parsed.entries)) {
    errors.push('allowlist must be an object with version 1 and entries array');
    return { entries: [], errors };
  }

  const seen = new Set();
  const entries = [];
  parsed.entries.forEach((entry, index) => {
    const prefix = `entries[${index}]`;
    if (!entry || typeof entry !== 'object') {
      errors.push(`${prefix} must be an object`);
      return;
    }
    for (const field of ['id', 'category', 'owner', 'reason', 'resolution']) {
      if (typeof entry[field] !== 'string' || entry[field].trim() === '') {
        errors.push(`${prefix}.${field} must be a non-empty string`);
      }
    }
    if (!Number.isInteger(entry.issue) || entry.issue <= 0) {
      errors.push(`${prefix}.issue must be a positive integer`);
    }
    if (!CATEGORIES.has(entry.category)) {
      errors.push(`${prefix}.category must be one of ${Array.from(CATEGORIES).join(', ')}`);
    }
    const key = keyOf(entry);
    if (seen.has(key)) {
      errors.push(`${prefix} duplicates category/id ${entry.category} ${entry.id}`);
    }
    seen.add(key);
    entries.push(entry);
  });
  return { entries, errors };
}

function compare(findings, warnings, entries) {
  const findingKeys = new Set(findings.map(keyOf));
  const allowKeys = new Set(entries.map(keyOf));
  const unallowlisted = findings.filter((finding) => !allowKeys.has(keyOf(finding)));
  const stale = entries.filter((entry) => !findingKeys.has(keyOf(entry)));
  const allowlisted = findings.filter((finding) => allowKeys.has(keyOf(finding)));
  return { allowlisted, unallowlisted, stale, warnings };
}

function summarizeCategory(findings, comparison, category) {
  const discovered = findings.filter((item) => item.category === category).length;
  const allowlisted = comparison.allowlisted.filter((item) => item.category === category).length;
  const unallowlisted = comparison.unallowlisted.filter((item) => item.category === category).length;
  return { discovered, allowlisted, unallowlisted };
}

function printReport(findings, comparison, shapeErrors) {
  const ignoredRust = summarizeCategory(findings, comparison, 'ignored-rust-test');
  const placeholders = findings.filter((item) => item.category.includes('placeholder'));
  const placeholderAllowlisted = comparison.allowlisted.filter((item) => item.category.includes('placeholder')).length;
  const placeholderUnallowlisted = comparison.unallowlisted.filter((item) => item.category.includes('placeholder')).length;
  const skippedFrontend = summarizeCategory(findings, comparison, 'skipped-frontend-test');

  console.log(`Ignored Rust tests: ${ignoredRust.discovered} discovered, ${ignoredRust.allowlisted} allowlisted, ${ignoredRust.unallowlisted} unallowlisted`);
  console.log(`Placeholder tests: ${placeholders.length} discovered, ${placeholderAllowlisted} allowlisted, ${placeholderUnallowlisted} unallowlisted`);
  console.log(`Skipped frontend tests: ${skippedFrontend.discovered} discovered, ${skippedFrontend.allowlisted} allowlisted, ${skippedFrontend.unallowlisted} unallowlisted`);

  for (const finding of findings) {
    const status = comparison.allowlisted.some((item) => keyOf(item) === keyOf(finding)) ? 'allowlisted' : 'unallowlisted';
    console.log(`${status}: ${finding.category} ${finding.id} (${finding.file}:${finding.line})`);
  }
  for (const warning of comparison.warnings) {
    console.log(`parse-warning: ${warning.id} (${warning.file}:${warning.line}) ${warning.message}`);
  }
  for (const stale of comparison.stale) {
    console.log(`stale-allowlist: ${stale.category} ${stale.id}`);
  }
  for (const error of shapeErrors) {
    console.log(`allowlist-error: ${error}`);
  }
}

function runGuard(options) {
  const root = path.resolve(options.root ?? process.cwd());
  const allowlistPath = path.resolve(root, options.allowlist ?? 'test-debt.allowlist.json');
  const { findings, warnings } = scan(root);
  const { entries, errors } = loadAllowlist(allowlistPath);
  const comparison = compare(findings, warnings, entries);
  printReport(findings, comparison, errors);
  return errors.length === 0 &&
    comparison.unallowlisted.length === 0 &&
    comparison.stale.length === 0 &&
    comparison.warnings.length === 0 ? 0 : 1;
}

function writeFile(filePath, content) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, content);
}

function writeAllowlist(root, entries) {
  const file = path.join(root, 'test-debt.allowlist.json');
  fs.writeFileSync(file, `${JSON.stringify({ version: 1, entries }, null, 2)}\n`);
  return file;
}

function assertSelf(condition, message) {
  if (!condition) throw new Error(message);
}

function runFixture(root, entries = []) {
  writeAllowlist(root, entries);
  const { findings, warnings } = scan(root);
  const { entries: allowEntries, errors } = loadAllowlist(path.join(root, 'test-debt.allowlist.json'));
  const comparison = compare(findings, warnings, allowEntries);
  return {
    findings,
    warnings,
    errors,
    comparison,
    code: errors.length === 0 &&
      comparison.unallowlisted.length === 0 &&
      comparison.stale.length === 0 &&
      comparison.warnings.length === 0 ? 0 : 1,
  };
}

function selfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ac-test-debt-'));
  try {
    writeFile(path.join(root, 'src-tauri/tests/clean.rs'), `
#[test]
fn clean_rust() {
    assert_eq!(1, 1);
}

#[test]
fn string_with_line_comment_marker_is_not_placeholder() {
    let s = "see https://example.com/x for detail";
    assert_eq!(s.len() > 0, true);
}

#[test]
fn string_with_block_comment_marker_is_not_placeholder() {
    let s = "literal /* marker */ stays data";
    let raw = r#"raw /* marker */ and https://example.com/x"#;
    assert_eq!(s.len() > 0, true);
    assert_eq!(raw.len() > 0, true);
}
`);
    writeFile(path.join(root, 'src/clean.test.ts'), `
import { describe, expect, it } from "vitest";
describe("clean suite", () => {
  it("clean frontend", () => {
    expect(1).toBe(1);
  });
});
`);
    let result = runFixture(root);
    assertSelf(result.code === 0, 'clean fixture should pass');
    assertSelf(
      !result.findings.some((f) => f.category === 'placeholder-rust-test' && f.id.endsWith('clean.rs::string_with_line_comment_marker_is_not_placeholder')),
      'line comment marker in Rust string false positive detected',
    );
    assertSelf(
      !result.findings.some((f) => f.category === 'placeholder-rust-test' && f.id.endsWith('clean.rs::string_with_block_comment_marker_is_not_placeholder')),
      'block comment marker in Rust string false positive detected',
    );

    writeFile(path.join(root, 'src-tauri/tests/ignored.rs'), `
#[test]
#[ignore = "manual"]
fn ignored_case() {
    assert!(true);
}

#[test]
fn empty_case() {}

#[test]
fn comment_only_case() {
    /* future */
}

#[test]
fn assignment_only_placeholder() {
    let value = 1;
    let _copy = value;
}

#[test]
#[ignore = "manual placeholder"]
fn ignored_empty_case() {}
`);
    writeFile(path.join(root, 'src/frontend/example.test.ts'), `
import { describe, expect, it, test } from "vitest";
describe("frontend debt", () => {
  it.skip("skipped", () => {});
  it.concurrent.skip("concurrent skipped", () => {});
  test.todo("todo");
  test.skip.each([[1]])("skip each", () => {});
  test.each([[1]]).skip("each skip", () => {});
  describe.skip.each([[1]])("describe skip each", () => {});
  it("empty", () => {});
  test.each([[1]])("empty parameterized %s", () => {});
  test("tautology", () => {
    expect(true).toBe(true);
  });
  // Example only: it.skip("not real debt", () => {});
});
`);
    writeFile(path.join(root, 'src-tauri/tests/comment_false_positive.rs'), `
// #[ignore]
#[test]
fn real_test() {
    assert!(true);
}
`);
    result = runFixture(root);
    assertSelf(result.code === 1, 'debt fixture without allowlist should fail');
    assertSelf(result.findings.some((f) => f.category === 'ignored-rust-test' && f.id.endsWith('ignored.rs::ignored_case')), 'ignored Rust finding missing');
    assertSelf(result.findings.some((f) => f.category === 'placeholder-rust-test' && f.id.endsWith('ignored.rs::empty_case')), 'empty Rust finding missing');
    assertSelf(result.findings.some((f) => f.category === 'placeholder-rust-test' && f.id.endsWith('ignored.rs::comment_only_case')), 'comment-only Rust finding missing');
    assertSelf(result.findings.some((f) => f.category === 'placeholder-rust-test' && f.id.endsWith('ignored.rs::assignment_only_placeholder')), 'assignment-only Rust finding missing');
    assertSelf(result.findings.some((f) => f.category === 'skipped-frontend-test' && f.id.includes('skip each')), 'skip.each finding missing');
    assertSelf(result.findings.some((f) => f.category === 'skipped-frontend-test' && f.id.includes('each skip')), 'each(...).skip finding missing');
    assertSelf(result.findings.some((f) => f.category === 'placeholder-frontend-test' && f.id.includes('empty parameterized')), 'parameterized frontend placeholder missing');
    assertSelf(!result.findings.some((f) => f.category === 'skipped-frontend-test' && f.id.includes('not real debt')), 'commented frontend skip false positive detected');
    assertSelf(!result.findings.some((f) => f.id.endsWith('comment_false_positive.rs::real_test') && f.category === 'ignored-rust-test'), 'comment false positive detected');

    const entries = result.findings.map((finding) => ({
      id: finding.id,
      category: finding.category,
      owner: finding.category.includes('frontend') ? 'dev-webpage-ui' : 'dev-rust',
      issue: 489,
      reason: `Self-test allowlist for ${finding.category}.`,
      resolution: 'Self-test fixture debt.',
    }));
    result = runFixture(root, entries);
    assertSelf(result.code === 0, 'exact allowlist should pass');

    const ignoredOnly = entries.filter((entry) => !(entry.category === 'placeholder-rust-test' && entry.id.endsWith('ignored.rs::ignored_empty_case')));
    result = runFixture(root, ignoredOnly);
    assertSelf(result.code === 1, 'ignored empty Rust test needs separate placeholder allowlist entry');
    assertSelf(result.comparison.unallowlisted.some((f) => f.category === 'placeholder-rust-test' && f.id.endsWith('ignored.rs::ignored_empty_case')), 'missing placeholder debt was not reported');

    result = runFixture(root, entries.filter((entry) => !entry.id.endsWith('ignored.rs::ignored_case')));
    assertSelf(result.code === 1, 'missing allowlist entry should fail');

    result = runFixture(root, [...entries, {
      id: 'rust:src-tauri/tests/stale.rs::stale_case',
      category: 'ignored-rust-test',
      owner: 'dev-rust',
      issue: 489,
      reason: 'stale entry self-test',
      resolution: 'remove stale entry',
    }]);
    assertSelf(result.code === 1 && result.comparison.stale.length === 1, 'stale allowlist entry should fail');

    const missingReason = entries.map((entry, index) => index === 0 ? { ...entry, reason: '' } : entry);
    result = runFixture(root, missingReason);
    assertSelf(result.code === 1 && result.errors.some((error) => error.includes('reason')), 'missing rationale should fail');

    const duplicate = [...entries, entries[0]];
    result = runFixture(root, duplicate);
    assertSelf(result.code === 1 && result.errors.some((error) => error.includes('duplicates')), 'duplicate category/id should fail');

    console.log('check-test-debt self-test passed');
    return 0;
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--self-test') {
      options.selfTest = true;
    } else if (arg === '--root') {
      i += 1;
      options.root = argv[i];
    } else if (arg === '--allowlist') {
      i += 1;
      options.allowlist = argv[i];
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

try {
  const options = parseArgs(process.argv.slice(2));
  process.exitCode = options.selfTest ? selfTest() : runGuard(options);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
