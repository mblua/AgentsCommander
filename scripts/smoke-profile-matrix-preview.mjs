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

const headers = Array.from(dom.window.document.querySelectorAll('thead th'), (th) =>
  th.textContent?.trim()
);
assert.deepEqual(headers, ['Profile', 'Codex', 'Claude Code', 'OpenCode']);

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

const data = JSON.parse(text);
assert.equal(data.acRoot, expectedAcRoot);
assert.equal(data.configPath, expectedConfigPath);
assert.deepEqual(data.codingAgents, ['codex', 'claude-code', 'opencode']);
assert.ok(!text.toLowerCase().includes('gemini'), 'JSON preview should not include Gemini');
assert.ok(!dom.window.document.body.textContent.toLowerCase().includes('gemini'), 'prototype should not render Gemini');
