import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';

const prototypePath = resolve('_prototypes/coding-agent-profile-matrix-modal.html');
const html = await readFile(prototypePath, 'utf8');

const dom = new JSDOM(html, {
  runScripts: 'dangerously',
});

const preview = dom.window.document.getElementById('jsonPreview');
assert.ok(preview, 'JSON preview element should exist');

const documentText = () => dom.window.document.body.textContent ?? '';
const parsePreview = () => JSON.parse(preview.textContent ?? '{}');
const click = (element) => {
  element.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true }));
};

const headers = Array.from(dom.window.document.querySelectorAll('thead th'), (th) =>
  th.textContent?.trim()
);
assert.deepEqual(headers, ['Profile', 'Codex', 'Claude Code', 'OpenCode']);
assert.ok(!headers.some((header) => header?.toLowerCase().includes('gemini')));

assert.match(documentText(), /A-FULL POWER/);

const profileAName = dom.window.document.querySelector('[data-profile-name="A"]');
assert.ok(profileAName, 'profile A should expose an editable custom name');
profileAName.value = 'MAXIMUM';
profileAName.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
assert.match(documentText(), /A-MAXIMUM/);

assert.match(
  documentText(),
  /Fallback:\s*each missing profile uses the immediately higher row; D uses C, C uses B, B uses A, and A is the final fallback\./
);
assert.equal(dom.window.document.getElementById('prioritySelect'), null);
assert.equal(dom.window.document.getElementById('fallbackInput'), null);
assert.ok(!documentText().includes('Priority'));

const text = preview.textContent ?? '';
const expectedAcRoot = 'C:\\Users\\maria\\0_repos\\AgentsCommander_ac\\.ac';
const expectedConfigPath = `${expectedAcRoot}\\coding-agent-profiles.json`;
const expectedEscapedAcRoot = 'C:\\\\Users\\\\maria\\\\0_repos\\\\AgentsCommander_ac\\\\.ac';

assert.ok(
  text.includes(expectedEscapedAcRoot),
  'serialized JSON preview should show the escaped Windows AC root path'
);
assert.ok(!text.includes('\\u0000'), 'serialized JSON preview should not contain escaped NUL bytes');
assert.ok(!text.includes('C:Usersmaria'), 'serialized JSON preview should not collapse path separators');

let data = parsePreview();
assert.equal(data.acRoot, expectedAcRoot);
assert.equal(data.configPath, expectedConfigPath);
assert.deepEqual(
  data.codingAgents.map((agent) => agent.id),
  ['codex', 'claude-code', 'opencode']
);
assert.deepEqual(
  data.codingAgents.map((agent) => agent.colorFromCodingAgentConfig),
  ['#00d4ff', '#a78bfa', '#f97316']
);
assert.equal(data.profileLetters[0].label, 'A-MAXIMUM');
assert.equal(data.profileLetters[0].fixedFallback, 'final fallback');
assert.equal(data.profileLetters[1].fixedFallback, 'A-MAXIMUM');
assert.ok(!JSON.stringify(data.profiles).includes('"priority"'));
assert.ok(!JSON.stringify(data.profiles).includes('"fallback"'));
assert.ok(!text.toLowerCase().includes('gemini'), 'JSON preview should not include Gemini');
assert.ok(!documentText().toLowerCase().includes('gemini'), 'prototype should not render Gemini');

assert.equal(
  dom.window.document.querySelector('[data-remove-agent="codex"][data-remove-letter="A"]'),
  null,
  'row A cells should not expose remove buttons'
);
assert.equal(
  dom.window.document.querySelector('[data-add-agent="codex"][data-add-letter="A"]'),
  null,
  'row A cells should not expose add buttons'
);

const removableCell = dom.window.document.querySelector(
  '[data-remove-agent="claude-code"][data-remove-letter="D"]'
);
assert.ok(removableCell, 'a non-A configured cell should expose a remove button');
click(removableCell);
data = parsePreview();
assert.deepEqual(data.profiles.D['claude-code'], {
  missing: true,
  usesFixedFallback: 'C-REVIEW',
});

const addCell = dom.window.document.querySelector('[data-add-agent="claude-code"][data-add-letter="D"]');
assert.ok(addCell, 'removed non-A cell should expose an add button');
click(addCell);
data = parsePreview();
assert.equal(data.profiles.D['claude-code'].model, 'claude/default');
assert.equal(data.profiles.D['claude-code'].enabled, true);
