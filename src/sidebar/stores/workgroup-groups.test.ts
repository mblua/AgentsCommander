import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { WorkgroupGroupsConfig } from "../../shared/types";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  appendExactGroupToken,
  defaultGroupsConfig,
  exactGroupRegexForWorkgroup,
  validateGroupsConfig,
  workgroupGroupsStore,
} from "./workgroup-groups";

const projectPath = "C:\\Project";

function config(overrides: Partial<WorkgroupGroupsConfig> = {}): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    ...overrides,
    groups: overrides.groups ?? [],
  };
}

describe("workgroupGroupsStore", () => {
  let restoreTransport: (() => void) | null = null;

  beforeEach(() => {
    const fake = new FakeTransport();
    restoreTransport = __setTransportForTests(fake);
    workgroupGroupsStore.resetForTests();
  });

  afterEach(() => {
    workgroupGroupsStore.resetForTests();
    restoreTransport?.();
    restoreTransport = null;
  });

  it("deduplicates concurrent loads for equivalent project paths", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);

    let resolveLoad!: (value: WorkgroupGroupsConfig) => void;
    const loadPromise = new Promise<WorkgroupGroupsConfig>((resolve) => {
      resolveLoad = resolve;
    });
    fake.onInvoke("get_project_groups", () => loadPromise);

    const first = workgroupGroupsStore.ensureLoaded("C:\\Project\\");
    const second = workgroupGroupsStore.ensureLoaded("c:/project");
    expect(fake.callsFor("get_project_groups")).toHaveLength(1);

    resolveLoad?.(
      config({
        groups: [{ id: "ui", name: "UI", regex: "^wg-1-" }],
      })
    );
    await Promise.all([first, second]);

    expect(workgroupGroupsStore.config(projectPath).groups).toEqual([
      { id: "ui", name: "UI", regex: "^wg-1-" },
    ]);
  });

  it("preserves the previous config and exposes the error when a save fails", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);

    fake.resolve(
      "get_project_groups",
      config({ groups: [{ id: "a", name: "Alpha", regex: "^wg-1-" }] })
    );
    fake.reject("update_project_groups", "disk denied");

    await workgroupGroupsStore.ensureLoaded(projectPath);
    await expect(
      workgroupGroupsStore.save(
        projectPath,
        config({ groups: [{ id: "b", name: "Beta", regex: "^wg-2-" }] })
      )
    ).rejects.toThrow("disk denied");

    expect(workgroupGroupsStore.config(projectPath).groups).toEqual([
      { id: "a", name: "Alpha", regex: "^wg-1-" },
    ]);
    expect(workgroupGroupsStore.error(projectPath)).toBe("disk denied");
  });

  it("does not let a stale initial load overwrite a completed save", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);

    let resolveLoad!: (value: WorkgroupGroupsConfig) => void;
    const loadPromise = new Promise<WorkgroupGroupsConfig>((resolve) => {
      resolveLoad = resolve;
    });
    fake.onInvoke("get_project_groups", () => loadPromise);
    fake.onInvoke("update_project_groups", (args) => args.config);

    const load = workgroupGroupsStore.ensureLoaded(projectPath);
    await workgroupGroupsStore.save(
      projectPath,
      config({ groups: [{ id: "saved", name: "Saved", regex: "^wg-9-" }] })
    );
    resolveLoad(config({ groups: [{ id: "stale", name: "Stale", regex: "^wg-1-" }] }));
    await load;

    expect(workgroupGroupsStore.config(projectPath).groups).toEqual([
      { id: "saved", name: "Saved", regex: "^wg-9-" },
    ]);
  });

  it("adds an exact workgroup token to an existing generated regex", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);

    fake.resolve(
      "get_project_groups",
      config({
        groups: [
          {
            id: "frontend",
            name: "Frontend",
            regex: exactGroupRegexForWorkgroup("wg-1-dev-team"),
          },
        ],
      })
    );
    fake.onInvoke("update_project_groups", (args) => args.config);

    await workgroupGroupsStore.ensureLoaded(projectPath);
    await workgroupGroupsStore.addWorkgroupToGroup(
      projectPath,
      "frontend",
      "wg-2-dev-team"
    );

    expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
      groups: [
        {
          id: "frontend",
          regex: "^(wg-1-dev-team|wg-2-dev-team)$",
        },
      ],
    });
  });
});

describe("workgroup group validation helpers", () => {
  it("rejects duplicate names, blank toggles, and invalid regex syntax", () => {
    expect(
      validateGroupsConfig(
        config({
          showAll: false,
          showUngrouped: false,
          groups: [
            { id: "a", name: "UI", regex: "^wg-1-" },
            { id: "b", name: " ui ", regex: "(" },
          ],
        }),
        { validateRegexSyntax: true }
      )
    ).toEqual([
      "At least one of All or Ungrouped must be visible.",
      "Duplicate group name.",
      "Group 2: regex is invalid.",
    ]);
  });

  it("returns null when appending to an invalid regex", () => {
    expect(appendExactGroupToken("(", "wg-2-dev-team")).toBeNull();
  });
});
