import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcWorkgroup, WorkgroupGroupsConfig } from "../../shared/types";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  appendExactGroupToken,
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
  nonStopMatchesWorkgroup,
  normalizeNonStop,
  removeExactGroupToken,
  reorderGroups,
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

  it("applies external project group updates", () => {
    workgroupGroupsStore.applyExternalUpdate(
      projectPath,
      config({
        groups: [{ id: "live", name: "Live", regex: "^wg-2-" }],
        showAll: false,
        showUngrouped: true,
      })
    );

    expect(workgroupGroupsStore.config(projectPath)).toMatchObject({
      groups: [{ id: "live", name: "Live", regex: "^wg-2-" }],
      showAll: false,
      showUngrouped: true,
    });
    expect(workgroupGroupsStore.error(projectPath)).toBeNull();
    expect(workgroupGroupsStore.loading(projectPath)).toBe(false);
  });

  it("does not let a stale initial load overwrite an external update", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);

    let resolveLoad!: (value: WorkgroupGroupsConfig) => void;
    const loadPromise = new Promise<WorkgroupGroupsConfig>((resolve) => {
      resolveLoad = resolve;
    });
    fake.onInvoke("get_project_groups", () => loadPromise);

    const load = workgroupGroupsStore.ensureLoaded(projectPath);
    expect(fake.callsFor("get_project_groups")).toHaveLength(1);

    workgroupGroupsStore.applyExternalUpdate(
      projectPath,
      config({ groups: [{ id: "external", name: "External", regex: "^wg-9-" }] })
    );

    resolveLoad(config({ groups: [{ id: "stale", name: "Stale", regex: "^wg-1-" }] }));
    await load;

    expect(workgroupGroupsStore.config(projectPath).groups).toEqual([
      { id: "external", name: "External", regex: "^wg-9-" },
    ]);
    expect(workgroupGroupsStore.loading(projectPath)).toBe(false);
  });

  it("keys external updates by project path", () => {
    const otherProject = "C:\\OtherProject";

    workgroupGroupsStore.applyExternalUpdate(
      projectPath,
      config({ groups: [{ id: "primary", name: "Primary", regex: "^wg-1-" }] })
    );
    workgroupGroupsStore.applyExternalUpdate(
      otherProject,
      config({ groups: [{ id: "other", name: "Other", regex: "^wg-2-" }] })
    );

    expect(workgroupGroupsStore.config(projectPath).groups).toEqual([
      { id: "primary", name: "Primary", regex: "^wg-1-" },
    ]);
    expect(workgroupGroupsStore.config(otherProject).groups).toEqual([
      { id: "other", name: "Other", regex: "^wg-2-" },
    ]);
  });

  it("reorders groups by final insertion index and clamps to the list bounds", () => {
    const groups = [
      { id: "a", name: "A", regex: "a" },
      { id: "b", name: "B", regex: "b" },
      { id: "c", name: "C", regex: "c" },
    ];

    expect(reorderGroups(groups, "a", 2).map((group) => group.id)).toEqual(["b", "c", "a"]);
    expect(reorderGroups(groups, "c", 0).map((group) => group.id)).toEqual(["c", "a", "b"]);
    expect(reorderGroups(groups, "b", 99).map((group) => group.id)).toEqual(["a", "c", "b"]);
    expect(reorderGroups(groups, "missing", 0)).toBe(groups);
  });

  it("persists a reordered group list without changing built-in group settings", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);

    fake.resolve(
      "get_project_groups",
      config({
        showAll: false,
        showUngrouped: true,
        nonStop: { ...defaultNonStop(), show: true },
        groups: [
          { id: "a", name: "A", regex: "a" },
          { id: "b", name: "B", regex: "b" },
          { id: "c", name: "C", regex: "c" },
        ],
      })
    );
    fake.onInvoke("update_project_groups", (args) => args.config);

    await workgroupGroupsStore.ensureLoaded(projectPath);
    await workgroupGroupsStore.reorderGroup(projectPath, "a", 2);

    expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
      showAll: false,
      showUngrouped: true,
      nonStop: { show: true },
      groups: [{ id: "b" }, { id: "c" }, { id: "a" }],
    });
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

  it("removes an exact workgroup token from a generated regex union", () => {
    expect(removeExactGroupToken("^(wg-1-dev-team|wg-2-dev-team)$", "wg-1-dev-team")).toBe(
      exactGroupRegexForWorkgroup("wg-2-dev-team")
    );
    expect(removeExactGroupToken(exactGroupRegexForWorkgroup("wg-1-dev-team"), "wg-1-dev-team")).toBe(
      "(?!)"
    );
  });

  it("does not remove regex-derived custom memberships without an exact token", () => {
    expect(removeExactGroupToken("^wg-1-", "wg-1-dev-team")).toBeNull();
  });

  it("does not report a token as removable when another regex branch would still match", () => {
    expect(removeExactGroupToken("^(wg-1-.*|wg-1-dev-team)$", "wg-1-dev-team")).toBeNull();
  });
});

describe("#777 Non-stop group", () => {
  let restoreTransport: (() => void) | null = null;

  beforeEach(() => {
    restoreTransport = __setTransportForTests(new FakeTransport());
    workgroupGroupsStore.resetForTests();
  });

  afterEach(() => {
    workgroupGroupsStore.resetForTests();
    restoreTransport?.();
    restoreTransport = null;
  });

  const wg = (name: string): AcWorkgroup => ({
    name,
    path: `C:\\Project\\.ac\\${name}`,
    task: null,
    agents: [],
  });

  it("nonStopMatchesWorkgroup matches by regex; no-match / invalid / oversize -> false (no throw)", () => {
    const cfg = { ...defaultNonStop(), regex: "^wg-1-" };
    expect(nonStopMatchesWorkgroup(cfg, wg("wg-1-dev-team"))).toBe(true);
    expect(nonStopMatchesWorkgroup(cfg, wg("wg-2-dev-team"))).toBe(false);
    expect(nonStopMatchesWorkgroup({ ...defaultNonStop(), regex: "(" }, wg("wg-1-dev-team"))).toBe(false);
    expect(nonStopMatchesWorkgroup(cfg, wg("w".repeat(200)))).toBe(false);
  });

  it("normalizeNonStop clamps numerics, trims the name, repairs a bad regex, and keeps null/undefined absent", () => {
    const clamped = normalizeNonStop({
      show: true,
      name: "  Watcher  ",
      regex: "(",
      toleranceSeconds: 0,
      telegram: { enabled: true, botId: "bot-1" },
      sound: { enabled: true, seconds: 999 },
    });
    const legacyDefaultName = normalizeNonStop({ ...defaultNonStop(), name: " Non-stop " });
    expect(clamped).toMatchObject({
      show: true,
      name: "Watcher",
      regex: "(?!)",
      toleranceSeconds: 1,
      telegram: { enabled: true, botId: "bot-1" },
      sound: { enabled: true, seconds: 60 },
    });
    expect(defaultNonStop().name).toBe("Alert me!");
    expect(legacyDefaultName?.name).toBe("Alert me!");
    expect(normalizeNonStop(null)).toBeNull();
    expect(normalizeNonStop(undefined)).toBeUndefined();
  });

  it("clamps a bad persisted nonStop on load and NEVER wipes the user's groups (dev-webpage-ui #4)", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve(
      "get_project_groups",
      config({
        groups: [
          { id: "a", name: "Alpha", regex: "^wg-1-" },
          { id: "b", name: "Beta", regex: "^wg-2-" },
        ],
        nonStop: { ...defaultNonStop(), show: true, toleranceSeconds: 0, sound: { enabled: false, seconds: 999 } },
      })
    );
    await workgroupGroupsStore.ensureLoaded(projectPath);
    const loaded = workgroupGroupsStore.config(projectPath);
    expect(loaded.groups).toHaveLength(2);
    expect(loaded.nonStop).toMatchObject({ toleranceSeconds: 1, sound: { seconds: 60 } });
  });

  it("selecting Non-stop falls back to All when hidden/absent, and sticks when shown", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve("get_project_groups", config({ nonStop: { ...defaultNonStop(), show: false } }));
    await workgroupGroupsStore.ensureLoaded(projectPath);
    workgroupGroupsStore.select(projectPath, { kind: "nonstop" });
    expect(workgroupGroupsStore.selection(projectPath)).toEqual({ kind: "all" });

    workgroupGroupsStore.resetForTests();
    fake.resolve("get_project_groups", config({ nonStop: { ...defaultNonStop(), show: true } }));
    await workgroupGroupsStore.ensureLoaded(projectPath);
    workgroupGroupsStore.select(projectPath, { kind: "nonstop" });
    expect(workgroupGroupsStore.selection(projectPath)).toEqual({ kind: "nonstop" });
  });

  it("addWorkgroupToNonStop materializes the shown slot then appends; removeWorkgroupFromNonStop strips", async () => {
    const fake = new FakeTransport();
    restoreTransport?.();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve("get_project_groups", config({}));
    fake.onInvoke("update_project_groups", (args) => args.config);
    await workgroupGroupsStore.ensureLoaded(projectPath);

    await workgroupGroupsStore.addWorkgroupToNonStop(projectPath, "wg-1-dev-team");
    expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
      nonStop: { show: true, regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
    });

    await workgroupGroupsStore.addWorkgroupToNonStop(projectPath, "wg-2-dev-team");
    expect((fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig).nonStop?.regex).toBe(
      "^(wg-1-dev-team|wg-2-dev-team)$"
    );

    await workgroupGroupsStore.removeWorkgroupFromNonStop(projectPath, "wg-1-dev-team");
    expect((fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig).nonStop?.regex).toBe(
      exactGroupRegexForWorkgroup("wg-2-dev-team")
    );
  });
});
