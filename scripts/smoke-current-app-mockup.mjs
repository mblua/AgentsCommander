import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
import { JSDOM, VirtualConsole } from 'jsdom';

const prototypePath = resolve('_prototypes/agentscommander-current-app-mockup.html');
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

const documentText = () => dom.window.document.body.textContent ?? '';
const assertNoRuntimeErrors = () => {
  assert.equal(
    runtimeErrors.length,
    0,
    `prototype should not throw runtime errors: ${runtimeErrors.map((error) => error?.stack ?? error).join('\n')}`
  );
};

assert.equal(typeof dom.window.installComponentCapture, 'function');
assert.equal(typeof dom.window.captureComponentAtTarget, 'function');
assert.match(html, /installComponentCapture\("AgentsCommander Current App"\)/);
assert.ok(dom.window.document.querySelector('[data-component="top header with brand version workgroup identity and window controls"]'));
assert.ok(dom.window.document.querySelector('[data-component="terminal transcript area with current task prompt and command blocks"]'));
assert.ok(dom.window.document.querySelector('[data-component="right AgentsCommander navigation control sidebar"]'));
assert.ok(dom.window.document.querySelector('[data-component="selected project coding agent model and effort task"]'));
assert.ok(dom.window.document.querySelector('[data-component="selected workgroup panel"]'));
assert.match(documentText(), /Agents Commander/);
assert.match(documentText(), /v0\.8\.50/);
assert.match(documentText(), /WG-7-DEV-TEAM/);
assert.match(documentText(), /tech-lead@AgentsCommander_ac/);
assert.match(documentText(), /TASK:\s*CODING AGENT, MODEL AND EFFORT POR AGENTE\./);
assert.match(documentText(), /LAST PROMPT/);
assert.match(documentText(), /Working \(2m 25s - esc to interrupt\)/);
assert.match(documentText(), /codex resume --last -m gpt-5\.5 -c reasoning_effort=xhigh --yolo/);
assert.match(documentText(), /ac-cli-tester/);
assert.match(documentText(), /dev-webpage-ui/);
assert.match(documentText(), /WG-1-DEV-TEAM/);
assert.equal(/<img\b/i.test(html), false, 'mockup should not embed the screenshot as an image');
assertNoRuntimeErrors();

const captureTarget = dom.window.document.querySelector('[data-component="selected project coding agent model and effort task"] .card-title');
assert.ok(captureTarget, 'selected project capture target should exist');
const capturedComponent = captureTarget.closest('[data-component]');
capturedComponent.getBoundingClientRect = () => ({
  left: 380,
  top: 156,
  width: 340,
  height: 72,
  right: 720,
  bottom: 228,
  x: 380,
  y: 156,
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
  '"AgentsCommander Current App / article / selected project coding agent model and effort task"'
);
assert.equal(copiedText, captureResult.quotedIdentifier, 'fallback clipboard copy should include double quotes');

const toast = dom.window.document.querySelector('.component-capture-toast');
assert.ok(toast, 'component capture toast should appear');
assert.match(toast?.textContent ?? '', /"AgentsCommander Current App \/ article \/ selected project coding agent model and effort task"/);
assert.match(toast?.textContent ?? '', /article \.project-card\.selected/);

const toastTimers = getTimersByDelay(15000);
assert.equal(toastTimers.length, 1, 'component capture toast should schedule a 15-second removal timer');
assert.equal(
  dom.window.document.querySelector('.component-capture-toast'),
  toast,
  'component capture toast should stay visible before the 15-second timer fires'
);

const highlight = dom.window.document.querySelector('.component-capture-highlight');
assert.ok(highlight, 'component capture highlight should appear');
assert.equal(highlight?.style.left, '380px');
assert.equal(highlight?.style.top, '156px');
const highlightStyle = dom.window.getComputedStyle(highlight);
assert.match(highlightStyle.animation, /component-capture-blink/);
assert.match(highlightStyle.animation, /2/);
highlight?.dispatchEvent(new dom.window.Event('animationend', { bubbles: true }));
assert.equal(
  dom.window.document.querySelector('.component-capture-highlight'),
  null,
  'component capture highlight should remove after its two-blink animation completes'
);

const secondCaptureResult = await dom.window.captureComponentAtTarget('AgentsCommander Current App', captureTarget);
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
