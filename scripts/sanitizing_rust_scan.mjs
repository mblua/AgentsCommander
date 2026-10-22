// Finite lexical Rust analysis, not type resolution or general macro expansion.
import { GateError } from './sanitizing_baseline.mjs';
const identStart = /[\p{ID_Start}_]/u, identPart = /[\p{ID_Continue}_]/u;
const opens = { '(': ')', '[': ']', '{': '}' };
const closes = new Set(Object.values(opens));
function error(path, line, reason) { throw new GateError(`${path}:${line}: ${reason}; review Rust syntax`); }

// Rust escapes, rather than JSON/JavaScript string escapes.
export function decodeRustString(raw, byte = false) {
  const prefix = /^(?:b)?r(#+|)"/.exec(raw);
  if (prefix) return raw.slice(prefix[0].length, -prefix[1].length - 1);
  const source = raw.slice(byte ? 2 : 1, -1);
  let value = '', opaque = false;
  for (let i = 0; i < source.length; i++) {
    const c = source[i];
    if (c.charCodeAt(0) !== 92) { value += c; continue; }
    const e = source[++i], simple = { n: '\n', r: '\r', t: '\t', '0': '\0', [String.fromCharCode(92)]: String.fromCharCode(92), '"': '"', "'": "'" };
    if (Object.hasOwn(simple, e)) value += simple[e];
    else if (e === 'x') {
      const hex = source.slice(i + 1, i + 3);
      if (!/^[a-f\d]{2}$/i.test(hex)) throw new GateError('invalid Rust hex escape');
      const code = parseInt(hex, 16);
      if (!byte && code > 127) throw new GateError('non-ASCII Rust string hex escape');
      if (byte && code > 127) opaque = true; // Still validate all remaining escapes.
      value += String.fromCharCode(code); i += 2;
    } else if (e === 'u' && source[i + 1] === '{' && !byte) {
      const end = source.indexOf('}', i + 2), hex = source.slice(i + 2, end).replaceAll('_', '');
      if (end < 0 || !/^[a-f\d]{1,6}$/i.test(hex)) throw new GateError('invalid Rust Unicode escape');
      const code = parseInt(hex, 16);
      if (code > 0x10ffff || code >= 0xd800 && code <= 0xdfff) throw new GateError('invalid Rust Unicode scalar');
      value += String.fromCodePoint(code); i = end;
    } else if (e === '\n' || e === '\r' && source[i + 1] === '\n') {
      if (e === '\r') i++;
      while (/[\t\n\r ]/.test(source[i + 1] ?? '') && i + 1 < source.length) i++;
    } else throw new GateError('invalid Rust escape');
  }
  return opaque || byte && /[^\x00-\x7f]/.test(value) ? null : value;
}
export function lexRust(path, text) {
  const tokens = [], groups = [], stack = []; let i = 0, line = 1;
  function add(kind, start, end, tokenLine, extra = {}) {
    const token = { kind, text: text.slice(start, end), start, end, line: tokenLine, index: tokens.length, ...extra };
    tokens.push(token); return token;
  }
  function advance(end) { for (; i < end; i++) if (text[i] === '\n') line++; }
  while (i < text.length) {
    const start = i, tokenLine = line, c = String.fromCodePoint(text.codePointAt(i));
    if (/\s/.test(c)) { advance(i + 1); continue; }
    if (text.startsWith('//', i)) { let end = text.indexOf('\n', i); if (end < 0) end = text.length; advance(end); continue; }
    if (text.startsWith('/*', i)) {
      i += 2; let depth = 1;
      while (i < text.length && depth) {
        if (text.startsWith('/*', i)) { depth++; advance(i + 2); }
        else if (text.startsWith('*/', i)) { depth--; advance(i + 2); }
        else advance(i + 1);
      }
      if (depth) error(path, tokenLine, 'unclosed block comment'); continue;
    }
    const raw = /^(?:br|r)(#*)"/.exec(text.slice(i));
    if (raw) {
      const suffix = '"' + raw[1], end = text.indexOf(suffix, i + raw[0].length);
      if (end < 0) error(path, line, 'unclosed raw string');
      advance(end + suffix.length);
      const byte = raw[0].startsWith('b');
      const value = decodeRustString(text.slice(start, i), byte);
      add(byte ? 'byteString' : 'string', start, i, tokenLine, { value: byte && /[^\x00-\x7f]/.test(value) ? null : value, opaque: byte && /[^\x00-\x7f]/.test(value) }); continue;
    }
    if (c === '"' || c === 'b' && text[i + 1] === '"') {
      const byte = c === 'b'; advance(i + (byte ? 2 : 1)); let closed = false;
      while (i < text.length) {
        if (text.charCodeAt(i) === 92) { advance(Math.min(i + 2, text.length)); continue; }
        if (text[i] === '"') { advance(i + 1); closed = true; break; }
        advance(i + 1);
      }
      if (!closed) error(path, tokenLine, 'unclosed string');
      let value; try { value = decodeRustString(text.slice(start, i), byte); } catch (e) { error(path, tokenLine, e.message); }
      add(byte ? 'byteString' : 'string', start, i, tokenLine, { value, opaque: value === null }); continue;
    }
    if (c === "'" || c === 'b' && text[i + 1] === "'") {
      const byte = c === 'b', quote = i + (byte ? 1 : 0); let end = quote + 1;
      if (text.charCodeAt(end) === 92) {
        end++;
        if (text[end] === 'u' && text[end + 1] === '{') { const close = text.indexOf('}', end + 2); end = close < 0 ? text.length : close + 1; }
        else end += text[end] === 'x' ? 3 : 1;
      } else end += (text.codePointAt(end) ?? 0) > 0xffff ? 2 : 1;
      if (text[end] === "'") {
        const content = text.slice(quote + 1, end);
        try { decodeRustString((byte ? 'b"' : '"') + content + '"', byte); }
        catch (e) { error(path, tokenLine, e.message); }
        advance(end + 1); add(byte ? 'byteChar' : 'char', start, i, tokenLine); continue;
      }
      const life = /^'[\p{ID_Start}_][\p{ID_Continue}_]*/u.exec(text.slice(i));
      if (!byte && life) { advance(i + life[0].length); add('lifetime', start, i, tokenLine); continue; }
      error(path, tokenLine, 'invalid or unclosed character');
    }
    const rawIdent = /^r#([\p{ID_Start}_][\p{ID_Continue}_]*)/u.exec(text.slice(i));
    if (rawIdent || identStart.test(c)) {
      if (rawIdent) advance(i + rawIdent[0].length);
      else { advance(i + c.length); while (i < text.length && identPart.test(String.fromCodePoint(text.codePointAt(i)))) advance(i + (text.codePointAt(i) > 0xffff ? 2 : 1)); }
      add('ident', start, i, tokenLine, { name: text.slice(start, i).replace(/^r#/, '') }); continue;
    }
    if (/\d/.test(c)) {
      // A tuple index cannot absorb the following projection, call or range.
      const numeric = tokens.at(-1)?.text === '.' ? /^\d[\d_]*/ :
        /^(?:0(?:x[\da-fA-F_]+|o[0-7_]+|b[01_]+)|\d[\d_]*(?:\.(?!\.|[_\p{ID_Start}])[\d_]*)?(?:[eE][+-]?[\d_]+)?)(?:[_\p{ID_Start}][\p{ID_Continue}_]*)?/u;
      advance(i + numeric.exec(text.slice(i))[0].length);
      add('number', start, i, tokenLine); continue;
    }
    const punctuation = ['::', '=>', '->', '==', '!=', '+=', '-=', '*=', '/=', '..=', '..', '&&', '||'].find(p => text.startsWith(p, i)) ?? c;
    advance(i + punctuation.length); const token = add('punct', start, i, tokenLine);
    if (opens[c]) stack.push(token.index);
    else if (closes.has(c)) {
      const open = stack.pop();
      if (open === undefined || opens[tokens[open].text] !== c) error(path, tokenLine, 'mismatched delimiter');
      tokens[open].mate = token.index; token.mate = open; groups.push({ start: open, end: token.index, kind: tokens[open].text });
    }
  }
  if (stack.length) error(path, tokens[stack.at(-1)].line, 'unclosed delimiter');
  return { tokens, groups };
}
function splitTop(tokens, separator = ',') {
  const result = []; let start = 0;
  for (let i = 0; i < tokens.length; i++) {
    if (opens[tokens[i].text]) { const mate = tokens.findIndex(t => t.index === tokens[i].mate); if (mate >= i) i = mate; }
    else if (tokens[i].text === separator) { if (i > start) result.push(tokens.slice(start, i)); start = i + 1; }
  }
  if (start < tokens.length) result.push(tokens.slice(start)); return result;
}
export function parseMeta(tokens) {
  let i = 0;
  const fail = () => { throw new GateError(`line ${tokens[i]?.line ?? tokens.at(-1)?.line ?? 1}: invalid attribute meta`); };
  function parse() {
    const absolute = tokens[i]?.text === '::'; if (absolute) i++;
    if (tokens[i]?.kind !== 'ident') fail();
    let name = (absolute ? '::' : '') + tokens[i++].name;
    while (tokens[i]?.text === '::') { i++; if (tokens[i]?.kind !== 'ident') fail(); name += '::' + tokens[i++].name; }
    const node = { name, args: null };
    if (tokens[i]?.text === '=') {
      i++; const begin = i;
      while (i < tokens.length && ![',', ')'].includes(tokens[i].text)) {
        if (opens[tokens[i].text]) {
          const end = tokens.findIndex(t => t.index === tokens[i].mate);
          if (end < i) fail(); i = end + 1;
        } else i++;
      }
      if (begin === i) fail();
      const value = tokens.slice(begin, i);
      node.value = value.length === 1 ? value[0].value ?? value[0].text : value.map(t => t.text).join('');
      node.valueTokens = value;
    } else if (tokens[i]?.text === '(') {
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
    return node;
  }
  const meta = parse(); if (i !== tokens.length) fail(); return meta;
}
export function impliesTest(meta) {
  if (meta.name === 'test' && meta.args === null && meta.value === undefined) return true;
  if (meta.name === 'all') return !!meta.args?.some(impliesTest);
  if (meta.name === 'any') return !!meta.args?.length && meta.args.every(impliesTest);
  return false;
}
function attributeSignals(meta) {
  return [meta, ...(meta.name === 'cfg_attr' ? (meta.args ?? []).slice(1).flatMap(attributeSignals) : [])];
}
const testPaths = new Set(['test', 'tokio::test', 'async_std::test']);
const itemKinds = new Set(['fn', 'mod', 'struct', 'enum', 'union', 'trait', 'impl', 'const', 'static', 'type', 'use', 'extern', 'macro_rules']);
const simplePath = tokens => {
  const absolute = tokens[0]?.text === '::', parts = absolute ? tokens.slice(1) : tokens;
  return parts.length > 0 && parts.length % 2 === 1 && parts.every((t, i) => i % 2 === 0 ? t.kind === 'ident' : t.text === '::')
    ? (absolute ? '::' : '') + parts.filter(t => t.kind === 'ident').map(t => t.name).join('::') : null;
};
function imports(tokens, prefix = []) {
  if (tokens[0]?.text === '::') tokens = tokens.slice(1);
  const open = tokens.findIndex(t => t.text === '{');
  if (open >= 0) {
    const head = tokens.slice(0, open).filter(t => t.kind === 'ident').map(t => t.name);
    return splitTop(tokens.slice(open + 1, -1)).flatMap(part => imports(part, [...prefix, ...head]));
  }
  const as = tokens.findIndex(t => t.name === 'as');
  const parts = (as < 0 ? tokens : tokens.slice(0, as)).filter(t => t.kind === 'ident' || t.text === '*').map(t => t.name ?? t.text);
  const self = parts.at(-1) === 'self'; if (self) parts.pop();
  const full = [...prefix, ...parts];
  return [{ name: as < 0 ? full.at(-1) : tokens[as + 1]?.name, target: full.join('::'), glob: full.at(-1) === '*' }];
}
export function resolvePath(input, scope) {
  const at = typeof input === 'string' ? Infinity : input[0]?.start ?? Infinity;
  const text = typeof input === 'string' ? input : simplePath(input);
  if (!text) return { path: null, ambiguous: true, reason: 'non-simple path' };
  if (text.startsWith('::')) return { path: text.slice(2), ambiguous: false };
  const parts = text.replace(/^::/, '').split('::'); let binding;
  for (let s = scope; s; s = s.parent) {
    if (s.bindings.has(parts[0])) {
      binding = s.bindings.get(parts[0]);
      while (binding && binding.start > at) binding = binding.previous;
      if (binding) break;
    }
    if (s.globs.length) return { path: text, ambiguous: true, reason: 'glob import may bind path' };
  }
  if (!binding) return { path: parts.join('::'), ambiguous: false };
  if (binding.ambiguous) return { path: binding.target ?? text, ambiguous: true, reason: binding.reason ?? 'ambiguous binding', binding };
  if (!binding.target) return { path: null, value: binding.value, ambiguous: false, shadowed: binding.value === undefined, binding };
  // Exactly one local path/constant alias hop; import qualification is preserved.
  let target = binding.target;
  if (binding.kind !== 'import') {
    const first = target.split('::')[0]; let other;
    for (let s = binding.scope; s; s = s.parent) { if (s.bindings.has(first)) { other = s.bindings.get(first); break; } }
    if (other === binding || other?.kind !== 'import' && other?.target)
      return { path: text, ambiguous: true, reason: 'cyclic or multi-hop constant/path alias', binding };
    if (other?.ambiguous) return { path: text, ambiguous: true, reason: 'ambiguous alias target', binding };
    if (other?.value !== undefined) return { path: null, value: other.value, ambiguous: false, binding: other };
    if (other?.target) target = other.target + target.slice(first.length);
    else if (other && other.kind !== 'import') return { path: null, shadowed: true, ambiguous: false, binding };
  }
  return { path: [target, ...parts.slice(1)].join('::').replace(/^::/, ''), ambiguous: false, binding };
}

export function scanRust(path, text) {
  const { tokens, groups } = lexRust(path, text);
  const root = { id: 0, start: 0, end: text.length, parent: null, bindings: new Map(), imports: [], globs: [], attributes: [] };
  const scopes = [root], items = [], attributes = [], diagnostics = [];
  let current = root;
  for (const token of tokens) {
    token.scope = current;
    if (token.text === '{') {
      const scope = { id: scopes.length, start: token.start, end: tokens[token.mate].end, open: token.index,
        parent: current, bindings: new Map(), imports: [], globs: [], attributes: [] };
      scopes.push(scope); current = scope;
    } else if (token.text === '}') current = current.parent;
  }
  const macroGroups = groups.filter(g => tokens[g.start - 1]?.text === '!' || tokens[g.start - 2]?.text === '!');
  const inMacro = index => macroGroups.some(g => index > g.start && index < g.end);
  // Mask comments by rebuilding only code tokens; retain whitespace/line offsets.
  const code = text.split('').map(c => c === '\n' || c === '\r' ? c : ' ');
  for (const token of tokens.filter(t => !['string', 'byteString', 'char', 'byteChar'].includes(t.kind)))
    for (let i = token.start; i < token.end; i++) code[i] = text[i];
  const attrEnds = new Map();
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i].text !== '#' || inMacro(i)) continue;
    const inner = tokens[i + 1]?.text === '!', open = i + (inner ? 2 : 1);
    if (tokens[open]?.text !== '[') error(path, tokens[i].line, 'attribute without brackets');
    const end = tokens[open].mate, body = tokens.slice(open + 1, end);
    // Rust attributes may contain token expressions (derive/path/doc); strictly parse cfg/test meta.
    const relevant = ['cfg', 'cfg_attr'].includes(body[0]?.name);
    let meta;
    if (relevant) meta = parseMeta(body);
    else { const call = body.findIndex(t => !['ident'].includes(t.kind) && t.text !== '::'); meta = { name: simplePath(call < 0 ? body : body.slice(0, call)) ?? body[0]?.name, args: null }; }
    if (!meta.name) error(path, tokens[i].line, 'invalid attribute path');
    if (meta.name === 'cfg' && meta.args?.length !== 1) error(path, tokens[i].line, 'cfg requires one predicate');
    if (meta.name === 'cfg_attr' && (meta.args?.length ?? 0) < 2) error(path, tokens[i].line, 'cfg_attr requires predicate and attribute');
    const attr = { start: tokens[i].start, end: tokens[end].end, line: tokens[i].line, inner, meta,
      signals: attributeSignals(meta), scope: tokens[i].scope, first: i, last: end };
    attributes.push(attr); attrEnds.set(end, attr); if (inner) attr.scope.attributes.push(attr);
    i = end;
  }
  const insideAttribute = index => attributes.some(a => index >= a.first && index <= a.last);
  const bind = (scope, name, data) => {
    if (!name) return;
    const prior = scope.bindings.get(name);
    if (prior && prior.kind !== 'shadow' && data.kind !== 'shadow' && data.kind !== 'local') data = { ...data, ambiguous: true, reason: 'duplicate binding' };
    scope.bindings.set(name, { ...data, name, scope, previous: prior });
  };
  const genericConsts = new Set();
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i], kind = token.name;
    if (!itemKinds.has(kind) || insideAttribute(i) || inMacro(i) || tokens[i - 1]?.text === '::' || tokens[i - 1]?.text === '.') continue;
    if (['fn', 'mod', 'struct', 'enum', 'union', 'trait', 'type'].includes(kind) && tokens[i + 1]?.kind !== 'ident') continue;
    if (kind === 'const' && (tokens[i + 1]?.kind !== 'ident' || tokens[i - 1]?.text === '*')) continue;
    if (kind === 'const' && genericConsts.has(i)) continue;
    if (items.some(item => item.kind === 'fn' && i > item.first && token.start < item.bodyStart)) continue;
    // const qualifiers and generic parameters are not standalone items.
    if (kind === 'const' && ['fn', 'unsafe', 'extern'].includes(tokens[i + 1]?.name)) continue;
    if (kind === 'extern' && tokens[i + 1]?.name !== 'crate' && tokens[i + 1]?.text !== '{') continue;
    if (kind === 'use' && tokens[i - 1]?.text === '<') continue;
    let body = null, end = null, j = i + 1, angleDepth = 0;
    const genericHeader = ['fn', 'struct', 'enum', 'union', 'trait', 'impl', 'type'].includes(kind);
    for (; j < tokens.length; j++) {
      const t = tokens[j];
      if (genericHeader) {
        if (t.text === '<') angleDepth++;
        else if (t.text === '>') angleDepth = Math.max(0, angleDepth - 1);
        else if (angleDepth && t.name === 'const') genericConsts.add(j);
      }
      if (t.text === ';') { end = j; break; }
      if (t.text === '{') {
        if (angleDepth) { j = t.mate; continue; }
        if (kind === 'use' || ['const', 'static', 'type'].includes(kind)) { j = t.mate; continue; }
        body = j; end = t.mate; break;
      }
      if (t.text === '(' || t.text === '[') j = t.mate;
      else if (t.text === '}') break;
    }
    if (end === null) error(path, token.line, `${kind} item not delimitable`);
    let first = i;
    while (first > 0 && ['pub', 'async', 'unsafe', 'default', 'const'].includes(tokens[first - 1].name)) first--;
    if (tokens[first - 1]?.text === ')' && tokens[tokens[first - 1].mate - 1]?.name === 'pub') first = tokens[first - 1].mate - 1;
    const attached = [];
    while (attrEnds.has(first - 1) && !attrEnds.get(first - 1).inner) { const a = attrEnds.get(first - 1); attached.unshift(a); first = a.first; }
    const name = ['impl', 'use', 'extern'].includes(kind) ? null : tokens[i + (tokens[i + 1]?.name === 'mut' || tokens[i + 1]?.text === '!' ? 2 : 1)]?.name;
    const bodyScope = body === null ? null : scopes.find(s => s.open === body);
    const item = { kind, name, start: tokens[first].start, end: tokens[end].end, line: token.line,
      attributes: attached, scope: token.scope, bodyScope, bodyStart: body === null ? null : tokens[body].start, testOnly: false, first: i, last: end };
    items.push(item);
    if (kind === 'use') {
      for (const entry of imports(tokens.slice(i + 1, end))) {
        token.scope.imports.push(entry);
        if (entry.glob) token.scope.globs.push(entry); else bind(token.scope, entry.name, { ...entry, kind: 'import' });
      }
    } else if (['const', 'static'].includes(kind)) {
      const eq = tokens.slice(i, end).findIndex(t => t.text === '=');
      const valueTokens = eq < 0 ? [] : tokens.slice(i + eq + 1, end);
      const literal = valueTokens.length === 1 && ['string', 'byteString', 'number'].includes(valueTokens[0].kind) ? valueTokens[0] : null;
      bind(token.scope, name, { kind: 'const', target: simplePath(valueTokens), value: literal?.value ?? literal?.text,
        tokens: valueTokens, ambiguous: !literal && !simplePath(valueTokens), reason: 'non-simple constant expression' });
    } else if (name) bind(token.scope, name, { kind: 'shadow', target: null });
    if (kind === 'fn' && bodyScope) {
      const params = tokens.findIndex((t, k) => k > i && k < (body ?? end) && t.text === '(');
      if (params >= 0) for (const part of splitTop(tokens.slice(params + 1, tokens[params].mate))) {
        const colon = part.findIndex(t => t.text === ':');
        const names = part.slice(0, colon < 0 ? part.length : colon).filter(t => t.kind === 'ident' && !['mut', 'ref'].includes(t.name));
        for (const n of names) bind(bodyScope, n.name, { kind: 'shadow', target: null });
      }
    }
  }
  // Locals use a conservative scope map: uncertain reassignments/destructuring never erase signals.
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i]; if (insideAttribute(i)) continue;
    if (t.name === 'let') {
      let end = i + 1;
      for (; end < tokens.length && tokens[end].text !== ';'; end++) {
        if (opens[tokens[end].text]) end = tokens[end].mate;
        else if (tokens[end].text === '}') break;
      }
      const segment = tokens.slice(i + 1, end), eq = segment.findIndex(x => x.text === '=');
      const lhs = segment.slice(0, eq < 0 ? segment.length : eq), colon = lhs.findIndex(x => x.text === ':');
      const pattern = (colon < 0 ? lhs : lhs.slice(0, colon)).filter(x => !['mut', 'ref'].includes(x.name));
      const rhs = eq < 0 ? [] : segment.slice(eq + 1);
      const names = pattern.filter(x => x.kind === 'ident'), simple = pattern.length === 1 && names.length === 1;
      const literal = rhs.length === 1 && ['string', 'byteString', 'number'].includes(rhs[0].kind) ? rhs[0] : null;
      for (const n of names) { n.declaration = true; bind(t.scope, n.name, { kind: 'local', target: simple ? simplePath(rhs) : null,
        value: literal?.value ?? literal?.text, tokens: rhs, ambiguous: !simple,
        reason: !simple ? 'destructuring may shadow relevant alias' : undefined, start: t.start }); }
    } else if (t.kind === 'ident' && !t.declaration && ['=', '+=', '-=', '*=', '/='].includes(tokens[i + 1]?.text) && tokens[i - 1]?.name !== 'let') {
      for (let s = t.scope; s; s = s.parent) {
        const binding = s.bindings.get(t.name);
        if (binding && binding.kind !== 'const' && binding.start !== undefined && t.start > binding.start) {
          binding.ambiguous = true; binding.reason = 'reassigned binding'; break;
        }
      }
    }
  }
  const cfgTest = attrs => attrs.some(a => a.meta.name === 'cfg' && impliesTest(a.meta.args[0]));
  const fileTest = /(?:(?:^|\/)(?:tests\/|tests\.rs$)|_tests\.rs$)/.test(path);
  for (const item of items) {
    let cfg = cfgTest(item.attributes);
    for (let s = item.scope; s; s = s.parent) cfg ||= cfgTest(s.attributes);
    for (const container of items) if (container !== item && container.start <= item.start && container.end >= item.end) cfg ||= cfgTest(container.attributes);
    const directTest = item.kind === 'fn' && item.attributes.some(a => a.meta.name !== 'cfg_attr' && testPaths.has(resolvePath(a.meta.name, item.scope).path));
    item.cfgTest = cfg; item.directTest = directTest; item.testOnly = fileTest || cfg || directTest;
    if (item.bodyScope) item.bodyScope.item = item;
  }
  const isTestOffset = offset => fileTest || scopes.some(s => offset >= s.start && offset < s.end && cfgTest(s.attributes)) ||
    items.some(item => offset >= item.start && offset < item.end && item.cfgTest ||
      item.directTest && offset >= item.bodyStart && offset < item.end);
  for (const scope of scopes) for (const binding of scope.bindings.values()) if (binding.ambiguous)
    diagnostics.push({ path, line: tokens.find(t => t.scope === scope && t.name === binding.name)?.line ?? 1,
      reason: binding.reason, name: binding.name });
  const sites = compatibilitySites(path, text, tokens);
  return { path, text, tokens, groups, items, scopes, root, attributes, diagnostics, sites,
    code: code.join(''), isTestOffset,
    productionTokens: tokens.filter(t => !isTestOffset(t.start)),
    resolvePath(input, scope = root) { return resolvePath(input, scope); } };
}

// Compatibility ONLY. Pure mask and source.sinks grammar/ordering from frozen inputs:
// round3-dev-extract.cjs SHA256 7A6CCF4529F9420887A3BF8EEC8F7A811929FFB3C5598B8C773BD4720DD511A6
// round3-dev-build.cjs SHA256 54AAA090354596DD1971833047FA84249546D046955759001071CC19583B8951
// Deliberately preserves historical function matching; not the production lexer.
function mask(s){let a=s.split(''),i=0;const blank=(b,e)=>{for(let j=b;j<e;j++)if(a[j]!='\n')a[j]=' ';};while(i<s.length){let b=i;if(s.startsWith('//',i)){i=s.indexOf('\n',i);if(i<0)i=s.length;blank(b,i);}else if(s.startsWith('/*',i)){i+=2;let n=1;while(i<s.length&&n){if(s.startsWith('/*',i)){n++;i+=2;}else if(s.startsWith('*/',i)){n--;i+=2;}else i++;}blank(b,i);}else{const raw=/^(?:br|r)(#*)"/.exec(s.slice(i));if(raw){const end='"'+raw[1];let e=s.indexOf(end,i+raw[0].length);i=e<0?s.length:e+end.length;blank(b,i);}else if(s[i]=='"'){i++;while(i<s.length){if(s.charCodeAt(i)==92){i+=2;continue;}if(s[i++]=='"')break;}blank(b,i);}else if(s[i]=="'"){let k=i+1;if(s.charCodeAt(k)==92){k++;if(s[k]=='u'&&s[k+1]=='{'){const close=s.indexOf('}',k+2);k=close<0?k:close+1;}else if(s[k]=='x')k+=3;else k++;}else k+=s.codePointAt(k)>65535?2:1;if(s[k]=="'"){i=k+1;blank(b,i);}else i++;}else i++;}}return a.join('');}
export function compatibilitySites(p,text,tokens=[]) {
const code=mask(text),lines=text.split('\n'),starts=[0];for(let i=0;i<text.length;i++)if(text[i]=='\n')starts.push(i+1);const functions=[];for(const m of code.matchAll(/\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<|\()/g)){let b=code.indexOf('{',m.index),semi=code.indexOf(';',m.index);if(b<0||semi>=0&&semi<b)continue;let d=1,e=b+1;while(e<code.length&&d){if(code[e]=='{')d++;if(code[e]=='}')d--;e++;}functions.push({name:m[1],start:m.index,end:e});}const sinks=[];const re=/\.(?:join|push|with_file_name|with_extension|open|create_new_private_file|write_all|write_fmt|sync_all|sync_data|set_len|read_to_end|read_to_string|save|persist|flush|write_to)\s*\(|\b(?:Path|PathBuf)::(?:new|from)\s*\(|\b(?:fs|filesystem)::[A-Za-z_]+\s*\(|\b(?:File|OpenOptions|Connection)::[A-Za-z_]+\s*\(|\b(?:write_file_atomic|atomic_write\w*|write_json\w*|rename_with_retry|CreateFileW|MoveFileExW|ReplaceFileW|writeln|write)\s*!?\s*\(/g;for(const m of code.matchAll(re)){const f=functions.filter(f=>m.index>=f.start&&m.index<f.end).sort((a,b)=>(a.end-a.start)-(b.end-b.start))[0];const item=f?f.name:'<module>';const ordinal=sinks.filter(x=>x.item===item).length+1;let line=starts.findIndex(x=>x>m.index);if(line<0)line=starts.length;sinks.push({path:p,item,ordinal,line,offset:m.index,operation:m[0],evidence:lines[line-1].trim()});}
return sinks.map(site=>({...site,tokens:tokens.filter(t=>t.start>=site.offset&&t.start<site.offset+site.operation.length)}));
}
