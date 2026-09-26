// Shared setup for the #2573 restart-failure toast tests (wake, discovery
// menu, session row). Each test renders its component next to ToastHost, makes
// `restart_session` reject with the backend's unresolved-reference string (or
// resolve), triggers the restart, and asserts the toast or its absence.
import type { JSX } from "solid-js";
import { afterEach, beforeEach, expect, it } from "vitest";
import ToastHost from "../../shared/components/ToastHost";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";

export const UNRESOLVED_REJECT =
  "unresolved_coding_agent_reference: 'claude-old' matches no configured coding agent";
export const UNRESOLVED_TEXT =
  "Can't restart this session: its coding agent 'claude-old' is no longer in Settings. Right-click the session, choose Coding Agent, pick one, then restart.";

function toastItem(): HTMLElement | null {
  return document.body.querySelector<HTMLElement>('[data-ac-testid="toast.item"]');
}

async function expectPlainErrorToast(): Promise<void> {
  await waitFor(() => expect(toastItem()).toBeTruthy());
  expect(toastItem()?.getAttribute("data-ac-kind")).toBe("error");
  const text = toastItem()?.textContent ?? "";
  expect(text).toContain(UNRESOLVED_TEXT);
  expect(text).not.toContain("unresolved_coding_agent_reference");
}

async function expectNoToastAfterRestart(fake: FakeTransport): Promise<void> {
  await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
  // Let the resolved restart settle so a late toast would already be rendered.
  await new Promise((r) => setTimeout(r, 0));
  expect(toastItem()).toBeNull();
}

export interface RestartToastCase {
  failName: string;
  okName: string;
  setup: (fake: FakeTransport) => void;
  ui: () => JSX.Element;
  trigger: (root: HTMLElement) => Promise<void>;
}

/** Registers the failure test and its absence leg (a successful restart shows
 *  no toast, so the failure test cannot pass by toasting unconditionally). */
export function describeRestartToast(c: RestartToastCase): void {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  for (const outcome of ["reject", "resolve"] as const) {
    it(outcome === "reject" ? c.failName : c.okName, async () => {
      const fake = new FakeTransport();
      c.setup(fake);
      if (outcome === "reject") fake.reject("restart_session", UNRESOLVED_REJECT);
      else fake.resolve("restart_session", session());
      const rendered = renderWithFakeTransport(
        () => (
          <>
            {c.ui()}
            <ToastHost />
          </>
        ),
        fake,
      );
      try {
        await c.trigger(rendered.root);
        if (outcome === "reject") await expectPlainErrorToast();
        else await expectNoToastAfterRestart(fake);
        expect(fake.callsFor("restart_session")).toHaveLength(1);
      } finally {
        rendered.cleanup();
      }
    });
  }
}
