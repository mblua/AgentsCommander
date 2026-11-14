// Finite lexical Rust analysis, not type resolution or general macro expansion.
import { GateError } from './sanitizing_baseline.mjs';
const identStart = /[\p{ID_Start}_]/u, identPart = /[\p{ID_Continue}_]/u;
const opens = { '(': ')', '[': ']', '{': '}' };
const closes = new Set(Object.values(opens));
function error(path, line, reason) { throw new GateError(
  `${path}:${line}: ${reason}; review Rust syntax`); }

// Rust escapes, rather than JSON/JavaScript string escapes.
const RUST_SIMPLE_ESCAPES = { n: '\n', r: '\r', t: '\t', '0': '\0',
  [String.fromCodePoint(92)]: String.fromCodePoint(92), '"': '"', "'": "'" };
function decodeHexEscape(source, i, byte) {
  const hex = source.slice(i + 1, i + 3);
  if (!/^[a-f\d]{2}$/i.test(hex)) throw new GateError('invalid Rust hex escape');
  const code = Number.parseInt(hex, 16);
  if (!byte && code > 127) throw new GateError('non-ASCII Rust string hex escape');
  // A byte above 127 makes the value opaque; still validate all remaining escapes.
  return { text: String.fromCodePoint(code), i: i + 2, opaque: byte && code > 127 };
}
function decodeUnicodeEscape(source, i) {
  const end = source.indexOf('}', i + 2), hex = source.slice(i + 2, end).replaceAll('_', '');
  if (end < 0 || !/^[a-f\d]{1,6}$/i.test(hex)) throw new GateError(
    'invalid Rust Unicode escape');
  const code = Number.parseInt(hex, 16);
  if (code > 0x10ffff || code >= 0xd800 &&
    code <= 0xdfff) throw new GateError('invalid Rust Unicode scalar');
  return { text: String.fromCodePoint(code), i: end };
}
function skipLineContinuation(source, i, e) {
  if (e === '\r') i++;
  while (/[\t\n\r ]/.test(source[i + 1] ?? '') && i + 1 < source.length) i++;
  return i;
}
// Decodes the escape after the backslash at source[i - 1]; returns its text and last index.
function decodeEscape(source, i, byte) {
  const e = source[i];
  if (Object.hasOwn(RUST_SIMPLE_ESCAPES, e)) return { text: RUST_SIMPLE_ESCAPES[e], i };
  if (e === 'x') return decodeHexEscape(source, i, byte);
  if (e === 'u' && source[i + 1] === '{' && !byte) return decodeUnicodeEscape(source, i);
  if (e === '\n' || e === '\r' && source[i + 1] === '\n') {
    return { text: '', i: skipLineContinuation(source, i, e) };
  }
  throw new GateError('invalid Rust escape');
}
export function decodeRustString(raw, byte = false) {
  const prefix = /^(?:b)?r(#+|)"/.exec(raw);
  if (prefix) return raw.slice(prefix[0].length, -prefix[1].length - 1);
  const source = raw.slice(byte ? 2 : 1, -1);
  let value = '', opaque = false;
  for (let i = 0; i < source.length; i++) {
    const c = source[i];
    if (c.codePointAt(0) !== 92) { value += c; continue; }
    const escape = decodeEscape(source, i + 1, byte);
    value += escape.text; i = escape.i;
    if (escape.opaque) opaque = true;
  }
  return opaque || byte && /[^\x00-\x7f]/.test(value) ? null : value;
}
// Lexer state s: { path, text, tokens, groups, stack, i, line }.
function lexAdd(s, kind, start, tokenLine, extra = {}) {
  const token = { kind, text: s.text.slice(start, s.i), start, end: s.i, line: tokenLine,
    index: s.tokens.length, ...extra };
  s.tokens.push(token); return token;
}
function lexAdvance(s, end) { for (; s.i < end; s.i++) if (s.text[s.i] === '\n') s.line++; }
function lexBlockComment(s, tokenLine) {
  s.i += 2; let depth = 1;
  while (s.i < s.text.length && depth) {
    if (s.text.startsWith('/*', s.i)) { depth++; lexAdvance(s, s.i + 2); }
    else if (s.text.startsWith('*/', s.i)) { depth--; lexAdvance(s, s.i + 2); }
    else lexAdvance(s, s.i + 1);
  }
  if (depth) error(s.path, tokenLine, 'unclosed block comment');
}
function lexTrivia(s, c, tokenLine) {
  if (/\s/.test(c)) { lexAdvance(s, s.i + 1); return true; }
  if (s.text.startsWith('//', s.i)) {
    let end = s.text.indexOf('\n', s.i); if (end < 0) end = s.text.length;
    lexAdvance(s, end); return true;
  }
  if (s.text.startsWith('/*', s.i)) { lexBlockComment(s, tokenLine); return true; }
  return false;
}
function lexRawString(s, start, tokenLine) {
  const text = s.text, raw = /^(?:br|r)(#*)"/.exec(text.slice(s.i));
  if (!raw) return false;
  const suffix = '"' + raw[1], end = text.indexOf(suffix, s.i + raw[0].length);
  if (end < 0) error(s.path, s.line, 'unclosed raw string');
  lexAdvance(s, end + suffix.length);
  const byte = raw[0].startsWith('b');
  const value = decodeRustString(text.slice(start, s.i), byte);
  const opaque = byte && /[^\x00-\x7f]/.test(value);
  lexAdd(s, byte ? 'byteString' : 'string', start, tokenLine,
    { value: opaque ? null : value, opaque });
  return true;
}
function lexStringBody(s) {
  while (s.i < s.text.length) {
    if (s.text.codePointAt(s.i) === 92) {
      lexAdvance(s, Math.min(s.i + 2, s.text.length)); continue;
    }
    if (s.text[s.i] === '"') { lexAdvance(s, s.i + 1); return true; }
    lexAdvance(s, s.i + 1);
  }
  return false;
}
function lexString(s, c, start, tokenLine) {
  if (c !== '"' && (c !== 'b' || s.text[s.i + 1] !== '"')) return false;
  const byte = c === 'b'; lexAdvance(s, s.i + (byte ? 2 : 1));
  if (!lexStringBody(s)) error(s.path, tokenLine, 'unclosed string');
  let value; try { value = decodeRustString(s.text.slice(start, s.i), byte);
    } catch (e) { error(s.path, tokenLine, e.message); }
  lexAdd(s, byte ? 'byteString' : 'string', start, tokenLine, { value, opaque: value === null });
  return true;
}
// Index just past a char literal body that starts at `end`.
function charLiteralEnd(text, end) {
  if (text.codePointAt(end) === 92) {
    end++;
    if (text[end] === 'u' && text[end + 1] === '{') {
      const close = text.indexOf('}', end + 2);
      return close < 0 ? text.length : close + 1;
    }
    return end + (text[end] === 'x' ? 3 : 1);
  }
  return end + ((text.codePointAt(end) ?? 0) > 0xffff ? 2 : 1);
}
function lexChar(s, c, start, tokenLine) {
  const text = s.text;
  if (c !== "'" && (c !== 'b' || text[s.i + 1] !== "'")) return false;
  const byte = c === 'b', quote = s.i + (byte ? 1 : 0), end = charLiteralEnd(text, quote + 1);
  if (text[end] === "'") {
    const content = text.slice(quote + 1, end);
    try { decodeRustString((byte ? 'b"' : '"') + content + '"', byte); }
    catch (e) { error(s.path, tokenLine, e.message); }
    lexAdvance(s, end + 1); lexAdd(s, byte ? 'byteChar' : 'char', start, tokenLine); return true;
  }
  const life = /^'[\p{ID_Start}_][\p{ID_Continue}_]*/u.exec(text.slice(s.i));
  if (!byte && life) {
    lexAdvance(s, s.i + life[0].length); lexAdd(s, 'lifetime', start, tokenLine); return true;
  }
  error(s.path, tokenLine, 'invalid or unclosed character');
}
function lexIdent(s, c, start, tokenLine) {
  const text = s.text, rawIdent = /^r#([\p{ID_Start}_][\p{ID_Continue}_]*)/u.exec(
    text.slice(s.i));
  if (!rawIdent && !identStart.test(c)) return false;
  if (rawIdent) lexAdvance(s, s.i + rawIdent[0].length);
  else {
    lexAdvance(s, s.i + c.length);
    while (s.i < text.length && identPart.test(String.fromCodePoint(text.codePointAt(s.i)))) {
      lexAdvance(s, s.i + (text.codePointAt(s.i) > 0xffff ? 2 : 1));
    }
  }
  lexAdd(s, 'ident', start, tokenLine, { name: text.slice(start, s.i).replace(/^r#/, '') });
  return true;
}
function lexNumber(s, c, start, tokenLine) {
  if (!/\d/.test(c)) return false;
  // A tuple index cannot absorb the following projection, call or range.
  const rest = s.text.slice(s.i);
  lexAdvance(s, s.i + (s.tokens.at(-1)?.text === '.' ? /^\d[\d_]*/.exec(rest)[0].length
    : rustNumberLength(rest)));
  lexAdd(s, 'number', start, tokenLine); return true;
}
const RUST_PUNCTUATION = ['::', '=>', '->', '==', '!=', '+=', '-=', '*=', '/=', '..=', '..',
  '&&', '||'];
function lexCloseGroup(s, c, token, tokenLine) {
  const tokens = s.tokens, open = s.stack.pop();
  if (open === undefined || opens[tokens[open].text] !== c) error(s.path, tokenLine,
    'mismatched delimiter');
  tokens[open].mate = token.index; token.mate = open; s.groups.push({ start: open,
    end: token.index, kind: tokens[open].text });
}
function lexPunct(s, c, start, tokenLine) {
  const punctuation = RUST_PUNCTUATION.find(p => s.text.startsWith(p, s.i)) ?? c;
  lexAdvance(s, s.i + punctuation.length); const token = lexAdd(s, 'punct', start, tokenLine);
  if (opens[c]) s.stack.push(token.index);
  else if (closes.has(c)) lexCloseGroup(s, c, token, tokenLine);
}
function lexToken(s) {
  const start = s.i, tokenLine = s.line, c = String.fromCodePoint(s.text.codePointAt(s.i));
  if (lexTrivia(s, c, tokenLine) || lexRawString(s, start, tokenLine)
    || lexString(s, c, start, tokenLine) || lexChar(s, c, start, tokenLine)
    || lexIdent(s, c, start, tokenLine) || lexNumber(s, c, start, tokenLine)) return;
  lexPunct(s, c, start, tokenLine);
}
export function lexRust(path, text) {
  const s = { path, text, tokens: [], groups: [], stack: [], i: 0, line: 1 };
  while (s.i < text.length) lexToken(s);
  if (s.stack.length) error(path, s.tokens[s.stack.at(-1)].line, 'unclosed delimiter');
  return { tokens: s.tokens, groups: s.groups };
}
// Rust numeric literal: radix or decimal head, then an optional type suffix.
const RUST_RADIX_INT = /^0(?:x[\da-fA-F_]+|o[0-7_]+|b[01_]+)/;
const RUST_DECIMAL = /^\d[\d_]*(?:\.(?!\.|[_\p{ID_Start}])[\d_]*)?(?:[eE][+-]?[\d_]+)?/u;
const RUST_SUFFIX = /^[_\p{ID_Start}][\p{ID_Continue}_]*/u;
function rustNumberLength(text) {
  const head = (RUST_RADIX_INT.exec(text) ?? RUST_DECIMAL.exec(text))[0];
  return head.length + (RUST_SUFFIX.exec(text.slice(head.length))?.[0].length ?? 0);
}
function splitTop(tokens, separator = ',') {
  const result = []; let start = 0;
  for (let i = 0; i < tokens.length; i++) {
    if (opens[tokens[i].text]) { const mate = tokens.findIndex(t => t.index === tokens[i].mate);
      if (mate >= i) i = mate; }
    else if (tokens[i].text === separator) { if (i > start) result.push(tokens.slice(start, i));
      start = i + 1; }
  }
  if (start < tokens.length) result.push(tokens.slice(start));
  return result;
}
export function parseMeta(tokens) {
  let i = 0;
  const fail = () => { throw new GateError(
    `line ${tokens[i]?.line ?? tokens.at(-1)?.line ?? 1}: invalid attribute meta`); };
  function parse() {
    const absolute = tokens[i]?.text === '::'; if (absolute) i++;
    if (tokens[i]?.kind !== 'ident') fail();
    let name = (absolute ? '::' : '') + tokens[i++].name;
    while (tokens[i]?.text === '::') { i++; if (tokens[i]?.kind !== 'ident') fail();
      name += '::' + tokens[i++].name; }
    const node = { name, args: null };
    if (tokens[i]?.text === '=') parseMetaValue(node);
    else if (tokens[i]?.text === '(') parseMetaArgs(node);
    return node;
  }
  function parseMetaValue(node) {
    i++; const begin = i;
    while (i < tokens.length && ![',', ')'].includes(tokens[i].text)) {
      if (opens[tokens[i].text]) {
        const end = tokens.findIndex(t => t.index === tokens[i].mate);
        if (end < i) fail();
        i = end + 1;
      } else i++;
    }
    if (begin === i) fail();
    const value = tokens.slice(begin, i);
    node.value = value.length === 1 ? value[0].value ??
      value[0].text : value.map(t => t.text).join('');
    node.valueTokens = value;
  }
  function parseMetaArgs(node) {
    i++; node.args = [];
    while (tokens[i]?.text !== ')') {
      if (!tokens[i]) fail();
      // cfg_attr conditions may contain literal key/value leaves.
      node.args.push(parse());
      if (tokens[i]?.text === ')') break;
      if (tokens[i++]?.text !== ',') fail();
    }
    i++;
  }
  const meta = parse();
  if (i !== tokens.length) fail();
  return meta;
}
export function impliesTest(meta) {
  if (meta.name === 'test' && meta.args === null && meta.value === undefined) return true;
  if (meta.name === 'all') return !!meta.args?.some(impliesTest);
  if (meta.name === 'any') return !!meta.args?.length && meta.args.every(impliesTest);
  return false;
}
function attributeSignals(meta) {
  return [meta,
    ...(meta.name === 'cfg_attr' ? (meta.args ?? []).slice(1).flatMap(attributeSignals) : [])];
}
const testPaths = new Set(['test', 'tokio::test', 'async_std::test']);
const itemKinds = new Set(['fn', 'mod', 'struct', 'enum', 'union', 'trait', 'impl', 'const',
  'static', 'type', 'use', 'extern', 'macro_rules']);
const simplePath = tokens => {
  const absolute = tokens[0]?.text === '::', parts = absolute ? tokens.slice(1) : tokens;
  if (!(parts.length > 0 && parts.length % 2 === 1 && parts.every((t,
    i) => i % 2 === 0 ? t.kind === 'ident' : t.text === '::'))) return null;
  return (absolute ? '::' : '') + parts.filter(t => t.kind === 'ident').map(t =>
    t.name).join('::');
};
function imports(tokens, prefix = []) {
  if (tokens[0]?.text === '::') tokens = tokens.slice(1);
  const open = tokens.findIndex(t => t.text === '{');
  if (open >= 0) {
    const head = tokens.slice(0, open).filter(t => t.kind === 'ident').map(t => t.name);
    return splitTop(tokens.slice(open + 1, -1)).flatMap(part => imports(part, [...prefix,
      ...head]));
  }
  const as = tokens.findIndex(t => t.name === 'as');
  const parts = (as < 0 ? tokens : tokens.slice(0,
    as)).filter(t => t.kind === 'ident' || t.text === '*').map(t => t.name ?? t.text);
  const self = parts.at(-1) === 'self'; if (self) parts.pop();
  const full = [...prefix, ...parts];
  return [{ name: as < 0 ? full.at(-1) : tokens[as + 1]?.name, target: full.join('::'),
    glob: full.at(-1) === '*' }];
}
// Innermost binding of `name` visible at offset `at`, or the glob-ambiguity result.
function scopeBinding(scope, name, at, text) {
  for (let s = scope; s; s = s.parent) {
    if (s.bindings.has(name)) {
      let binding = s.bindings.get(name);
      while (binding && binding.start > at) binding = binding.previous;
      if (binding) return { binding };
    }
    if (s.globs.length) {
      return { result: { path: text, ambiguous: true, reason: 'glob import may bind path' } };
    }
  }
  return {};
}
function firstBinding(scope, name) {
  for (let s = scope; s; s = s.parent) if (s.bindings.has(name)) return s.bindings.get(name);
  return undefined;
}
// Exactly one local path/constant alias hop; import qualification is preserved.
function resolveAliasHop(binding, text, parts) {
  let target = binding.target;
  if (binding.kind !== 'import') {
    const first = target.split('::')[0], other = firstBinding(binding.scope, first);
    if (other === binding || other?.kind !== 'import' && other?.target)
      return { path: text, ambiguous: true, reason: 'cyclic or multi-hop constant/path alias',
        binding };
    if (other?.ambiguous) return { path: text, ambiguous: true, reason: 'ambiguous alias target',
      binding };
    if (other?.value !== undefined) return { path: null, value: other.value, ambiguous: false,
      binding: other };
    if (other?.target) target = other.target + target.slice(first.length);
    else if (other && other.kind !== 'import') return { path: null, shadowed: true,
      ambiguous: false, binding };
  }
  return { path: [target, ...parts.slice(1)].join('::').replace(/^::/, ''), ambiguous: false,
    binding };
}
export function resolvePath(input, scope) {
  const at = typeof input === 'string' ? Infinity : input[0]?.start ?? Infinity;
  const text = typeof input === 'string' ? input : simplePath(input);
  if (!text) return { path: null, ambiguous: true, reason: 'non-simple path' };
  if (text.startsWith('::')) return { path: text.slice(2), ambiguous: false };
  const parts = text.replace(/^::/, '').split('::');
  const found = scopeBinding(scope, parts[0], at, text);
  if (found.result) return found.result;
  const binding = found.binding;
  if (!binding) return { path: parts.join('::'), ambiguous: false };
  if (binding.ambiguous) return { path: binding.target ?? text, ambiguous: true,
    reason: binding.reason ?? 'ambiguous binding', binding };
  if (!binding.target) return { path: null, value: binding.value, ambiguous: false,
    shadowed: binding.value === undefined, binding };
  return resolveAliasHop(binding, text, parts);
}

// scanRust phases share one context: { path, tokens, scopes, items, attributes, diagnostics,
// macroGroups, attrEnds, genericConsts }.
function buildScopes(tokens, text) {
  const root = { id: 0, start: 0, end: text.length, parent: null, bindings: new Map(), imports: [],
    globs: [], attributes: [] };
  const scopes = [root];
  let current = root;
  for (const token of tokens) {
    token.scope = current;
    if (token.text === '{') {
      const scope = { id: scopes.length, start: token.start, end: tokens[token.mate].end,
        open: token.index,
        parent: current, bindings: new Map(), imports: [], globs: [], attributes: [] };
      scopes.push(scope); current = scope;
    } else if (token.text === '}') current = current.parent;
  }
  return { root, scopes };
}
function scanInMacro(ctx, index) {
  return ctx.macroGroups.some(g => index > g.start && index < g.end);
}
// Mask comments by rebuilding only code tokens; retain whitespace/line offsets.
function maskComments(text, tokens) {
  const code = text.split('').map(c => c === '\n' || c === '\r' ? c : ' ');
  for (const token of tokens.filter(t => !['string', 'byteString', 'char',
    'byteChar'].includes(t.kind)))
    for (let i = token.start; i < token.end; i++) code[i] = text[i];
  return code;
}
// Rust attributes may contain token expressions (derive/path/doc); strictly parse cfg/test
// meta.
function attributeMeta(body) {
  if (['cfg', 'cfg_attr'].includes(body[0]?.name)) return parseMeta(body);
  const call = body.findIndex(t => !['ident'].includes(t.kind) && t.text !== '::');
  return { name: simplePath(call < 0 ? body : body.slice(0, call)) ?? body[0]?.name, args: null };
}
function checkAttributeMeta(path, line, meta) {
  if (!meta.name) error(path, line, 'invalid attribute path');
  if (meta.name === 'cfg' && meta.args?.length !== 1) error(path, line,
    'cfg requires one predicate');
  if (meta.name === 'cfg_attr' && (meta.args?.length ?? 0) < 2) error(path, line,
    'cfg_attr requires predicate and attribute');
}
// Records the attribute whose '#' is tokens[i]; returns the index of its closing ']'.
function scanAttribute(ctx, i) {
  const { path, tokens } = ctx;
  const inner = tokens[i + 1]?.text === '!', open = i + (inner ? 2 : 1);
  if (tokens[open]?.text !== '[') error(path, tokens[i].line, 'attribute without brackets');
  const end = tokens[open].mate, meta = attributeMeta(tokens.slice(open + 1, end));
  checkAttributeMeta(path, tokens[i].line, meta);
  const attr = { start: tokens[i].start, end: tokens[end].end, line: tokens[i].line, inner, meta,
    signals: attributeSignals(meta), scope: tokens[i].scope, first: i, last: end };
  ctx.attributes.push(attr); ctx.attrEnds.set(end, attr);
  if (inner) attr.scope.attributes.push(attr);
  return end;
}
function scanAttributes(ctx) {
  for (let i = 0; i < ctx.tokens.length; i++) {
    if (ctx.tokens[i].text !== '#' || scanInMacro(ctx, i)) continue;
    i = scanAttribute(ctx, i);
  }
}
function scanInsideAttribute(ctx, index) {
  return ctx.attributes.some(a => index >= a.first && index <= a.last);
}
function bindName(scope, name, data) {
  if (!name) return;
  const prior = scope.bindings.get(name);
  if (prior && prior.kind !== 'shadow' && data.kind !== 'shadow' &&
    data.kind !== 'local') data = { ...data, ambiguous: true, reason: 'duplicate binding' };
  scope.bindings.set(name, { ...data, name, scope, previous: prior });
}
const NAMED_ITEM_KINDS = new Set(['fn', 'mod', 'struct', 'enum', 'union', 'trait', 'type']);
// True when the item keyword at tokens[i] does not start a standalone item.
function itemKeywordSkipped(ctx, i) {
  const tokens = ctx.tokens, kind = tokens[i].name;
  if (!itemKinds.has(kind) || scanInsideAttribute(ctx, i) || scanInMacro(ctx, i) ||
    tokens[i - 1]?.text === '::' || tokens[i - 1]?.text === '.') return true;
  if (NAMED_ITEM_KINDS.has(kind) && tokens[i + 1]?.kind !== 'ident') return true;
  if (kind === 'const' && (tokens[i + 1]?.kind !== 'ident' ||
    tokens[i - 1]?.text === '*')) return true;
  if (kind === 'const' && ctx.genericConsts.has(i)) return true;
  return itemQualifierSkipped(ctx, i, kind);
}
function itemQualifierSkipped(ctx, i, kind) {
  const tokens = ctx.tokens, token = tokens[i];
  if (ctx.items.some(item => item.kind === 'fn' && i > item.first &&
    token.start < item.bodyStart)) return true;
  // const qualifiers and generic parameters are not standalone items.
  if (kind === 'const' && ['fn', 'unsafe', 'extern'].includes(tokens[i + 1]?.name)) return true;
  if (kind === 'extern' && tokens[i + 1]?.name !== 'crate' &&
    tokens[i + 1]?.text !== '{') return true;
  return kind === 'use' && tokens[i - 1]?.text === '<';
}
const GENERIC_HEADER_KINDS = new Set(['fn', 'struct', 'enum', 'union', 'trait', 'impl', 'type']);
const BRACE_VALUE_KINDS = new Set(['use', 'const', 'static', 'type']);
const SKIPPED_GROUPS = new Set(['(', '[']);
function trackGenericAngle(ctx, t, j, angleDepth) {
  if (t.text === '<') return angleDepth + 1;
  if (t.text === '>') return Math.max(0, angleDepth - 1);
  if (angleDepth && t.name === 'const') ctx.genericConsts.add(j);
  return angleDepth;
}
// Body '{' index (or null) and last token index (null when not delimitable) of the item.
function itemExtent(ctx, i, kind) {
  const tokens = ctx.tokens, genericHeader = GENERIC_HEADER_KINDS.has(kind);
  let body = null, end = null, angleDepth = 0;
  for (let j = i + 1; j < tokens.length; j++) {
    const t = tokens[j];
    if (genericHeader) angleDepth = trackGenericAngle(ctx, t, j, angleDepth);
    if (t.text === ';') { end = j; break; }
    if (t.text === '{') {
      if (angleDepth || BRACE_VALUE_KINDS.has(kind)) { j = t.mate; continue; }
      body = j; end = t.mate; break;
    }
    if (SKIPPED_GROUPS.has(t.text)) j = t.mate;
    else if (t.text === '}') break;
  }
  return { body, end };
}
function attachedAttributes(attrEnds, first) {
  const attached = [];
  while (attrEnds.has(first - 1) && !attrEnds.get(first - 1).inner) {
    const attr = attrEnds.get(first - 1);
    attached.unshift(attr); first = attr.first;
  }
  return { first, attached };
}
const ITEM_QUALIFIERS = new Set(['pub', 'async', 'unsafe', 'default', 'const']);
// Widens the item back over qualifiers, pub(...) and attached outer attributes.
function itemStart(ctx, i) {
  const tokens = ctx.tokens;
  let first = i;
  while (first > 0 && ITEM_QUALIFIERS.has(tokens[first - 1].name)) first--;
  if (tokens[first - 1]?.text === ')' &&
    tokens[tokens[first - 1].mate - 1]?.name === 'pub') first = tokens[first - 1].mate - 1;
  return attachedAttributes(ctx.attrEnds, first);
}
function itemName(tokens, i, kind) {
  if (['impl', 'use', 'extern'].includes(kind)) return null;
  return tokens[i + (tokens[i + 1]?.name === 'mut' || tokens[i + 1]?.text === '!' ? 2 : 1)]?.name;
}
function bindUse(scope, entries) {
  for (const entry of entries) {
    scope.imports.push(entry);
    if (entry.glob) scope.globs.push(entry); else bindName(scope, entry.name,
      { ...entry, kind: 'import' });
  }
}
function bindConst(tokens, i, end, scope, name) {
  const eq = tokens.slice(i, end).findIndex(t => t.text === '=');
  const valueTokens = eq < 0 ? [] : tokens.slice(i + eq + 1, end);
  const literal = valueTokens.length === 1 && ['string', 'byteString',
    'number'].includes(valueTokens[0].kind) ? valueTokens[0] : null;
  bindName(scope, name, { kind: 'const', target: simplePath(valueTokens),
    value: literal?.value ?? literal?.text,
    tokens: valueTokens, ambiguous: !literal && !simplePath(valueTokens),
      reason: 'non-simple constant expression' });
}
function bindItem(tokens, i, end, kind, name) {
  const scope = tokens[i].scope;
  if (kind === 'use') bindUse(scope, imports(tokens.slice(i + 1, end)));
  else if (['const', 'static'].includes(kind)) bindConst(tokens, i, end, scope, name);
  else if (name) bindName(scope, name, { kind: 'shadow', target: null });
}
function bindFnParams(tokens, i, limit, bodyScope) {
  const params = tokens.findIndex((t, k) => k > i && k < limit && t.text === '(');
  if (params < 0) return;
  for (const part of splitTop(tokens.slice(params + 1, tokens[params].mate))) {
    const colon = part.findIndex(t => t.text === ':');
    const names = part.slice(0, colon < 0 ? part.length : colon).filter(t => t.kind ===
      'ident' && !['mut', 'ref'].includes(t.name));
    for (const n of names) bindName(bodyScope, n.name, { kind: 'shadow', target: null });
  }
}
function scanItem(ctx, i) {
  const tokens = ctx.tokens, token = tokens[i], kind = token.name;
  const { body, end } = itemExtent(ctx, i, kind);
  if (end === null) error(ctx.path, token.line, `${kind} item not delimitable`);
  const { first, attached } = itemStart(ctx, i), name = itemName(tokens, i, kind);
  const bodyScope = body === null ? null : ctx.scopes.find(s => s.open === body);
  const item = { kind, name, start: tokens[first].start, end: tokens[end].end, line: token.line,
    attributes: attached, scope: token.scope, bodyScope, bodyStart: body === null ?
      null : tokens[body].start, testOnly: false, first: i, last: end };
  ctx.items.push(item);
  bindItem(tokens, i, end, kind, name);
  if (kind === 'fn' && bodyScope) bindFnParams(tokens, i, body ?? end, bodyScope);
}
function scanItems(ctx) {
  for (let i = 0; i < ctx.tokens.length; i++) if (!itemKeywordSkipped(ctx, i)) scanItem(ctx, i);
}
function letEnd(tokens, i) {
  let end = i + 1;
  for (; end < tokens.length && tokens[end].text !== ';'; end++) {
    if (opens[tokens[end].text]) end = tokens[end].mate;
    else if (tokens[end].text === '}') break;
  }
  return end;
}
function scanLet(tokens, i, t) {
  const end = letEnd(tokens, i);
  const segment = tokens.slice(i + 1, end), eq = segment.findIndex(x => x.text === '=');
  const lhs = segment.slice(0, eq < 0 ? segment.length : eq),
    colon = lhs.findIndex(x => x.text === ':');
  const pattern = (colon < 0 ? lhs : lhs.slice(0, colon)).filter(x => !['mut',
    'ref'].includes(x.name));
  const rhs = eq < 0 ? [] : segment.slice(eq + 1);
  const names = pattern.filter(x => x.kind === 'ident'),
    simple = pattern.length === 1 && names.length === 1;
  const literal = rhs.length === 1 && ['string', 'byteString',
    'number'].includes(rhs[0].kind) ? rhs[0] : null;
  for (const n of names) { n.declaration = true; bindName(t.scope, n.name, { kind: 'local',
    target: simple ? simplePath(rhs) : null,
    value: literal?.value ?? literal?.text, tokens: rhs, ambiguous: !simple,
    reason: !simple ? 'destructuring may shadow relevant alias' : undefined, start: t.start });
      }
}
function markReassigned(t) {
  for (let s = t.scope; s; s = s.parent) {
    const binding = s.bindings.get(t.name);
    if (binding && binding.kind !== 'const' && binding.start !== undefined &&
      t.start > binding.start) {
      binding.ambiguous = true; binding.reason = 'reassigned binding'; return;
    }
  }
}
const ASSIGN_OPS = new Set(['=', '+=', '-=', '*=', '/=']);
// Locals use a conservative scope map: uncertain reassignments/destructuring never erase signals.
function scanLocals(ctx) {
  const tokens = ctx.tokens;
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i]; if (scanInsideAttribute(ctx, i)) continue;
    if (t.name === 'let') scanLet(tokens, i, t);
    else if (t.kind === 'ident' && !t.declaration && ASSIGN_OPS.has(tokens[i + 1]?.text) &&
      tokens[i - 1]?.name !== 'let') markReassigned(t);
  }
}
function cfgTest(attrs) {
  return attrs.some(a => a.meta.name === 'cfg' && impliesTest(a.meta.args[0]));
}
function threadLocalTestRange(ctx, group) {
  const tokens = ctx.tokens;
  if (tokens[group.start - 1]?.text !== '!' ||
      tokens[group.start - 2]?.name !== 'thread_local') return null;
  let first = group.start - 2;
  if (scanInMacro(ctx, first)) return null;
  while (tokens[first - 1]?.text === '::' && tokens[first - 2]?.kind === 'ident')
    first -= 2;
  if (tokens[first - 1]?.text === '::') first--;
  const attrs = attachedAttributes(ctx.attrEnds, first);
  if (!cfgTest(attrs.attached)) return null;
  return { start: tokens[attrs.first].start, end: tokens[group.end].end };
}
function threadLocalTestRanges(ctx) {
  const ranges = [];
  for (const group of ctx.macroGroups) {
    const range = threadLocalTestRange(ctx, group);
    if (range) ranges.push(range);
  }
  return ranges;
}
function itemCfgTest(ctx, item) {
  let cfg = cfgTest(item.attributes);
  for (let s = item.scope; s; s = s.parent) cfg ||= cfgTest(s.attributes);
  for (const container of ctx.items) if (container !== item && container.start <= item.start &&
    container.end >= item.end) cfg ||= cfgTest(container.attributes);
  return cfg;
}
function classifyItems(ctx, fileTest) {
  for (const item of ctx.items) {
    const cfg = itemCfgTest(ctx, item);
    const directTest = item.kind === 'fn' && item.attributes.some(a => a.meta.name !== 'cfg_attr' &&
      testPaths.has(resolvePath(a.meta.name, item.scope).path));
    item.cfgTest = cfg; item.directTest = directTest; item.testOnly = fileTest || cfg || directTest;
    if (item.bodyScope) item.bodyScope.item = item;
  }
}
function isTestOffsetIn(ctx, fileTest, ranges, offset) {
  return fileTest || ctx.scopes.some(s => offset >= s.start && offset < s.end &&
    cfgTest(s.attributes)) ||
    ranges.some(range => offset >= range.start && offset < range.end) ||
    ctx.items.some(item => offset >= item.start && offset < item.end && item.cfgTest ||
      item.directTest && offset >= item.bodyStart && offset < item.end);
}
function pushAmbiguousBindings(ctx) {
  const { path, tokens } = ctx;
  for (const scope of ctx.scopes) for (const binding of scope.bindings.values()) {
    if (binding.ambiguous) ctx.diagnostics.push({ path,
      line: tokens.find(t => t.scope === scope && t.name === binding.name)?.line ?? 1,
      reason: binding.reason, name: binding.name });
  }
}
export function scanRust(path, text) {
  const { tokens, groups } = lexRust(path, text);
  const { root, scopes } = buildScopes(tokens, text);
  const macroGroups = groups.filter(g => tokens[g.start - 1]?.text === '!' ||
    tokens[g.start - 2]?.text === '!');
  const code = maskComments(text, tokens);
  const ctx = { path, tokens, scopes, items: [], attributes: [], diagnostics: [], macroGroups,
    attrEnds: new Map(), genericConsts: new Set() };
  scanAttributes(ctx);
  scanItems(ctx);
  scanLocals(ctx);
  const ranges = threadLocalTestRanges(ctx);
  const fileTest = /(?:(?:^|\/)(?:tests\/|tests\.rs$)|_tests\.rs$)/.test(path);
  classifyItems(ctx, fileTest);
  const isTestOffset = offset => isTestOffsetIn(ctx, fileTest, ranges, offset);
  pushAmbiguousBindings(ctx);
  const sites = compatibilitySites(path, text, tokens);
  const { items, attributes, diagnostics } = ctx;
  return { path, text, tokens, groups, items, scopes, root, attributes, diagnostics, sites,
    code: code.join(''), isTestOffset,
    productionTokens: tokens.filter(t => !isTestOffset(t.start)),
    resolvePath(input, scope = root) { return resolvePath(input, scope); } };
}

// Compatibility ONLY. Pure mask and source.sinks grammar/ordering from frozen inputs:
// round3-dev-extract.cjs SHA256 7A6CCF4529F9420887A3BF8EEC8F7A811929FFB3C5598B8C773BD4720DD511A6
// round3-dev-build.cjs SHA256 54AAA090354596DD1971833047FA84249546D046955759001071CC19583B8951
// Deliberately preserves historical function matching; not the production lexer.
// Blanks a[b..e) except newlines; an end past the text grows `a`, as the frozen mask did.
function maskBlank(a, b, e) {
  for (let j = b; j < e; j++) if (a[j] !== '\n') a[j] = ' ';
}
function maskLineCommentEnd(s, i) {
  const end = s.indexOf('\n', i);
  return end < 0 ? s.length : end;
}
function maskBlockCommentEnd(s, i) {
  i += 2; let n = 1;
  while (i < s.length && n) {
    if (s.startsWith('/*', i)) { n++; i += 2; }
    else if (s.startsWith('*/', i)) { n--; i += 2; }
    else i++;
  }
  return i;
}
function maskRawStringEnd(s, i, raw) {
  const end = '"' + raw[1], e = s.indexOf(end, i + raw[0].length);
  return e < 0 ? s.length : e + end.length;
}
function maskStringEnd(s, i) {
  i++;
  while (i < s.length) {
    if (s.codePointAt(i) === 92) { i += 2; continue; }
    if (s[i++] === '"') break;
  }
  return i;
}
// Index of the closing quote candidate of a char literal whose body starts at k.
function maskCharClose(s, k) {
  if (s.codePointAt(k) !== 92) return k + (s.codePointAt(k) > 65535 ? 2 : 1);
  k++;
  if (s[k] === 'u' && s[k + 1] === '{') {
    const close = s.indexOf('}', k + 2);
    return close < 0 ? k : close + 1;
  }
  return k + (s[k] === 'x' ? 3 : 1);
}
// End of the comment or literal starting at s[i], or -1 when none starts there.
function maskSpanEnd(s, i) {
  if (s.startsWith('//', i)) return maskLineCommentEnd(s, i);
  if (s.startsWith('/*', i)) return maskBlockCommentEnd(s, i);
  const raw = /^(?:br|r)(#*)"/.exec(s.slice(i));
  if (raw) return maskRawStringEnd(s, i, raw);
  if (s[i] === '"') return maskStringEnd(s, i);
  if (s[i] !== "'") return -1;
  const k = maskCharClose(s, i + 1);
  return s[k] === "'" ? k + 1 : -1;
}
function mask(s) {
  const a = s.split('');
  let i = 0;
  while (i < s.length) {
    const end = maskSpanEnd(s, i);
    if (end < 0) { i++; continue; }
    maskBlank(a, i, end); i = end;
  }
  return a.join('');
}
function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text[i] === '\n') starts.push(i + 1);
  return starts;
}
function braceBodyEnd(code, b) {
  let d = 1, e = b + 1;
  while (e < code.length && d) {
    if (code[e] === '{') d++;
    if (code[e] === '}') d--;
    e++;
  }
  return e;
}
function maskedFunctions(code) {
  const functions = [];
  for (const m of code.matchAll(/\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<|\()/g)) {
    const b = code.indexOf('{', m.index), semi = code.indexOf(';', m.index);
    if (b < 0 || semi >= 0 && semi < b) continue;
    functions.push({ name: m[1], start: m.index, end: braceBodyEnd(code, b) });
  }
  return functions;
}
function innermostFunction(functions, index) {
  const f = functions.filter(f => index >= f.start && index < f.end)
    .sort((a, b) => (a.end - a.start) - (b.end - b.start))[0];
  return f ? f.name : '<module>';
}
const COMPAT_SINK_SOURCE = [
  String.raw`\.(?:join|push|with_file_name|with_extension|open|create_new_private_file|write_all|`,
  'write_fmt|sync_all|sync_data|set_len|read_to_end|read_to_string|save|persist|flush|',
  String.raw`write_to)\s*\(|\b(?:Path|PathBuf)::(?:new|from)\s*\(|`,
  String.raw`\b(?:fs|filesystem)::[A-Za-z_]+\s*\(|`,
  String.raw`\b(?:File|OpenOptions|Connection)::[A-Za-z_]+\s*\(|`,
  String.raw`\b(?:write_file_atomic|atomic_write\w*|write_json\w*|rename_with_retry|CreateFileW|`,
  String.raw`MoveFileExW|ReplaceFileW|writeln|write)\s*!?\s*\(`,
].join('');
export function compatibilitySites(p, text, tokens = []) {
  const code = mask(text), lines = text.split('\n'), starts = lineStarts(text);
  const functions = maskedFunctions(code), sinks = [];
  for (const m of code.matchAll(new RegExp(COMPAT_SINK_SOURCE, 'g'))) {
    const item = innermostFunction(functions, m.index);
    const ordinal = sinks.filter(x => x.item === item).length + 1;
    let line = starts.findIndex(x => x > m.index);
    if (line < 0) line = starts.length;
    sinks.push({ path: p, item, ordinal, line, offset: m.index, operation: m[0],
      evidence: lines[line - 1].trim() });
  }
  return sinks.map(site => ({ ...site,
    tokens: tokens.filter(t => t.start >= site.offset && t.start < site.offset +
      site.operation.length) }));
}

// Positive, deliberately small TOML subset; no unconsumed statement can enable inheritance.
function manifestGrammar() {
  const gap = String.raw`(?:[ \t\r\n]|#[^\r\n]*(?![^\r\n]))*`;
  const string = String.raw`(?:"[^"\\\r\n]*"|'[^'\\\r\n]*')`;
  const scalar = `(?:${string}|true|false|[+-]?[0-9][0-9_.]*)`;
  const array = String.raw`\[${gap}(?:${scalar}${gap}(?:,${gap}${scalar}${gap})*,?${gap})?\]`;
  const pair = String.raw`[A-Za-z0-9_-]+[ \t]*=[ \t]*(?:${scalar}|${array})`;
  const inline = String.raw`\{[ \t]*${pair}(?:[ \t]*,[ \t]*${pair})*[ \t]*\}`;
  const ending = String.raw`[ \t]*(?:#[^\r\n]*)?(?:\r?\n|$)`;
  const assignment = new RegExp(
    String.raw`^([A-Za-z0-9_-]+)[ \t]*=[ \t]*(${scalar}|${array}|${inline})${ending}`);
  return { gap, string, ending, assignment };
}
function manifestSectionAllowed(section, external) {
  const simple = /^(?:package|lib|dependencies|dev-dependencies|build-dependencies|features)$/;
  const lint = /^(?:lints|workspace\.lints\.clippy)$/;
  const target = /^target\.'cfg\([^'\\\r\n]*\)'\.(?:dev-|build-)?dependencies$/;
  return simple.test(section) || lint.test(section) || target.test(section) ||
    external && section === 'workspace';
}
// Consumes a table header: its length, -1 when unsupported, 0 when cargo starts with none.
function manifestHeader(cargo, state) {
  const arrayHeader = new RegExp(
    String.raw`^\[\[[ \t]*(bin|test|bench|example)[ \t]*\]\]` + state.grammar.ending).exec(cargo);
  if (arrayHeader) {
    if (!state.external) return -1;
    state.section = arrayHeader[1];
    state.targetTable = true;
    return arrayHeader[0].length;
  }
  const header = /^\[([^[\]\r\n]+)\][ \t]*(?:#[^\r\n]*)?(?:\r?\n|$)/.exec(cargo);
  if (!header) return 0;
  state.section = header[1].trim();
  state.targetTable = false;
  if (!manifestSectionAllowed(state.section, state.external)) return -1;
  return header[0].length;
}
function workspaceEntryAllowed(state, key, value) {
  if (!['members', 'default-members', 'exclude', 'resolver'].includes(key)) return false;
  if (key === 'resolver') return true;
  const { path, tree } = state;
  for (const m of value.matchAll(new RegExp(state.grammar.string, 'g'))) {
    const member = m[0].slice(1, -1);
    const base = path.includes('/') ? path.slice(0, path.lastIndexOf('/') + 1) : '';
    if (key !== 'exclude' && !tree.has(base + member + '/Cargo.toml')) return false;
  }
  return true;
}
function manifestEntryAllowed(state, key, value) {
  const { section, external } = state;
  if (!section) return false;
  if (section === 'workspace' && !workspaceEntryAllowed(state, key, value)) return false;
  if (section === 'lib' && !['name', 'crate-type'].includes(key) &&
      !(external && key === 'doctest' && value === 'false')) return false;
  if (state.targetTable && (key === 'path'
    ? !/^(?:"[A-Za-z0-9_-]+\.rs"|'[A-Za-z0-9_-]+\.rs')$/.test(value)
    : key !== 'name')) return false;
  return !(section === 'package' && key === 'build' && value !== 'false');
}
function ordinaryManifest(cargo, external, path, tree) {
  const grammar = manifestGrammar();
  const state = { grammar, external, path, tree, section: undefined, targetTable: false };
  while (cargo.length) {
    cargo = cargo.replace(new RegExp('^' + grammar.gap), '');
    if (!cargo.length) break;
    const header = manifestHeader(cargo, state);
    if (header < 0) return false;
    if (header) { cargo = cargo.slice(header); continue; }
    const entry = grammar.assignment.exec(cargo);
    if (!entry) return false;
    const [, key, value] = entry;
    if (!manifestEntryAllowed(state, key, value)) return false;
    cargo = cargo.slice(entry[0].length);
  }
  return true;
}

// Snapshot-local, no filesystem or process state; unsupported syntax keeps production.
function manifestsUnsupported(tree, readBlob) {
  let unsupported = [...tree.keys()].some(p => p.startsWith('src-tauri/src/bin/'));
  for (const [path, manifest] of tree) {
    if (!/(^|\/)Cargo\.toml$/.test(path)) continue;
    if (path.startsWith('src-tauri/') && path !== 'src-tauri/Cargo.toml') {
      unsupported = true; continue;
    }
    unsupported ||= !ordinaryManifest(readBlob(manifest),
      path !== 'src-tauri/Cargo.toml', path, tree);
  }
  return unsupported;
}
// Same per-path cut as the caller's `continue` for paths outside src-tauri/src/.
function rustFileUnsupported(path, scan, mods) {
  let unsupported = false;
  if (scan.attributes.some(a => a.signals.some(m => m.name === 'path')) ||
      scan.tokens.some(t => t.name === 'include' && scan.tokens[t.index + 1]?.text === '!'))
    unsupported = true;
  if (scan.tokens.some(t => t.name === 'mod' && !mods.some(i => i.first === t.index)))
    unsupported = true;
  if (!path.startsWith('src-tauri/src/')) {
    if (path.slice(0, path.lastIndexOf('/')) === 'src-tauri' &&
        scan.tokens.some(t => t.name === 'mod')) unsupported = true;
  } else if (mods.some(i => i.bodyScope || i.scope !== scan.root)) unsupported = true;
  return unsupported;
}
function addModTargets(incoming, tree, path, mods) {
  let ambiguous = false;
  const crateRoot = ['src-tauri/src/lib.rs', 'src-tauri/src/main.rs'].includes(path);
  const base = crateRoot || path.endsWith('/mod.rs')
    ? path.slice(0, path.lastIndexOf('/')) : path.slice(0, -3);
  for (const item of mods) {
    const targets = [base + '/' + item.name + '.rs', base + '/' + item.name + '/mod.rs']
      .filter(p => tree.has(p));
    if (targets.length > 1) ambiguous = true;
    for (const target of targets) {
      if (!incoming.has(target)) incoming.set(target, []);
      incoming.get(target).push({ parent: path, test: item.cfgTest });
    }
  }
  return ambiguous;
}
function testOnlyFiles(incoming) {
  const testOnly = new Set();
  let changed;
  do {
    changed = false;
    for (const [path, declarations] of incoming) {
      if (testOnly.has(path) ||
          ['src-tauri/src/lib.rs', 'src-tauri/src/main.rs'].includes(path)) continue;
      if (declarations.every(d => d.test || testOnly.has(d.parent))) {
        testOnly.add(path); changed = true;
      }
    }
  } while (changed);
  return testOnly;
}
export function createRustScanner(tree, readBlob) {
  const scans = new Map(), incoming = new Map();
  let unsupported = manifestsUnsupported(tree, readBlob);
  for (const [path, entry] of tree) {
    if (!path.endsWith('.rs')) continue;
    const scan = scanRust(path, readBlob(entry));
    scans.set(path, scan);
    const mods = scan.items.filter(i => i.kind === 'mod');
    if (rustFileUnsupported(path, scan, mods)) unsupported = true;
    if (!path.startsWith('src-tauri/src/')) continue;
    if (addModTargets(incoming, tree, path, mods)) unsupported = true;
  }
  const testOnly = unsupported ? new Set() : testOnlyFiles(incoming);
  return path => {
    const scan = scans.get(path);
    if (!scan) return scanRust(path, readBlob(tree.get(path)));
    if (!testOnly.has(path)) return scan;
    for (const item of scan.items) item.testOnly = true;
    scan.isTestOffset = () => true;
    scan.productionTokens = [];
    return scan;
  };
}
