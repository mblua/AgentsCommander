#!/usr/bin/env node
// Validates SKILL.md structure the way AgentsCommander's indexer does, for issue #1213.
//
// SOURCE OF TRUTH: src-tauri/src/config/session_context.rs:206-714
// This script is a second, independent implementation of the rules that module enforces.
// If that range changes, this file is stale until it is changed to match.
//
// Discovery is case-insensitive so a wrongly cased entrypoint is FOUND; validation is
// byte-exact because session_context.rs:401 compares entry.file_name() against "SKILL.md"
// byte-for-byte, so wrong casing fails on every platform including Windows.
//
// The two path constants are NOT symmetric, and this is the surface's biggest trap:
//   "SKILL.md" (:401) is an entry-name comparison  -> case-sensitive on EVERY platform.
//   "skills"   (:509) is a path join               -> case-INsensitive on Windows only.
// So `_agent_x/Skills/y/SKILL.md` is indexed on Windows and invisible on Linux, while
// `_agent_x/skills/y/skill.md` is rejected on both.
//
// YAML semantics below are pinned to serde_yaml 0.9.34+deprecated (Cargo.lock:5483-5486):
// duplicate mapping keys are a HARD parse error (mapping.rs:813-822), and plain
// yes/no/on/off resolve as STRINGS, not booleans (de.rs:932-938, YAML 1.2 core schema).
// A serde_yaml bump invalidates both.
//
// Usage:
//   node scripts/01-skills-checker.mjs [root] [--json]
//   node scripts/01-skills-checker.mjs --help
//   node scripts/01-skills-checker.mjs --self-test
//
// Exit codes:
//   0 → no error findings (warnings and notes may still be present)
//   1 → at least one error finding: one or more skills would not be indexed
//   2 → usage error, or the root does not exist / is not a directory / cannot be read

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const TOOL = '01-skills-checker';
const TAG = '[skills-check]';
const SOURCE_OF_TRUTH = 'src-tauri/src/config/session_context.rs:206-714';
const OUTPUT_VERSION = 1;

const SKILL_MD = 'SKILL.md';
const SKILLS_DIR = 'skills';
const FRONTMATTER_MAX_BYTES = 16384;
const FIRST_LINE_MAX_BYTES = 1024;
const NAME_RE = /^[a-z0-9-]{1,64}$/;
const SKIP_DIRS = new Set(['.git', 'node_modules', 'target']);

const MATRIX_DIR_RE = /^_agent_.+$/;
const REPLICA_PREFIX = '__agent_';
const READ_CHUNK_BYTES = 65536;

// ---------------------------------------------------------------------------
// Primitive helpers (Section 5.1.2 constraints; each is a correctness rule)
// ---------------------------------------------------------------------------

// Unicode White_Space exactly, mirroring Rust str::trim used by yaml_field_string (:453).
// U+180E is deliberately absent: removed from White_Space in Unicode 6.3.
// String.prototype.trim() is NOT equivalent (it strips U+FEFF, which Rust keeps, and
// keeps U+0085, which Rust strips) and is banned for field values.
function isUnicodeWhitespace(code) {
  return (
    (code >= 0x0009 && code <= 0x000d) ||
    code === 0x0020 ||
    code === 0x0085 ||
    code === 0x00a0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function trimYamlValue(value) {
  const chars = [...value];
  let start = 0;
  let end = chars.length;
  while (start < end && isUnicodeWhitespace(chars[start].codePointAt(0))) start += 1;
  while (end > start && isUnicodeWhitespace(chars[end - 1].codePointAt(0))) end -= 1;
  return chars.slice(start, end).join('');
}

// A-Z to a-z only, mirroring Rust to_ascii_lowercase(). String.prototype.toLowerCase()
// is locale- and Unicode-aware and would diverge (:588-591).
function asciiLower(value) {
  let out = '';
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i);
    out += code >= 0x41 && code <= 0x5a ? String.fromCharCode(code + 0x20) : value[i];
  }
  return out;
}

// Rust String: Ord compares UTF-8 bytes; JS `<` compares UTF-16 code units and sorts
// astral characters below U+E000-U+FFFF. The duplicate-name race depends on this order.
function compareUtf8(a, b) {
  return Buffer.compare(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8'));
}

// Presentation only, never a verdict. Mirrors sanitize_skill_metadata_for_context
// (:420-446): collapse every run of C0 controls and Unicode whitespace to one U+0020,
// drop the remaining non-printables, trim, then truncate. Truncation alone does nothing
// about a `\n` or a `\x1b` at offset 10, which is why the collapse comes first.
function sanitizeForReport(value) {
  const text = String(value);
  let out = '';
  let pendingSpace = false;
  for (const ch of text) {
    const code = ch.codePointAt(0);
    if (code <= 0x1f || isUnicodeWhitespace(code)) {
      pendingSpace = true;
      continue;
    }
    if (code === 0x7f || (code >= 0x80 && code <= 0x9f)) continue;
    if (pendingSpace && out.length > 0) out += ' ';
    pendingSpace = false;
    out += ch;
  }
  return [...out].slice(0, 200).join('');
}

function errnoOf(err) {
  if (err && typeof err.code === 'string' && err.code.length > 0) return err.code;
  if (err && typeof err.message === 'string') return err.message;
  return String(err);
}

function toPosix(value) {
  return value.split(path.sep).join('/');
}

function relativeTo(root, target) {
  const rel = path.relative(root, target);
  if (rel === '') return '.';
  return toPosix(rel);
}

function plural(count, word) {
  return `${count} ${word}${count === 1 ? '' : 's'}`;
}

// ---------------------------------------------------------------------------
// Finding taxonomy (Section 6.8)
// ---------------------------------------------------------------------------

const SEVERITY = { error: 'error', warning: 'warning', info: 'info' };

const RULE_OF = {
  'E-SKILLS-DIR-UNREADABLE': 'R4',
  'E-SKILL-DIR-UNREADABLE': 'R4',
  'E-SKILLS-NOT-DIR': 'R5',
  'E-SKILL-DIR-LINK': 'R8',
  'E-NO-ENTRYPOINT': 'E1',
  'E-ENTRYPOINT-CASE': 'E1',
  'E-ENTRYPOINT-LINK': 'E2',
  'E-ENTRYPOINT-NOT-FILE': 'E3',
  'E-ENTRYPOINT-UNREADABLE': 'E4',
  'E-FM-NO-OPEN': 'F1',
  'E-FM-FIRST-LINE-TOO-LONG': 'F7',
  'E-FM-NO-CLOSE': 'F5',
  'E-FM-TOO-LARGE': 'F6',
  'E-FM-NOT-UTF8': 'F8',
  'E-YAML-PARSE': 'Y1',
  'E-YAML-DUPLICATE-KEY': 'Y1',
  'E-YAML-NOT-MAPPING': 'Y2',
  'E-NAME-NOT-STRING': 'N2',
  'E-NAME-INVALID': 'N3',
  'E-NAME-DUPLICATE': 'N5',
  'W-DESC-MISSING': 'D1',
  'W-DESC-NOT-STRING': 'D2',
  'W-WHEN-NOT-STRING': 'W2',
  'W-YAML-UNDECIDABLE': null,
  'W-NAME-UNDECIDABLE': 'N3',
  'W-ENTRYPOINT-CASE-SHADOWED': null,
  'W-SKILL-DIR-LINK': 'R8',
  'W-DIR-UNREADABLE': 'R4',
  'I-NOT-INDEXABLE': 'R1',
  'I-NO-ENTRYPOINT': 'E1',
  'I-KEY-HYPHEN-VARIANT': 'X1',
};

function severityOf(code) {
  if (code.startsWith('E-')) return SEVERITY.error;
  if (code.startsWith('W-')) return SEVERITY.warning;
  return SEVERITY.info;
}

// The `Skipped skill directory` / `Skipped skill` prefixes are the real wrappers at
// :601 and :613; every embedded value is sanitized there too (:596, :603, :615).
function skippedDirectory(folder, tail) {
  return `Skipped skill directory \`${sanitizeForReport(folder)}\`: ${tail}`;
}

function skippedSkill(folder, tail) {
  return `Skipped skill \`${sanitizeForReport(folder)}\`: ${tail}`;
}

// ---------------------------------------------------------------------------
// Report collector
// ---------------------------------------------------------------------------

function createCollector(root) {
  const findings = [];
  let seq = 0;
  return {
    findings,
    add(finding) {
      const code = finding.code;
      const absolutePath = finding.absolutePath;
      findings.push({
        seq: seq++,
        code,
        severity: severityOf(code),
        rule: Object.prototype.hasOwnProperty.call(RULE_OF, code) ? RULE_OF[code] : null,
        path: relativeTo(root, absolutePath),
        absolutePath,
        skillDirectory:
          finding.skillDirectory === undefined || finding.skillDirectory === null
            ? null
            : relativeTo(root, finding.skillDirectory),
        skillName: finding.skillName === undefined ? null : finding.skillName,
        line: finding.line === undefined ? null : finding.line,
        reason: finding.reason === undefined ? null : finding.reason,
        message: finding.message,
        indexerMessage: finding.indexerMessage === undefined ? null : finding.indexerMessage,
      });
    },
  };
}

// ---------------------------------------------------------------------------
// 6.7 Position and indexability
// ---------------------------------------------------------------------------

function classifyPosition(entrypointPath) {
  const skillDir = path.dirname(entrypointPath);
  const parent = path.dirname(skillDir);
  const grandparent = path.dirname(parent);
  const parentName = path.basename(parent);
  const ownerName = path.basename(grandparent);

  if (asciiLower(parentName) !== SKILLS_DIR) {
    return { canonical: false, reason: 'not-under-skills-dir' };
  }
  if (parentName !== SKILLS_DIR) {
    return { canonical: false, reason: 'skills-dir-case' };
  }
  if (MATRIX_DIR_RE.test(ownerName)) {
    return { canonical: true, reason: null };
  }
  if (ownerName.startsWith(REPLICA_PREFIX)) {
    return { canonical: false, reason: 'replica-root' };
  }
  return { canonical: false, reason: 'owner-root-not-recognized' };
}

const POSITION_MESSAGE = {
  'not-under-skills-dir':
    'Depth is fixed at one: <owner-root>/skills/<skill-name>/SKILL.md. This file is structurally checkable but is never indexed.',
  'skills-dir-case':
    'The parent directory matches `skills` only after ASCII lowercasing. The indexer builds this path by joining the constant `skills`, so a differently cased directory may still resolve on Windows and will not on Linux.',
  'replica-root':
    'A `__agent_*` replica\'s own `skills/` directory is never scanned. has_agent_matrix_dir_name tests starts_with("_agent_") and "__agent_x" fails it (:1437-1442).',
  'owner-root-not-recognized':
    'The owner root is not an `_agent_*` matrix directory. The Root Agent directory is a third legal owner root that this checker does not attempt to identify, so this note may be a false positive there. It is informational only and never affects the exit code.',
};

// D is a SKILL DIRECTORY in canonical position. `skills` byte-exact: absence-type
// findings must not fail a run differently on Windows and Linux (Section 4.1 rule 2).
function isCanonicalSkillDir(dir) {
  return (
    path.basename(path.dirname(dir)) === SKILLS_DIR &&
    MATRIX_DIR_RE.test(path.basename(path.dirname(path.dirname(dir))))
  );
}

// D is a SKILLS ROOT in canonical position, `skills` byte-exact, same reason.
function isCanonicalSkillsRoot(dir) {
  return path.basename(dir) === SKILLS_DIR && MATRIX_DIR_RE.test(path.basename(path.dirname(dir)));
}

// Present-entry shape findings test `skills` ASCII case-insensitively, so a case-variant
// `Skills/` yields the same finding on both platforms (Section 4.1 rule 2).
function isSkillsNameCi(name) {
  return asciiLower(name) === SKILLS_DIR;
}

// ---------------------------------------------------------------------------
// 6.4.1 isFrontmatterDelimiter: exact port of session_context.rs:276-302
// ---------------------------------------------------------------------------

// Rust u8::is_ascii_whitespace: \t \n \x0C \r and space. Vertical tab \x0B is NOT in it,
// so a JS \s or String.prototype.trim() port would wrongly accept a \x0B-padded fence.
function isAsciiWhitespaceByte(byte) {
  return byte === 0x09 || byte === 0x0a || byte === 0x0c || byte === 0x0d || byte === 0x20;
}

function isFrontmatterDelimiter(lineBytes, allowBom) {
  let start = 0;
  let end = lineBytes.length;
  if (end > start && lineBytes[end - 1] === 0x0a) end -= 1;
  if (end > start && lineBytes[end - 1] === 0x0d) end -= 1;
  if (
    allowBom &&
    end - start >= 3 &&
    lineBytes[start] === 0xef &&
    lineBytes[start + 1] === 0xbb &&
    lineBytes[start + 2] === 0xbf
  ) {
    start += 3;
  }
  while (start < end && isAsciiWhitespaceByte(lineBytes[start])) start += 1;
  while (end > start && isAsciiWhitespaceByte(lineBytes[end - 1])) end -= 1;
  return (
    end - start === 3 &&
    lineBytes[start] === 0x2d &&
    lineBytes[start + 1] === 0x2d &&
    lineBytes[start + 2] === 0x2d
  );
}

// ---------------------------------------------------------------------------
// 6.4 Frontmatter extraction: byte-level, streaming, bounded
// ---------------------------------------------------------------------------

function readFrontmatter(filePath) {
  let fd;
  try {
    fd = fs.openSync(filePath, 'r');
  } catch (err) {
    return { ok: false, code: 'E-ENTRYPOINT-UNREADABLE', detail: errnoOf(err), verb: 'open' };
  }

  try {
    const chunk = Buffer.allocUnsafe(READ_CHUNK_BYTES);
    let chunkLength = 0;
    let chunkPos = 0;
    let readFailure = null;

    const nextByte = () => {
      if (chunkPos >= chunkLength) {
        try {
          chunkLength = fs.readSync(fd, chunk, 0, chunk.length, null);
        } catch (err) {
          readFailure = err;
          return -1;
        }
        chunkPos = 0;
        if (chunkLength === 0) return -1;
      }
      chunkPos += 1;
      return chunk[chunkPos - 1];
    };

    let sawOpening = false;
    let currentLine = [];
    const frontmatter = [];

    const appendLine = () => {
      for (let i = 0; i < currentLine.length; i += 1) frontmatter.push(currentLine[i]);
    };

    for (;;) {
      const byte = nextByte();
      if (byte === -1) break;
      currentLine.push(byte);

      // Size guards, mirroring the if/else on saw_opening at :342-351. Both run after
      // the byte is pushed and before the `\n` short-circuit at :353, so the terminator
      // is counted on both arms.
      if (!sawOpening) {
        if (currentLine.length > FIRST_LINE_MAX_BYTES) {
          return { ok: false, code: 'E-FM-FIRST-LINE-TOO-LONG' };
        }
      } else {
        // Guard A, mid-line (:346-351). The `+ 8` is verbatim from
        // remaining.saturating_add(8). It also covers the closing delimiter line,
        // which never reaches the append path.
        const remaining = Math.max(0, FRONTMATTER_MAX_BYTES - frontmatter.length);
        if (currentLine.length > remaining + 8) {
          return { ok: false, code: 'E-FM-TOO-LARGE' };
        }
      }

      if (byte !== 0x0a) continue;

      if (!sawOpening) {
        if (!isFrontmatterDelimiter(currentLine, true)) {
          return { ok: false, code: 'E-FM-NO-OPEN' };
        }
        sawOpening = true;
        currentLine = [];
        continue;
      }

      if (isFrontmatterDelimiter(currentLine, false)) {
        return decodeFrontmatter(frontmatter);
      }

      // Guard B, on append (:311-317).
      if (frontmatter.length + currentLine.length > FRONTMATTER_MAX_BYTES) {
        return { ok: false, code: 'E-FM-TOO-LARGE' };
      }
      appendLine();
      currentLine = [];
    }

    if (readFailure !== null) {
      return { ok: false, code: 'E-ENTRYPOINT-UNREADABLE', detail: errnoOf(readFailure), verb: 'read' };
    }

    // EOF with an unterminated final line (:374-386). Not symmetric, and row 3 below is
    // the common shape every editor without "insert final newline" produces.
    if (currentLine.length > 0) {
      if (!sawOpening) {
        if (isFrontmatterDelimiter(currentLine, true)) return { ok: false, code: 'E-FM-NO-CLOSE' };
        return { ok: false, code: 'E-FM-NO-OPEN' };
      }
      if (isFrontmatterDelimiter(currentLine, false)) {
        return decodeFrontmatter(frontmatter);
      }
      // :385 appends before the `missing closing` return at :389, so an overflowing
      // unterminated body line yields E-FM-TOO-LARGE, not E-FM-NO-CLOSE.
      if (frontmatter.length + currentLine.length > FRONTMATTER_MAX_BYTES) {
        return { ok: false, code: 'E-FM-TOO-LARGE' };
      }
      appendLine();
      return { ok: false, code: 'E-FM-NO-CLOSE' };
    }

    if (!sawOpening) return { ok: false, code: 'E-FM-NO-OPEN' };
    return { ok: false, code: 'E-FM-NO-CLOSE' };
  } finally {
    try {
      fs.closeSync(fd);
    } catch {
      // Closing a descriptor we already finished with cannot change a verdict.
    }
  }
}

function decodeFrontmatter(bytes) {
  // fatal: true is required. Decoding the buffer through Buffer#toString with the utf8
  // encoding replaces invalid sequences with U+FFFD and would silently pass a file the
  // indexer rejects under F8 (:319-321), so that call is banned anywhere in this file and
  // acceptance criterion 10 greps for it.
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(Buffer.from(bytes));
    return { ok: true, text };
  } catch (err) {
    return { ok: false, code: 'E-FM-NOT-UTF8', detail: err instanceof Error ? err.message : String(err) };
  }
}

// ---------------------------------------------------------------------------
// 6.5 The YAML mini-parser
// ---------------------------------------------------------------------------

function splitFrontmatterLines(text) {
  if (text === '') return [];
  const parts = text.split('\n');
  if (parts.length > 0 && parts[parts.length - 1] === '') parts.pop();
  return parts.map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line));
}

function isBlankLine(line) {
  return /^[\t\n\v\f\r ]*$/.test(line);
}

// YAML indentation is spaces only; a tab is never indentation.
function indentWidth(line) {
  let n = 0;
  while (n < line.length && line[n] === ' ') n += 1;
  return n;
}

function leadingWsRun(line) {
  let n = 0;
  while (n < line.length && (line[n] === ' ' || line[n] === '\t')) n += 1;
  return line.slice(0, n);
}

function readQuotedScalar(text, start) {
  const quote = text[start];
  let i = start + 1;
  let out = '';
  if (quote === "'") {
    while (i < text.length) {
      const ch = text[i];
      if (ch === "'") {
        if (text[i + 1] === "'") {
          out += "'";
          i += 2;
          continue;
        }
        return { ok: true, value: out, end: i + 1 };
      }
      out += ch;
      i += 1;
    }
    return { ok: false, reason: 'a single-quoted scalar whose closing quote is not on the same line' };
  }
  while (i < text.length) {
    const ch = text[i];
    if (ch === '\\') {
      const esc = text[i + 1];
      if (esc === '\\') out += '\\';
      else if (esc === '"') out += '"';
      else if (esc === '/') out += '/';
      else if (esc === 'n') out += '\n';
      else if (esc === 'r') out += '\r';
      else if (esc === 't') out += '\t';
      else if (esc === 'u') {
        const hex = text.slice(i + 2, i + 6);
        if (!/^[0-9a-fA-F]{4}$/.test(hex)) {
          return { ok: false, reason: 'a double-quoted scalar with a malformed \\u escape' };
        }
        out += String.fromCharCode(parseInt(hex, 16));
        i += 6;
        continue;
      } else {
        return { ok: false, reason: 'a double-quoted scalar with a backslash escape outside the supported set' };
      }
      i += 2;
      continue;
    }
    if (ch === '"') return { ok: true, value: out, end: i + 1 };
    out += ch;
    i += 1;
  }
  return { ok: false, reason: 'a double-quoted scalar whose closing quote is not on the same line' };
}

// 6.5.1a. A mapping entry is a key, then `:`, then end of line or at least one space or
// TAB. YAML permits a tab as separation whitespace, so `name:<TAB>x` is an ordinary entry.
function parseKeyValueLine(body) {
  let key;
  let rest;
  if (body[0] === "'" || body[0] === '"') {
    const quoted = readQuotedScalar(body, 0);
    if (!quoted.ok) return { ok: false, reason: quoted.reason };
    // Whitespace may sit between the key and the `:` here for the same reason it may in
    // the unquoted branch below: YAML treats it as separation, not as part of the key.
    let after = quoted.end;
    while (body[after] === ' ' || body[after] === '\t') after += 1;
    if (body[after] !== ':') {
      return { ok: false, reason: 'a quoted scalar at column 0 that is not a mapping key' };
    }
    key = quoted.value;
    rest = body.slice(after + 1);
  } else {
    let separator = -1;
    for (let i = 0; i < body.length; i += 1) {
      if (body[i] !== ':') continue;
      const next = body[i + 1];
      if (next === undefined || next === ' ' || next === '\t') {
        separator = i;
        break;
      }
    }
    if (separator === -1) return { ok: false, reason: 'a line that is not a `key:` mapping entry' };
    // YAML strips trailing white space from a plain scalar, so `name :` and `name<TAB>:`
    // are both the key `name`. 6.5.4 defines key identity as the unquoted key text, and
    // the unquoted key text of `name :` is `name`. Keeping the space here made the whole
    // line look like an ordinary entry for a key nothing reads, so a plain typo produced
    // a run with no finding at all, not even W-YAML-UNDECIDABLE. Leading whitespace is
    // already routed to W-YAML-UNDECIDABLE by the `indent > 0` branch of parseMiniYaml.
    key = body.slice(0, separator).replace(/[ \t]+$/, '');
    rest = body.slice(separator + 1);
  }

  const value = rest.replace(/^[ \t]+/, '');
  const header = /^([|>])([0-9]*)([-+]?)([0-9]*)[ \t]*(?:#.*)?$/.exec(value);
  if (header) {
    const explicit = header[2] || header[4];
    const blockIndent = explicit === '' ? null : Number(explicit);
    // serde_yaml rejects a zero indentation indicator outright ("found an indentation
    // indicator equal to 0"), so `|0` and `>0` are outside the subset rather than block
    // scalars. Falling through leaves the value starting with `|` or `>`, which
    // resolveScalar reports as W-YAML-UNDECIDABLE.
    if (blockIndent !== 0) {
      return {
        ok: true,
        key,
        value,
        blockScalar: true,
        blockStyle: header[1],
        blockIndent,
      };
    }
  }
  return { ok: true, key, value, blockScalar: false };
}

// The block's indentation is the explicit indicator when given, otherwise the indentation
// of the first non-empty line. Every subsequent line indented at least that far is
// content WHATEVER it contains, so a `name: x` inside a `description: |` block is content
// and never a second top-level key.
function readBlockScalar(lines, startIndex, explicitIndent, style) {
  let indent = explicitIndent;
  if (indent === null) {
    let probe = startIndex;
    while (probe < lines.length && isBlankLine(lines[probe])) probe += 1;
    if (probe >= lines.length) return { endIndex: startIndex, content: '' };
    const width = indentWidth(lines[probe]);
    if (width === 0) return { endIndex: startIndex, content: '' };
    indent = width;
  }
  if (indent <= 0) return { endIndex: startIndex, content: '' };

  const content = [];
  let i = startIndex;
  while (i < lines.length) {
    const line = lines[i];
    if (isBlankLine(line)) {
      content.push('');
      i += 1;
      continue;
    }
    if (indentWidth(line) < indent) break;
    content.push(line.slice(indent));
    i += 1;
  }
  while (content.length > 0 && content[content.length - 1] === '') content.pop();

  // Exact folded whitespace is never load-bearing: no outcome depends on anything but
  // whether the value trims to empty, and both foldings agree on that.
  if (style === '>') {
    let folded = '';
    for (let j = 0; j < content.length; j += 1) {
      if (j > 0) folded += content[j] === '' || content[j - 1] === '' ? '\n' : ' ';
      folded += content[j];
    }
    return { endIndex: i, content: folded };
  }
  return { endIndex: i, content: content.join('\n') };
}

function resolveScalar(value) {
  if (value === '') return { kind: 'null', value: null, raw: value };
  const lead = value[0];
  if (lead === '#') return { kind: 'null', value: null, raw: value };
  if (lead === '{' || lead === '[') {
    return { kind: 'undecidable', construct: 'a flow collection value', raw: value };
  }
  if (lead === '&') return { kind: 'undecidable', construct: 'an anchored value', raw: value };
  if (lead === '*') return { kind: 'undecidable', construct: 'an alias value', raw: value };
  if (lead === '!') return { kind: 'undecidable', construct: 'a tagged value', raw: value };
  if (lead === '|' || lead === '>') {
    return { kind: 'undecidable', construct: 'a block-scalar header outside the supported subset', raw: value };
  }
  if (lead === "'" || lead === '"') {
    const quoted = readQuotedScalar(value, 0);
    if (!quoted.ok) return { kind: 'undecidable', construct: quoted.reason, raw: value };
    const tail = value.slice(quoted.end);
    if (tail !== '' && !/^[ \t]+(#.*)?$/.test(tail)) {
      return { kind: 'undecidable', construct: 'trailing text after a quoted scalar', raw: value };
    }
    return { kind: 'string', value: quoted.value, raw: value };
  }

  // Plain scalar. An unquoted `#` preceded by a space or tab starts a comment and
  // terminates the value; a `#` not preceded by whitespace is an ordinary character.
  let plain = value;
  const comment = /[ \t]#/.exec(plain);
  if (comment) plain = plain.slice(0, comment.index);
  plain = plain.replace(/[ \t]+$/, '');
  if (plain === '') return { kind: 'null', value: null, raw: value };

  // de.rs:932-938 matches exactly these six bool spellings and nothing else; de.rs:925-930
  // exactly these four nulls. yes/no/on/off/y/n are plain STRINGS under the 1.2 core schema.
  if (
    plain === 'true' ||
    plain === 'True' ||
    plain === 'TRUE' ||
    plain === 'false' ||
    plain === 'False' ||
    plain === 'FALSE'
  ) {
    return { kind: 'bool', value: null, raw: value };
  }
  if (plain === 'null' || plain === 'Null' || plain === 'NULL' || plain === '~') {
    return { kind: 'null', value: null, raw: value };
  }
  if (/^[-+]?[0-9]+$/.test(plain) || /^0x[0-9a-fA-F]+$/.test(plain) || /^0o[0-7]+$/.test(plain)) {
    return { kind: 'number', value: null, raw: value };
  }
  if (
    /^[-+]?[0-9]*\.[0-9]*([eE][-+]?[0-9]+)?$/.test(plain) ||
    /^[-+]?\.(inf|Inf|INF)$/.test(plain) ||
    /^\.(nan|NaN|NAN)$/.test(plain)
  ) {
    return { kind: 'number', value: null, raw: value };
  }
  return { kind: 'string', value: plain, raw: value };
}

function parseMiniYaml(text) {
  const result = {
    entries: new Map(),
    undecidables: [],
    duplicate: null,
    certainError: null,
    recognized: 0,
    significant: 0,
  };

  const lines = splitFrontmatterLines(text);
  let firstSignificantSeen = false;
  let mappingCandidate = false;
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    const lineNo = i + 1;
    i += 1;

    if (isBlankLine(line)) continue;

    // 6.5.3 row 1, as corrected in Step 6: a tab in the leading whitespace is a certain
    // error only on a line the parser is treating as a mapping entry, that is a line NOT
    // consumed as block-scalar content. Block content is consumed by readBlockScalar and
    // never reaches this point, so tab-indented text inside a `description: |` block is
    // content and not an error.
    if (leadingWsRun(line).includes('\t')) {
      result.certainError = {
        code: 'E-YAML-PARSE',
        line: lineNo,
        detail: 'a tab character appears in the indentation of a mapping line; YAML forbids tabs in indentation',
      };
      return result;
    }

    const indent = indentWidth(line);
    const body = line.slice(indent);
    if (body.startsWith('#')) continue;

    result.significant += 1;

    if (indent > 0) {
      result.undecidables.push({
        line: lineNo,
        construct: 'an indented line at top level (a nested mapping or sequence)',
      });
      continue;
    }

    if (!firstSignificantSeen) {
      firstSignificantSeen = true;
      if (body === '-' || body.startsWith('- ')) {
        result.certainError = {
          code: 'E-YAML-NOT-MAPPING',
          line: lineNo,
          detail: 'the document root is a sequence, not a mapping',
        };
        return result;
      }
    }

    const lead = body[0];
    if (lead === '{') {
      mappingCandidate = true;
      result.undecidables.push({ line: lineNo, construct: 'a flow mapping at the document root' });
      continue;
    }
    if (lead === '[') {
      result.undecidables.push({ line: lineNo, construct: 'a flow sequence at the document root' });
      continue;
    }
    if (lead === '%' || lead === '!' || lead === '&' || lead === '*') {
      mappingCandidate = true;
      result.undecidables.push({
        line: lineNo,
        construct: 'a directive, tag, anchor or alias at the document root',
      });
      continue;
    }
    if (body === '?' || body.startsWith('? ')) {
      mappingCandidate = true;
      result.undecidables.push({ line: lineNo, construct: 'an explicit key' });
      continue;
    }
    if (body === '...' || body.startsWith('... ')) {
      result.undecidables.push({ line: lineNo, construct: 'the document-end marker' });
      continue;
    }

    const entry = parseKeyValueLine(body);
    if (!entry.ok) {
      result.undecidables.push({ line: lineNo, construct: entry.reason });
      continue;
    }
    if (entry.key === '<<') {
      result.undecidables.push({ line: lineNo, construct: 'a merge key' });
      continue;
    }

    let resolved;
    if (entry.blockScalar) {
      const block = readBlockScalar(lines, i, entry.blockIndent, entry.blockStyle);
      i = block.endIndex;
      resolved = { kind: 'string', value: block.content, raw: entry.value };
    } else {
      resolved = resolveScalar(entry.value);
    }
    if (resolved.kind === 'undecidable') {
      result.undecidables.push({ line: lineNo, construct: resolved.construct });
    }

    result.recognized += 1;

    // 6.5.4. Key identity is the unquoted key text, byte-exact and case-sensitive,
    // matching Value::String equality at :449-450. The check lives in Mapping's
    // deserializer (mapping.rs:813-822) and runs before any field lookup, so it covers
    // EVERY top-level key, not just the three the indexer reads.
    if (result.entries.has(entry.key)) {
      result.duplicate = {
        key: entry.key,
        firstLine: result.entries.get(entry.key).line,
        line: lineNo,
      };
      return result;
    }
    result.entries.set(entry.key, {
      line: lineNo,
      kind: resolved.kind,
      value: resolved.value === undefined ? null : resolved.value,
      raw: resolved.raw === undefined ? entry.value : resolved.raw,
    });
  }

  if (result.significant === 0) {
    result.certainError = {
      code: 'E-YAML-NOT-MAPPING',
      line: null,
      detail: 'the frontmatter block is empty or contains only blank and comment lines',
    };
    return result;
  }

  // 6.5.3 row 4. Zero recognized entries alone is not proof the root is a scalar; zero
  // recognized entries AND nothing that could itself resolve to a mapping is.
  if (result.recognized === 0 && !mappingCandidate) {
    result.certainError = {
      code: 'E-YAML-NOT-MAPPING',
      line: null,
      detail: 'the document root is a scalar, not a mapping',
    };
  }
  return result;
}

// ---------------------------------------------------------------------------
// 6.2 Traversal + 6.3 entrypoint discovery
// ---------------------------------------------------------------------------

function walk(root, collector) {
  const candidates = [];
  const stack = [root];
  const visited = new Set([path.resolve(root)]);
  let directoriesScanned = 0;
  let entrypointsFound = 0;

  while (stack.length > 0) {
    const dir = stack.pop();
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (err) {
      const code = errnoOf(err);
      if (isCanonicalSkillsRoot(dir)) {
        collector.add({
          code: 'E-SKILLS-DIR-UNREADABLE',
          absolutePath: dir,
          message: `The \`skills\` directory could not be listed (${code}). The indexer treats this as a hard error and indexes no skill from this matrix.`,
          indexerMessage: `\`skills\` directory could not be read: ${sanitizeForReport(code)}`,
        });
      } else if (isCanonicalSkillDir(dir)) {
        collector.add({
          code: 'E-SKILL-DIR-UNREADABLE',
          absolutePath: dir,
          skillDirectory: dir,
          message: `The skill directory could not be listed (${code}). The indexer skips this skill.`,
          indexerMessage: skippedDirectory(
            path.basename(dir),
            `unable to read skill directory: ${sanitizeForReport(code)}`,
          ),
        });
      } else {
        collector.add({
          code: 'W-DIR-UNREADABLE',
          absolutePath: dir,
          message: `The directory could not be listed (${code}); it was skipped and the walk continued. If this is a case-variant \`skills\` directory it may still be the live skills root on Windows, where the indexer reaches it by joining the constant \`skills\`.`,
        });
      }
      continue;
    }
    directoriesScanned += 1;

    // 6.3 discovery. toLowerCase() is deliberate here and is the single exception to the
    // asciiLower() rule: over-matching is harmless because the byte-exact test follows.
    const matches = entries.filter((entry) => entry.name.toLowerCase() === 'skill.md');
    entrypointsFound += matches.length;

    if (matches.length === 0) {
      if (isSkillsNameCi(path.basename(path.dirname(dir)))) {
        const canonical = isCanonicalSkillDir(dir);
        collector.add({
          code: canonical ? 'E-NO-ENTRYPOINT' : 'I-NO-ENTRYPOINT',
          absolutePath: dir,
          skillDirectory: dir,
          message: canonical
            ? 'No `SKILL.md` entrypoint in any casing. The indexer requires an exact `SKILL.md` and skips this directory.'
            : 'No `SKILL.md` entrypoint in any casing. This directory sits under a `skills/` folder that the indexer never scans, so it is a note only.',
          indexerMessage: canonical
            ? skippedDirectory(path.basename(dir), 'missing exact SKILL.md entrypoint')
            : null,
        });
      }
    } else {
      const exact = matches.filter((entry) => entry.name === SKILL_MD);
      for (const entry of matches) {
        if (exact.length > 0 && entry.name !== SKILL_MD) {
          // The skill works today; the stray variant is a hazard, not a rejection.
          collector.add({
            code: 'W-ENTRYPOINT-CASE-SHADOWED',
            absolutePath: path.join(dir, entry.name),
            skillDirectory: dir,
            message: `\`${entry.name}\` sits beside an exact \`SKILL.md\`, which is the file the indexer uses. This stray variant is never read and is a hazard for the next reader.`,
          });
          continue;
        }
        candidates.push({
          dir,
          entryName: entry.name,
          entrypoint: path.join(dir, entry.name),
          dirent: entry,
          casingError: entry.name !== SKILL_MD,
        });
      }
    }

    // Step 3. The order and the guard shape are load-bearing: readdirSync returns Dirents
    // with lstat semantics, so isDirectory() is false for every symlink and any symlink
    // test nested under an isDirectory() guard never runs. Mirrors :578-585.
    for (const entry of entries) {
      const entryPath = path.join(dir, entry.name);

      if (entry.isSymbolicLink()) {
        // Never descend, and never apply a directory test. Node reports Windows junctions
        // and reparse points as symbolic links, so one code path covers both platforms.
        if (isSkillsNameCi(path.basename(dir))) {
          const canonical = MATRIX_DIR_RE.test(path.basename(path.dirname(dir)));
          if (canonical) {
            collector.add({
              code: 'E-SKILL-DIR-LINK',
              absolutePath: entryPath,
              skillDirectory: entryPath,
              message:
                'A linked entry directly under `skills/` is never followed. :578 tests only is_symlink() and never inspects the target, so this holds whether the link resolves to a directory, to a file, or to nothing.',
              indexerMessage: `Skipped linked skill directory \`${sanitizeForReport(entry.name)}\`: linked/reparse-point directories are not followed`,
            });
          } else {
            collector.add({
              code: 'W-SKILL-DIR-LINK',
              absolutePath: entryPath,
              skillDirectory: entryPath,
              message:
                'A linked entry directly under a `skills/` folder that is not in canonical position. The indexer never scans this tree, so this is a note about a shape that would be fatal if the folder were a real skills root.',
            });
          }
        }
        continue;
      }

      if (entry.isDirectory()) {
        // SKIP_DIRS never applies inside a `skills/` directory: the indexer applies no
        // name filter at all, and `target` matches ^[a-z0-9-]{1,64}$ so it is a legal
        // skill name. Skipping it would hide a real skill from the checker.
        if (!isSkillsNameCi(path.basename(dir)) && SKIP_DIRS.has(asciiLower(entry.name))) continue;
        const resolved = path.resolve(entryPath);
        if (visited.has(resolved)) continue;
        visited.add(resolved);
        stack.push(entryPath);
        continue;
      }

      // Step 4. An entry named `skills` that is not a directory, under an `_agent_*`
      // parent. A plain file directly inside `skills/` produces nothing, agreeing with
      // :583's `else if is_dir()`.
      if (isSkillsNameCi(entry.name) && MATRIX_DIR_RE.test(path.basename(dir))) {
        collector.add({
          code: 'E-SKILLS-NOT-DIR',
          absolutePath: entryPath,
          message:
            '`skills` exists but is not a directory, so the indexer finds no skills in this matrix at all.',
          indexerMessage: `\`skills\` exists but is not a directory: ${sanitizeForReport(entryPath)}`,
        });
      }
    }

    // Step 4, symlink arm. Handled separately because a symlink is excluded from the
    // isDirectory() branch above and must be probed to tell a resolving link from a
    // broken one: :520 gates on Path::exists, which follows links, so a broken `skills`
    // link makes exists() false and the indexer returns at :521 with NO warning at all.
    // This statSync is the only place in the walk that follows a link.
    for (const entry of entries) {
      if (!entry.isSymbolicLink()) continue;
      if (!isSkillsNameCi(entry.name)) continue;
      if (!MATRIX_DIR_RE.test(path.basename(dir))) continue;
      const entryPath = path.join(dir, entry.name);
      let resolves = false;
      try {
        resolves = fs.statSync(entryPath, { throwIfNoEntry: false }) !== undefined;
      } catch {
        // Path::exists returns false on ANY stat error, and the indexer is silent there.
        resolves = false;
      }
      if (!resolves) continue;
      collector.add({
        code: 'E-SKILLS-NOT-DIR',
        absolutePath: entryPath,
        message:
          '`skills` is a symlink that resolves, and :534 rejects it on is_symlink() even when the target is a directory. The indexer finds no skills in this matrix at all.',
        indexerMessage: `\`skills\` exists but is not a directory: ${sanitizeForReport(entryPath)}`,
      });
    }
  }

  return { candidates, directoriesScanned, entrypointsFound };
}

// ---------------------------------------------------------------------------
// 6.6 Field validation
// ---------------------------------------------------------------------------

function validateFields(candidate, mapping, collector, lineOffset) {
  const folder = path.basename(candidate.dir);
  const entrypoint = candidate.entrypoint;
  const fileLine = (bodyLine) => (bodyLine === null ? null : bodyLine + lineOffset);

  const hyphenVariant = mapping.entries.get('when-to-use');
  if (hyphenVariant) {
    collector.add({
      code: 'I-KEY-HYPHEN-VARIANT',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      line: fileLine(hyphenVariant.line),
      message:
        'The key `when-to-use` is never read. The indexer reads only the underscored `when_to_use` (:687), so this spelling is silently ignored.',
    });
  }

  // ---- name (:641-660) ----
  const nameEntry = mapping.entries.get('name');
  let resolvedName = null;
  let nameSource = 'folder';
  let nameLine = null;
  let nameResolved = false;

  if (nameEntry === undefined) {
    // :652 uses folder_name.clone() straight from the directory entry. The folder
    // fallback is NEVER trimmed; trimming it would hide a directory named " x ".
    resolvedName = folder;
    nameSource = 'folder';
    nameResolved = true;
  } else if (nameEntry.kind === 'undecidable') {
    // 6.5.2/6.6.1 step 3. W-YAML-UNDECIDABLE is already emitted. Run the charset check
    // for the author's benefit but report any violation as a WARNING: the run is never
    // failed on a value the checker has declared itself unable to judge, and this
    // entrypoint does not participate in the duplicate-name race.
    const literal = nameEntry.raw;
    if (!NAME_RE.test(literal)) {
      collector.add({
        code: 'W-NAME-UNDECIDABLE',
        absolutePath: entrypoint,
        skillDirectory: candidate.dir,
        line: fileLine(nameEntry.line),
        message: `The \`name:\` value \`${sanitizeForReport(literal)}\` is outside the parser's YAML subset, so the checker cannot resolve it. The indexer may index this skill under a name the checker could not resolve, or reject it with \`name must be a string\`, or reject it as an invalid name. The checker cannot tell which.`,
      });
    }
  } else if (nameEntry.kind !== 'string') {
    collector.add({
      code: 'E-NAME-NOT-STRING',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      line: fileLine(nameEntry.line),
      message: 'The `name:` value is not a YAML string. `yaml_field_string` accepts only Value::String (:448-462).',
      indexerMessage: skippedSkill(folder, 'name must be a string'),
    });
    // 6.6.1 step 2: STOP. :641-650 is `Err(e) => { push; continue }`, so `description`
    // (:674) and `when_to_use` (:687) are never reached and any finding about them would
    // carry an indexerMessage the startup log will never print.
    return { resolvedName: null, nameLine: null };
  } else {
    // :453 trims with Rust str::trim; Y5 maps an empty result to absent (:452-459).
    const trimmed = trimYamlValue(nameEntry.value);
    if (trimmed === '') {
      resolvedName = folder;
      nameSource = 'folder';
    } else {
      resolvedName = trimmed;
      nameSource = 'key';
      nameLine = fileLine(nameEntry.line);
    }
    nameResolved = true;
  }

  if (nameResolved) {
    // :464-470 is chars().count() in 1..=64 plus is_ascii_lowercase()/is_ascii_digit()/'-'.
    // NAME_RE covers both, and for a charset-valid name the UTF-16 length equals the
    // scalar count, so the two formulations agree; the scalar count is what the message
    // reports, because `name.length` would misreport astral characters.
    const scalarLength = [...resolvedName].length;
    if (!NAME_RE.test(resolvedName)) {
      const source =
        nameSource === 'key'
          ? `the \`name:\` key on line ${nameLine}.`
          : 'the containing directory name, because no usable `name:` key is present. Either rename the directory or add a valid `name:` key.';
      collector.add({
        code: 'E-NAME-INVALID',
        absolutePath: entrypoint,
        skillDirectory: candidate.dir,
        skillName: resolvedName,
        line: nameLine,
        message: `Resolved name \`${sanitizeForReport(resolvedName)}\` is not valid; expected 1-64 characters from [a-z0-9-] and this one is ${scalarLength}. Source: ${source}`,
        indexerMessage: skippedSkill(
          folder,
          `invalid skill name \`${sanitizeForReport(resolvedName)}\`; expected 1-64 lowercase ASCII letters, digits, or hyphens`,
        ),
      });
      // Same stop, for the same reason: :653-660 also `continue`s past the field layer.
      // The plan states the stop only for the type failure at step 2, so this half is a
      // tech-lead decision recorded in the Step 9 dispatch rather than plan text.
      return { resolvedName: null, nameLine };
    }
  }

  // ---- description (:674-684) ----
  const descEntry = mapping.entries.get('description');
  if (descEntry === undefined) {
    collector.add({
      code: 'W-DESC-MISSING',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      skillName: resolvedName,
      message:
        '`description` is missing. The skill is still indexed, and flagged at startup. A bad description is the only metadata problem that keeps a skill alive.',
      indexerMessage: 'description metadata is missing; inspect SKILL.md before use.',
    });
  } else if (descEntry.kind === 'string') {
    if (trimYamlValue(descEntry.value) === '') {
      collector.add({
        code: 'W-DESC-MISSING',
        absolutePath: entrypoint,
        skillDirectory: candidate.dir,
        skillName: resolvedName,
        line: fileLine(descEntry.line),
        message:
          '`description` trims to empty, which Y5 maps to absent. The skill is still indexed, and flagged at startup.',
        indexerMessage: 'description metadata is missing; inspect SKILL.md before use.',
      });
    }
  } else if (descEntry.kind !== 'undecidable') {
    collector.add({
      code: 'W-DESC-NOT-STRING',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      skillName: resolvedName,
      line: fileLine(descEntry.line),
      message: '`description` is present but is not a YAML string. The skill is still indexed, and flagged at startup.',
      indexerMessage: 'description must be a string; inspect SKILL.md before use.',
    });
  }

  // ---- when_to_use (:687-692) ----
  const whenEntry = mapping.entries.get('when_to_use');
  if (whenEntry !== undefined && whenEntry.kind !== 'string' && whenEntry.kind !== 'undecidable') {
    collector.add({
      code: 'W-WHEN-NOT-STRING',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      skillName: resolvedName,
      line: fileLine(whenEntry.line),
      message: '`when_to_use` is present but is not a YAML string; the indexer omits the metadata and indexes the skill.',
      indexerMessage: 'when_to_use must be a string; omitted when_to_use metadata.',
    });
  }

  return { resolvedName: nameResolved ? resolvedName : null, nameLine };
}

// ---------------------------------------------------------------------------
// 6.6.4 Duplicate names
// ---------------------------------------------------------------------------

function detectDuplicates(validated, collector) {
  const groups = new Map();
  for (const candidate of validated) {
    const parent = path.dirname(candidate.dir);
    if (!isSkillsNameCi(path.basename(parent))) continue;
    if (!groups.has(parent)) groups.set(parent, []);
    groups.get(parent).push(candidate);
  }

  for (const group of groups.values()) {
    // Exact port of :588-591, a two-element tuple compare of
    // (to_ascii_lowercase(), original), both as Rust String, that is UTF-8 byte order.
    // Sort the DIRECTORIES, not the resolved names: :588-591 runs on folder_name before
    // any frontmatter is read.
    group.sort((a, b) => {
      const an = path.basename(a.dir);
      const bn = path.basename(b.dir);
      const byLower = compareUtf8(asciiLower(an), asciiLower(bn));
      if (byLower !== 0) return byLower;
      return compareUtf8(an, bn);
    });

    const seen = new Map();
    for (const candidate of group) {
      const name = candidate.resolvedName;
      if (name === null) continue;
      const first = seen.get(name);
      if (first === undefined) {
        seen.set(name, path.basename(candidate.dir));
        continue;
      }
      collector.add({
        code: 'E-NAME-DUPLICATE',
        absolutePath: candidate.entrypoint,
        skillDirectory: candidate.dir,
        skillName: name,
        line: candidate.nameLine,
        message: `Resolved name \`${sanitizeForReport(name)}\` is already claimed by \`${sanitizeForReport(first)}\`, which sorts first in the same \`skills/\` directory. The indexer keeps the first and skips this one.`,
        indexerMessage: skippedSkill(
          path.basename(candidate.dir),
          `duplicate skill name \`${sanitizeForReport(name)}\` already used by \`${sanitizeForReport(first)}\``,
        ),
      });
    }
  }
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

function validateEntrypoint(candidate, collector) {
  const folder = path.basename(candidate.dir);
  const entrypoint = candidate.entrypoint;

  if (candidate.casingError) {
    collector.add({
      code: 'E-ENTRYPOINT-CASE',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      message: `Entrypoint is named \`${candidate.entryName}\`; the indexer compares entry.file_name() byte-exactly against \`SKILL.md\` (:401) and rejects this on every platform, Windows included.`,
      indexerMessage: skippedDirectory(folder, 'missing exact SKILL.md entrypoint'),
    });
  }

  // Symlink first, again for lstat semantics.
  if (candidate.dirent.isSymbolicLink()) {
    collector.add({
      code: 'E-ENTRYPOINT-LINK',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      message: 'The `SKILL.md` entrypoint is a symlink or reparse point, which the indexer refuses to follow (:409).',
      indexerMessage: skippedDirectory(folder, 'exact SKILL.md entrypoint is linked/reparse-point'),
    });
    return null;
  }
  if (!candidate.dirent.isFile()) {
    collector.add({
      code: 'E-ENTRYPOINT-NOT-FILE',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      message: 'The `SKILL.md` entrypoint is not a regular file (:412).',
      indexerMessage: skippedDirectory(folder, 'exact SKILL.md entrypoint is not a regular file'),
    });
    return null;
  }

  const frontmatter = readFrontmatter(entrypoint);
  if (!frontmatter.ok) {
    const detail = frontmatter.detail === undefined ? '' : frontmatter.detail;
    const messages = {
      'E-ENTRYPOINT-UNREADABLE': `The entrypoint could not be ${frontmatter.verb === 'read' ? 'read' : 'opened'} (${detail}).`,
      'E-FM-NO-OPEN':
        'No opening frontmatter delimiter on line 1. The first line must reduce to exactly `---` after stripping one terminator, an optional leading BOM and ASCII whitespace.',
      'E-FM-FIRST-LINE-TOO-LONG': `The first line, including its terminator, exceeds ${FIRST_LINE_MAX_BYTES} bytes. The indexer reports this as a missing opening delimiter (:342-345).`,
      'E-FM-NO-CLOSE': 'End of file reached with no closing frontmatter delimiter.',
      'E-FM-TOO-LARGE': `The frontmatter exceeds the ${FRONTMATTER_MAX_BYTES} byte limit. Line terminators count, and \`\\r\\n\` costs two bytes.`,
      'E-FM-NOT-UTF8': `The frontmatter block is not valid UTF-8 (${detail}). Only the frontmatter must be valid UTF-8; the body may be arbitrary bytes.`,
    };
    const tails = {
      'E-ENTRYPOINT-UNREADABLE': `failed to ${frontmatter.verb === 'read' ? 'read' : 'open'} SKILL.md frontmatter: ${sanitizeForReport(detail)}`,
      'E-FM-NO-OPEN': 'missing opening frontmatter delimiter',
      'E-FM-FIRST-LINE-TOO-LONG': 'missing opening frontmatter delimiter',
      'E-FM-NO-CLOSE': 'missing closing frontmatter delimiter',
      'E-FM-TOO-LARGE': `frontmatter exceeds ${FRONTMATTER_MAX_BYTES} byte limit`,
      'E-FM-NOT-UTF8': `frontmatter is not valid UTF-8: ${sanitizeForReport(detail)}`,
    };
    collector.add({
      code: frontmatter.code,
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      line: frontmatter.code === 'E-FM-NO-OPEN' || frontmatter.code === 'E-FM-FIRST-LINE-TOO-LONG' ? 1 : null,
      message: messages[frontmatter.code],
      indexerMessage: skippedSkill(folder, tails[frontmatter.code]),
    });
    return null;
  }

  // The frontmatter block excludes both delimiters, and the opening one is always line 1,
  // so body line N is file line N + 1.
  const lineOffset = 1;
  const mapping = parseMiniYaml(frontmatter.text);

  if (mapping.certainError !== null) {
    const certain = mapping.certainError;
    collector.add({
      code: certain.code,
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      line: certain.line === null ? null : certain.line + lineOffset,
      message: `${certain.detail}. The indexer never resolves any field of this skill, so no field-level finding would be truthful.`,
      indexerMessage: skippedSkill(
        folder,
        certain.code === 'E-YAML-PARSE'
          ? `YAML parse error: ${certain.detail}`
          : 'frontmatter must be a YAML mapping',
      ),
    });
    return null;
  }

  if (mapping.duplicate !== null) {
    const dup = mapping.duplicate;
    collector.add({
      code: 'E-YAML-DUPLICATE-KEY',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      line: dup.line + lineOffset,
      message: `The top-level key \`${sanitizeForReport(dup.key)}\` appears on line ${dup.firstLine + lineOffset} and again on line ${dup.line + lineOffset}. serde_yaml 0.9.34 rejects duplicate mapping keys (mapping.rs:813-822), so the parse fails and the skill is skipped outright. This covers every top-level key, not just the three the indexer reads.`,
      indexerMessage: skippedSkill(
        folder,
        `YAML parse error: duplicate entry with key "${sanitizeForReport(dup.key)}"`,
      ),
    });
    return null;
  }

  for (const undecidable of mapping.undecidables) {
    collector.add({
      code: 'W-YAML-UNDECIDABLE',
      absolutePath: entrypoint,
      skillDirectory: candidate.dir,
      line: undecidable.line + lineOffset,
      message: `The mini-parser cannot judge ${undecidable.construct}. The construct is outside its fixed YAML subset, so the checker refuses to approve or reject it.`,
    });
  }

  const fields = validateFields(candidate, mapping, collector, lineOffset);
  return fields;
}

function runCheck(root) {
  const collector = createCollector(root);
  const walked = walk(root, collector);

  const validated = [];
  for (const candidate of walked.candidates) {
    const before = collector.findings.length;
    const fields = validateEntrypoint(candidate, collector);
    const emitted = collector.findings.slice(before);
    const hasError = emitted.some((finding) => finding.severity === SEVERITY.error);

    // 6.6.4: a candidate carrying ANY hard error never claims its name. :595-670 is a
    // single loop whose every failure path is a `continue`, and seen_skill_names is
    // written only at :671, after every hard check has passed.
    if (fields !== null && !hasError && fields.resolvedName !== null) {
      validated.push({
        dir: candidate.dir,
        entrypoint: candidate.entrypoint,
        resolvedName: fields.resolvedName,
        nameLine: fields.nameLine,
      });
    }
  }

  detectDuplicates(validated, collector);

  // A duplicate finding lands after the fact, so recompute indexability from the findings.
  const erroredEntrypoints = new Set();
  for (const finding of collector.findings) {
    if (finding.severity !== SEVERITY.error) continue;
    if (finding.code === 'W-ENTRYPOINT-CASE-SHADOWED') continue;
    const owner = walked.candidates.find((candidate) => candidate.entrypoint === finding.absolutePath);
    if (owner !== undefined) erroredEntrypoints.add(finding.absolutePath);
  }

  // A shadowed casing variant is excluded from skillsIndexable: it carries only
  // W-ENTRYPOINT-CASE-SHADOWED and is never validated, so counting it would report one
  // directory as two indexable skills.
  const skillsIndexable = walked.candidates.filter(
    (candidate) => !erroredEntrypoints.has(candidate.entrypoint),
  ).length;

  for (const candidate of walked.candidates) {
    const position = classifyPosition(candidate.entrypoint);
    if (position.canonical) continue;
    collector.add({
      code: 'I-NOT-INDEXABLE',
      absolutePath: candidate.entrypoint,
      skillDirectory: candidate.dir,
      reason: position.reason,
      message: POSITION_MESSAGE[position.reason],
    });
  }

  const findings = collector.findings.slice().sort((a, b) => {
    if (a.path < b.path) return -1;
    if (a.path > b.path) return 1;
    const aLine = a.line === null ? 0 : a.line;
    const bLine = b.line === null ? 0 : b.line;
    if (aLine !== bLine) return aLine - bLine;
    if (a.code < b.code) return -1;
    if (a.code > b.code) return 1;
    return a.seq - b.seq;
  });

  const errors = findings.filter((finding) => finding.severity === SEVERITY.error).length;
  const warnings = findings.filter((finding) => finding.severity === SEVERITY.warning).length;
  const infos = findings.filter((finding) => finding.severity === SEVERITY.info).length;
  const exitCode = errors > 0 ? 1 : 0;

  return {
    tool: TOOL,
    version: OUTPUT_VERSION,
    sourceOfTruth: SOURCE_OF_TRUTH,
    root,
    summary: {
      directoriesScanned: walked.directoriesScanned,
      entrypointsFound: walked.entrypointsFound,
      skillsIndexable,
      errors,
      warnings,
      infos,
    },
    findings: findings.map((finding) => ({
      code: finding.code,
      severity: finding.severity,
      rule: finding.rule,
      path: finding.path,
      absolutePath: finding.absolutePath,
      skillDirectory: finding.skillDirectory,
      skillName: finding.skillName,
      line: finding.line,
      reason: finding.reason,
      message: finding.message,
      indexerMessage: finding.indexerMessage,
    })),
    exitCode,
    erroredEntrypoints: erroredEntrypoints.size,
  };
}

// ---------------------------------------------------------------------------
// 6.9 Human report / 6.10 JSON output
// ---------------------------------------------------------------------------

// Fixed-width tokens, padded to five characters as the report format requires.
const SEVERITY_LABEL = { error: 'ERROR', warning: 'WARN ', info: 'NOTE ' };
const LABEL_GAP = ' ';

function wrapContinuation(text) {
  const width = 92;
  const words = text.split(' ');
  const lines = [];
  let current = '';
  for (const word of words) {
    if (current === '') {
      current = word;
    } else if (`${current} ${word}`.length <= width) {
      current += ` ${word}`;
    } else {
      lines.push(current);
      current = word;
    }
  }
  if (current !== '') lines.push(current);
  return lines;
}

function renderHuman(report, out) {
  const scanned = report.summary.directoriesScanned;
  out(`${TAG} root: ${report.root}`);
  out(
    `${TAG} scanned ${scanned} ${scanned === 1 ? 'directory' : 'directories'}, found ${plural(report.summary.entrypointsFound, 'SKILL.md entrypoint')}`,
  );

  if (report.summary.entrypointsFound === 0 && report.findings.length === 0) {
    out('');
    out(`${TAG} OK: no SKILL.md entrypoints found`);
    return;
  }

  for (const finding of report.findings) {
    out('');
    const anchor = finding.line === null ? finding.path : `${finding.path}:${finding.line}`;
    const suffix =
      finding.reason === null ? `${finding.code}  (rule ${finding.rule ?? 'checker-only'})` : `${finding.code}  (reason: ${finding.reason})`;
    out(`${SEVERITY_LABEL[finding.severity]}${LABEL_GAP} ${anchor}  ${suffix}`);
    for (const line of wrapContinuation(finding.message)) out(`       ${line}`);
    if (finding.indexerMessage !== null) {
      const indexerLines = wrapContinuation(`Indexer: ${finding.indexerMessage}`);
      for (const line of indexerLines) out(`       ${line}`);
    }
  }

  out('');
  out(
    `${TAG} ${plural(report.summary.errors, 'error')}, ${plural(report.summary.warnings, 'warning')}, ${plural(report.summary.infos, 'note')} across ${plural(report.summary.entrypointsFound, 'entrypoint')}`,
  );

  if (report.exitCode === 0) {
    if (report.summary.entrypointsFound === 0) {
      out(`${TAG} OK: no SKILL.md entrypoints found`);
    } else {
      out(`${TAG} OK: ${report.summary.skillsIndexable} skills would be indexed`);
    }
    return;
  }

  // Five error codes attach to no entrypoint at all, so the entrypoint count alone can
  // print `FAIL: 0`, which flatly contradicts the exit code.
  const implicated = report.erroredEntrypoints;
  if (implicated === 0) {
    out(`${TAG} FAIL: ${plural(report.summary.errors, 'error finding')}, no entrypoint implicated`);
  } else {
    out(
      `${TAG} FAIL: ${implicated} skills would not be indexed, and ${plural(report.summary.errors, 'error finding')} in total`,
    );
  }
}

function renderJson(report) {
  const document = {
    tool: report.tool,
    version: report.version,
    sourceOfTruth: report.sourceOfTruth,
    root: report.root,
    summary: report.summary,
    findings: report.findings,
    exitCode: report.exitCode,
  };
  return `${JSON.stringify(document, null, 2)}\n`;
}

// ---------------------------------------------------------------------------
// 6.1 CLI
// ---------------------------------------------------------------------------

const USAGE = [
  `${TOOL} - validate SKILL.md structure the way AgentsCommander's indexer does (issue #1213).`,
  '',
  `Source of truth: ${SOURCE_OF_TRUTH}`,
  '',
  'Usage:',
  '  node scripts/01-skills-checker.mjs [root] [--json]',
  '  node scripts/01-skills-checker.mjs --help | -h',
  '  node scripts/01-skills-checker.mjs --self-test',
  '',
  'Arguments:',
  '  root          Directory to walk recursively. Defaults to the current directory.',
  '  --json        Write one JSON document to stdout and nothing else.',
  '  --self-test   Run the in-script test suite. May not be combined with any other argument.',
  '  --            Terminate flag parsing, so a root may begin with a hyphen.',
  '',
  'Exit codes:',
  '  0  no error findings (warnings and notes may still be present)',
  '  1  at least one error finding: one or more skills would not be indexed',
  '  2  usage error, or the root does not exist / is not a directory / cannot be read',
];

function printUsage(out) {
  for (const line of USAGE) out(line);
}

function parseArgs(argv) {
  const result = { help: false, selfTest: false, json: false, root: null, error: null };
  const flags = [];
  const positionals = [];
  let flagsTerminated = false;

  for (const arg of argv) {
    if (!flagsTerminated && arg === '--') {
      flagsTerminated = true;
      continue;
    }
    if (!flagsTerminated && arg.startsWith('-')) {
      flags.push(arg);
      continue;
    }
    positionals.push(arg);
  }

  // `--help` wins over every other argument, including an invalid one. It is scanned
  // among flags only, so `-- --help` still names a root, which is what `--` is for.
  if (flags.includes('--help') || flags.includes('-h')) {
    result.help = true;
    return result;
  }

  for (const flag of flags) {
    if (flag === '--json') result.json = true;
    else if (flag === '--self-test') result.selfTest = true;
  }

  const unknown = flags.find((flag) => flag !== '--json' && flag !== '--self-test');
  if (unknown !== undefined) {
    result.error = `usage: unknown option '${unknown}'`;
    return result;
  }
  if (result.selfTest && (result.json || positionals.length > 0 || flags.length > 1)) {
    result.error = 'usage: --self-test may not be combined with any other argument';
    return result;
  }
  if (positionals.length > 1) {
    result.error = 'usage: at most one root may be given';
    return result;
  }

  result.root = positionals.length === 1 ? positionals[0] : null;
  return result;
}

function runCli(argv, out, err) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    err(`internal error: ${error instanceof Error ? error.message : String(error)}`);
    if (error instanceof Error && error.stack) err(error.stack);
    return 2;
  }

  if (options.help) {
    printUsage(out);
    return 0;
  }

  const fail = (message) => {
    if (options.json) {
      err(JSON.stringify({ tool: TOOL, version: OUTPUT_VERSION, error: message, exitCode: 2 }));
    } else {
      err(message);
    }
    return 2;
  };

  if (options.error !== null) return fail(options.error);
  if (options.selfTest) return selfTest(out);

  const root = path.resolve(options.root === null ? process.cwd() : options.root);

  let stats;
  try {
    stats = fs.statSync(root);
  } catch (error) {
    return fail(`cannot read root '${root}': ${errnoOf(error)}`);
  }
  if (!stats.isDirectory()) return fail(`root '${root}' is not a directory`);
  try {
    // Exit 2 is only ever about the invocation and the root itself; a directory that
    // cannot be read INSIDE the walk is a finding, not an exit-2 failure.
    fs.readdirSync(root);
  } catch (error) {
    return fail(`cannot read root '${root}': ${errnoOf(error)}`);
  }

  let report;
  try {
    report = runCheck(root);
  } catch (error) {
    err(`internal error: ${error instanceof Error ? error.message : String(error)}`);
    if (error instanceof Error && error.stack) err(error.stack);
    return 2;
  }

  if (options.json) {
    out(renderJson(report).replace(/\n$/, ''));
  } else {
    renderHuman(report, out);
  }
  return report.exitCode;
}

// ---------------------------------------------------------------------------
// Section 9: the self-test
// ---------------------------------------------------------------------------

function assertSelf(condition, message) {
  if (!condition) throw new Error(message);
}

function writeFixture(root, relative, contents) {
  const target = path.join(root, relative);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, contents);
  return target;
}

function makeDir(root, relative) {
  const target = path.join(root, relative);
  fs.mkdirSync(target, { recursive: true });
  return target;
}

// On win32 a junction needs NO Developer Mode and no elevation, works for directories,
// and Node reports it through Dirent.isSymbolicLink(). That is what keeps cases 11, 11a,
// 11b, 12, 12b, 63e and 63f from skipping on ordinary Windows.
function makeDirLink(target, linkPath) {
  if (process.platform === 'win32') {
    fs.symlinkSync(path.resolve(target), linkPath, 'junction');
  } else {
    fs.symlinkSync(target, linkPath, 'dir');
  }
}

function codesOf(report) {
  return report.findings.map((finding) => finding.code);
}

function hasCode(report, code) {
  return report.findings.some((finding) => finding.code === code);
}

function countCode(report, code) {
  return report.findings.filter((finding) => finding.code === code).length;
}

function validSkillMd(name) {
  return `---\nname: ${name}\ndescription: A valid skill.\n---\n\nBody.\n`;
}

function tempRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'ac-skills-check-'));
}

function removeRoot(root) {
  try {
    fs.rmSync(root, { recursive: true, force: true });
  } catch {
    // A fixture that resists removal must not turn a passing case into a failure.
  }
}

// Probe: can this filesystem hold `SKILL.md` and `skill.md` side by side? Default Windows
// and default macOS cannot, and there the second write silently overwrites the first.
function probeCaseSensitiveFs() {
  const root = tempRoot();
  try {
    fs.writeFileSync(path.join(root, 'SKILL.md'), 'a');
    fs.writeFileSync(path.join(root, 'skill.md'), 'b');
    return fs.readdirSync(root).length === 2;
  } catch {
    return false;
  } finally {
    removeRoot(root);
  }
}

// Probe: can a directory be made unlistable to the current user? icacls denials do not
// bind the owner on Windows and chmod 000 does not bind root on Linux, and this script
// may only use node:fs, so there is no way to shell out to icacls either.
function probeUnreadableDir() {
  const root = tempRoot();
  try {
    const target = path.join(root, 'probe');
    fs.mkdirSync(target);
    try {
      fs.chmodSync(target, 0o000);
    } catch {
      return false;
    }
    try {
      fs.readdirSync(target);
      return false;
    } catch {
      return true;
    } finally {
      try {
        fs.chmodSync(target, 0o700);
      } catch {
        // Best effort; removeRoot uses force.
      }
    }
  } catch {
    return false;
  } finally {
    removeRoot(root);
  }
}

function probeFileSymlink() {
  const root = tempRoot();
  try {
    const target = path.join(root, 'target.txt');
    fs.writeFileSync(target, 'x');
    fs.symlinkSync(target, path.join(root, 'link.txt'), 'file');
    return true;
  } catch {
    return false;
  } finally {
    removeRoot(root);
  }
}

function selfTest(out) {
  const caseSensitiveFs = probeCaseSensitiveFs();
  const unreadableDirSupported = probeUnreadableDir();
  const fileSymlinkSupported = probeFileSymlink();

  const skipped = [];
  let ran = 0;
  let total = 0;
  let failure = null;

  const skip = (id, reason) => {
    skipped.push(id);
    out(`${TAG} self-test: SKIP case ${id} (${reason})`);
  };

  // Returns 0 when every case passes and 1 on the FIRST failure, naming the case; every
  // later case is short-circuited so the printed failure is the one that matters.
  const run = (id, body) => {
    if (failure !== null) return;
    total += 1;
    const root = tempRoot();
    try {
      const outcome = body(root);
      if (outcome === 'skip') return;
      ran += 1;
    } catch (error) {
      failure = `case ${id}: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      removeRoot(root);
    }
  };

  // ---- CLI (6.1) ----
  run('1', () => {
    const lines = [];
    const code = runCli(['--help'], (line) => lines.push(line), () => {});
    assertSelf(code === 0, '--help must exit 0');
    assertSelf(lines.join('\n').includes('Exit codes:'), '--help must print the usage block');
  });

  run('2', (root) => {
    const missing = path.join(root, 'nope');
    const errs = [];
    const code = runCli([missing], () => {}, (line) => errs.push(line));
    assertSelf(code === 2, 'a nonexistent root must exit 2');
    assertSelf(errs.join('\n').includes(missing), 'the message must name the path');
  });

  run('3', (root) => {
    const file = writeFixture(root, 'a-file.txt', 'x');
    const errs = [];
    const code = runCli([file], () => {}, (line) => errs.push(line));
    assertSelf(code === 2, 'a root that is a file must exit 2');
    assertSelf(errs.join('\n').includes('is not a directory'), 'the message must say it is not a directory');
  });

  run('4', (root) => {
    const code = runCli([root, root], () => {}, () => {});
    assertSelf(code === 2, 'two positionals must exit 2');
  });

  run('5', (root) => {
    const errs = [];
    const code = runCli([root, '--bogus'], () => {}, (line) => errs.push(line));
    assertSelf(code === 2, 'an unknown flag must exit 2');
    assertSelf(errs.join('\n').includes("unknown option '--bogus'"), 'the message must name the flag');
  });

  run('6', (root) => {
    const dashed = path.join(root, '-dashed');
    fs.mkdirSync(dashed);
    const code = runCli(['--', dashed], () => {}, () => {});
    assertSelf(code === 0, 'a root after `--` must be treated as the root, not a flag');
    const parsed = parseArgs(['--', '-dashed']);
    assertSelf(parsed.root === '-dashed' && parsed.error === null, '`--` must terminate flag parsing');
  });

  run('7', (root) => {
    const report = runCheck(root);
    assertSelf(report.exitCode === 0, 'an empty root must exit 0');
    assertSelf(report.summary.entrypointsFound === 0, 'an empty root must find no entrypoints');
  });

  // ---- Traversal (6.2) ----
  run('8', (root) => {
    writeFixture(root, 'node_modules/pkg/skills/x/SKILL.md', validSkillMd('x'));
    writeFixture(root, '.git/skills/y/SKILL.md', validSkillMd('y'));
    writeFixture(root, 'target/skills/z/SKILL.md', validSkillMd('z'));
    const report = runCheck(root);
    assertSelf(report.summary.entrypointsFound === 0, 'SKIP_DIRS must not be traversed');
  });

  run('9', (root) => {
    writeFixture(root, 'Node_Modules/pkg/skills/x/SKILL.md', validSkillMd('x'));
    const report = runCheck(root);
    assertSelf(report.summary.entrypointsFound === 0, 'the SKIP_DIRS comparison must be case-insensitive');
  });

  run('10', (root) => {
    writeFixture(root, 'a/b/c/d/e/SKILL.md', validSkillMd('deep'));
    const report = runCheck(root);
    assertSelf(report.summary.entrypointsFound === 1, 'the walk must have no depth limit');
  });

  run('11', (root) => {
    const target = makeDir(root, 'link-target');
    writeFixture(root, 'link-target/SKILL.md', validSkillMd('linked'));
    makeDir(root, '_agent_x/skills');
    makeDirLink(target, path.join(root, '_agent_x/skills/linked'));
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-SKILL-DIR-LINK'), 'a linked skill directory in canonical position is a hard error');
    assertSelf(
      !report.findings.some((f) => f.absolutePath.includes(`skills${path.sep}linked${path.sep}`)),
      'the walk must not descend into a linked skill directory',
    );
  });

  run('11a', (root) => {
    const target = makeDir(root, 'link-target');
    makeDir(root, 'docs/skills');
    makeDirLink(target, path.join(root, 'docs/skills/linked'));
    const report = runCheck(root);
    assertSelf(hasCode(report, 'W-SKILL-DIR-LINK'), 'a linked skill directory outside canonical position is a warning');
    assertSelf(report.exitCode === 0, 'a docs/skills link must not fail the run');
  });

  run('11b', (root) => {
    const fileTarget = makeDir(root, 'file-target');
    const goneTarget = makeDir(root, 'gone-target');
    makeDir(root, '_agent_x/skills');
    makeDirLink(fileTarget, path.join(root, '_agent_x/skills/to-file'));
    makeDirLink(goneTarget, path.join(root, '_agent_x/skills/to-nothing'));
    // :578 tests only is_symlink() and never inspects the target, so repointing one link
    // at a file and breaking the other must change nothing.
    fs.rmSync(fileTarget, { recursive: true, force: true });
    fs.writeFileSync(fileTarget, 'now a file');
    fs.rmSync(goneTarget, { recursive: true, force: true });
    const report = runCheck(root);
    assertSelf(countCode(report, 'E-SKILL-DIR-LINK') === 2, 'both links must be hard errors regardless of target');
  });

  run('12', (root) => {
    const target = makeDir(root, 'link-target');
    makeDir(root, 'plain');
    makeDirLink(target, path.join(root, 'plain/linked'));
    const report = runCheck(root);
    assertSelf(report.findings.length === 0, 'a link outside any skills/ folder produces no finding');
  });

  run('12a', (root) => {
    const broken = '---\nname: ok-name\n';
    writeFixture(root, '_agent_x/skills/target/SKILL.md', broken);
    writeFixture(root, '_agent_x/skills/node_modules/SKILL.md', broken);
    writeFixture(root, '_agent_x/skills/.git/SKILL.md', broken);
    const report = runCheck(root);
    assertSelf(
      countCode(report, 'E-FM-NO-CLOSE') === 3,
      'SKIP_DIRS must not apply inside a skills/ directory: target, node_modules and .git are legal skill names there',
    );
  });

  run('12b', (root) => {
    const target = makeDir(root, 'real-dir');
    makeDir(root, '_agent_x');
    makeDirLink(target, path.join(root, '_agent_x/skills'));
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-SKILLS-NOT-DIR'), 'a resolving `skills` link is E-SKILLS-NOT-DIR');
    assertSelf(!hasCode(report, 'E-SKILL-DIR-LINK'), 'the symlink branch of step 3 must not claim it first');
  });

  // ---- Entrypoint (6.3) ----
  run('13', (root) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', validSkillMd('x'));
    const report = runCheck(root);
    assertSelf(report.summary.errors === 0, 'a valid skill produces no error');
    assertSelf(report.summary.skillsIndexable === 1, 'a valid skill counts as indexable');
  });

  run('14', (root) => {
    writeFixture(root, '_agent_x/skills/x/skill.md', validSkillMd('x'));
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-ENTRYPOINT-CASE'), 'a wrongly cased entrypoint is a hard error');
    assertSelf(report.exitCode === 1, 'a wrongly cased entrypoint fails the run');
  });

  run('15', (root) => {
    writeFixture(root, '_agent_x/skills/x/Skill.md', '---\nname: Bad_Name\ndescription: x\n---\n');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-ENTRYPOINT-CASE'), 'the casing error is reported');
    assertSelf(hasCode(report, 'E-NAME-INVALID'), 'validation continues after a casing error');
  });

  run('16', (root) => {
    makeDir(root, '_agent_x/skills/x');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-NO-ENTRYPOINT'), 'a canonical skill directory with no entrypoint is a hard error');
  });

  run('17', (root) => {
    makeDir(root, 'docs/skills/x');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'I-NO-ENTRYPOINT'), 'outside canonical position the same absence is a note');
    assertSelf(report.exitCode === 0, 'a note must not fail the run');
  });

  run('18', (root) => {
    makeDir(root, '_agent_x/skills/x/SKILL.md');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-ENTRYPOINT-NOT-FILE'), 'a directory named SKILL.md is a hard error');
  });

  run('19', (root) => {
    if (!fileSymlinkSupported) {
      skip('19', 'file symlinks need Developer Mode or elevation on this platform');
      return 'skip';
    }
    const target = writeFixture(root, 'real.md', validSkillMd('x'));
    makeDir(root, '_agent_x/skills/x');
    fs.symlinkSync(target, path.join(root, '_agent_x/skills/x/SKILL.md'), 'file');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-ENTRYPOINT-LINK'), 'a linked SKILL.md is a hard error');
    return undefined;
  });

  run('20', (root) => {
    if (!caseSensitiveFs) {
      skip('20', 'case-insensitive filesystem: SKILL.md and skill.md cannot coexist');
      return 'skip';
    }
    const dir = makeDir(root, '_agent_x/skills/x');
    fs.writeFileSync(path.join(dir, 'SKILL.md'), validSkillMd('x'));
    fs.writeFileSync(path.join(dir, 'skill.md'), validSkillMd('x'));
    assertSelf(fs.readdirSync(dir).length === 2, 'the probe promised two entries');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'W-ENTRYPOINT-CASE-SHADOWED'), 'the stray variant is a warning');
    assertSelf(report.exitCode === 0, 'the working SKILL.md keeps the run green');
    return undefined;
  });

  // ---- Frontmatter (6.4) ----
  const fm = (root, contents) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', contents);
    return runCheck(root);
  };

  run('21', (root) => {
    assertSelf(hasCode(fm(root, 'Just a body.\n'), 'E-FM-NO-OPEN'), 'no frontmatter at all is E-FM-NO-OPEN');
  });
  run('22', (root) => {
    assertSelf(hasCode(fm(root, '\n---\nname: x\n---\n'), 'E-FM-NO-OPEN'), 'a blank first line is E-FM-NO-OPEN');
  });
  run('23', (root) => {
    const report = fm(root, '---\r\nname: ok-name\r\ndescription: x\r\n---\r\n');
    assertSelf(report.summary.errors === 0, 'CRLF delimiters are valid');
  });
  run('24', (root) => {
    const report = fm(root, '﻿---\nname: ok-name\ndescription: x\n---\n');
    assertSelf(report.summary.errors === 0, 'a BOM on the opening delimiter is valid');
  });
  run('25', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: x\n﻿---\n');
    assertSelf(hasCode(report, 'E-FM-NO-CLOSE'), 'a BOM on the closing delimiter is not tolerated');
  });
  run('26', (root) => {
    const report = fm(root, '  ---  \nname: ok-name\ndescription: x\n  ---  \n');
    assertSelf(report.summary.errors === 0, 'whitespace around the fences is trimmed');
  });
  run('27', (root) => {
    assertSelf(hasCode(fm(root, '----\nname: x\n----\n'), 'E-FM-NO-OPEN'), 'a longer fence is rejected');
  });
  run('28', (root) => {
    assertSelf(hasCode(fm(root, '+++\nname = "x"\n+++\n'), 'E-FM-NO-OPEN'), 'a TOML fence is rejected');
  });
  run('29', (root) => {
    const report = fm(root, '---');
    assertSelf(hasCode(report, 'E-FM-NO-CLOSE'), 'a bare `---` with no newline is E-FM-NO-CLOSE, not E-FM-NO-OPEN');
    assertSelf(!hasCode(report, 'E-FM-NO-OPEN'), 'and specifically not E-FM-NO-OPEN');
  });
  run('29a', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: x\n---');
    assertSelf(report.summary.errors === 0, 'a closing delimiter as the unterminated final line is valid');
    assertSelf(report.exitCode === 0, 'and the run stays green');
  });
  run('29b', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: x');
    assertSelf(hasCode(report, 'E-FM-NO-CLOSE'), 'an unterminated final BODY line is E-FM-NO-CLOSE');
  });
  run('29c', (root) => {
    // Sized into the 8-byte window between Guard A (> remaining + 8) and Guard B
    // (> remaining), so Guard B is what decides, pinning that the append at :385 runs
    // before the `missing closing` return at :389.
    const body = `${'a'.repeat(16000)}\n`;
    const remaining = FRONTMATTER_MAX_BYTES - body.length;
    const report = fm(root, `---\n${body}${'b'.repeat(remaining + 4)}`);
    assertSelf(hasCode(report, 'E-FM-TOO-LARGE'), 'an overflowing unterminated final line is E-FM-TOO-LARGE');
    assertSelf(!hasCode(report, 'E-FM-NO-CLOSE'), 'and not E-FM-NO-CLOSE');
  });
  run('29d', (root) => {
    assertSelf(hasCode(fm(root, ''), 'E-FM-NO-OPEN'), 'a zero-byte file is E-FM-NO-OPEN');
  });
  run('30', (root) => {
    assertSelf(hasCode(fm(root, '---\nname: ok-name\n'), 'E-FM-NO-CLOSE'), 'EOF before a closing fence');
  });
  run('31', (root) => {
    const report = fm(root, `---\n${'a'.repeat(16384)}\n---\n`);
    assertSelf(hasCode(report, 'E-FM-TOO-LARGE'), 'a 16385-byte body overflows');
  });
  run('32', (root) => {
    const report = fm(root, `---\nname: ok-name\ndescription: ${'a'.repeat(16000 - 30)}\n---\n`);
    assertSelf(!hasCode(report, 'E-FM-TOO-LARGE'), 'a 16000-byte body fits');
  });
  run('33', (root) => {
    const bytes = Buffer.concat([
      Buffer.from('---\nname: ', 'utf8'),
      Buffer.from([0xff, 0xfe]),
      Buffer.from('\n---\n', 'utf8'),
    ]);
    writeFixture(root, '_agent_x/skills/x/SKILL.md', bytes);
    assertSelf(hasCode(runCheck(root), 'E-FM-NOT-UTF8'), 'invalid UTF-8 in the frontmatter is a hard error');
  });
  run('34', (root) => {
    const bytes = Buffer.concat([
      Buffer.from('---\nname: ok-name\ndescription: x\n---\n', 'utf8'),
      Buffer.from([0xff, 0xfe, 0xff]),
    ]);
    writeFixture(root, '_agent_x/skills/x/SKILL.md', bytes);
    const report = runCheck(root);
    assertSelf(report.summary.errors === 0, 'invalid UTF-8 in the BODY is never read');
  });
  run('35', (root) => {
    const report = fm(root, `${'a'.repeat(2000)}\n---\nname: ok-name\n---\n`);
    assertSelf(hasCode(report, 'E-FM-FIRST-LINE-TOO-LONG'), 'a 2000-byte first line is too long');
  });
  run('35a', (root) => {
    const report = fm(root, `${' '.repeat(1020)}---\nname: ok-name\ndescription: x\n---\n`);
    assertSelf(report.summary.errors === 0, '1020 spaces + `---\\n` is exactly 1024 bytes and is valid');
  });
  run('35b', (root) => {
    const report = fm(root, `${' '.repeat(1021)}---\nname: ok-name\n---\n`);
    assertSelf(hasCode(report, 'E-FM-FIRST-LINE-TOO-LONG'), '1025 bytes fails, proving the terminator is counted');
  });
  run('35c', (root) => {
    const body = `${'a'.repeat(16000)}\n`;
    const remaining = FRONTMATTER_MAX_BYTES - body.length;
    const report = fm(root, `---\n${body}${' '.repeat(remaining + 20)}---\n`);
    assertSelf(hasCode(report, 'E-FM-TOO-LARGE'), 'Guard A rejects an over-padded closing delimiter');
    assertSelf(!hasCode(report, 'E-FM-NO-CLOSE'), 'and it is not treated as a successful close');
  });
  run('35d', (root) => {
    const line = `${'a'.repeat(30)}\r\n`; // 32 bytes counting both terminator bytes
    const fits = line.repeat(FRONTMATTER_MAX_BYTES / line.length);
    assertSelf(fits.length === FRONTMATTER_MAX_BYTES, 'the fixture must be exactly at the limit');
    const okReport = fm(root, `---\r\n${fits}---\r\n`);
    assertSelf(!hasCode(okReport, 'E-FM-TOO-LARGE'), 'exactly 16384 bytes of CRLF body fits');
    const over = tempRoot();
    try {
      writeFixture(over, '_agent_x/skills/x/SKILL.md', `---\r\n${fits}${line}---\r\n`);
      assertSelf(hasCode(runCheck(over), 'E-FM-TOO-LARGE'), 'one more CRLF line tips it over');
    } finally {
      removeRoot(over);
    }
  });
  run('35e', (root) => {
    const report = fm(root, '---\rname: ok-name\r---\r');
    assertSelf(hasCode(report, 'E-FM-NO-OPEN'), 'classic-Mac line endings are not supported');
  });
  run('35f', (root) => {
    const report = fm(root, '\v---\v\nname: ok-name\n---\n');
    assertSelf(hasCode(report, 'E-FM-NO-OPEN'), '\\x0B is not ASCII whitespace in Rust, so the fence is rejected');
  });

  // ---- YAML (6.5) ----
  run('36', (root) => {
    // Three of 6.5.3's rows all emit E-YAML-NOT-MAPPING, so a bare hasCode() here is
    // satisfied by whichever row happens to fire: disable the empty-body row and the
    // zero-entries row at the bottom of parseMiniYaml produces the same code. The
    // message is what distinguishes which rule actually ran.
    const report = fm(root, '---\n---\n');
    const finding = report.findings.find((f) => f.code === 'E-YAML-NOT-MAPPING');
    assertSelf(finding !== undefined, 'empty frontmatter is not a mapping');
    assertSelf(
      finding.message.includes('empty or contains only blank'),
      'and it must be reported as an EMPTY body, not as a scalar root',
    );
  });
  run('37', (root) => {
    // Same trap as case 36: disable the sequence-root row and the colon-free lines fall
    // through to the zero-entries row, which emits the identical code.
    const report = fm(root, '---\n- a\n- b\n---\n');
    const finding = report.findings.find((f) => f.code === 'E-YAML-NOT-MAPPING');
    assertSelf(finding !== undefined, 'a sequence root is not a mapping');
    assertSelf(
      finding.message.includes('sequence'),
      'and it must be reported as a SEQUENCE root, not as a scalar root',
    );
  });
  run('38', (root) => {
    assertSelf(hasCode(fm(root, '---\nname: ok-name\n\tdescription: x\n---\n'), 'E-YAML-PARSE'), 'a tab in indentation');
  });
  run('39', (root) => {
    const report = fm(root, '---\nname: \'ok-name\'\ndescription: "a quoted value"\n---\n');
    assertSelf(report.summary.errors === 0, 'quoted scalars are strings');
    assertSelf(!hasCode(report, 'W-DESC-MISSING'), 'a double-quoted description is present');
  });
  run('40', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: |\n  line one\n  line two\nwhen_to_use: >\n  folded text\n---\n');
    assertSelf(report.findings.length === 0, 'block scalars are strings and produce no finding');
  });
  run('41', (root) => {
    for (const value of ['42', 'true', 'null', '']) {
      const inner = tempRoot();
      try {
        // No `description` and a non-string `when_to_use`, so both field-layer findings
        // WOULD fire if validation did not stop at the name failure.
        writeFixture(
          inner,
          '_agent_x/skills/x/SKILL.md',
          `---\nname:${value === '' ? '' : ` ${value}`}\nwhen_to_use: 9\n---\n`,
        );
        const report = runCheck(inner);
        assertSelf(hasCode(report, 'E-NAME-NOT-STRING'), `name: ${value} must be E-NAME-NOT-STRING`);
        // 6.6.1 step 2 says Stop, and :641-650 continues past the field layer, so an
        // indexerMessage about description or when_to_use would name a startup-log line
        // that is never printed.
        assertSelf(!hasCode(report, 'W-DESC-MISSING'), 'field validation must stop at the name failure');
        assertSelf(!hasCode(report, 'W-WHEN-NOT-STRING'), 'and it must not reach when_to_use either');
        assertSelf(report.findings.length === 1, 'the name failure is the only finding');
      } finally {
        removeRoot(inner);
      }
    }
  });
  run('42', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: yes\n---\n');
    assertSelf(report.findings.length === 0, '`yes` is a plain STRING under the YAML 1.2 core schema');
    const other = tempRoot();
    try {
      writeFixture(other, '_agent_x/skills/x/SKILL.md', '---\nname: ok-name\ndescription: on\n---\n');
      assertSelf(runCheck(other).findings.length === 0, '`on` is a plain STRING too');
    } finally {
      removeRoot(other);
    }
  });
  run('42a', (root) => {
    const report = fm(root, '---\nThis skill helps with X.\nIt does Y.\n---\n');
    assertSelf(hasCode(report, 'E-YAML-NOT-MAPPING'), 'a multi-line plain scalar root is not a mapping');
    assertSelf(report.exitCode === 1, 'and it fails the run');
  });
  run('42b', (root) => {
    const report = fm(root, '---\n{name: ok-name}\n---\n');
    assertSelf(hasCode(report, 'W-YAML-UNDECIDABLE'), 'a flow mapping at the root is undecidable');
    assertSelf(!hasCode(report, 'E-YAML-NOT-MAPPING'), 'zero recognized entries is not proof the root is a scalar');
    assertSelf(report.exitCode === 0, 'and it must not fail the run');
  });
  run('42c', (root) => {
    const report = fm(root, '---\nname: "ok-name"  # renamed\ndescription: x\n---\n');
    assertSelf(report.findings.length === 0, 'a trailing comment after a quoted scalar is not a finding');
    const other = tempRoot();
    try {
      writeFixture(other, '_agent_x/skills/x/SKILL.md', '---\nname:\tok-name\ndescription: x\n---\n');
      assertSelf(runCheck(other).findings.length === 0, 'a TAB is legal separation whitespace after `key:`');
    } finally {
      removeRoot(other);
    }
  });
  run('42d', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: |\n  \ttab-indented example line\n  plain line\n---\n');
    assertSelf(report.findings.length === 0, 'a tab INSIDE block-scalar content is not E-YAML-PARSE');
  });
  run('42e', (root) => {
    const report = fm(root, '---\ndescription: |\n  name: shadowed\nname: ok-name\n---\n');
    assertSelf(!hasCode(report, 'E-YAML-DUPLICATE-KEY'), 'the block consumes `name: shadowed` as content');
    assertSelf(report.summary.errors === 0, 'and the file is valid');
  });
  run('42f', (root) => {
    for (const value of ['!!str ok-name', '&a ok-name']) {
      const inner = tempRoot();
      try {
        writeFixture(inner, '_agent_x/skills/x/SKILL.md', `---\nname: ${value}\ndescription: x\n---\n`);
        const report = runCheck(inner);
        assertSelf(hasCode(report, 'W-YAML-UNDECIDABLE'), `${value} must be undecidable`);
        assertSelf(hasCode(report, 'W-NAME-UNDECIDABLE'), `${value} must warn, never E-NAME-INVALID`);
        assertSelf(!hasCode(report, 'E-NAME-INVALID'), 'an undecidable value may never produce an error');
        assertSelf(report.exitCode === 0, 'and the run stays green');
      } finally {
        removeRoot(inner);
      }
    }
  });
  run('42g', (root) => {
    // serde_yaml rejects a zero indentation indicator outright, so `|0` is a hard parse
    // error there. Treating it as a block scalar made the checker exit 0 on a file the
    // indexer refuses to parse.
    for (const header of ['|0', '>0']) {
      const inner = tempRoot();
      try {
        writeFixture(inner, '_agent_x/skills/x/SKILL.md', `---\nname: ok-name\ndescription: ${header}\n  text\n---\n`);
        const report = runCheck(inner);
        // A bare hasCode('W-YAML-UNDECIDABLE') pins NOTHING here: the orphaned `  text`
        // line is undecidable whether or not the header was rejected, so the assertion
        // is satisfied by the wrong finding. Both assertions below discriminate.
        // Rejected header: the header AND the orphan are undecidable, and `description`
        // resolves to `undecidable`, which asserts no W-DESC-*.
        // Accepted header: `|0` yields an empty block scalar, so `description` trims to
        // empty and W-DESC-MISSING fires, leaving only ONE undecidable.
        assertSelf(
          countCode(report, 'W-YAML-UNDECIDABLE') === 2,
          `\`${header}\` itself must be undecidable, not only the orphaned content line`,
        );
        assertSelf(
          !hasCode(report, 'W-DESC-MISSING'),
          `\`${header}\` must not resolve to an empty block scalar`,
        );
      } finally {
        removeRoot(inner);
      }
    }
    // A non-zero indicator is still an ordinary block scalar.
    const report = fm(root, '---\nname: ok-name\ndescription: |2\n  text\n---\n');
    assertSelf(!hasCode(report, 'W-YAML-UNDECIDABLE'), '`|2` stays inside the subset');
    assertSelf(report.findings.length === 0, 'and resolves to a non-empty string');
  });
  run('42h', () => {
    // The negative half of the quoted-key branch. Allowing whitespace between a quoted
    // key and its `:` widened what that branch accepts, so this pins the boundary from
    // the other side: a quoted scalar at column 0 that is NOT a mapping key must never
    // become one. Each body is a single line, so the root really is a scalar and 6.5.3
    // row 4 fires; `findings.length === 1` also proves the certain error is terminal and
    // no W-YAML-UNDECIDABLE leaks past it.
    const notKeys = [
      "'just a quoted doc'",
      "'quoted' trailing text",
      "'quoted'   ",
      "'quoted' # comment",
      "'a' 'b': c",
      '"double quoted doc"',
    ];
    for (const body of notKeys) {
      const inner = tempRoot();
      try {
        writeFixture(inner, '_agent_x/skills/x/SKILL.md', `---\n${body}\n---\n`);
        const report = runCheck(inner);
        assertSelf(
          hasCode(report, 'E-YAML-NOT-MAPPING'),
          `\`${body}\` must not become a mapping key`,
        );
        assertSelf(report.exitCode === 1, `\`${body}\` must fail the run`);
        assertSelf(report.findings.length === 1, `\`${body}\` must yield exactly one finding`);
      } finally {
        removeRoot(inner);
      }
    }
  });
  run('43', (root) => {
    const report = fm(root, '---\nname: ok-name\nnested:\n  a: 1\ndescription: x\n---\n');
    assertSelf(hasCode(report, 'W-YAML-UNDECIDABLE'), 'a nested mapping is undecidable');
  });
  run('44', (root) => {
    const report = fm(root, '---\nname: ok-name\nother: {a: 1}\ndescription: x\n---\n');
    assertSelf(hasCode(report, 'W-YAML-UNDECIDABLE'), 'a flow mapping value is undecidable');
  });
  run('45', (root) => {
    const report = fm(root, '---\nname: ok-name\na: &anchor x\nb: *anchor\nc: !!str y\ndescription: z\n---\n');
    assertSelf(countCode(report, 'W-YAML-UNDECIDABLE') === 3, 'an anchor, an alias and a tag are each undecidable');
  });
  run('46', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: x\nname: other-name\n---\n');
    assertSelf(hasCode(report, 'E-YAML-DUPLICATE-KEY'), 'a duplicate key is a hard error');
    assertSelf(report.exitCode === 1, 'and it fails the run');
    assertSelf(!hasCode(report, 'W-DESC-MISSING'), 'no field-level finding is emitted for that entrypoint');
    assertSelf(report.findings.length === 1, 'the duplicate is the only finding');
  });
  run('46b', (root) => {
    const report = fm(root, '---\nname: ok-name\nfoo: 1\ndescription: x\nfoo: 2\n---\n');
    assertSelf(hasCode(report, 'E-YAML-DUPLICATE-KEY'), 'duplicate detection covers keys the indexer never reads');
  });
  run('46c', (root) => {
    const report = fm(root, "---\nname: ok-name\n'name': other\n---\n");
    assertSelf(hasCode(report, 'E-YAML-DUPLICATE-KEY'), '`name` and `\'name\'` are the same key after unquoting');
    // Whitespace before the `:` is separation, not part of the key, so these collide too.
    for (const second of ['name : other', 'name\t: other', "'name' : other"]) {
      const inner = tempRoot();
      try {
        writeFixture(inner, '_agent_x/skills/x/SKILL.md', `---\nname: ok-name\n${second}\n---\n`);
        assertSelf(
          hasCode(runCheck(inner), 'E-YAML-DUPLICATE-KEY'),
          `\`${second}\` must collide with \`name\`: YAML strips the trailing space from the key`,
        );
      } finally {
        removeRoot(inner);
      }
    }
    // The line the strip must not cross. A space INSIDE the quotes is part of the key, so
    // `'name '` is a different key from `name` and the two must not merge. The folder is
    // deliberately invalid: if `name` had been mis-resolved, the fallback would fire and
    // this would exit 1 on E-NAME-INVALID instead of staying clean.
    const distinct = tempRoot();
    try {
      writeFixture(distinct, '_agent_x/skills/Bad_Folder/SKILL.md', "---\nname: ok-name\n'name ': other\ndescription: x\n---\n");
      const report = runCheck(distinct);
      assertSelf(!hasCode(report, 'E-YAML-DUPLICATE-KEY'), "`'name '` and `name` are two distinct keys");
      assertSelf(!hasCode(report, 'E-NAME-INVALID'), '`name` still resolves through the key, not the folder');
      assertSelf(report.findings.length === 0, 'and the file is otherwise clean');
    } finally {
      removeRoot(distinct);
    }
  });
  run('46d', (root) => {
    const report = fm(root, '---\nname: ok-name\nName: other\ndescription: x\n---\n');
    assertSelf(!hasCode(report, 'E-YAML-DUPLICATE-KEY'), 'key matching is case-sensitive');
    assertSelf(report.summary.errors === 0, 'and the file is otherwise valid');
  });
  run('47', (root) => {
    const report = fm(root, '---\nname: ok-name\nfoo: bar\ndescription: x\n---\n');
    assertSelf(report.findings.length === 0, 'an unknown key produces no finding');
  });
  run('48', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: x\nwhen-to-use: never read\n---\n');
    assertSelf(hasCode(report, 'I-KEY-HYPHEN-VARIANT'), 'the hyphenated spelling is surfaced');
    assertSelf(report.exitCode === 0, 'as a note only');
  });
  run('49', (root) => {
    const report = fm(root, '---\n# a comment\n\nname: ok-name\n\n  # an indented comment\ndescription: x\n---\n');
    assertSelf(report.findings.length === 0, 'blank and comment lines are ignored');
  });

  // ---- Fields (6.6) ----
  run('50', (root) => {
    writeFixture(root, '_agent_x/skills/my-skill/SKILL.md', '---\ndescription: x\n---\n');
    const report = runCheck(root);
    assertSelf(report.summary.errors === 0, 'the folder fallback resolves a valid name');
  });
  run('50a', (root) => {
    // A space or tab before the `:` is separation whitespace; the key is still `name`.
    // Keeping it made the parser record a key nothing reads, so `name` resolved from the
    // folder instead and the run came back with NO finding at all.
    writeFixture(root, '_agent_x/skills/ok-folder/SKILL.md', '---\nname : Bad_Name\ndescription: x\n---\n');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-NAME-INVALID'), '`name : Bad_Name` must resolve through the key, not the folder');
    assertSelf(report.exitCode === 1, 'and it must fail the run');

    const tabbed = tempRoot();
    try {
      writeFixture(tabbed, '_agent_x/skills/ok-folder/SKILL.md', '---\nname\t: Bad_Name\ndescription: x\n---\n');
      assertSelf(hasCode(runCheck(tabbed), 'E-NAME-INVALID'), 'a tab before the `:` behaves the same');
    } finally {
      removeRoot(tabbed);
    }

    // The mirror image: the key resolves, so the bad FOLDER name is never consulted and
    // the skill is clean. Keeping the space here produced a spurious E-NAME-INVALID.
    const mirrored = tempRoot();
    try {
      writeFixture(mirrored, '_agent_x/skills/Bad_Folder/SKILL.md', '---\nname : ok-name\ndescription: x\n---\n');
      const clean = runCheck(mirrored);
      assertSelf(clean.findings.length === 0, '`name : ok-name` in a bad folder is indexed cleanly');
      assertSelf(clean.exitCode === 0, 'and must not fail the run');
    } finally {
      removeRoot(mirrored);
    }
  });
  run('51', (root) => {
    writeFixture(root, '_agent_x/skills/My_Skill/SKILL.md', '---\ndescription: x\n---\n');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-NAME-INVALID'), 'the folder fallback is charset-checked too');
    const finding = report.findings.find((f) => f.code === 'E-NAME-INVALID');
    assertSelf(finding.message.includes('containing directory name'), 'the message must name the folder fallback');
  });
  run('51a', (root) => {
    // :653-660 also continues past the field layer, so the charset failure stops
    // validation exactly like the type failure in case 41 does.
    writeFixture(root, '_agent_x/skills/x/SKILL.md', '---\nname: Bad_Name\nwhen_to_use: 9\n---\n');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-NAME-INVALID'), 'the charset failure is reported');
    assertSelf(!hasCode(report, 'W-DESC-MISSING'), 'and validation stops before description');
    assertSelf(!hasCode(report, 'W-WHEN-NOT-STRING'), 'and before when_to_use');
    assertSelf(report.findings.length === 1, 'the name failure is the only finding');
  });
  run('52', (root) => {
    writeFixture(root, '_agent_x/skills/ok-name/SKILL.md', "---\nname: ''\ndescription: x\n---\n");
    const report = runCheck(root);
    assertSelf(report.summary.errors === 0, 'an empty name trims to absent and falls back to the folder');
  });
  run('53', (root) => {
    writeFixture(root, '_agent_x/skills/folder-name/SKILL.md', validSkillMd('other-name'));
    assertSelf(runCheck(root).findings.length === 0, 'name need not match the directory name');
  });
  run('54', (root) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', validSkillMd('a'.repeat(64)));
    assertSelf(runCheck(root).summary.errors === 0, 'a 64-character name is valid');
  });
  run('55', (root) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', validSkillMd('a'.repeat(65)));
    assertSelf(hasCode(runCheck(root), 'E-NAME-INVALID'), 'a 65-character name is invalid');
  });
  run('56', (root) => {
    const report = fm(root, '---\nname: Café\ndescription: x\n---\n');
    assertSelf(hasCode(report, 'E-NAME-INVALID'), 'a non-ASCII name is invalid');
    const finding = report.findings.find((f) => f.code === 'E-NAME-INVALID');
    assertSelf(finding.message.includes(' 4'), 'the length must be counted in Unicode scalar values');
  });
  run('57', (root) => {
    writeFixture(root, '_agent_x/skills/aaa/SKILL.md', validSkillMd('shared'));
    writeFixture(root, '_agent_x/skills/bbb/SKILL.md', validSkillMd('shared'));
    const report = runCheck(root);
    assertSelf(countCode(report, 'E-NAME-DUPLICATE') === 1, 'exactly the loser is reported');
    const finding = report.findings.find((f) => f.code === 'E-NAME-DUPLICATE');
    assertSelf(finding.path.includes('bbb'), 'the second in sort order loses');
    assertSelf(finding.message.includes('aaa'), 'and the message names the winner');
  });
  run('58', (root) => {
    writeFixture(root, '_agent_x/skills/aaa/SKILL.md', validSkillMd('shared'));
    writeFixture(root, '_agent_y/skills/bbb/SKILL.md', validSkillMd('shared'));
    assertSelf(!hasCode(runCheck(root), 'E-NAME-DUPLICATE'), 'two matrices are separate indexes');
  });
  run('59', (root) => {
    const report = fm(root, '---\nname: ok-name\n---\n');
    assertSelf(hasCode(report, 'W-DESC-MISSING'), 'a missing description is a warning');
    assertSelf(report.exitCode === 0, 'and never fails the run');
    assertSelf(report.summary.skillsIndexable === 1, 'and the skill is still indexable');
  });
  run('60', (root) => {
    assertSelf(hasCode(fm(root, "---\nname: ok-name\ndescription: ''\n---\n"), 'W-DESC-MISSING'), 'an empty description');
  });
  run('61', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: 42\n---\n');
    assertSelf(hasCode(report, 'W-DESC-NOT-STRING'), 'a numeric description is a warning');
    assertSelf(report.exitCode === 0, 'and never fails the run');
  });
  run('62', (root) => {
    const report = fm(root, '---\nname: ok-name\ndescription: x\nwhen_to_use: 42\n---\n');
    assertSelf(hasCode(report, 'W-WHEN-NOT-STRING'), 'a numeric when_to_use is a warning');
    assertSelf(report.exitCode === 0, 'and never fails the run');
  });
  run('63', (root) => {
    assertSelf(fm(root, '---\nname: ok-name\ndescription: x\n---\n').findings.length === 0, 'absent when_to_use');
  });
  run('63a', (root) => {
    const report = fm(root, '---\nname: "﻿ok-name"\ndescription: x\n---\n');
    assertSelf(hasCode(report, 'E-NAME-INVALID'), 'Rust str::trim does NOT strip U+FEFF, so this name is invalid');
  });
  run('63b', (root) => {
    const report = fm(root, '---\nname: "ok-name"\ndescription: x\n---\n');
    assertSelf(report.summary.errors === 0, 'Rust str::trim DOES strip U+0085, so this name is valid');
  });
  run('63c', (root) => {
    if (!unreadableDirSupported) {
      skip('63c', 'directory permissions do not bind the current user on this platform');
      return 'skip';
    }
    const dir = makeDir(root, '_agent_x/skills/x');
    fs.chmodSync(dir, 0o000);
    try {
      const report = runCheck(root);
      assertSelf(hasCode(report, 'E-SKILL-DIR-UNREADABLE'), 'an unreadable canonical skill directory is a hard error');
      assertSelf(!hasCode(report, 'W-DIR-UNREADABLE'), 'and not a warning');
    } finally {
      fs.chmodSync(dir, 0o700);
    }
    return undefined;
  });
  run('63g', (root) => {
    if (!unreadableDirSupported) {
      skip('63g', 'directory permissions do not bind the current user on this platform');
      return 'skip';
    }
    const dir = makeDir(root, 'docs/skills/x');
    fs.chmodSync(dir, 0o000);
    try {
      const report = runCheck(root);
      assertSelf(hasCode(report, 'W-DIR-UNREADABLE'), 'outside canonical position the same failure is a warning');
      assertSelf(report.exitCode === 0, 'and it must not fail the run');
    } finally {
      fs.chmodSync(dir, 0o700);
    }
    return undefined;
  });
  run('63d', (root) => {
    writeFixture(root, '_agent_x/skills/aaa/SKILL.md', '---\nname: shared\n');
    writeFixture(root, '_agent_x/skills/bbb/SKILL.md', validSkillMd('shared'));
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-FM-NO-CLOSE'), 'the broken candidate is reported');
    assertSelf(!hasCode(report, 'E-NAME-DUPLICATE'), 'a candidate with any hard error never claims its name');
    assertSelf(
      !report.findings.some((f) => f.path.includes('bbb')),
      'and the innocent skill gets no finding at all',
    );
  });
  run('63e', (root) => {
    const target = makeDir(root, 'gone');
    makeDir(root, '_agent_x');
    makeDirLink(target, path.join(root, '_agent_x/skills'));
    fs.rmSync(target, { recursive: true, force: true });
    const report = runCheck(root);
    assertSelf(report.findings.length === 0, 'a broken `skills` symlink is silent, exactly as the indexer is');
    assertSelf(report.exitCode === 0, 'and never fails the run');
  });
  run('63f', (root) => {
    const target = makeDir(root, 'real');
    makeDir(root, '_agent_x');
    makeDirLink(target, path.join(root, '_agent_x/skills'));
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-SKILLS-NOT-DIR'), 'a resolving `skills` symlink is a hard error');
    assertSelf(report.exitCode === 1, 'and it fails the run');
  });

  // ---- Position (6.7) ----
  run('64', (root) => {
    writeFixture(root, '_agent_x/skills/y/SKILL.md', validSkillMd('y'));
    assertSelf(!hasCode(runCheck(root), 'I-NOT-INDEXABLE'), 'canonical position produces no position finding');
  });
  run('65', (root) => {
    writeFixture(root, '__agent_x/skills/y/SKILL.md', validSkillMd('y'));
    const report = runCheck(root);
    const finding = report.findings.find((f) => f.code === 'I-NOT-INDEXABLE');
    assertSelf(finding !== undefined && finding.reason === 'replica-root', 'a replica root is never scanned');
    assertSelf(report.exitCode === 0, 'and it is a note only');
  });
  run('66', (root) => {
    writeFixture(root, '_agent_x/skills/y/z/SKILL.md', validSkillMd('z'));
    const finding = runCheck(root).findings.find((f) => f.code === 'I-NOT-INDEXABLE');
    assertSelf(finding.reason === 'not-under-skills-dir', 'depth is fixed at one');
  });
  run('67', (root) => {
    writeFixture(root, '_agent_x/Skills/y/SKILL.md', validSkillMd('y'));
    const finding = runCheck(root).findings.find((f) => f.code === 'I-NOT-INDEXABLE');
    assertSelf(finding.reason === 'skills-dir-case', 'a case-variant skills dir is platform-dependent');
  });
  run('68', (root) => {
    // 6.7 and the 6.9 worked example both give `not-under-skills-dir` for this exact
    // path; the 9.2 row naming `owner-root-not-recognized` for it contradicts them and
    // the numbered section wins. The second fixture exercises the reason that row meant.
    writeFixture(root, 'docs/examples/SKILL.md', validSkillMd('examples'));
    const finding = runCheck(root).findings.find((f) => f.code === 'I-NOT-INDEXABLE');
    assertSelf(finding.reason === 'not-under-skills-dir', 'a SKILL.md not under a skills/ folder');
    const other = tempRoot();
    try {
      writeFixture(other, 'not-a-matrix/skills/y/SKILL.md', validSkillMd('y'));
      const owner = runCheck(other).findings.find((f) => f.code === 'I-NOT-INDEXABLE');
      assertSelf(owner.reason === 'owner-root-not-recognized', 'an unrecognized owner root is a note');
    } finally {
      removeRoot(other);
    }
  });
  run('69', (root) => {
    writeFixture(root, 'docs/skills/y/SKILL.md', '---\nname: Bad_Name\ndescription: x\n---\n');
    const report = runCheck(root);
    assertSelf(hasCode(report, 'E-NAME-INVALID'), 'severity rule 1: a present entrypoint is held to the rules everywhere');
    assertSelf(report.exitCode === 1, 'and it fails the run wherever it sits');
  });

  // ---- Output (6.9, 6.10) ----
  run('70', (root) => {
    writeFixture(root, '_agent_x/skills/Bad_Name/SKILL.md', '---\ndescription: x\n---\n');
    writeFixture(root, '_agent_x/skills/warn/SKILL.md', '---\nname: warn\n---\n');
    writeFixture(root, 'docs/examples/SKILL.md', validSkillMd('examples'));
    const lines = [];
    const code = runCli([root, '--json'], (line) => lines.push(line), () => {});
    const document = JSON.parse(lines.join('\n'));
    assertSelf(document.exitCode === code, '`exitCode` must match the process exit code');
    assertSelf(
      document.summary.errors === document.findings.filter((f) => f.severity === 'error').length,
      'summary.errors must match the array',
    );
    assertSelf(
      document.summary.warnings === document.findings.filter((f) => f.severity === 'warning').length,
      'summary.warnings must match the array',
    );
    assertSelf(
      document.summary.infos === document.findings.filter((f) => f.severity === 'info').length,
      'summary.infos must match the array',
    );
  });
  run('71', (root) => {
    writeFixture(root, '_agent_x/skills/b-skill/SKILL.md', '---\nname: Bad_Name\ndescription: x\n---\n');
    writeFixture(root, '_agent_x/skills/a-skill/SKILL.md', '---\nname: ok-name\n---\n');
    const first = renderJson(runCheck(root));
    const second = renderJson(runCheck(root));
    assertSelf(first === second, 'two runs must produce byte-identical output');
    const document = JSON.parse(first);
    for (let i = 1; i < document.findings.length; i += 1) {
      const previous = document.findings[i - 1];
      const current = document.findings[i];
      const ordered =
        previous.path < current.path ||
        (previous.path === current.path && (previous.line ?? 0) <= (current.line ?? 0));
      assertSelf(ordered, 'findings must be sorted by path then line');
    }
  });
  run('72', (root) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', '---\nname: ok-name\n---\n');
    const document = runCheck(root);
    for (const finding of document.findings) {
      assertSelf(!finding.path.includes('\\'), '`path` must be /-separated on every platform');
      assertSelf(
        finding.skillDirectory === null || !finding.skillDirectory.includes('\\'),
        '`skillDirectory` must be /-separated on every platform',
      );
    }
  });
  run('73', (root) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', validSkillMd('x'));
    const lines = [];
    const code = runCli([root], (line) => lines.push(line), () => {});
    assertSelf(code === 0, 'a clean tree exits 0');
    assertSelf(lines[lines.length - 1].includes('OK:'), 'and the human report ends with OK:');
  });
  run('74', (root) => {
    writeFixture(root, '_agent_x/skills/x/SKILL.md', '---\nname: ok-name\n---\n');
    writeFixture(root, 'docs/examples/SKILL.md', validSkillMd('examples'));
    const report = runCheck(root);
    assertSelf(report.summary.warnings > 0 && report.summary.infos > 0, 'the fixture must carry both');
    assertSelf(report.exitCode === 0, 'warnings and notes never change the exit code');
  });
  run('75', (root) => {
    writeFixture(root, '_agent_x/skills/x/skill.md', validSkillMd('x'));
    assertSelf(runCheck(root).exitCode === 1, 'one error exits 1');
  });
  run('76', (root) => {
    makeDir(root, '_agent_x');
    fs.writeFileSync(path.join(root, '_agent_x/skills'), 'not a directory');
    const lines = [];
    const code = runCli([root], (line) => lines.push(line), () => {});
    assertSelf(code === 1, 'a `skills` file is a hard error');
    const last = lines[lines.length - 1];
    assertSelf(last.includes('no entrypoint implicated'), 'the FAIL line must say no entrypoint is implicated');
    assertSelf(!last.includes('FAIL: 0 skills'), 'and must never read `FAIL: 0 skills would not be indexed`');
  });
  run('77', (root) => {
    if (!caseSensitiveFs) {
      skip('77', 'case-insensitive filesystem: SKILL.md and skill.md cannot coexist');
      return 'skip';
    }
    const dir = makeDir(root, '_agent_x/skills/x');
    fs.writeFileSync(path.join(dir, 'SKILL.md'), validSkillMd('x'));
    fs.writeFileSync(path.join(dir, 'skill.md'), validSkillMd('x'));
    const report = runCheck(root);
    assertSelf(report.summary.entrypointsFound === 2, 'every discovered match counts, shadowed ones included');
    assertSelf(report.summary.skillsIndexable === 1, 'but a shadowed variant is not a second indexable skill');
    return undefined;
  });
  run('78', (root) => {
    // The parser emits at most one W-YAML-UNDECIDABLE per line, so two of them against
    // the SAME line of the same file is unreachable here and the emission-order tiebreak
    // is defensive. What is reachable, and what this case pins, is that several findings
    // sharing a code order identically across runs, so the sort is total in practice.
    writeFixture(root, '_agent_x/skills/x/SKILL.md', '---\nname: ok-name\ndescription: x\nother: {a: 1}\nmore: {b: 2}\n---\n');
    writeFixture(root, '_agent_x/skills/y/SKILL.md', '---\nname: ok-two\ndescription: x\nother: {a: 1}\nmore: {b: 2}\n---\n');
    const first = renderJson(runCheck(root));
    const second = renderJson(runCheck(root));
    assertSelf(first === second, 'the emission-order tiebreak must make the sort total');
    assertSelf(JSON.parse(first).summary.warnings === 4, 'the fixture must carry four same-code findings');
  });
  run('79', (root) => {
    if (!unreadableDirSupported) {
      skip('79', 'directory permissions do not bind the current user on this platform');
      return 'skip';
    }
    const dir = makeDir(root, '_agent_x/skills');
    fs.chmodSync(dir, 0o000);
    try {
      const report = runCheck(root);
      assertSelf(hasCode(report, 'E-SKILLS-DIR-UNREADABLE'), 'an unreadable skills root is its own code');
      const finding = report.findings.find((f) => f.code === 'E-SKILLS-DIR-UNREADABLE');
      assertSelf(
        finding.indexerMessage.includes('`skills` directory could not be read'),
        'and it carries the skills-root message, distinct from case 63c',
      );
    } finally {
      fs.chmodSync(dir, 0o700);
    }
    return undefined;
  });
  run('80', (root) => {
    writeFixture(
      root,
      '_agent_x/skills/x/SKILL.md',
      `---\nname: "bad\\nname\\u001b[31m${'z'.repeat(300)}"\ndescription: x\n---\n`,
    );
    const lines = [];
    runCli([root], (line) => lines.push(line), () => {});
    for (const line of lines) {
      const controls = /[\u0000-\u0008\u000B\u000C\u000E-\u001F]/;
      assertSelf(!controls.test(line), 'no raw control character may reach the report');
    }
    const finding = runCheck(root).findings.find((f) => f.code === 'E-NAME-INVALID');
    assertSelf(finding.message.length < 400, 'and the value must be truncated');
  });

  // Acceptance criterion 12: every code in Section 6.8 must be emitted by at least one
  // case, with ONE named exemption. E-ENTRYPOINT-UNREADABLE needs a file whose open or
  // read throws and is not reliably constructible as the owning user on Windows or as
  // root on Linux (9.2.2), so it has no case by design and this comment is what marks
  // it as an exemption rather than an oversight.

  if (failure !== null) {
    out(`${TAG} self-test FAILED: ${failure}`);
    return 1;
  }

  const skippedNote = skipped.length === 0 ? '0 skipped' : `${skipped.length} skipped (${skipped.join(', ')})`;
  out(`${TAG} self-test passed: ${total} cases, ${ran} run, ${skippedNote}`);
  return 0;
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

function main() {
  const write = (line) => process.stdout.write(`${line}\n`);
  const warn = (line) => process.stderr.write(`${line}\n`);
  try {
    process.exitCode = runCli(process.argv.slice(2), write, warn);
  } catch (error) {
    warn(`internal error: ${error instanceof Error ? error.message : String(error)}`);
    if (error instanceof Error && error.stack) warn(error.stack);
    process.exitCode = 2;
  }
}

main();
