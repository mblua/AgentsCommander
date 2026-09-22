// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
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
import { CoManagedAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { automationIdPart } from "./replica-repo-badges";
import type { CoManagedConfig, CoManagedState, OffReason } from "../../shared/types";

// #2232 phase 9 / #2408 - per-room Co-managed enablement is a check item in the
// orchestrator's replica context menu (active and inactive). Every test drives
// the real panel against a FakeTransport, so a call the UI never makes fails
// here rather than in a running app.

const projectPath = "C:\\Project";
const wgName = "wg-9-comanaged-team";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const ORCHESTRATOR = "orchestrator";
const WORKER = "worker";
const ORCHESTRATOR_SESSION = "session-orchestrator";
const WORKER_SESSION = "session-worker";

type MenuKind = "active" | "inactive";

// The removed in-row control's class prefix, split so a repo-wide grep for it
// finds no survivor here.
const REMOVED_ROW_CONTROL = `[class*="${["replica", "comanaged"].join("-")}"]`;

const replicaPath = (name: string): string => `${workgroupPath}\\__agent_${name}`;

const rowTestId = (replica: string): string =>
  `replica.row.workgroups.${automationIdPart(wgName)}.${automationIdPart(replica)}`;

const coManagedTestId = (kind: MenuKind, suffix: "" | ".reason" | ".error" = ""): string =>
  `replica.coManaged.${kind}.${automationIdPart(wgName)}.${automationIdPart(ORCHESTRATOR)}${suffix}`;

function byTestId<T extends Element = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function row(replica: string): HTMLElement {
  const el = byTestId<HTMLElement>(rowTestId(replica));
  if (!el) throw new Error(`missing row: ${replica}`);
  return el;
}

function openMenu(replica = ORCHESTRATOR): void {
  row(replica).dispatchEvent(
    new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })
  );
}

function menuEl(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".session-context-menu");
}

function item(kind: MenuKind = "active"): HTMLButtonElement {
  const el = byTestId<HTMLButtonElement>(coManagedTestId(kind));
  if (!el) throw new Error(`missing Co-managed item: ${coManagedTestId(kind)}`);
  return el;
}

const checkOf = (el: HTMLElement): string =>
  el.querySelector(".session-context-option-check")?.textContent ?? "<none>";

const reasonLine = (kind: MenuKind = "active") => byTestId(coManagedTestId(kind, ".reason"));
const errorLine = (kind: MenuKind = "active") => byTestId(coManagedTestId(kind, ".error"));

/** Wait until the open-time read has settled: the item is known and enabled or
 *  its blocker is rendered. */
async function openAndSettle(kind: MenuKind = "active", replica = ORCHESTRATOR) {
  openMenu(replica);
  await waitFor(() => expect(item(kind).getAttribute("aria-pressed")).not.toBeNull());
  await settle();
}

/** One macrotask, so an immediately-resolving fake's continuation has certainly
 *  run. Negative assertions (no reason line) need it to be meaningful. */
const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

const coManagedDiscovery = () =>
  discovery({
    workgroups: [
      {
        name: wgName,
        path: workgroupPath,
        task: null,
        taskTitle: "Co-managed enablement",
        agents: [
          {
            name: ORCHESTRATOR,
            path: replicaPath(ORCHESTRATOR),
            repoPaths: [],
            isCoordinator: true,
          },
          { name: WORKER, path: replicaPath(WORKER), repoPaths: [], isCoordinator: false },
        ],
      },
    ],
  });

interface MountOptions {
  config?: CoManagedConfig;
  get?: (args: Record<string, unknown>) => CoManagedConfig | Promise<CoManagedConfig>;
  effective?: () => CoManagedState;
  setEnabled?: (args: Record<string, unknown>) => CoManagedConfig | Promise<CoManagedConfig>;
  rejectSetEnabled?: string;
  /** Leave the orchestrator without a session so its menu is the inactive one. */
  inactive?: boolean;
}

async function mountCoManagedPanel(options: MountOptions = {}) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", coManagedDiscovery());
  const config = options.config ?? { enabled: false, catalogPath: null };
  fake.onInvoke("co_managed_get", (args) => options.get?.(args) ?? config);
  const effective = options.effective ?? (() => ({ Off: { reason: "RoomFlagOff" } } as CoManagedState));
  fake.onInvoke("co_managed_effective_state", () => effective());
  if (options.rejectSetEnabled !== undefined) {
    fake.reject("co_managed_set_enabled", options.rejectSetEnabled);
  } else {
    fake.onInvoke(
      "co_managed_set_enabled",
      (args) =>
        options.setEnabled?.(args) ?? { enabled: Boolean(args.enabled), catalogPath: null }
    );
  }

  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(byTestId(rowTestId(ORCHESTRATOR))).not.toBeNull());

  const sessions = [
    session({
      id: WORKER_SESSION,
      name: `${wgName}/${WORKER}`,
      workingDirectory: replicaPath(WORKER),
      isCoordinator: false,
      status: "idle",
    }),
  ];
  if (!options.inactive) {
    sessions.unshift(
      session({
        id: ORCHESTRATOR_SESSION,
        name: `${wgName}/${ORCHESTRATOR}`,
        workingDirectory: replicaPath(ORCHESTRATOR),
        isCoordinator: true,
        status: "idle",
      })
    );
  }
  sessionsStore.setSessions(sessions);

  return { fake, rendered };
}

/** Mount with one pinned reason, open the active menu, and wait for the reads. */
async function mountWithReason(reason: OffReason, config?: CoManagedConfig) {
  const mounted = await mountCoManagedPanel({ config, effective: () => ({ Off: { reason } }) });
  await openAndSettle();
  await waitFor(() =>
    expect(mounted.fake.callsFor("co_managed_effective_state").length).toBeGreaterThan(0)
  );
  await settle();
  return mounted;
}

describe("ProjectPanel Co-managed menu entry (#2232 phase 9, #2408)", () => {
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

  it("1. the entry exists only in the orchestrator's active and inactive menus; the row has no control", async () => {
    const active = await mountCoManagedPanel();
    try {
      // No in-row markup, before or after any menu opens.
      expect(document.querySelector(REMOVED_ROW_CONTROL)).toBeNull();
      expect(document.querySelector('[data-ac-testid^="replica.coManaged."]')).toBeNull();

      openMenu(WORKER);
      await waitFor(() => expect(menuEl()).not.toBeNull());
      await settle();
      expect(menuEl()!.querySelector('[data-ac-testid^="replica.coManaged."]')).toBeNull();
      expect(menuEl()!.textContent).not.toContain("Co-managed");

      await openAndSettle("active");
      expect(item("active").textContent).toContain("Co-managed");
      expect(item("active").classList.contains("session-context-option")).toBe(true);
      expect(byTestId(coManagedTestId("inactive"))).toBeNull();
      expect(document.querySelector(REMOVED_ROW_CONTROL)).toBeNull();
    } finally {
      active.rendered.cleanup();
    }
    document.body.replaceChildren();
    resetUiStoresForTests();

    const inactive = await mountCoManagedPanel({ inactive: true });
    try {
      await openAndSettle("inactive");
      expect(item("inactive").textContent).toContain("Co-managed");
      expect(byTestId(coManagedTestId("active"))).toBeNull();
    } finally {
      inactive.rendered.cleanup();
    }
  });

  it("2. the check reflects co_managed_get; one click sends the room root and the negated value", async () => {
    const setCalls: Array<Record<string, unknown>> = [];
    const { fake, rendered } = await mountCoManagedPanel({
      config: { enabled: true, catalogPath: null },
      setEnabled: (args) => {
        setCalls.push(args);
        return { enabled: Boolean(args.enabled), catalogPath: null };
      },
    });
    try {
      await openAndSettle();
      await waitFor(() => expect(checkOf(item())).toBe("\u2713"));
      expect(item().getAttribute("aria-pressed")).toBe("true");

      const afterRead = fake.callsFor("co_managed_effective_state").length;
      item().click();
      await waitFor(() => expect(setCalls.length).toBe(1));
      expect(setCalls[0]).toEqual({ roomRoot: workgroupPath, enabled: false });
      await waitFor(() => expect(checkOf(item())).toBe(""));
      expect(item().getAttribute("aria-pressed")).toBe("false");
      await waitFor(() =>
        expect(fake.callsFor("co_managed_effective_state")).toHaveLength(afterRead + 1)
      );
      // The menu stays open so the check updates in place.
      expect(menuEl()).not.toBeNull();

      await waitFor(() => expect(item().disabled).toBe(false));
      item().click();
      await waitFor(() => expect(setCalls.length).toBe(2));
      expect(setCalls[1]).toEqual({ roomRoot: workgroupPath, enabled: true });
      await waitFor(() => expect(checkOf(item())).toBe("\u2713"));
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("3. rendering the panel and opening menus issue no write", async () => {
    const { fake, rendered } = await mountCoManagedPanel();
    try {
      await settle();
      // No read before a menu opens either: the row no longer owns the flag.
      expect(fake.callsFor("co_managed_get")).toHaveLength(0);
      await openAndSettle();
      openMenu(WORKER);
      await settle();
      expect(fake.callsFor("co_managed_get")).toHaveLength(1);
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("4. an inactive orchestrator reads the flag and toggles without any effective-state call", async () => {
    const { fake, rendered } = await mountCoManagedPanel({
      inactive: true,
      config: { enabled: false, catalogPath: null },
    });
    try {
      await openAndSettle("inactive");
      expect(checkOf(item("inactive"))).toBe("");
      expect(item("inactive").disabled).toBe(false);
      item("inactive").click();
      await waitFor(() => expect(checkOf(item("inactive"))).toBe("\u2713"));
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(1);
      expect(fake.callsFor("co_managed_effective_state")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("5. a successful toggle re-queries effective_state exactly once and updates the note", async () => {
    let state: CoManagedState = { Off: { reason: "RoomFlagOff" } };
    const { fake, rendered } = await mountCoManagedPanel({
      effective: () => state,
      setEnabled: () => ({ enabled: true, catalogPath: null }),
    });
    try {
      await openAndSettle();
      expect(reasonLine()).toBeNull();
      const before = fake.callsFor("co_managed_effective_state").length;
      state = { Off: { reason: "NoApiKey" } };

      item().click();
      await waitFor(() =>
        expect(reasonLine()?.textContent).toBe("Add a Jev API key in Settings.")
      );
      expect(reasonLine()?.classList.contains("session-context-note")).toBe(true);
      expect(fake.callsFor("co_managed_effective_state")).toHaveLength(before + 1);
    } finally {
      rendered.cleanup();
    }
  });

  it("6. a failed write keeps the previous check and shows .session-context-error", async () => {
    const { rendered } = await mountCoManagedPanel({
      config: { enabled: false, catalogPath: null },
      rejectSetEnabled: "coManagedDirCreateFailed: room is not writable",
    });
    try {
      await openAndSettle();
      item().click();
      await waitFor(() => expect(errorLine()?.textContent).toContain("room is not writable"));
      expect(errorLine()?.classList.contains("session-context-error")).toBe(true);
      expect(checkOf(item())).toBe("");
      expect(item().getAttribute("aria-pressed")).toBe("false");
      expect(item().disabled).toBe(false);
      expect(reasonLine()).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("7. an off room with a visible blocker cannot be activated", async () => {
    const { fake, rendered } = await mountWithReason({ UnsupportedProvider: { agent: "pi" } });
    try {
      expect(reasonLine()?.textContent).toBe(
        "pi has no transcript reader, so nothing can be captured."
      );
      expect(item().disabled).toBe(true);
      expect(item().classList.contains("context-option-disabled")).toBe(true);
      item().click();
      await settle();
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("8. an enabled but ineffective room can still be turned off, with its note", async () => {
    const { fake, rendered } = await mountWithReason("NoApiKey", { enabled: true, catalogPath: null });
    try {
      expect(reasonLine()?.textContent).toBe("Add a Jev API key in Settings.");
      expect(item().disabled).toBe(false);
      item().click();
      await waitFor(() => expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(1));
      expect(fake.callsFor("co_managed_set_enabled")[0].args).toEqual({
        roomRoot: workgroupPath,
        enabled: false,
      });
      await waitFor(() => expect(checkOf(item())).toBe(""));
    } finally {
      rendered.cleanup();
    }
  });

  it("9. the item is disabled while the initial read and while a write are pending", async () => {
    const read = deferred<CoManagedConfig>();
    const write = deferred<CoManagedConfig>();
    const { fake, rendered } = await mountCoManagedPanel({
      get: () => read.promise,
      setEnabled: () => write.promise,
    });
    try {
      openMenu();
      await waitFor(() => expect(item().disabled).toBe(true));
      item().click();
      await settle();
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(0);

      read.resolve({ enabled: false, catalogPath: null });
      await waitFor(() => expect(item().disabled).toBe(false));
      item().click();
      await waitFor(() => expect(item().disabled).toBe(true));
      item().click();
      await settle();
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(1);

      write.resolve({ enabled: true, catalogPath: null });
      await waitFor(() => expect(checkOf(item())).toBe("\u2713"));
      await waitFor(() => expect(item().disabled).toBe(false));
    } finally {
      rendered.cleanup();
    }
  });

  it("10. a close or A-to-B replacement ignores stale read and write completions", async () => {
    const reads: Array<ReturnType<typeof deferred<CoManagedConfig>>> = [];
    const writes: Array<ReturnType<typeof deferred<CoManagedConfig>>> = [];
    const { rendered } = await mountCoManagedPanel({
      get: () => {
        const d = deferred<CoManagedConfig>();
        reads.push(d);
        return d.promise;
      },
      setEnabled: () => {
        const d = deferred<CoManagedConfig>();
        writes.push(d);
        return d.promise;
      },
    });
    try {
      // Stale read after A-to-B: menu A's late answer never reaches menu B.
      openMenu();
      await waitFor(() => expect(reads).toHaveLength(1));
      openMenu();
      await waitFor(() => expect(reads).toHaveLength(2));
      reads[1].resolve({ enabled: false, catalogPath: null });
      await waitFor(() => expect(item().getAttribute("aria-pressed")).toBe("false"));
      reads[0].resolve({ enabled: true, catalogPath: null });
      await settle();
      await settle();
      expect(checkOf(item())).toBe("");

      // Stale write after close: reopening shows the fresh read, not the late write.
      item().click();
      await waitFor(() => expect(writes).toHaveLength(1));
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      await waitFor(() => expect(menuEl()).toBeNull());
      openMenu();
      await waitFor(() => expect(reads).toHaveLength(3));
      writes[0].resolve({ enabled: true, catalogPath: null });
      await settle();
      await settle();
      expect(item().disabled).toBe(true);
      expect(checkOf(item())).toBe("");
      reads[2].resolve({ enabled: false, catalogPath: null });
      await waitFor(() => expect(item().disabled).toBe(false));
      expect(checkOf(item())).toBe("");
      expect(errorLine()).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("11. turning the flag on does not light the dot and does not touch comanagedBySessionId", async () => {
    const { fake, rendered } = await mountCoManagedPanel({
      config: { enabled: false, catalogPath: null },
      setEnabled: () => ({ enabled: true, catalogPath: null }),
    });
    try {
      const dot = row(ORCHESTRATOR).querySelector(".session-item-status")!;
      expect(dot.classList.contains("comanaged")).toBe(false);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");

      await openAndSettle();
      item().click();
      await waitFor(() => expect(checkOf(item())).toBe("\u2713"));
      await settle();

      // Only a real capture cycle lights the ring.
      expect(dot.classList.contains("comanaged")).toBe(false);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");
      expect(sessionsStore.comanagedBySessionId[ORCHESTRATOR_SESSION] ?? false).toBe(false);
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  const REASON_LINES: Array<{ name: string; reason: OffReason; line: string | null }> = [
    { name: "NotAnOrchestrator", reason: "NotAnOrchestrator", line: "Only this room's orchestrator can be co-managed." },
    { name: "UnsupportedProvider", reason: { UnsupportedProvider: { agent: "antigravity" } }, line: "antigravity has no transcript reader, so nothing can be captured." },
    { name: "RoomFlagOff", reason: "RoomFlagOff", line: null },
    { name: "NoApiKey", reason: "NoApiKey", line: "Add a Jev API key in Settings." },
    { name: "NoCatalogFile", reason: "NoCatalogFile", line: "Set a category catalog file for this room." },
    { name: "CatalogUnreadable", reason: "CatalogUnreadable", line: "The category catalog could not be read." },
  ];

  for (const { name, reason, line } of REASON_LINES) {
    it(`12. Off ${name} renders ${line === null ? "no note" : "its .session-context-note"}`, async () => {
      const { rendered } = await mountWithReason(reason, { enabled: true, catalogPath: null });
      try {
        if (line === null) {
          expect(reasonLine()).toBeNull();
          expect(item()).toBeTruthy();
        } else {
          expect(reasonLine()?.textContent).toBe(line);
          expect(reasonLine()?.classList.contains("session-context-note")).toBe(true);
        }
      } finally {
        rendered.cleanup();
      }
    });
  }

  it("13. the three pinned serde literals decode through the wrapper and render", async () => {
    const cases: Array<{ literal: CoManagedState; line: string | null }> = [
      { literal: "Ready", line: null },
      { literal: { Off: { reason: "RoomFlagOff" } }, line: null },
      {
        literal: { Off: { reason: { UnsupportedProvider: { agent: "pi" } } } },
        line: "pi has no transcript reader, so nothing can be captured.",
      },
    ];

    for (const { literal, line } of cases) {
      const { fake, rendered } = await mountCoManagedPanel({ effective: () => literal });
      try {
        const decoded = await CoManagedAPI.effectiveState(workgroupPath, ORCHESTRATOR_SESSION);
        expect(decoded).toEqual(literal);

        await openAndSettle();
        await waitFor(() =>
          expect(fake.callsFor("co_managed_effective_state").length).toBeGreaterThan(1)
        );
        await settle();
        if (line === null) {
          expect(reasonLine()).toBeNull();
        } else {
          expect(reasonLine()?.textContent).toBe(line);
        }
      } finally {
        rendered.cleanup();
        document.body.replaceChildren();
      }
    }
  });
});
