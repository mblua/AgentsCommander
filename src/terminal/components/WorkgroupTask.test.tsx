// @vitest-environment jsdom
//
// #1614 section 9.1 frontend tests / section 15.4. F6
// (`WorkgroupTask.tsx:74`, `hasWorkgroupContext`) gates the TASK.md Edit and
// Clean buttons. Left unrewired it leaves both buttons permanently disabled in
// every Room, and it fails silently: nothing errors, the buttons are simply
// never clickable.
//
// The gate's case sensitivity is load-bearing and the component says so at
// :72-73: the backend is byte-exact (`session/session.rs:249` now calls
// `has_entity_prefix`), so a case-insensitive UX gate would enable buttons
// whose every click fails. Section 5.4 preserves each call site's exact case
// sensitivity, which is why F6 is a different helper from the rail's F4/F5.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import WorkgroupTask from "./WorkgroupTask";
import { terminalStore } from "../stores/terminal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";

/** The two buttons F6 gates, in render order: Edit then Clean. */
function actionButtons(): HTMLButtonElement[] {
  return Array.from(document.querySelectorAll<HTMLButtonElement>("button.workgroup-task-action"));
}

/** Bind a live session whose cwd is `cwd`, then render the component. */
async function renderWithCwd(cwd: string): Promise<{ cleanup: () => void }> {
  terminalStore.bindLockedSession(
    session({
      id: "session-1614",
      name: "agent",
      workingDirectory: cwd,
      status: "running",
    }),
    0
  );
  const fake = new FakeTransport();
  const rendered = renderWithFakeTransport(() => <WorkgroupTask />, fake);
  await waitFor(() => expect(actionButtons().length).toBe(2));
  return rendered;
}

describe("WorkgroupTask, F6 dual-prefix gate (#1614)", () => {
  beforeEach(() => {
    resetUiStoresForTests();
    terminalStore.resetForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    terminalStore.resetForTests();
    document.body.replaceChildren();
  });

  it("enables the Task buttons in a Room cwd", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\room-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(false);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("still enables the Task buttons in a legacy Workgroup cwd", async () => {
    // Rule P2: the legacy case is kept, not converted. Dual-prefix acceptance
    // is only testable while a wg-* case still exists.
    const rendered = await renderWithCwd("C:\\P\\.ac\\wg-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(false);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("stays disabled for an uppercase ROOM- directory, matching the byte-exact backend", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\ROOM-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("stays disabled for an uppercase WG- directory, exactly as it does today", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\WG-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("stays disabled outside any entity directory", async () => {
    const rendered = await renderWithCwd("C:\\P\\some\\other\\place");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("is not fooled by a directory that merely starts with the prefix letters", async () => {
    const rendered = await renderWithCwd("C:\\P\\.ac\\roomy-1-t\\__agent_x");
    try {
      for (const button of actionButtons()) {
        expect(button.disabled).toBe(true);
      }
    } finally {
      rendered.cleanup();
    }
  });
});
