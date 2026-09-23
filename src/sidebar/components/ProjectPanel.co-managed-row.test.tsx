// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { initialSelection } from "../../shared/testing/session-selection";
import { toastStore } from "../../shared/stores/toasts";
import type { Session, SessionCommunication } from "../../shared/types";

vi.mock("../attention", () => ({ requestTaskbarAttention: vi.fn() }));

// #2452 - the backend publishes communication.kind = "coManaged" with the
// message text (#2232). These tests drive the real
// session_communication_changed event through the fake transport so the store
// handler, not the test, applies it to the row.
const projectPath = "C:\\Project";
const wgName = "room-comanaged";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const sessionId = "comanaged-session";
const updatedAt = "2026-09-23T16:00:00.000Z";
const coManagedMessage = "Prefieres A o B?";

const CO_MANAGED_SELECTOR = "[data-ac-testid$='.communicationSlot.coManaged']";

interface RowShape {
  replicaName: string;
  isCoordinator: boolean;
  taskTitle: string | null;
}

const coordWithTitle: RowShape = {
  replicaName: "orchestrator",
  isCoordinator: true,
  taskTitle: "Co-managed task",
};
const plainNoTitle: RowShape = {
  replicaName: "worker",
  isCoordinator: false,
  taskTitle: null,
};

function replicaPath(shape: RowShape): string {
  return `${workgroupPath}\\__agent_${shape.replicaName}`;
}

function rowSession(shape: RowShape): Session {
  return session({
    id: sessionId,
    name: `${wgName}/${shape.replicaName}`,
    workingDirectory: replicaPath(shape),
    status: "running",
    isCoordinator: shape.isCoordinator,
    communication: null,
  });
}

function setupTransport(fake: FakeTransport, shape: RowShape): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("get_update_status", null);
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      workgroups: [
        {
          name: wgName,
          path: workgroupPath,
          task: null,
          taskTitle: shape.taskTitle,
          agents: [
            {
              name: shape.replicaName,
              path: replicaPath(shape),
              repoPaths: [],
              isCoordinator: shape.isCoordinator,
            },
          ],
        },
      ],
    })
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", [rowSession(shape)]);
  fake.resolve("get_active_session", initialSelection());
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("resolve_blocking_menu", undefined);
  fake.resolve("switch_session", undefined);
}

// Always a fresh object: the store merges key by key (issue 1863).
function communication(
  kind: string,
  visible: boolean,
  message: string | null
): SessionCommunication {
  return { kind, visible, updatedAt, message } as SessionCommunication;
}

describe("ProjectPanel Co-managed row text (#2452)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    toastStore.clear();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    toastStore.clear();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  async function mount(shape: RowShape): Promise<FakeTransport> {
    const fake = new FakeTransport();
    setupTransport(fake, shape);
    rendered = renderWithFakeTransport(() => <SidebarApp />, fake);
    await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));
    await waitFor(() =>
      expect(document.body.querySelector(".ac-discovery-badges")).not.toBeNull()
    );
    return fake;
  }

  function emit(fake: FakeTransport, comm: SessionCommunication | null): void {
    fake.emitFromBackend("session_communication_changed", { sessionId, communication: comm });
  }

  function coManagedElement(): HTMLElement | null {
    return document.body.querySelector<HTMLElement>(CO_MANAGED_SELECTOR);
  }

  // A real-timer flush, so an absence assertion is not satisfied before the
  // event has had a chance to land.
  async function settle(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }

  // Reproduction 1: the message is rendered as TEXT, not only as a title.
  it("renders the Co-managed message as row text", async () => {
    const fake = await mount(coordWithTitle);
    emit(fake, communication("coManaged", true, coManagedMessage));
    await waitFor(() => expect(coManagedElement()?.textContent).toBe(coManagedMessage));
  });

  // Reproduction 2: the same element carries the full message as title.
  it("carries the full message as the element title", async () => {
    const fake = await mount(coordWithTitle);
    emit(fake, communication("coManaged", true, coManagedMessage));
    await waitFor(() => expect(coManagedElement()?.textContent).toBe(coManagedMessage));
    expect(coManagedElement()?.getAttribute("title")).toBe(coManagedMessage);
  });

  // Reproduction 3: no gate, a non-coordinator row with no task title shows it.
  it("shows the text on a non-coordinator row with no task title", async () => {
    const fake = await mount(plainNoTitle);
    emit(fake, communication("coManaged", true, coManagedMessage));
    await waitFor(() => expect(coManagedElement()?.textContent).toBe(coManagedMessage));
  });

  // Guard 4: visible false renders nothing.
  it("renders nothing when visible is false", async () => {
    const fake = await mount(coordWithTitle);
    emit(fake, communication("coManaged", false, coManagedMessage));
    await settle();
    expect(coManagedElement()).toBeNull();
  });

  // Guard 5: a null message renders nothing, even when visible.
  it("renders nothing when the message is null", async () => {
    const fake = await mount(coordWithTitle);
    emit(fake, communication("coManaged", true, null));
    await settle();
    expect(coManagedElement()).toBeNull();
  });

  // Guard 6: the three kinds stay distinguishable on the same row shape.
  it("keeps blockedMenu and raiseHand icons apart from the Co-managed element", async () => {
    const fake = await mount(coordWithTitle);
    const slot = "[data-ac-testid$='.communicationSlot']";

    emit(fake, communication("blockedMenu", true, "Menu"));
    await waitFor(() =>
      expect(document.body.querySelector(`${slot}[data-kind='blockedMenu'] svg`)).not.toBeNull()
    );
    expect(coManagedElement()).toBeNull();

    emit(fake, communication("raiseHand", true, null));
    await waitFor(() =>
      expect(document.body.querySelector(`${slot}[data-kind='raiseHand'] svg`)).not.toBeNull()
    );
    expect(coManagedElement()).toBeNull();
  });

  // Guard 7: the narrow-width keep-list (sidebar.css, obsidian-mesh) spares
  // only .coord-communication-slot. That rule is a [data-sidebar-style]
  // descendant selector jsdom will not resolve, so assert the class token.
  it("carries the coord-communication-slot class", async () => {
    const fake = await mount(coordWithTitle);
    emit(fake, communication("coManaged", true, coManagedMessage));
    await waitFor(() => expect(coManagedElement()).not.toBeNull());
    expect(coManagedElement()!.classList.contains("coord-communication-slot")).toBe(true);
  });

  // Guard 8: the keep-list uses the child combinator, so a wrapped element
  // escapes it while the class token of guard 7 still passes.
  it("is a direct child of .ac-discovery-badges", async () => {
    const fake = await mount(coordWithTitle);
    emit(fake, communication("coManaged", true, coManagedMessage));
    await waitFor(() => expect(coManagedElement()).not.toBeNull());
    expect(coManagedElement()!.parentElement?.matches(".ac-discovery-badges")).toBe(true);
  });
});

describe("Co-managed slot stylesheet rule (#2452)", () => {
  // Same file: URL construction as ProjectPanel.menu-guard.test.tsx; the
  // two-argument URL form resolves against the jsdom document base.
  const marker = "/src/sidebar/components/";
  const markerAt = import.meta.url.indexOf(marker);
  if (markerAt < 0) {
    throw new Error(`cannot locate the repo root in import.meta.url: ${import.meta.url}`);
  }
  const repoRootUrl = import.meta.url.slice(0, markerAt);
  const sidebarCss = readFileSync(
    new URL(`${repoRootUrl}/src/sidebar/styles/sidebar.css`),
    "utf8"
  );

  // Guard 9: the override undoes the 15px icon box and sits after the base slot.
  it("defines the override and places it after the base slot rule", () => {
    const selector = ".coord-communication-slot--co-managed";
    const ruleAt = sidebarCss.indexOf(selector);
    expect(ruleAt).toBeGreaterThan(-1);
    const block = sidebarCss.slice(ruleAt, sidebarCss.indexOf("}", ruleAt));
    for (const decl of [
      "display: block",
      "width: auto",
      "height: auto",
      "min-width: 0",
      "flex: 0 1 auto",
      "white-space: nowrap",
      "overflow: hidden",
      "text-overflow: ellipsis",
    ]) {
      expect(block).toContain(decl);
    }
    // justify-content is dead on a block container; its presence invites the
    // flex display back, which is what clipped the text with no ellipsis.
    expect(block).not.toContain("justify-content");
    expect(ruleAt).toBeGreaterThan(sidebarCss.indexOf(".coord-communication-slot {"));
  });
});
