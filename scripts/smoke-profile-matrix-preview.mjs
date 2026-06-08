import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
import { JSDOM, VirtualConsole } from 'jsdom';

const prototypePath = resolve('_prototypes/coding-agent-profile-matrix-modal.html');
const html = await readFile(prototypePath, 'utf8');
const runtimeErrors = [];
let copiedText = '';
const virtualConsole = new VirtualConsole();
virtualConsole.sendTo(console);
virtualConsole.on('jsdomError', (error) => {
  runtimeErrors.push(error);
});

const dom = new JSDOM(html, {
  runScripts: 'dangerously',
  virtualConsole,
  beforeParse(window) {
    window.addEventListener('error', (event) => {
      runtimeErrors.push(event.error ?? event.message);
    });
    window.document.execCommand = (command) => {
      if (command === 'copy') {
        copiedText = window.document.querySelector('textarea')?.value ?? '';
        return true;
      }
      return false;
    };
  },
});

const preview = dom.window.document.getElementById('jsonPreview');
assert.ok(preview, 'JSON preview element should exist');

const documentText = () => dom.window.document.body.textContent ?? '';
const parsePreview = () => JSON.parse(preview.textContent ?? '{}');
const click = (element) => {
  element.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true }));
};
const change = (element, value) => {
  element.value = value;
  element.dispatchEvent(new dom.window.Event('change', { bubbles: true }));
};
const assertNoRuntimeErrors = () => {
  assert.equal(
    runtimeErrors.length,
    0,
    `prototype should not throw runtime errors: ${runtimeErrors.map((error) => error?.stack ?? error).join('\n')}`
  );
};

const headers = Array.from(dom.window.document.querySelectorAll('table[aria-label="Coding agent profile matrix"] thead th'), (th) =>
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
assert.ok(
  !documentText().includes('uses A-FULL POWER'),
  'matrix fallback labels should not retain stale profile A names after rename'
);
assertNoRuntimeErrors();

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
assert.equal(data.profiles.B.codex.usesFixedFallback, undefined);
assert.equal(data.profiles.B.opencode.usesFixedFallback, 'A-MAXIMUM');
assert.ok(!JSON.stringify(data.profiles).includes('"priority"'));
assert.ok(!JSON.stringify(data.profiles).includes('"fallback"'));
assert.ok(!text.toLowerCase().includes('gemini'), 'JSON preview should not include Gemini');
assert.ok(!documentText().toLowerCase().includes('gemini'), 'prototype should not render Gemini');
assert.match(documentText(), /Defaults tab in Profile Matrix settings/);
assert.match(documentText(), /No template \/ global/);
assert.match(documentText(), /Local role template: dev-webpage-ui/);
assert.match(documentText(), /Agent type: coding-agent/);
assert.match(documentText(), /Mold: review-helper/);
assert.match(documentText(), /OpenCode B missing; fixed fallback A/);
assert.match(documentText(), /New Agent creation modal/);
assert.match(documentText(), /Individual Agent profile assignment modal/);
assert.ok(
  dom.window.document.querySelector('[aria-label="Assign profile to individual agent"]'),
  'individual agent assignment modal mockup should exist'
);
assert.match(documentText(), /from template: dev-webpage-ui/);
assert.match(documentText(), /Legacy no-profile agents inherit the current default at launch/);
assert.match(documentText(), /Diagnostics and resolution view/);
assert.match(documentText(), /No raw shell string/);
assert.match(documentText(), /Workgroup dispatch and agent list visibility/);

const newAgentTemplate = dom.window.document.getElementById('newAgentTemplate');
const newAgentMold = dom.window.document.getElementById('newAgentMold');
const newAgentOverride = dom.window.document.getElementById('newAgentOverride');
const newAgentRuleSource = dom.window.document.getElementById('newAgentRuleSource');
const newAgentRequested = dom.window.document.getElementById('newAgentRequested');
const newAgentResolved = dom.window.document.getElementById('newAgentResolved');
const newAgentFallbackNotice = dom.window.document.getElementById('newAgentFallbackNotice');
assert.ok(newAgentTemplate, 'new agent template selector should exist');
assert.ok(newAgentMold, 'new agent mold selector should exist');
assert.ok(newAgentOverride, 'new agent override selector should exist');
assert.equal(newAgentRuleSource?.textContent, 'Local role template: dev-webpage-ui');
assert.equal(newAgentRequested?.textContent, 'B - BALANCED');
assert.match(newAgentResolved?.textContent ?? '', /B - BALANCED for Codex/);
assert.match(newAgentResolved?.textContent ?? '', /B - BALANCED for Claude Code/);
assert.match(newAgentResolved?.textContent ?? '', /A - FULL POWER for OpenCode/);
assert.match(newAgentResolved?.textContent ?? '', /B -> A - FULL POWER/);
assert.match(newAgentFallbackNotice?.textContent ?? '', /OpenCode has no B cell/);
assert.equal(newAgentFallbackNotice?.hidden, false);

change(newAgentMold, 'review');
assert.equal(newAgentRuleSource?.textContent, 'Mold rule: review-helper');
assert.equal(newAgentRequested?.textContent, 'C - REVIEW');
assert.match(newAgentResolved?.textContent ?? '', /C - REVIEW for Codex/);
assert.match(newAgentResolved?.textContent ?? '', /C - REVIEW for Claude Code/);
assert.match(newAgentResolved?.textContent ?? '', /C - REVIEW for OpenCode/);
assert.equal(newAgentFallbackNotice?.hidden, true);

change(newAgentOverride, 'D');
assert.equal(newAgentRuleSource?.textContent, 'User override before Create');
assert.equal(newAgentRequested?.textContent, 'D - LOW COST');
assert.match(newAgentResolved?.textContent ?? '', /C - REVIEW for Codex/);
assert.match(newAgentResolved?.textContent ?? '', /D -> C - REVIEW/);
assert.match(newAgentResolved?.textContent ?? '', /D - LOW COST for Claude Code/);
assert.match(newAgentResolved?.textContent ?? '', /D - LOW COST for OpenCode/);
assert.match(newAgentFallbackNotice?.textContent ?? '', /Codex has no D cell/);
assert.equal(newAgentFallbackNotice?.hidden, false);
assertNoRuntimeErrors();

const individualAgentTool = dom.window.document.getElementById('individualAgentTool');
const individualAgentProfile = dom.window.document.getElementById('individualAgentProfile');
const individualAgentHeaderToken = dom.window.document.getElementById('individualAgentHeaderToken');
const individualAgentSource = dom.window.document.getElementById('individualAgentSource');
const individualAgentRequested = dom.window.document.getElementById('individualAgentRequested');
const individualAgentEffective = dom.window.document.getElementById('individualAgentEffective');
const individualAgentFallbackNotice = dom.window.document.getElementById('individualAgentFallbackNotice');
const legacyAgentAssignProfile = dom.window.document.getElementById('legacyAgentAssignProfile');
const legacyAgentAssignmentNotice = dom.window.document.getElementById('legacyAgentAssignmentNotice');
assert.ok(individualAgentTool, 'individual agent coding tool selector should exist');
assert.ok(individualAgentProfile, 'individual agent profile selector should exist');
assert.equal(individualAgentTool?.value, 'opencode');
assert.equal(individualAgentProfile?.value, 'B');
assert.equal(individualAgentHeaderToken?.textContent, 'B - BALANCED');
assert.equal(individualAgentSource?.textContent, 'from template: dev-webpage-ui');
assert.equal(individualAgentRequested?.textContent, 'B - BALANCED');
assert.match(individualAgentEffective?.textContent ?? '', /A - FULL POWER for OpenCode/);
assert.match(individualAgentEffective?.textContent ?? '', /B -> A - FULL POWER/);
assert.match(individualAgentFallbackNotice?.textContent ?? '', /OpenCode has no B cell/);
assert.equal(individualAgentFallbackNotice?.hidden, false);

change(individualAgentProfile, 'inherit');
assert.equal(individualAgentSource?.textContent, 'from template: dev-webpage-ui');
assert.equal(individualAgentRequested?.textContent, 'B - BALANCED');
assert.match(individualAgentEffective?.textContent ?? '', /A - MAXIMUM for OpenCode/);
assert.equal(individualAgentFallbackNotice?.hidden, false);

change(individualAgentTool, 'codex');
assert.match(individualAgentEffective?.textContent ?? '', /B - BALANCED for Codex/);
assert.equal(individualAgentFallbackNotice?.hidden, true);

assert.ok(legacyAgentAssignProfile, 'legacy no-profile assignment selector should exist');
assert.match(legacyAgentAssignmentNotice?.textContent ?? '', /until the user explicitly saves an assigned profile/);
change(legacyAgentAssignProfile, 'B');
assert.match(legacyAgentAssignmentNotice?.textContent ?? '', /Saving records explicit B - BALANCED/);
assert.match(legacyAgentAssignmentNotice?.textContent ?? '', /not silent migration/);
assertNoRuntimeErrors();

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

const addProfileBtn = dom.window.document.getElementById('addProfileBtn');
const matrixWrap = dom.window.document.querySelector('.matrix-wrap');
assert.ok(addProfileBtn, '+ Profile button should be present');
assert.ok(matrixWrap, 'matrix wrapper should be present');
assert.equal(
  Boolean(matrixWrap.compareDocumentPosition(addProfileBtn) & dom.window.Node.DOCUMENT_POSITION_FOLLOWING),
  true,
  '+ Profile should render after the matrix'
);
click(addProfileBtn);
data = parsePreview();
assert.equal(data.profileLetters.at(-1).letter, 'E');
assert.equal(dom.window.document.getElementById('letterSelect')?.value, 'E');
assert.equal(data.profiles.E.codex.model, 'codex/default');
assertNoRuntimeErrors();

const letterSelect = dom.window.document.getElementById('letterSelect');
assert.ok(letterSelect, 'profile selector should be present');
change(letterSelect, 'D');
assertNoRuntimeErrors();
assert.match(
  dom.window.document.getElementById('selectedKey')?.textContent ?? '',
  /D-LOW COST \/ Codex - Missing; uses C-REVIEW/
);
assert.equal(dom.window.document.getElementById('modelInput')?.disabled, true);
assert.equal(dom.window.document.getElementById('addSelectedCellBtn')?.hidden, false);

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
assertNoRuntimeErrors();

const captureTarget = dom.window.document.querySelector('[data-agent="claude-code"][data-letter="D"] .cell-value');
assert.ok(captureTarget, 'profile cell content should exist for component capture');
const capturedCell = captureTarget.closest('[data-component]');
assert.ok(capturedCell, 'profile cell should expose a data-component capture marker');
capturedCell.getBoundingClientRect = () => ({
  left: 24,
  top: 32,
  width: 180,
  height: 90,
  right: 204,
  bottom: 122,
  x: 24,
  y: 32,
  toJSON: () => {},
});

let fakeTimerId = 0;
const activeTimers = new Map();
const clearedTimers = [];
dom.window.setTimeout = (callback, delay, ...args) => {
  const id = ++fakeTimerId;
  activeTimers.set(id, { callback, delay, args });
  return id;
};
dom.window.clearTimeout = (id) => {
  clearedTimers.push(id);
  activeTimers.delete(id);
};
const getTimersByDelay = (delay) => Array.from(activeTimers.entries()).filter(([, timer]) => timer.delay === delay);

const contextMenu = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
});
const contextMenuAllowed = captureTarget.dispatchEvent(contextMenu);
assert.equal(contextMenuAllowed, false, 'right click should prevent the native context menu path');
assert.equal(contextMenu.defaultPrevented, true, 'contextmenu event should be marked defaultPrevented');

const captureResult = await dom.window.__componentCaptureLastResult;
assert.equal(
  captureResult.quotedIdentifier,
  '"Coding Agent Profile Matrix / td / Claude Code D-LOW COST configured profile cell"'
);
assert.equal(copiedText, captureResult.quotedIdentifier, 'fallback clipboard copy should include double quotes');

const toast = dom.window.document.querySelector('.component-capture-toast');
assert.ok(toast, 'component capture toast should appear');
assert.match(toast?.textContent ?? '', /"Coding Agent Profile Matrix \/ td \/ Claude Code D-LOW COST configured profile cell"/);
assert.match(toast?.textContent ?? '', /td \.profile-cell/);

const toastTimers = getTimersByDelay(15000);
assert.equal(toastTimers.length, 1, 'component capture toast should schedule a 15-second removal timer');
assert.equal(
  dom.window.document.querySelector('.component-capture-toast'),
  toast,
  'component capture toast should stay visible before the 15-second timer fires'
);

const highlight = dom.window.document.querySelector('.component-capture-highlight');
assert.ok(highlight, 'component capture highlight should appear');
assert.equal(highlight?.style.left, '24px');
assert.equal(highlight?.style.top, '32px');
highlight?.dispatchEvent(new dom.window.Event('animationend', { bubbles: true }));
assert.equal(
  dom.window.document.querySelector('.component-capture-highlight'),
  null,
  'component capture highlight should remove after its two-blink animation completes'
);

const secondCaptureResult = await dom.window.captureComponentAtTarget('Coding Agent Profile Matrix', captureTarget);
assert.equal(secondCaptureResult.quotedIdentifier, captureResult.quotedIdentifier);
assert.equal(
  clearedTimers.includes(toastTimers[0][0]),
  true,
  'a new component capture should reset the previous toast timer'
);
assert.equal(
  getTimersByDelay(15000).length,
  1,
  'a new component capture should leave one active 15-second toast timer'
);
const latestToast = dom.window.document.querySelector('.component-capture-toast');
assert.ok(latestToast, 'replacement component capture toast should appear');
const latestToastTimer = getTimersByDelay(15000)[0][1];
latestToastTimer.callback(...latestToastTimer.args);
assert.equal(
  dom.window.document.querySelector('.component-capture-toast'),
  null,
  'component capture toast should remove when the 15-second timer fires'
);
assertNoRuntimeErrors();
