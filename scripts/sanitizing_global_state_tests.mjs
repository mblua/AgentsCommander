import test from 'node:test';
import assert from 'node:assert/strict';
import { runGate } from './sanitizing_global_state.mjs';

const root = 'src-tauri/src/';
const p = root + 'p.rs';
const mutex = 'static X: std::sync::Mutex<u8> = std::sync::Mutex::new(0);';
function trees(base, head = base) {
  const tree = files => new Map(Object.entries(files).map(([path, text]) =>
    [path, { path, text, mode: '100644', type: 'blob' }]));
  return { baseTree: tree(base), headTree: tree(head), baseOid: 'base-fixture',
    headOid: 'head-fixture', refOid: 'ref-fixture', requestedRef: 'fixture',
    readBlob: entry => entry?.text ?? '', assertStableHead() {} };
}
function pass(base, head = base) {
  const result = runGate(trees(base, head));
  assert.equal(result.exit, 0, JSON.stringify(result));
  assert.equal(result.diagnostics.some(d => d.rule === 'new_exposed_global'), false);
  return result;
}
function fail(base, head, rules, expected = [{ path: p, line: 1 }]) {
  const result = runGate(trees(base, head));
  assert.equal(result.exit, 1, JSON.stringify(result));
  const failures = result.diagnostics.filter(d => d.status === 'FAIL');
  const compare = (a, b) => a.localeCompare(b);
  assert.deepEqual([...new Set(failures.map(d => d.rule))].sort(compare),
    [...rules].sort(compare));
  const exposed = failures.filter(d => d.rule === 'new_exposed_global');
  assert.deepEqual(exposed.map(({ path, line }) => ({ path, line })), expected);
  for (const d of exposed) {
    assert.equal(d.base, 0);
    assert.equal(d.head, 1);
    assert.equal(d.baseOid, 'base-fixture');
    assert.equal(d.headOid, 'head-fixture');
    assert.ok(d.remedy);
    assert.equal(d.reason, 'recognized mutable global has new explicit visibility');
  }
  return result;
}
function visibility(code, prefix = 'pub ', line = 1) {
  pass({ [p]: code });
  const result = fail({ [p]: code }, { [p]: prefix + code }, ['new_exposed_global'],
    [{ path: p, line }]);
  const count = side => result.measurements[side][0].counts.globals;
  assert.equal(count('base'), count('head'));
  assert.equal(result.totals[0].globals, result.totals[1].globals);
  pass({ [p]: code });
  return result;
}

test('08 exposure: existing private Mutex becomes public despite totals 1 to 1', () => {
  const result = visibility(mutex);
  assert.equal(result.measurements.base[0].counts.globals, 1);
  assert.equal(result.measurements.head[0].counts.globals, 1);
  assert.deepEqual(result.totals, [{ globals: 1 }, { globals: 1 }]);
  assert.equal(result.diagnostics.some(d => d.status === 'WARN'), false);
});

test('08 exposure: all explicit visibility spellings including comments and multiline', () => {
  for (const prefix of ['pub ', 'pub(crate) ', 'pub(super) ', 'pub(in crate::x) ',
    'pub(self) ', 'pub /* visibility */ (\n crate\n)\n']) {
    visibility(mutex, prefix, prefix.split('\n').length);
  }
});

test('08 exposure: attribute payload, strings and comments do not supply visibility', () => {
  const code = '#[marker(pub)]\n' + mutex;
  pass({ [p]: code }, { [p]: '// pub static\n#[marker(pub)]\n' + mutex });
  pass({ [p]: mutex }, { [p]: '// pub\nconst TEXT: &str = "pub static";\n' + mutex });
  const external = '#[marker(pub)]\npub ' + mutex;
  fail({ [p]: code }, { [p]: external }, ['new_exposed_global'], [{ path: p, line: 2 }]);
});

test('08 exposure: adding an attribute pub token keeps a static private', () => {
  const result = pass({ [p]: mutex }, { [p]: '#[marker(pub)]
' + mutex });
  assert.deepEqual(result.totals, [{ globals: 1 }, { globals: 1 }]);
});

test('08 exposure: earlier public function does not expose an edited private static', () => {
  const prefix = 'pub fn g() {}
';
  const result = pass({ [p]: prefix + mutex },
    { [p]: prefix + mutex.replace('new(0)', 'new(1)') });
  assert.deepEqual(result.totals, [{ globals: 1 }, { globals: 1 }]);
});

test('08 exposure: static mut and recognized mutable aliases', () => {
  visibility('static mut X: u8 = 0;');
  for (const type of ['Mutex', 'RwLock', 'OnceLock', 'AtomicU8']) {
    const namespace = type === 'AtomicU8' ? 'std::sync::atomic' : 'std::sync';
    const generic = type === 'AtomicU8' ? '' : '<u8>';
    const init = type === 'OnceLock' ? '' : '0';
    const code = `use ${namespace}::${type} as Alias;\n` +
      `static X: Alias${generic} = Alias::new(${init});`;
    fail({ [p]: code }, { [p]: code.replace('static X', 'pub static X') },
      ['new_exposed_global'], [{ path: p, line: 2 }]);
  }
});

test('08 exposure: thread-local Cell and RefCell prefixes stay within each declaration', () => {
  for (const type of ['Cell', 'RefCell']) {
    const item = `static X: std::cell::${type}<u8> = std::cell::${type}::new(0);`;
    const wrap = x => `thread_local! {\n${x}\n${item.replaceAll('X', 'Y')}\n}`;
    for (const prefix of ['pub ', 'pub(crate) ', 'pub (\nself\n) ']) {
      const line = 2 + prefix.split('\n').length - 1;
      const r = fail({ [p]: wrap(item) }, { [p]: wrap(prefix + item) },
        ['new_exposed_global'], [{ path: p, line }]);
      assert.equal(r.totals[0].globals, 2);
      assert.equal(r.totals[1].globals, 2);
    }
    pass({ [p]: wrap('pub ' + item) });
  }
});

test('08 exposure: unchanged public/private debt and removing visibility pass', () => {
  pass({ [p]: mutex });
  pass({ [p]: 'pub ' + mutex });
  pass({ [p]: 'pub ' + mutex }, { [p]: mutex });
});

test('08 exposure: deleting unrelated public debt cannot credit newly exposed private debt', () => {
  const other = 'static Y: std::sync::RwLock<u8> = std::sync::RwLock::new(0);';
  fail({ [p]: 'pub ' + mutex + '\n' + other }, { [p]: 'pub ' + other },
    ['new_exposed_global']);
});

test('08 exposure: exact public move to a new file passes; private to public does not', () => {
  const q = root + 'q.rs';
  pass({ [p]: 'pub ' + mutex }, { [q]: 'pub ' + mutex });
  const r = fail({ [p]: mutex }, { [q]: 'pub ' + mutex },
    ['globals', 'new_exposed_global'], [{ path: q, line: 1 }]);
  assert.deepEqual(r.totals, [{ globals: 1 }, { globals: 1 }]);
});

test('08 exposure: thread-local move identities include explicit visibility', () => {
  const q = root + 'q.rs';
  for (const type of ['Cell', 'RefCell']) {
    const wrap = visibility => 'thread_local! { ' + visibility +
      `static X: std::cell::${type}<u8> = std::cell::${type}::new(0); }`;
    pass({ [p]: wrap('pub(crate) ') }, { [q]: wrap('pub(crate) ') });
    fail({ [p]: wrap('') }, { [q]: wrap('pub(crate) ') },
      ['globals', 'new_exposed_global'], [{ path: q, line: 1 }]);
    pass({ [p]: wrap('') }, { [q]: wrap('') });
  }
});

test('08 exposure: private declarations still move and split freely', () => {
  const q = root + 'q.rs';
  const other = mutex.replaceAll('X', 'Y');
  pass({ [p]: mutex }, { [q]: mutex });
  pass({ [p]: mutex + '\n' + other }, { [p]: mutex, [q]: other });
});

test('08 exposure: one removed occurrence credits only the first identical new copy', () => {
  const old = root + 'old.rs', a = root + 'new-a.rs', b = root + 'new-b.rs';
  const code = 'pub ' + mutex;
  const r = fail({ [old]: code }, { [a]: code, [b]: code },
    ['globals', 'new_exposed_global'], [{ path: b, line: 1 }]);
  assert.deepEqual(r.measurements.head.map(m => m.path), [a, b]);
  assert.equal(r.diagnostics.filter(d => d.rule === 'new_exposed_global').length, 1);
});

test('08 exposure: retained occurrence cannot credit an added copy in the same file', () => {
  const code = 'pub ' + mutex;
  const r = fail({ [p]: code }, { [p]: code + '\n' + code },
    ['globals', 'new_exposed_global'], [{ path: p, line: 2 }]);
  assert.deepEqual(r.measurements.head[0].sites.globals.map(s => s.line), [1, 2]);
});

test('08 exposure: undefined ambiguous identities cannot match unchanged public debt', () => {
  const code = 'use a::*; use b::*; pub static X: Mutex<u8> = Mutex::new(0);';
  const r = fail({ [p]: code }, { [p]: code }, ['new_exposed_global']);
  assert.ok(r.diagnostics.some(d => d.status === 'WARN'));
  assert.equal(r.measurements.head[0].sites.globals[0].identity, undefined);
});

test('08 exposure: normal local statics retain their own item identity', () => {
  const code = 'fn f() { pub ' + mutex + ' let n = 0; }';
  pass({ [p]: code });
  pass({ [p]: code }, { [p]: code.replace('n = 0', 'n = 1') });
});

test('08 exposure: local thread-local statics use the enclosing function identity', () => {
  const code = 'fn f() { thread_local! { pub static X: std::cell::Cell<u8> = ' +
    'std::cell::Cell::new(0); } let n = 0; }';
  pass({ [p]: code });
  fail({ [p]: code }, { [p]: code.replace('n = 0', 'n = 1') }, ['new_exposed_global']);
});

test('08 exposure: readonly Regex stays exempt but private local OnceLock may not', () => {
  const lazy = 'static X: std::sync::LazyLock<regex::Regex> = ' +
    'std::sync::LazyLock::new(|| regex::Regex::new("x").unwrap());';
  const r = pass({ [p]: lazy }, { [p]: 'pub ' + lazy });
  assert.deepEqual(r.totals, [{ globals: 0 }, { globals: 0 }]);
  const once = 'fn f() {\nstatic X: std::sync::OnceLock<regex::Regex> = ' +
    'std::sync::OnceLock::new();\nX.get_or_init(|| regex::Regex::new("x").unwrap());\n}';
  fail({ [p]: once }, { [p]: once.replace('static X', 'pub static X') },
    ['globals', 'new_exposed_global'], [{ path: p, line: 2 }]);
});

test('08 exposure: test-only, arbitrary macro and reexports stay outside recognition', () => {
  for (const wrap of [x => '#[cfg(test)]\n' + x, x => '#[test]\nfn f() { ' + x + ' }',
    x => '#[cfg(test)] mod tests { ' + x + ' }', x => 'arbitrary! { ' + x + ' }']) {
    pass({ [p]: wrap(mutex) }, { [p]: wrap('pub ' + mutex) });
  }
  pass({ [p]: mutex }, { [p]: mutex + '\npub use other::X as Visible;' });
});
