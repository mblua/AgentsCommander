// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import type { ProjectPathIssue } from "../shared/types";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../shared/testing/ui-harness";
import { initialSelection } from "../shared/testing/session-selection";

const VALID_PROJECT = "C:\\bundle\\projects\\valid";
const VALID_AGENT = `${VALID_PROJECT}\\.ac\\_agent_General`;
const CONFLICT_ABS = "C:\\abs\\alpha";
const CONFLICT_REL = "D:\\rel\\alpha";

function conflictIssue(): ProjectPathIssue {
  return {
    kind: "conflict",
    id: "f".repeat(64),
    source: "projectPaths",
    absoluteCandidate: "C:\\bundle\\projects\\alpha",
    instanceRelativeCandidate: "..\\projects\\alpha",
    absoluteResolvedPath: CONFLICT_ABS,
    instanceRelativeResolvedPath: CONFLICT_REL,
    message: "backend message",
  };
}

/** The startup IPCs the sidebar drives on mount, minus get_settings (each test
 *  sets its own so it can inject the resolution report). */
function wireStartup(fake: FakeTransport): void {
  fake.resolve("open_project", { path: VALID_PROJECT, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      agents: [{ name: "General", path: VALID_AGENT, roleExists: true }],
    })
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", []);
  fake.resolve("get_active_session", initialSelection());
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("drain_session_warnings", []);
  fake.resolve("get_update_status", null);
}

function errorToasts(root: HTMLElement): HTMLElement[] {
  return Array.from(
    root.ownerDocument.querySelectorAll<HTMLElement>('.toast-item[role="alert"]')
  );
}

describe("SidebarApp #1077 project-path conflict", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("loads the valid project and shows one sticky red conflict toast with both paths", async () => {
    const fake = new FakeTransport();
    wireStartup(fake);
    fake.resolve(
      "get_settings",
      settingsSnapshot(
        { projectPaths: [VALID_PROJECT], projectPath: VALID_PROJECT },
        { activeRegistrationCount: 2, issues: [conflictIssue()] }
      )
    );

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      // The valid project loads and renders...
      await waitFor(() => expect(rendered.root.textContent).toContain("General"));

      // ...only the valid project is opened/discovered — never the conflict candidate.
      expect(fake.callsFor("open_project").map((c) => c.args.path)).toEqual([VALID_PROJECT]);
      expect(fake.callsFor("discover_project").map((c) => c.args.path)).toEqual([VALID_PROJECT]);

      // Exactly one sticky red error toast, with both resolved paths on their
      // own labelled lines and a working dismiss button.
      await waitFor(() => expect(errorToasts(rendered.root)).toHaveLength(1));
      const [toast] = errorToasts(rendered.root);
      expect(toast.classList.contains("toast-item--error")).toBe(true);
      const message = toast.querySelector(".toast-item__message")?.textContent ?? "";
      expect(message).toContain("Absolute path:");
      expect(message).toContain(CONFLICT_ABS);
      expect(message).toContain("Instance-relative path:");
      expect(message).toContain(CONFLICT_REL);
      expect(message).toContain("\n");

      const dismiss = toast.querySelector<HTMLButtonElement>(".toast-item__dismiss");
      expect(dismiss).not.toBeNull();
      dismiss?.click();
      await waitFor(() => expect(errorToasts(rendered.root)).toHaveLength(0));
    } finally {
      rendered.cleanup();
    }
  });

  it("makes zero open/discover calls for a conflict-only startup but still toasts", async () => {
    const fake = new FakeTransport();
    wireStartup(fake);
    fake.resolve(
      "get_settings",
      settingsSnapshot({}, { activeRegistrationCount: 1, issues: [conflictIssue()] })
    );

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(errorToasts(rendered.root)).toHaveLength(1));
      expect(fake.callsFor("open_project")).toHaveLength(0);
      expect(fake.callsFor("discover_project")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("surfaces two distinct conflicts as two toasts within the existing cap", async () => {
    const fake = new FakeTransport();
    wireStartup(fake);
    const second: ProjectPathIssue = { ...conflictIssue(), id: "e".repeat(64) };
    fake.resolve(
      "get_settings",
      settingsSnapshot({}, { activeRegistrationCount: 2, issues: [conflictIssue(), second] })
    );

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(errorToasts(rendered.root)).toHaveLength(2));
    } finally {
      rendered.cleanup();
    }
  });

  it("initializes the selected project under a legacy snapshot without a report", async () => {
    const fake = new FakeTransport();
    wireStartup(fake);
    // Legacy-shaped: no projectPathResolution → absent-report legacy fallback.
    fake.resolve(
      "get_settings",
      baseSettings({ projectPaths: [VALID_PROJECT], projectPath: VALID_PROJECT })
    );

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(rendered.root.textContent).toContain("General"));
      expect(errorToasts(rendered.root)).toHaveLength(0);
      expect(fake.callsFor("open_project").map((c) => c.args.path)).toEqual([VALID_PROJECT]);
    } finally {
      rendered.cleanup();
    }
  });

  it("fails closed on a present-but-malformed report: no project calls", async () => {
    const fake = new FakeTransport();
    wireStartup(fake);
    // A present report with an unknown issue kind must fail closed.
    const malformed = {
      ...baseSettings({ projectPaths: [VALID_PROJECT], projectPath: VALID_PROJECT }),
      projectPathResolution: {
        activeRegistrationCount: 1,
        archivedRegistrationCount: 0,
        issues: [{ kind: "bogus", id: "f".repeat(64), source: "projectPaths" }],
        reconciliationError: null,
      },
    };
    fake.resolve("get_settings", malformed);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="project.loadStatus"]')?.getAttribute("data-ac-state")
        ).toBe("error")
      );
      expect(fake.callsFor("open_project")).toHaveLength(0);
      expect(fake.callsFor("discover_project")).toHaveLength(0);
      expect(rendered.root.textContent).not.toContain("General");
    } finally {
      rendered.cleanup();
    }
  });
});
