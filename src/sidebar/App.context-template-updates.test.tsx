// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../shared/testing/ui-harness";
import { projectStore } from "./stores/project";
import { toastStore } from "../shared/stores/toasts";
import type { ContextTemplateUpdate } from "../shared/types";
import { initialSelection } from "../shared/testing/session-selection";

const projectPath = "C:\\Project";

function update(
  overrides: Partial<ContextTemplateUpdate> = {},
): ContextTemplateUpdate {
  return {
    projectPath,
    acRootPath: `${projectPath}\\.ac`,
    filePath: `${projectPath}\\.ac\\Context.coordinator.md`,
    filename: "Context.coordinator.md",
    label: "Coordinator context",
    currentFileSha256: "file-a",
    currentDefaultSha256: "default-2",
    currentDefaultVersion: 2,
    ...overrides,
  };
}

function setupTransport(fake: FakeTransport, updates: ContextTemplateUpdate[]): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("get_update_status", null);
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", discovery({ contextTemplateUpdates: updates }));
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", []);
  fake.resolve("get_active_session", initialSelection());
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

function target<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

describe("SidebarApp context-template update flow (#695)", () => {
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

  it("opens the modal for a pending discovery update", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, [update()]);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(
        () => expect(target("contextTemplateUpdate.modal")).not.toBeNull(),
        2000,
      );
      expect(target("contextTemplateUpdate.modal")!.textContent).toContain(
        "Coordinator context",
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("Keep my version calls keep_custom_context_template and closes the modal", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, [update()]);
    fake.resolve("keep_custom_context_template", null);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(
        () => expect(target("contextTemplateUpdate.keep")).not.toBeNull(),
        2000,
      );
      target<HTMLButtonElement>("contextTemplateUpdate.keep")!.click();

      await waitFor(() => expect(target("contextTemplateUpdate.modal")).toBeNull());
      const call = fake.lastCall("keep_custom_context_template");
      expect(call).toBeDefined();
      expect(call!.args).toMatchObject({
        path: projectPath,
        filename: "Context.coordinator.md",
        currentFileSha256: "file-a",
        currentDefaultSha256: "default-2",
      });
      // The resolved update is removed from the store.
      expect(projectStore.current?.contextTemplateUpdates).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  it("Overwrite with default calls the command, closes the modal, and shows a backup-path toast", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, [update()]);
    const backupPath = "C:\\Project\\.ac\\Context.coordinator.md.bak";
    fake.resolve("overwrite_context_template_with_default", {
      filePath: `${projectPath}\\.ac\\Context.coordinator.md`,
      backupPath,
      currentDefaultSha256: "default-2",
    });
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(
        () => expect(target("contextTemplateUpdate.overwrite")).not.toBeNull(),
        2000,
      );
      target<HTMLButtonElement>("contextTemplateUpdate.overwrite")!.click();

      await waitFor(() => expect(target("contextTemplateUpdate.modal")).toBeNull());
      const call = fake.lastCall("overwrite_context_template_with_default");
      expect(call).toBeDefined();
      expect(call!.args).toMatchObject({
        path: projectPath,
        filename: "Context.coordinator.md",
        currentFileSha256: "file-a",
        currentDefaultSha256: "default-2",
      });
      expect(projectStore.current?.contextTemplateUpdates).toEqual([]);
      // Sticky info toast carries the backup path so the user can find the .bak.
      await waitFor(() =>
        expect(
          toastStore.items.some((t) => t.message.includes(backupPath)),
        ).toBe(true),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the modal open and shows the error when overwrite is rejected", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, [update()]);
    fake.reject(
      "overwrite_context_template_with_default",
      "Context template changed on disk; reload the project before overwriting.",
    );
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(
        () => expect(target("contextTemplateUpdate.overwrite")).not.toBeNull(),
        2000,
      );
      target<HTMLButtonElement>("contextTemplateUpdate.overwrite")!.click();

      await waitFor(() =>
        expect(target("contextTemplateUpdate.error")).not.toBeNull(),
      );
      expect(target("contextTemplateUpdate.error")!.textContent).toContain(
        "changed on disk",
      );
      // The modal stays open and the pending update is still in the store.
      expect(target("contextTemplateUpdate.modal")).not.toBeNull();
      expect(projectStore.current?.contextTemplateUpdates).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  // grinch fix #5: two pending updates for the SAME project/filename/default but
  // different file hashes (the user edited the file again before resolving).
  // Resolving the first must NOT drop the second — both reach the backend.
  it("resolving an older update does not drop a newer pending update for the same file", async () => {
    const older = update({ currentFileSha256: "file-a" });
    const newer = update({ currentFileSha256: "file-b" });
    const fake = new FakeTransport();
    setupTransport(fake, [older, newer]);
    fake.resolve("keep_custom_context_template", null);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      // Two updates queued; the first modal is the older one.
      await waitFor(
        () =>
          expect(projectStore.current?.contextTemplateUpdates).toHaveLength(2),
        2000,
      );
      await waitFor(() =>
        expect(target("contextTemplateUpdate.modal")).not.toBeNull(),
      );

      // Resolve the first (older) update.
      target<HTMLButtonElement>("contextTemplateUpdate.keep")!.click();
      // Store drops exactly the older entry → length 1 (newer survives).
      await waitFor(() =>
        expect(projectStore.current?.contextTemplateUpdates).toHaveLength(1),
      );
      // The createEffect immediately surfaces the newer update — modal re-opens.
      await waitFor(() =>
        expect(target("contextTemplateUpdate.modal")).not.toBeNull(),
      );

      // Resolve the second (newer) update.
      target<HTMLButtonElement>("contextTemplateUpdate.keep")!.click();
      await waitFor(() =>
        expect(projectStore.current?.contextTemplateUpdates).toHaveLength(0),
      );
      await waitFor(() => expect(target("contextTemplateUpdate.modal")).toBeNull());

      // Both file hashes were sent to the backend — the newer update was never
      // silently dropped by resolving the older one.
      const sentFileHashes = fake
        .callsFor("keep_custom_context_template")
        .map((c) => c.args.currentFileSha256);
      expect(sentFileHashes).toEqual(["file-a", "file-b"]);
    } finally {
      rendered.cleanup();
    }
  });
});
