// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { discovery } from "../../shared/testing/ui-harness";
import { projectStore } from "./project";
import type { ContextTemplateUpdate } from "../../shared/types";

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

let restore: (() => void) | null = null;

function findProject(path: string) {
  return projectStore.projects.find((p) => p.path === path);
}

describe("projectStore context-template updates (#695)", () => {
  beforeEach(() => {
    projectStore.clear();
  });

  afterEach(() => {
    restore?.();
    restore = null;
    projectStore.clear();
  });

  it("loadProject stores contextTemplateUpdates from discovery", async () => {
    const fake = new FakeTransport();
    const pending = update();
    fake.resolve("open_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discovery({ contextTemplateUpdates: [pending] }));
    restore = __setTransportForTests(fake);

    await projectStore.loadProject(projectPath);

    expect(projectStore.current?.contextTemplateUpdates).toEqual([pending]);
  });

  it("reloadProject replaces contextTemplateUpdates with the new discovery result", async () => {
    const fake = new FakeTransport();
    const first = update({ currentFileSha256: "file-a" });
    fake.resolve("open_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discovery({ contextTemplateUpdates: [first] }));
    restore = __setTransportForTests(fake);

    await projectStore.loadProject(projectPath);
    expect(projectStore.current?.contextTemplateUpdates).toEqual([first]);

    // A later discovery (e.g. the user edited the file again, or resolved one)
    // returns a different pending set; reload must replace, not merge.
    const second = update({ currentFileSha256: "file-b", currentDefaultSha256: "default-3" });
    fake.resolve("discover_project", discovery({ contextTemplateUpdates: [second] }));
    await projectStore.reloadProject(projectStore.current!.path);

    expect(projectStore.current?.contextTemplateUpdates).toEqual([second]);
  });

  it("removeContextTemplateUpdate removes only the exact (project, filename, default, file) match", async () => {
    const projectA = "C:\\ProjectA";
    const projectB = "C:\\ProjectB";

    // Project A: the modal being resolved (file-a) plus a NEWER pending update
    // for the same filename+default but a different file hash (the user edited
    // the template again before resolving). The newer one must survive.
    const aRemove = update({ projectPath: projectA, currentFileSha256: "file-a" });
    const aKeepNewer = update({ projectPath: projectA, currentFileSha256: "file-b" });
    // Project B: an update whose key shape is identical to aRemove but in a
    // different project. Must survive (removal is project-scoped).
    const bSameKeyShape = update({ projectPath: projectB, currentFileSha256: "file-a" });

    const fake = new FakeTransport();
    fake.onInvoke("open_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      if (args.path === projectA) {
        return discovery({ contextTemplateUpdates: [aRemove, aKeepNewer] });
      }
      return discovery({ contextTemplateUpdates: [bSameKeyShape] });
    });
    restore = __setTransportForTests(fake);

    await projectStore.loadProject(projectA);
    await projectStore.loadProject(projectB);
    expect(findProject(projectA)?.contextTemplateUpdates).toHaveLength(2);
    expect(findProject(projectB)?.contextTemplateUpdates).toHaveLength(1);

    projectStore.removeContextTemplateUpdate(
      aRemove.projectPath,
      aRemove.filename,
      aRemove.currentDefaultSha256,
      aRemove.currentFileSha256,
    );

    const remainingA = findProject(projectA)?.contextTemplateUpdates ?? [];
    expect(remainingA).toHaveLength(1);
    // grinch fix #5: the newer edit (different file hash) must NOT be dropped.
    expect(remainingA).toContainEqual(aKeepNewer);
    expect(remainingA).not.toContainEqual(aRemove);
    // Project scoping: B's identical-key-shape update is untouched.
    expect(findProject(projectB)?.contextTemplateUpdates).toEqual([bSameKeyShape]);
  });

  it("removeContextTemplateUpdate is a no-op when filename/default/file do not all match", async () => {
    const fake = new FakeTransport();
    const pending = update({ currentFileSha256: "file-a", currentDefaultSha256: "default-2" });
    fake.resolve("open_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discovery({ contextTemplateUpdates: [pending] }));
    restore = __setTransportForTests(fake);

    await projectStore.loadProject(projectPath);

    // Same filename+default, but a stale file hash → keep (this is exactly the
    // scenario the exact-key guard protects against).
    projectStore.removeContextTemplateUpdate(
      projectPath,
      "Context.coordinator.md",
      "default-2",
      "stale-file-hash",
    );
    expect(projectStore.current?.contextTemplateUpdates).toEqual([pending]);
  });
});
