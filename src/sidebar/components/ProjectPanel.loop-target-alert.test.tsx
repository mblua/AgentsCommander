// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type { AcLoopSummary, UnresolvedLoopTarget } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { automationIdPart } from "./replica-repo-badges";

const projectPath = "C:\\Project";
const workgroupName = "wg-1-dev-team";
const workgroupPath = `${projectPath}\\.ac\\${workgroupName}`;

function loop(): AcLoopSummary {
  return {
    id: "daily-release",
    name: "Daily release",
    enabled: true,
    expr: "0 8 * * *",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: "room-29-git-npm-expert-team",
    promptPreview: "Short preview",
    busyCoordinator: "skip",
    path: `${projectPath}\\.ac\\_loop_daily-release`,
    configPath: `${projectPath}\\.ac\\_loop_daily-release\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
  };
}

function alert(overrides: Partial<UnresolvedLoopTarget> = {}): UnresolvedLoopTarget {
  return {
    projectPath,
    loopId: "daily-release",
    loopName: "Daily release",
    workgroup: "room-29-git-npm-expert-team",
    error: "Room 'room-29-git-npm-expert-team' not found in project C:\\Project",
    ...overrides,
  };
}

function setupProject(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      teams: [{ name: "dev-team", agents: ["architect"], coordinator: "architect" }],
      workgroups: [
        {
          name: workgroupName,
          path: workgroupPath,
          task: null,
          taskTitle: "Loop targets",
          teamName: "dev-team",
          agents: [
            {
              name: "architect",
              path: `${workgroupPath}\\__agent_architect`,
              repoPaths: [],
              isCoordinator: true,
            },
          ],
        },
      ],
      loops: [loop()],
    }),
  );
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

describe("ProjectPanel Loop target notice (#2171)", () => {
  let cleanupDom: (() => void) | null = null;
  let warn: ReturnType<typeof vi.spyOn> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    warn?.mockRestore();
    warn = null;
    vi.restoreAllMocks();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("shows the notice when the backend reports an unresolved target", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.resolve("list_unresolved_loop_targets", [alert()]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(byTestId("loopTargetMissing.modal")).toBeTruthy());
      expect(byTestId("loopTargetMissing.modal")!.textContent).toContain(alert().error);
    } finally {
      rendered.cleanup();
    }
  });

  it("opens EditLoopModal for that Loop and closes the notice", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.resolve("list_unresolved_loop_targets", [alert()]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(byTestId("loopTargetMissing.open.daily-release")).toBeTruthy());

      click(byTestId("loopTargetMissing.open.daily-release")!);

      await waitFor(() => expect(byTestId("loop.edit.save")).toBeTruthy());
      expect(byTestId("loopTargetMissing.modal")).toBeNull();
      expect(byTestId<HTMLInputElement>("loop.edit.id")!.value).toBe("daily-release");
    } finally {
      rendered.cleanup();
    }
  });

  it("renders no notice when the backend reports no unresolved target", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.resolve("list_unresolved_loop_targets", []);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() =>
        expect(fake.calls.some((c) => c.cmd === "list_unresolved_loop_targets")).toBe(true),
      );
      expect(byTestId("loopTargetMissing.modal")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("renders no notice and still mounts the panel when the command rejects", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.reject("list_unresolved_loop_targets", "command not found");

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() =>
        expect(fake.calls.some((c) => c.cmd === "list_unresolved_loop_targets")).toBe(true),
      );
      expect(byTestId("loopTargetMissing.modal")).toBeNull();
      expect(rendered.root.textContent).toContain("Daily release");
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the notice open with a row error when the alert's project is not loaded", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.resolve("list_unresolved_loop_targets", [
      alert({ projectPath: "C:\\Other", loopId: "other-loop", loopName: "Other loop" }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(byTestId("loopTargetMissing.open.other-loop")).toBeTruthy());

      click(byTestId("loopTargetMissing.open.other-loop")!);

      await waitFor(() => expect(document.querySelector(".new-agent-error")).toBeTruthy());
      expect(byTestId("loopTargetMissing.modal")).toBeTruthy();
      expect(document.querySelector(".new-agent-error")!.textContent).toContain(
        "not open in the sidebar",
      );
      expect(byTestId("loop.edit.save")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the notice open with a row error when reloadProject rejects", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.resolve("list_unresolved_loop_targets", [
      alert({ projectPath: "C:\\Other", loopId: "other-loop", loopName: "Other loop" }),
    ]);
    // projectStore.reloadProject swallows discover failures internally
    // (project.ts:568-600), so the rejection is stubbed at the store.
    const reload = vi
      .spyOn(projectStore, "reloadProject")
      .mockRejectedValue(new Error("reload failed"));
    const unhandled = vi.fn();
    window.addEventListener("unhandledrejection", unhandled);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(byTestId("loopTargetMissing.open.other-loop")).toBeTruthy());

      click(byTestId("loopTargetMissing.open.other-loop")!);

      await waitFor(() => expect(document.querySelector(".new-agent-error")).toBeTruthy());
      expect(reload).toHaveBeenCalledWith("C:\\Other");
      expect(byTestId("loopTargetMissing.modal")).toBeTruthy();
      expect(document.querySelector(".new-agent-error")!.textContent).toContain(
        "Could not open this Loop's configuration",
      );
      expect(byTestId("loop.edit.save")).toBeNull();
      expect(unhandled).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("unhandledrejection", unhandled);
      rendered.cleanup();
    }
  });

  it("does not surface an error written after the notice was dismissed mid-flight", async () => {
    const fake = new FakeTransport();
    setupProject(fake);
    fake.resolve("list_unresolved_loop_targets", [
      alert({ projectPath: "C:\\Other", loopId: "other-loop", loopName: "Other loop" }),
    ]);
    let rejectReload!: (reason?: unknown) => void;
    vi.spyOn(projectStore, "reloadProject").mockReturnValue(
      new Promise<void>((_resolve, reject) => {
        rejectReload = reject;
      }),
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(byTestId("loopTargetMissing.open.other-loop")).toBeTruthy());

      // Open attempt in flight, then dismissed before the reload settles.
      click(byTestId("loopTargetMissing.open.other-loop")!);
      click(byTestId("loopTargetMissing.dismiss")!);
      await waitFor(() => expect(byTestId("loopTargetMissing.modal")).toBeNull());

      rejectReload(new Error("reload failed"));
      await waitFor(() =>
        expect(
          warn!.mock.calls.some((call: unknown[]) =>
            String(call[0]).includes("failed to open the Loop configuration"),
          ),
        ).toBe(true),
      );

      // The notice comes back on the next refresh (EditLoopModal close) and must
      // not carry the error line written after the dismissal.
      const projectId = automationIdPart(projectPath);
      const loopId = automationIdPart("daily-release");
      const row = rendered.root.querySelector(`[data-ac-testid="loop.row.${projectId}.${loopId}"]`);
      if (!(row instanceof HTMLElement)) throw new Error("Loop row not found");
      contextMenu(row);
      await waitFor(() =>
        expect(byTestId(`loop.action.edit.${projectId}.${loopId}`)).toBeTruthy(),
      );
      click(byTestId(`loop.action.edit.${projectId}.${loopId}`)!);
      await waitFor(() => expect(byTestId("loop.edit.cancel")).toBeTruthy());
      click(byTestId("loop.edit.cancel")!);

      await waitFor(() => expect(byTestId("loopTargetMissing.modal")).toBeTruthy());
      expect(document.querySelector(".new-agent-error")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
