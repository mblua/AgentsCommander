const { after, test } = require('node:test');
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ac4583-'));
const ac = path.join(root, '.ac');
const hooks = path.join(ac, 'room-test', '__agent_t', '.claude', 'hooks');
const matrix = path.join(ac, '_agent_t');
fs.mkdirSync(hooks, { recursive: true });
fs.mkdirSync(matrix, { recursive: true });
const source = path.join(__dirname, '..', 'docs', 'integrations', 'rtk', 'claude', 'hooks');
for (const name of ['ac_rtk_shared.js', 'ac_rtk_claude_Bash.js', 'ac_rtk_claude_PowerShell.js']) {
  fs.copyFileSync(path.join(source, name), path.join(hooks, name));
}
const { NAG } = require(path.join(hooks, 'ac_rtk_shared.js'));
const FILTER = `exec 2> >(grep --line-buffered -v '${NAG}' >&2)\n`;
after(() => fs.rmSync(root, { recursive: true, force: true }));

function run(tool, command) {
  const result = spawnSync(process.execPath, [path.join(hooks, `ac_rtk_claude_${tool}.js`)], {
    input: JSON.stringify({ tool_input: { command, description: 'd' } }),
    encoding: 'utf8',
    timeout: 20000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}

for (const tool of ['Bash', 'PowerShell']) {
  test(`${tool} rewrites without deciding permission and preserves other input`, () => {
    const output = JSON.parse(run(tool, 'rtk git status'));
    const specific = output.hookSpecificOutput;
    assert.equal(specific.hookEventName, 'PreToolUse');
    assert.equal(Object.hasOwn(specific, 'permissionDecision'), false);
    assert.equal(Object.hasOwn(specific, 'permissionDecisionReason'), false);
    assert.deepEqual(specific.updatedInput, {
      command: (tool === 'Bash' ? FILTER : '') + 'rtk git status',
      description: 'd',
    });
  });
}

test('Bash leaves a builtin untouched and logs exactly one ignored command', () => {
  assert.equal(run('Bash', 'cd x'), '');
  const log = fs.readFileSync(path.join(matrix, 'rtk-ignored-tools-claude.md'), 'utf8');
  assert.match(log, /^\d{8}_\d{6} Bash: cd x\r?\n$/);
});
