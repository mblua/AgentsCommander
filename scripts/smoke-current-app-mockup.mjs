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
const cssBlockFor = (selector) => {
  const blockPattern = new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\{(?<body>[^}]*)\\}`);
  return html.match(blockPattern)?.groups?.body ?? '';
};
const assertNoRuntimeErrors = () => {
  assert.equal(
    runtimeErrors.length,
    0,
    `prototype should not throw runtime errors: ${runtimeErrors.map((error) => error?.stack ?? error).join('\n')}`
  );
};

assert.equal(typeof dom.window.installComponentCapture, 'function');
assert.equal(typeof dom.window.captureComponentAtTarget, 'function');
assert.equal(typeof dom.window.openSidebarContextMenu, 'function');
assert.equal(typeof dom.window.closeSidebarContextMenu, 'function');
assert.equal(typeof dom.window.openProfileModalContextMenu, 'function');
assert.equal(typeof dom.window.closeProfileModalContextMenu, 'function');
assert.equal(typeof dom.window.openCodingAgentProfileModal, 'function');
assert.equal(typeof dom.window.closeCodingAgentProfileModal, 'function');
assert.match(html, /installComponentCapture\("AgentsCommander Current App"\)/);
assert.ok(dom.window.document.querySelector('[data-component="top header with brand version workgroup identity and window controls"]'));
assert.ok(dom.window.document.querySelector('[data-component="terminal transcript area with current task prompt and command blocks"]'));
assert.ok(dom.window.document.querySelector('[data-component="right AgentsCommander navigation control sidebar"]'));
assert.ok(dom.window.document.querySelector('[data-component="selected project coding agent model and effort task"]'));
assert.ok(dom.window.document.querySelector('[data-component="selected workgroup panel"]'));
assert.ok(dom.window.document.querySelector('[data-component="nested selected team row"]'));
assert.ok(dom.window.document.querySelector('[data-sidebar-kind="session-agent"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile assignment modal"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal body"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding agent tool selector panel"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding agent provider selector"]'));
assert.ok(dom.window.document.querySelector('[data-component="Assigned coding agent profile selector"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent per-provider resolution panel"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent model argument field"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent effort argument field"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent default args field"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile args field"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal footer actions"]'));
assert.match(html, /Copy component description/);
assert.match(html, /A remains the immutable final fallback/);
assert.doesNotMatch(html, /Gemini/);
assert.doesNotMatch(html, /Priority/);
assert.doesNotMatch(html, /configurable fallback selector/i);
const sideScrollCss = cssBlockFor('.side-scroll');
assert.match(sideScrollCss, /overflow-y:\s*auto;/, 'sidebar content should allow vertical scrolling');
assert.match(sideScrollCss, /overflow-x:\s*hidden;/, 'sidebar content should keep horizontal overflow hidden');
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

const sideScroll = dom.window.document.querySelector('.side-scroll');
const workgroupsLabel = dom.window.document.querySelector('[data-component="workgroups section label"]');
assert.ok(sideScroll, 'sidebar scroll container should exist');
assert.ok(workgroupsLabel, 'workgroups label should exist');

const desktopSidebarTop = 46;
const desktopSidebarVisibleHeight = 682;
const workgroupsOffsetTop = 760;
Object.defineProperties(sideScroll, {
  clientHeight: { configurable: true, value: desktopSidebarVisibleHeight },
  scrollHeight: { configurable: true, value: 860 },
});
workgroupsLabel.getBoundingClientRect = () => ({
  left: 995,
  top: desktopSidebarTop + workgroupsOffsetTop - sideScroll.scrollTop,
  width: 340,
  height: 18,
  right: 1335,
  bottom: desktopSidebarTop + workgroupsOffsetTop - sideScroll.scrollTop + 18,
  x: 995,
  y: desktopSidebarTop + workgroupsOffsetTop - sideScroll.scrollTop,
  toJSON: () => {},
});
const isWorkgroupsReachable = () => {
  const rect = workgroupsLabel.getBoundingClientRect();
  return rect.top >= desktopSidebarTop && rect.bottom <= desktopSidebarTop + sideScroll.clientHeight;
};

assert.equal(isWorkgroupsReachable(), false, 'workgroups should start below the 1365x768 sidebar viewport');
sideScroll.scrollTop = sideScroll.scrollHeight - sideScroll.clientHeight;
assert.equal(sideScroll.scrollTop > 0, true, 'sidebar should accept a positive scrollTop');
assert.equal(isWorkgroupsReachable(), true, 'workgroups should be reachable after scrolling the sidebar');

const sidebarCaptureTarget = dom.window.document.querySelector('[data-component="selected project coding agent model and effort task"] .card-title');
assert.ok(sidebarCaptureTarget, 'selected project capture target should exist');
const capturedComponent = sidebarCaptureTarget.closest('[data-component]');
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

dom.window.__componentCaptureLastResult = undefined;
const sideScrollContextMenu = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 1200,
  clientY: 560,
});
const sideScrollContextMenuAllowed = sideScroll.dispatchEvent(sideScrollContextMenu);
assert.equal(sideScrollContextMenuAllowed, false, 'sidebar container right click should prevent native menu');
assert.equal(sideScrollContextMenu.defaultPrevented, true, 'sidebar container contextmenu should be marked defaultPrevented');
assert.equal(
  dom.window.__componentCaptureLastResult,
  undefined,
  'sidebar container right-click should not immediately capture'
);

const sideScrollMenu = dom.window.document.querySelector('.sidebar-context-menu');
assert.ok(sideScrollMenu, 'sidebar container right-click should open a custom context menu');
assert.equal(dom.window.document.querySelectorAll('.sidebar-context-menu').length, 1);
assert.match(sideScrollMenu?.textContent ?? '', /New Project/);
assert.match(sideScrollMenu?.textContent ?? '', /Open Project/);
assert.match(sideScrollMenu?.textContent ?? '', /Refresh Projects/);
assert.match(sideScrollMenu?.textContent ?? '', /Settings/);
assert.doesNotMatch(sideScrollMenu?.textContent ?? '', /Remove Project|Delete Workgroup|Delete Team|Delete Agent/);
assert.match(sideScrollMenu?.textContent ?? '', /Copy component description/);
assert.equal(dom.window.__sidebarContextLastMenu.kind, 'sidebar');

const sideScrollCopyAction = Array.from(dom.window.document.querySelectorAll('.sidebar-context-option'))
  .find((button) => button.textContent === 'Copy component description');
assert.ok(sideScrollCopyAction, 'sidebar container menu should include the copy component description action');
sideScrollCopyAction.click();
assert.equal(
  dom.window.document.querySelector('.sidebar-context-menu'),
  null,
  'sidebar container menu should close after invoking copy component description'
);
const sideScrollCaptureResult = await dom.window.__componentCaptureLastResult;
assert.equal(
  sideScrollCaptureResult.quotedIdentifier,
  '"AgentsCommander Current App / aside / right AgentsCommander navigation control sidebar"'
);
assert.equal(copiedText, sideScrollCaptureResult.quotedIdentifier, 'sidebar container copy should use capture helper');
dom.window.document.querySelector('.component-capture-highlight')
  ?.dispatchEvent(new dom.window.Event('animationend', { bubbles: true }));
getTimersByDelay(15000).forEach(([, timer]) => timer.callback(...timer.args));
activeTimers.clear();
clearedTimers.length = 0;
copiedText = '';
dom.window.__componentCaptureLastResult = undefined;

const codingAgentTarget = dom.window.document.querySelector('[data-component="agent row dev-webpage-ui active"] .agent-name');
assert.ok(codingAgentTarget, 'session-agent target with Coding Agent action should exist');
const codingAgentContextMenu = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 1214,
  clientY: 470,
});
const codingAgentContextMenuAllowed = codingAgentTarget.dispatchEvent(codingAgentContextMenu);
assert.equal(codingAgentContextMenuAllowed, false, 'session-agent right click should prevent the native context menu');
assert.equal(codingAgentContextMenu.defaultPrevented, true);
const codingAgentMenu = dom.window.document.querySelector('.sidebar-context-menu');
assert.ok(codingAgentMenu, 'session-agent right-click should open a custom context menu');
assert.match(codingAgentMenu?.textContent ?? '', /Restart Session/);
assert.match(codingAgentMenu?.textContent ?? '', /Coding Agent/);
assert.match(codingAgentMenu?.textContent ?? '', /Copy component description/);
assert.equal(dom.window.__sidebarContextLastMenu.kind, 'session-agent');

const codingAgentAction = Array.from(dom.window.document.querySelectorAll('.sidebar-context-option'))
  .find((button) => button.textContent === 'Coding Agent');
assert.ok(codingAgentAction, 'session-agent menu should include the Coding Agent action');
codingAgentAction.click();
const profileModal = dom.window.document.querySelector('#codingAgentProfileModal');
assert.ok(profileModal?.classList.contains('open'), 'Coding Agent action should open the profile modal');
assert.equal(profileModal?.getAttribute('aria-hidden'), 'false');
assert.equal(
  dom.window.document.querySelector('.sidebar-context-menu'),
  null,
  'Coding Agent action should close the sidebar context menu'
);
assert.match(dom.window.document.querySelector('#profileContextName')?.textContent ?? '', /dev-webpage-ui/);
assert.match(dom.window.document.querySelector('#profileContextProject')?.textContent ?? '', /AgentsCommander_ac/);
assert.match(dom.window.document.querySelector('#profileContextWorkgroup')?.textContent ?? '', /WG-7-DEV-TEAM/);
assert.equal(dom.window.document.querySelectorAll('.provider-card').length, 3);
assert.match(profileModal?.textContent ?? '', /Codex/);
assert.match(profileModal?.textContent ?? '', /Claude Code/);
assert.match(profileModal?.textContent ?? '', /OpenCode/);
assert.equal(dom.window.document.querySelector('#assignedProfileSelect')?.value, 'B');
assert.match(dom.window.document.querySelector('#assignedProfileSelect')?.textContent ?? '', /B - BALANCED/);
assert.equal(dom.window.document.querySelector('#profileModelInput')?.value, 'gpt-5.5');
assert.equal(dom.window.document.querySelector('#profileEffortInput')?.value, 'high');
assert.match(dom.window.document.querySelector('#profileDefaultArgsInput')?.value ?? '', /workspace-write/);
assert.match(dom.window.document.querySelector('#profileArgsInput')?.value ?? '', /--profile b/);

dom.window.document.querySelector('[data-provider-id="opencode"]')?.click();
assert.equal(dom.window.__codingAgentProfileModalState.provider, 'opencode');
assert.equal(dom.window.__codingAgentProfileModalState.requested, 'B');
assert.equal(dom.window.__codingAgentProfileModalState.resolved, 'A');
assert.match(dom.window.document.querySelector('#profileFallbackNotice')?.textContent ?? '', /A remains the immutable final fallback/);
assert.match(dom.window.document.querySelector('[data-resolution-provider="opencode"]')?.textContent ?? '', /B - BALANCED/);
assert.match(dom.window.document.querySelector('[data-resolution-provider="opencode"]')?.textContent ?? '', /A - FULL POWER/);
assert.equal(dom.window.document.querySelector('#profileModelInput')?.value, 'provider/default-large');
assert.match(dom.window.document.querySelector('#profileDefaultArgsInput')?.value ?? '', /opencode\.json/);
assert.match(dom.window.document.querySelector('#profileArgsInput')?.value ?? '', /--profile a/);

const modalField = dom.window.document.querySelector('[data-component="Coding Agent model argument field"]');
const modalFieldInput = dom.window.document.querySelector('#profileModelInput');
assert.ok(modalField, 'modal field capture target should exist');
assert.ok(modalFieldInput, 'modal field input should exist');
modalField.getBoundingClientRect = () => ({
  left: 420,
  top: 492,
  width: 520,
  height: 54,
  right: 940,
  bottom: 546,
  x: 420,
  y: 492,
  toJSON: () => {},
});
dom.window.__componentCaptureLastResult = undefined;
copiedText = '';
const modalContextMenuEvent = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 732,
  clientY: 516,
});
const modalContextMenuAllowed = modalFieldInput.dispatchEvent(modalContextMenuEvent);
assert.equal(modalContextMenuAllowed, false, 'modal right-click should prevent the native context menu');
assert.equal(modalContextMenuEvent.defaultPrevented, true, 'modal contextmenu should be marked defaultPrevented');
assert.equal(dom.window.__componentCaptureLastResult, undefined, 'modal menu open should not immediately capture');
assert.equal(
  dom.window.document.querySelector('.sidebar-context-menu'),
  null,
  'modal right-click should not open the sidebar context menu'
);

const modalMenu = dom.window.document.querySelector('.profile-modal-context-menu');
assert.ok(modalMenu, 'modal right-click should open a modal context menu');
assert.match(modalMenu?.textContent ?? '', /Copy component description/);
assert.match(modalMenu?.textContent ?? '', /Reset to inherited default/);
assert.match(modalMenu?.textContent ?? '', /Manage matrix\/defaults/);
assert.match(modalMenu?.textContent ?? '', /Inspect resolution details/);
assert.match(modalMenu?.textContent ?? '', /Close modal/);
assert.equal(dom.window.__profileModalContextLastMenu.target, modalField);

const modalCopyAction = Array.from(dom.window.document.querySelectorAll('.profile-modal-context-option'))
  .find((button) => button.textContent === 'Copy component description');
assert.ok(modalCopyAction, 'modal menu should include the copy component description action');
modalCopyAction.getBoundingClientRect = () => ({
  left: 704,
  top: 530,
  width: 214,
  height: 32,
  right: 918,
  bottom: 562,
  x: 704,
  y: 530,
  toJSON: () => {},
});
const modalMenuContextEvent = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 752,
  clientY: 544,
});
const modalMenuContextAllowed = modalCopyAction.dispatchEvent(modalMenuContextEvent);
assert.equal(modalMenuContextAllowed, false, 'right-clicking the modal menu itself should still suppress the native menu');
assert.equal(modalMenuContextEvent.defaultPrevented, true);
assert.equal(
  dom.window.__componentCaptureLastResult,
  undefined,
  'right-clicking the modal menu option should not capture the menu button'
);
assert.ok(
  dom.window.document.querySelector('.profile-modal-context-menu'),
  'right-clicking a modal menu option should leave the modal menu available for the click action'
);
modalCopyAction.click();
assert.equal(
  dom.window.document.querySelector('.profile-modal-context-menu'),
  null,
  'modal menu should close after invoking copy component description'
);
const modalCaptureResult = await dom.window.__componentCaptureLastResult;
assert.equal(
  modalCaptureResult.quotedIdentifier,
  '"AgentsCommander Current App / label / Coding Agent model argument field"'
);
assert.equal(modalCaptureResult.component, modalField, 'modal capture should resolve to the original modal component');
assert.notEqual(modalCaptureResult.component, modalCopyAction, 'modal capture must not highlight the context-menu action button');
assert.equal(copiedText, modalCaptureResult.quotedIdentifier, 'modal copy should use the shared capture helper');
const modalToast = dom.window.document.querySelector('.component-capture-toast');
assert.ok(modalToast, 'modal copy should show the component capture toast');
assert.match(modalToast?.textContent ?? '', /"AgentsCommander Current App \/ label \/ Coding Agent model argument field"/);
const modalToastTimers = getTimersByDelay(15000);
assert.equal(modalToastTimers.length, 1, 'modal copy should schedule a 15-second toast removal timer');
const modalHighlight = dom.window.document.querySelector('.component-capture-highlight');
assert.ok(modalHighlight, 'modal copy should show the component capture highlight');
assert.equal(modalHighlight?.style.left, '420px');
assert.equal(modalHighlight?.style.top, '492px');
assert.match(dom.window.getComputedStyle(modalHighlight).animation, /component-capture-blink/);
assert.match(dom.window.getComputedStyle(modalHighlight).animation, /2/);
modalHighlight?.dispatchEvent(new dom.window.Event('animationend', { bubbles: true }));
assert.equal(
  dom.window.document.querySelector('.component-capture-highlight'),
  null,
  'modal capture highlight should remove after its two-blink animation completes'
);
modalToastTimers.forEach(([, timer]) => timer.callback(...timer.args));
activeTimers.clear();
copiedText = '';

const modalHeader = dom.window.document.querySelector('[data-component="Coding Agent profile modal header"]');
assert.ok(modalHeader, 'modal header target should exist');
const modalCloseContextMenu = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 452,
  clientY: 190,
});
const modalCloseContextMenuAllowed = modalHeader.dispatchEvent(modalCloseContextMenu);
assert.equal(modalCloseContextMenuAllowed, false, 'modal close-menu right-click should prevent native context menu');
assert.equal(modalCloseContextMenu.defaultPrevented, true);
const modalCloseAction = Array.from(dom.window.document.querySelectorAll('.profile-modal-context-option'))
  .find((button) => button.textContent === 'Close modal');
assert.ok(modalCloseAction, 'modal menu should include a Close modal action');
modalCloseAction.click();
assert.equal(profileModal?.classList.contains('open'), false, 'Close modal menu action should close the profile modal');

dom.window.openCodingAgentProfileModal(codingAgentTarget.closest('[data-component]'));
assert.equal(profileModal?.classList.contains('open'), true, 'modal should reopen for Save coverage');
dom.window.document.querySelector('#profileSaveButton')?.click();
assert.equal(profileModal?.classList.contains('open'), false, 'Save should close the profile modal');
dom.window.openCodingAgentProfileModal(codingAgentTarget.closest('[data-component]'));
assert.equal(profileModal?.classList.contains('open'), true, 'modal should reopen for Cancel coverage');
dom.window.document.querySelector('#profileCancelButton')?.click();
assert.equal(profileModal?.classList.contains('open'), false, 'Cancel should close the profile modal');
assertNoRuntimeErrors();
dom.window.__componentCaptureLastResult = undefined;
copiedText = '';

const contextMenu = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 1210,
  clientY: 210,
});
const contextMenuAllowed = sidebarCaptureTarget.dispatchEvent(contextMenu);
assert.equal(contextMenuAllowed, false, 'right click should prevent the native context menu path');
assert.equal(contextMenu.defaultPrevented, true, 'contextmenu event should be marked defaultPrevented');
assert.equal(dom.window.__componentCaptureLastResult, undefined, 'sidebar right-click should not immediately capture');

const sidebarMenu = dom.window.document.querySelector('.sidebar-context-menu');
assert.ok(sidebarMenu, 'sidebar right-click should open a custom context menu');
assert.equal(dom.window.document.querySelectorAll('.sidebar-context-menu').length, 1);
assert.match(sidebarMenu?.textContent ?? '', /New Agent/);
assert.match(sidebarMenu?.textContent ?? '', /New Team/);
assert.match(sidebarMenu?.textContent ?? '', /New Workgroup/);
assert.match(sidebarMenu?.textContent ?? '', /Remove Project/);
assert.match(sidebarMenu?.textContent ?? '', /Copy component description/);
assert.equal(dom.window.__sidebarContextLastMenu.kind, 'project-card');
assert.equal(
  dom.window.document.querySelector('.component-capture-toast'),
  null,
  'sidebar menu open should not show the capture toast before copy is chosen'
);

const copyAction = Array.from(dom.window.document.querySelectorAll('.sidebar-context-option'))
  .find((button) => button.textContent === 'Copy component description');
assert.ok(copyAction, 'sidebar menu should include the copy component description action');
copyAction.click();
assert.equal(
  dom.window.document.querySelector('.sidebar-context-menu'),
  null,
  'sidebar menu should close after invoking copy component description'
);
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

const outsideCaptureTarget = dom.window.document.querySelector('[data-component="last prompt panel with copied message request"]');
assert.ok(outsideCaptureTarget, 'outside-sidebar capture target should exist');
outsideCaptureTarget.getBoundingClientRect = () => ({
  left: 18,
  top: 88,
  width: 650,
  height: 76,
  right: 668,
  bottom: 164,
  x: 18,
  y: 88,
  toJSON: () => {},
});
const outsideContextMenu = new dom.window.MouseEvent('contextmenu', {
  bubbles: true,
  cancelable: true,
  clientX: 120,
  clientY: 112,
});
const outsideContextMenuAllowed = outsideCaptureTarget.dispatchEvent(outsideContextMenu);
assert.equal(outsideContextMenuAllowed, false, 'outside right click should also prevent native menu');
assert.equal(outsideContextMenu.defaultPrevented, true);
assert.equal(
  dom.window.document.querySelector('.sidebar-context-menu'),
  null,
  'outside right-click should not open the sidebar context menu'
);
const secondCaptureResult = await dom.window.__componentCaptureLastResult;
assert.equal(
  secondCaptureResult.quotedIdentifier,
  '"AgentsCommander Current App / div / last prompt panel with copied message request"'
);
assert.equal(copiedText, secondCaptureResult.quotedIdentifier, 'outside direct capture should copy quoted identifier');
assert.equal(
  clearedTimers.includes(toastTimers[0][0]),
  true,
  'outside component capture should reset the previous toast timer'
);
assert.equal(
  getTimersByDelay(15000).length,
  1,
  'outside component capture should leave one active 15-second toast timer'
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
