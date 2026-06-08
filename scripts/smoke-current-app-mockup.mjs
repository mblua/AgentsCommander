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
const cssBlockForInMedia = (mediaQuery, selector) => {
  const escapedMediaQuery = mediaQuery.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const blockPattern = new RegExp(`@media\\s*${escapedMediaQuery}\\s*\\{[\\s\\S]*?${escapedSelector}\\s*\\{(?<body>[^}]*)\\}`);
  return html.match(blockPattern)?.groups?.body ?? '';
};
const assertNoRuntimeErrors = () => {
  assert.equal(
    runtimeErrors.length,
    0,
    `prototype should not throw runtime errors: ${runtimeErrors.map((error) => error?.stack ?? error).join('\n')}`
  );
};
const stubRect = (element, rect) => {
  element.getBoundingClientRect = () => ({
    ...rect,
    right: rect.left + rect.width,
    bottom: rect.top + rect.height,
    x: rect.left,
    y: rect.top,
    toJSON: () => {},
  });
};
const containsPoint = (rect, x, y) => x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
const isBrowserVisible = (element) => {
  for (let node = element; node instanceof dom.window.Element; node = node.parentElement) {
    const style = dom.window.getComputedStyle(node);
    if (
      style.display === 'none' ||
      style.visibility === 'hidden' ||
      style.pointerEvents === 'none' ||
      node.getAttribute('aria-hidden') === 'true'
    ) {
      return false;
    }
  }
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
};
const modalStackRank = (element) => {
  const overlay = dom.window.document.getElementById('codingAgentProfileModal');
  const overlayZ = Number(dom.window.getComputedStyle(overlay).zIndex);
  if (overlay?.classList.contains('open') && overlay.contains(element)) {
    let depth = 0;
    for (let node = element; node && node !== overlay; node = node.parentElement) {
      depth += 1;
    }
    return overlayZ + depth;
  }
  return Number(dom.window.getComputedStyle(element).zIndex) || 0;
};
const topClickableElementAt = (x, y) => {
  const candidates = [
    ...dom.window.document.querySelectorAll('.profile-modal-overlay.open, .profile-modal, .profile-modal-controls, .modal-variant-switcher, [data-modal-variant]'),
  ].filter((element) => isBrowserVisible(element) && containsPoint(element.getBoundingClientRect(), x, y));
  return candidates.sort((left, right) => modalStackRank(right) - modalStackRank(left))[0] ?? null;
};
const clickVisibleModalVariant = (variant) => {
  const overlay = dom.window.document.getElementById('codingAgentProfileModal');
  const modal = overlay?.querySelector('.profile-modal');
  const button = dom.window.document.querySelector(`[data-modal-variant="${variant}"]`);
  assert.ok(overlay?.classList.contains('open'), 'profile modal should be open before switching variants');
  assert.ok(button, `variant ${variant} button should exist`);
  assert.ok(modal?.contains(button), `variant ${variant} button should render inside the profile modal layer`);
  assert.ok(isBrowserVisible(button), `variant ${variant} button should be browser-visible`);

  const rect = button.getBoundingClientRect();
  const clientX = rect.left + rect.width / 2;
  const clientY = rect.top + rect.height / 2;
  assert.equal(
    topClickableElementAt(clientX, clientY),
    button,
    `variant ${variant} button should be the top clickable target at its center`
  );

  button.dispatchEvent(new dom.window.MouseEvent('pointerdown', { bubbles: true, clientX, clientY }));
  button.dispatchEvent(new dom.window.MouseEvent('mousedown', { bubbles: true, clientX, clientY }));
  button.dispatchEvent(new dom.window.MouseEvent('mouseup', { bubbles: true, clientX, clientY }));
  button.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true, clientX, clientY }));
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
assert.ok(dom.window.document.querySelector('[data-component="Coding Agents selector panel"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agents selector title"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding agent selector"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal variant switcher"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal variant A layout"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal variant B layout"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal variant C layout"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile selector panel"]'));
assert.ok(dom.window.document.querySelector('[data-component="Selected Coding Agent available profile cards"]'));
assert.ok(dom.window.document.querySelector('[data-component="Selected profile launch parameter summary"]'));
assert.ok(dom.window.document.querySelector('[data-component="Selected profile projected parameters panel"]'));
assert.ok(dom.window.document.querySelector('[data-component="Coding Agent profile modal footer actions"]'));
assert.match(html, /Copy component description/);
assert.match(html, /Coding Agents/);
assert.doesNotMatch(html, /data-component="Coding agent provider selector title"/);
assert.doesNotMatch(html, /data-component="Coding agent provider selector"/);
assert.doesNotMatch(html, /data-component="Coding agent tool selector panel"/);
assert.match(html, /A remains the immutable final fallback/);
assert.match(html, /Set profile as new default/);
assert.match(html, /Set just for instance/);
assert.doesNotMatch(html, /Gemini/);
assert.doesNotMatch(html, /Priority/);
assert.doesNotMatch(html, /configurable fallback selector/i);
assert.doesNotMatch(html, /<select[\s>]/i, 'current app modal should not use dropdown selectors for profile/model choices');
const sideScrollCss = cssBlockFor('.side-scroll');
assert.match(sideScrollCss, /overflow-y:\s*auto;/, 'sidebar content should allow vertical scrolling');
assert.match(sideScrollCss, /overflow-x:\s*hidden;/, 'sidebar content should keep horizontal overflow hidden');
const profileModalBodyCss = cssBlockFor('.profile-modal-body');
assert.match(profileModalBodyCss, /overflow:\s*hidden;/, 'profile modal body should not drag all Variant C columns while scrolling profiles');
const variantCScrollCss = cssBlockFor('.profile-variant-c-scroll');
assert.match(variantCScrollCss, /grid-column:\s*2\s*\/\s*-1;/, 'Variant C scroll area should span both remaining outer grid columns');
assert.match(variantCScrollCss, /overflow-y:\s*auto;/, 'Variant C profile area should own vertical scrolling');
assert.match(variantCScrollCss, /overflow-x:\s*hidden;/, 'Variant C profile area should not introduce horizontal scrolling');
const variantCProfileSelectorCss = cssBlockFor('.profile-variant-c [data-component="Coding Agent profile selector panel"]');
assert.match(
  variantCProfileSelectorCss,
  /overflow-x:\s*auto;/,
  'Variant C should scope horizontal scrolling to the Coding Agent profile selector panel'
);
const variantCProfileSelectorListCss = cssBlockFor('.profile-variant-c [data-component="Coding Agent profile selector panel"] .profile-picker-list');
assert.match(
  variantCProfileSelectorListCss,
  /min-width:\s*360px;/,
  'Variant C profile selector list should be wide enough to create local horizontal overflow when constrained'
);
const variantCProjectedPanelCss = cssBlockFor('.profile-variant-c [data-component="Selected profile projected parameters panel"]');
assert.match(
  variantCProjectedPanelCss,
  /overflow-x:\s*hidden;/,
  'Variant C projected parameters panel should not expose a horizontal scrollbar'
);
assert.match(
  variantCProjectedPanelCss,
  /overflow-wrap:\s*anywhere;/,
  'Variant C projected parameters panel should wrap long launch parameter text instead of scrolling sideways'
);
const variantCMobileScrollCss = cssBlockForInMedia('(max-width: 860px)', '.profile-variant-c-scroll');
assert.match(variantCMobileScrollCss, /grid-column:\s*1\s*\/\s*-1;/, 'Variant C mobile scroll area should reset to the single-column grid span');
assert.match(variantCMobileScrollCss, /grid-template-columns:\s*1fr;/, 'Variant C mobile scroll area should stack profile/details panels in one column');
assert.doesNotMatch(
  variantCMobileScrollCss,
  /grid-column:\s*2\s*\/\s*-1;/,
  'Variant C mobile scroll area should not keep the desktop two-column span'
);
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

const codingAgentTarget = dom.window.document.querySelector('[data-component="agent row architect"] .agent-name');
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
assert.match(dom.window.document.querySelector('#profileContextName')?.textContent ?? '', /architect/);
assert.match(dom.window.document.querySelector('#profileContextProject')?.textContent ?? '', /AgentsCommander_ac/);
assert.match(dom.window.document.querySelector('#profileContextWorkgroup')?.textContent ?? '', /WG-7-DEV-TEAM/);
assert.equal(dom.window.document.querySelectorAll('[data-tool-card]').length, 9);
assert.match(profileModal?.textContent ?? '', /Codex/);
assert.match(profileModal?.textContent ?? '', /Claude Code/);
assert.match(profileModal?.textContent ?? '', /OpenCode/);
assert.equal(dom.window.__codingAgentProfileModalState.variant, 'A');
assert.equal(dom.window.__codingAgentProfileModalState.provider, 'codex');
assert.equal(dom.window.__codingAgentProfileModalState.requested, 'B');
assert.equal(dom.window.__codingAgentProfileModalState.resolved, 'B');
assert.equal(dom.window.__codingAgentProfileModalState.configuredDefaultAgent, 'architect');
assert.equal(dom.window.__codingAgentProfileModalState.configuredDefaultProfile, 'B');
assert.match(profileModal?.textContent ?? '', /architect configured default B - BALANCED/);
assert.match(profileModal?.textContent ?? '', /Requested\/default B - BALANCED -> resolved B - BALANCED as configured/);

stubRect(profileModal, { left: 0, top: 0, width: 1365, height: 768 });
stubRect(profileModal.querySelector('.profile-modal'), { left: 192, top: 48, width: 980, height: 672 });
stubRect(profileModal.querySelector('.profile-modal-controls'), { left: 686, top: 61, width: 470, height: 46 });
stubRect(profileModal.querySelector('.modal-variant-switcher'), { left: 686, top: 65, width: 178, height: 38 });
['A', 'B', 'C'].forEach((variant, index) => {
  stubRect(dom.window.document.querySelector(`[data-modal-variant="${variant}"]`), {
    left: 792 + index * 41,
    top: 70,
    width: 34,
    height: 28,
  });
});

const modalActionLabels = Array.from(dom.window.document.querySelectorAll('.profile-modal-footer button'), (button) => button.textContent);
assert.deepEqual(modalActionLabels, ['Cancel', 'Set profile as new default', 'Set just for instance']);

for (const variant of ['A', 'B', 'C']) {
  clickVisibleModalVariant(variant);
  assert.equal(dom.window.__codingAgentProfileModalState.variant, variant, `variant ${variant} should be selected`);
  const activeVariant = dom.window.document.querySelector(`[data-profile-variant-panel="${variant}"]`);
  assert.ok(activeVariant?.classList.contains('active'), `variant ${variant} panel should be active`);
  assert.ok(activeVariant.querySelector('[data-component="Coding Agents selector title"]'), `variant ${variant} should expose Coding Agents title`);
  assert.ok(activeVariant.querySelector('[data-component="Coding agent selector"]'), `variant ${variant} should expose a Coding Agent selector`);
  assert.ok(activeVariant.querySelector('[data-component="Coding Agent profile selector panel"]'), `variant ${variant} should expose Profiles area`);
  assert.ok(activeVariant.querySelector('[data-profile-card].default'), `variant ${variant} should show the current default profile marker`);
  assert.match(activeVariant.textContent ?? '', /Model/);
  assert.match(activeVariant.textContent ?? '', /Effort/);
  assert.match(activeVariant.textContent ?? '', /Default args/);
  assert.match(activeVariant.textContent ?? '', /Profile args/);
}

clickVisibleModalVariant('C');
const variantCPanel = dom.window.document.querySelector('[data-profile-variant-panel="C"]');
const variantCCodingAgentsPanel = variantCPanel?.querySelector('[data-component="Coding Agents selector panel"]');
const variantCProfileScroll = variantCPanel?.querySelector('[data-component="Coding Agent profile selector independent scroll area"]');
const variantCProfilePanel = variantCPanel?.querySelector('[data-component="Coding Agent profile selector panel"]');
const variantCProjectedPanel = variantCPanel?.querySelector('[data-component="Selected profile projected parameters panel"]');
assert.ok(variantCCodingAgentsPanel, 'Variant C should expose a stable left Coding Agents selector panel');
assert.ok(variantCProfileScroll, 'Variant C should expose an independent profiles scroll area');
assert.ok(variantCProfilePanel, 'Variant C should keep the profile selector panel inside the scroll area');
assert.ok(variantCProjectedPanel, 'Variant C should expose projected parameters inside the independent scroll area');
assert.equal(variantCProfileScroll.contains(variantCProfilePanel), true, 'Variant C profile selector should be inside the independent scroll area');
assert.equal(variantCProfileScroll.contains(variantCProjectedPanel), true, 'Variant C projected parameters should be inside the independent scroll area');
assert.equal(variantCProfileScroll.contains(variantCCodingAgentsPanel), false, 'Variant C Coding Agents selector should sit outside the profile scroll area');
stubRect(variantCProfileScroll, { left: 394, top: 114, width: 764, height: 526 });
stubRect(variantCProfilePanel, { left: 394, top: 114, width: 370, height: 526 });
stubRect(variantCProjectedPanel, { left: 788, top: 114, width: 370, height: 526 });
assert.equal(
  variantCProfileScroll.getBoundingClientRect().right >= variantCProjectedPanel.getBoundingClientRect().right,
  true,
  'Variant C scroll area should cover the projected parameters column without horizontal clipping'
);
Object.defineProperties(variantCProfileScroll, {
  clientHeight: { configurable: true, value: 360 },
  scrollHeight: { configurable: true, value: 720 },
});
Object.defineProperties(variantCCodingAgentsPanel, {
  clientHeight: { configurable: true, value: 190 },
  scrollHeight: { configurable: true, value: 190 },
});
Object.defineProperties(variantCProfilePanel, {
  clientWidth: { configurable: true, value: 300 },
  scrollWidth: { configurable: true, value: 360 },
});
Object.defineProperties(variantCProjectedPanel, {
  clientWidth: { configurable: true, value: 370 },
  scrollWidth: { configurable: true, value: 370 },
});
variantCCodingAgentsPanel.scrollTop = 0;
variantCProfileScroll.scrollTop = variantCProfileScroll.scrollHeight - variantCProfileScroll.clientHeight;
assert.equal(variantCProfileScroll.scrollTop > 0, true, 'Variant C profile scroll area should accept a positive scrollTop');
assert.equal(variantCCodingAgentsPanel.scrollTop, 0, 'Variant C Coding Agents selector should remain stable when profiles scroll');
variantCProfilePanel.scrollLeft = variantCProfilePanel.scrollWidth - variantCProfilePanel.clientWidth;
assert.equal(variantCProfilePanel.scrollWidth > variantCProfilePanel.clientWidth, true, 'Variant C profile selector should allow local horizontal overflow');
assert.equal(variantCProfilePanel.scrollLeft > 0, true, 'Variant C profile selector panel should accept a positive scrollLeft');
assert.equal(
  ['auto', 'scroll'].includes(dom.window.getComputedStyle(variantCProfilePanel).overflowX),
  true,
  'Variant C profile selector panel should expose horizontal scroll behavior'
);
assert.equal(
  variantCProjectedPanel.scrollWidth <= variantCProjectedPanel.clientWidth,
  true,
  'Variant C projected parameters panel should fit the tested layout without horizontal overflow'
);
assert.equal(
  ['auto', 'scroll'].includes(dom.window.getComputedStyle(variantCProjectedPanel).overflowX),
  false,
  'Variant C projected parameters panel should not expose horizontal scroll behavior'
);

clickVisibleModalVariant('A');
dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-tool-card="claude-code"]')?.click();
assert.equal(dom.window.__codingAgentProfileModalState.provider, 'claude-code');
assert.equal(dom.window.__codingAgentProfileModalState.requested, 'B');
assert.equal(dom.window.__codingAgentProfileModalState.resolved, 'B');
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-resolution-status]')?.textContent ?? '', /Selected coding agent: Claude Code/);
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-resolution-status]')?.textContent ?? '', /Requested\/default B - BALANCED -> resolved B - BALANCED as configured/);

dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-tool-card="opencode"]')?.click();
assert.equal(dom.window.__codingAgentProfileModalState.provider, 'opencode');
assert.equal(dom.window.__codingAgentProfileModalState.requested, 'B');
assert.equal(dom.window.__codingAgentProfileModalState.resolved, 'A');
const opencodeMissingDefaultCards = dom.window.document.querySelectorAll('[data-profile-variant-panel] [data-profile-card="B"]');
assert.equal(opencodeMissingDefaultCards.length, 3, 'each variant should render the missing OpenCode default B card');
for (const card of opencodeMissingDefaultCards) {
  const text = card.textContent ?? '';
  assert.match(text, /Default/);
  assert.match(text, /missing; launches A - FULL POWER/);
  assert.match(text, /Unavailable for OpenCode/);
  assert.match(text, /Fallback B -> A/);
  assert.equal(card.querySelectorAll('[data-profile-param]').length, 0, 'missing OpenCode B card should not render concrete parameter rows');
  assert.doesNotMatch(text, /Model/);
  assert.doesNotMatch(text, /Effort/);
  assert.doesNotMatch(text, /Default args/);
  assert.doesNotMatch(text, /Profile args/);
  assert.doesNotMatch(text, /provider\/default-large/);
  assert.doesNotMatch(text, /opencode\.json/);
  assert.doesNotMatch(text, /--profile a/);
}
const opencodeResolvedCards = dom.window.document.querySelectorAll('[data-profile-variant-panel] [data-profile-card="A"]');
assert.equal(opencodeResolvedCards.length, 3, 'each variant should render the resolved OpenCode A card');
for (const card of opencodeResolvedCards) {
  const text = card.textContent ?? '';
  assert.match(text, /Model/);
  assert.match(text, /Effort/);
  assert.match(text, /Default args/);
  assert.match(text, /Profile args/);
  assert.match(text, /provider\/default-large/);
  assert.match(text, /opencode\.json/);
  assert.match(text, /--profile a/);
}
for (const summary of dom.window.document.querySelectorAll('[data-active-profile-summary]')) {
  const text = summary.textContent ?? '';
  assert.match(text, /OpenCode \/ B - BALANCED/);
  assert.match(text, /provider\/default-large/);
  assert.match(text, /opencode\.json/);
  assert.match(text, /--profile a/);
  assert.match(text, /B missing; A final fallback/);
}
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-resolution-status]')?.textContent ?? '', /Selected coding agent: OpenCode/);
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-resolution-status]')?.textContent ?? '', /Available profiles: A - FULL POWER, C - REVIEW/);
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-resolution-status]')?.textContent ?? '', /Requested\/default B - BALANCED -> resolved A - FULL POWER via fallback/);
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-fallback-notice]')?.textContent ?? '', /architect requests B - BALANCED by default/);
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-fallback-notice]')?.textContent ?? '', /OpenCode has no B - BALANCED profile/);
assert.match(dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-fallback-notice]')?.textContent ?? '', /A remains the immutable final fallback/);

const modalProfileCard = dom.window.document.querySelector('[data-profile-variant-panel="A"] [data-profile-card="B"]');
assert.ok(modalProfileCard, 'modal profile card capture target should exist');
modalProfileCard.getBoundingClientRect = () => ({
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
const modalContextMenuAllowed = modalProfileCard.dispatchEvent(modalContextMenuEvent);
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
const profileModalOverlayZIndex = Number(dom.window.getComputedStyle(profileModal).zIndex);
const profileModalMenuZIndex = Number(dom.window.getComputedStyle(modalMenu).zIndex);
assert.match(modalMenu?.textContent ?? '', /Copy component description/);
assert.match(modalMenu?.textContent ?? '', /Reset to inherited default/);
assert.match(modalMenu?.textContent ?? '', /Manage matrix\/defaults/);
assert.match(modalMenu?.textContent ?? '', /Inspect resolution details/);
assert.match(modalMenu?.textContent ?? '', /Close modal/);
assert.equal(dom.window.__profileModalContextLastMenu.target, modalProfileCard);

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
  '"AgentsCommander Current App / button / OpenCode B - BALANCED profile selector card"'
);
assert.equal(modalCaptureResult.component, modalProfileCard, 'modal capture should resolve to the original modal component');
assert.notEqual(modalCaptureResult.component, modalCopyAction, 'modal capture must not highlight the context-menu action button');
assert.equal(copiedText, modalCaptureResult.quotedIdentifier, 'modal copy should use the shared capture helper');
const modalToast = dom.window.document.querySelector('.component-capture-toast');
assert.ok(modalToast, 'modal copy should show the component capture toast');
assert.match(modalToast?.textContent ?? '', /"AgentsCommander Current App \/ button \/ OpenCode B - BALANCED profile selector card"/);
assert.ok(
  Number(dom.window.getComputedStyle(modalToast).zIndex) > profileModalOverlayZIndex,
  'modal capture toast should render above the open profile modal overlay'
);
assert.ok(
  Number(dom.window.getComputedStyle(modalToast).zIndex) > profileModalMenuZIndex,
  'modal capture toast should render above the profile modal context menu layer'
);
const modalToastTimers = getTimersByDelay(15000);
assert.equal(modalToastTimers.length, 1, 'modal copy should schedule a 15-second toast removal timer');
const modalHighlight = dom.window.document.querySelector('.component-capture-highlight');
assert.ok(modalHighlight, 'modal copy should show the component capture highlight');
assert.equal(modalHighlight?.style.left, '420px');
assert.equal(modalHighlight?.style.top, '492px');
assert.ok(
  Number(dom.window.getComputedStyle(modalHighlight).zIndex) > profileModalOverlayZIndex,
  'modal capture highlight should render above the open profile modal overlay'
);
assert.ok(
  Number(dom.window.getComputedStyle(modalHighlight).zIndex) > profileModalMenuZIndex,
  'modal capture highlight should render above the profile modal context menu layer'
);
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
assert.equal(profileModal?.classList.contains('open'), true, 'modal should reopen for set-default coverage');
dom.window.document.querySelector('#profileDefaultButton')?.click();
assert.equal(profileModal?.classList.contains('open'), false, 'Set profile as new default should close the profile modal');
dom.window.openCodingAgentProfileModal(codingAgentTarget.closest('[data-component]'));
assert.equal(profileModal?.classList.contains('open'), true, 'modal should reopen for instance coverage');
dom.window.document.querySelector('#profileInstanceButton')?.click();
assert.equal(profileModal?.classList.contains('open'), false, 'Set just for instance should close the profile modal');
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
