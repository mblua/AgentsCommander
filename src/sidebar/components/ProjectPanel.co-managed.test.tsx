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

// #2232 phase 9 tests 5 to 13 — the per-room Co-managed toggle and its reason
// line, on the orchestrator row of ProjectPanel (the row that already carries
// the phase-8 dot). Every test drives the real panel against a FakeTransport,
// so a call the UI never makes fails here rather than in a running app.

const projectPath = "C:\\Project";
const wgName = "wg-9-comanaged-team";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const ORCHESTRATOR = "orchestrator";
const WORKER = "worker";
const ORCHESTRATOR_SESSION = "session-orchestrator";
const WORKER_SESSION = "session-worker";

const replicaPath = (name: string): string => `${workgroupPath}\\__agent_${name}`;

const rowTestId = (context: string, replica: string): string =>
  `replica.row.${automationIdPart(context)}.${automationIdPart(wgName)}.${automationIdPart(replica)}`;

const coManagedTestId = (
  context: string,
  suffix: "" | ".toggle" | ".reason" | ".error" = ""
): string =>
  `replica.coManaged.${automationIdPart(context)}.${automationIdPart(wgName)}.${automationIdPart(ORCHESTRATOR)}${suffix}`;

function element<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function toggle(root: HTMLElement, context = "workgroups"): HTMLInputElement {
  const control = element<HTMLInputElement>(root, coManagedTestId(context, ".toggle"));
  if (!control) throw new Error(`missing Co-managed toggle: ${coManagedTestId(context, ".toggle")}`);
  return control;
}

function reasonLine(root: HTMLElement, context = "workgroups"): Element | null {
  return element(root, coManagedTestId(context, ".reason"));
}

function errorLine(root: HTMLElement, context = "workgroups"): Element | null {
  return element(root, coManagedTestId(context, ".error"));
}

/** A checkbox interaction: the DOM flips first, exactly like a real click; the
 *  component must be the one that decides whether the flip survives. */
function change(control: HTMLInputElement, checked: boolean): void {
  control.checked = checked;
  control.dispatchEvent(new Event("change", { bubbles: true }));
}

/** One macrotask, so an immediately-resolving fake's continuation has certainly
 *  run. Negative assertions (no reason line) need it to be meaningful. */
const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

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
  effective?: () => CoManagedState;
  setEnabled?: (args: Record<string, unknown>) => CoManagedConfig;
  rejectSetEnabled?: string;
}

async function mountCoManagedPanel(options: MountOptions = {}) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", coManagedDiscovery());
  fake.resolve("co_managed_get", options.config ?? { enabled: false, catalogPath: null });
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
  await waitFor(() => expect(element(rendered.root, rowTestId("workgroups", ORCHESTRATOR))).not.toBeNull());

  sessionsStore.setSessions([
    session({
      id: ORCHESTRATOR_SESSION,
      name: `${wgName}/${ORCHESTRATOR}`,
      workingDirectory: replicaPath(ORCHESTRATOR),
      isCoordinator: true,
      status: "idle",
    }),
    session({
      id: WORKER_SESSION,
      name: `${wgName}/${WORKER}`,
      workingDirectory: replicaPath(WORKER),
      isCoordinator: false,
      status: "idle",
    }),
  ]);

  return { fake, rendered };
}

/** Mount with one pinned reason and wait until its answer has been applied. */
async function mountWithReason(reason: OffReason) {
  const mounted = await mountCoManagedPanel({ effective: () => ({ Off: { reason } }) });
  await waitFor(() =>
    expect(mounted.fake.callsFor("co_managed_effective_state").length).toBeGreaterThan(0)
  );
  await settle();
  return mounted;
}

describe("ProjectPanel Co-managed enablement (#2232 phase 9)", () => {
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

  it("5. the toggle reflects co_managed_get and calls set_enabled with the room root and the negated value", async () => {
    const setCalls: Array<Record<string, unknown>> = [];
    const { fake, rendered } = await mountCoManagedPanel({
      config: { enabled: true, catalogPath: null },
      setEnabled: (args) => {
        setCalls.push(args);
        return { enabled: Boolean(args.enabled), catalogPath: null };
      },
    });
    try {
      const control = toggle(rendered.root);
      await waitFor(() => expect(control.checked).toBe(true));
      expect(control.dataset.acState).toBe("on");

      const afterFirstRead = fake.callsFor("co_managed_effective_state").length;
      change(control, false);
      await waitFor(() => expect(setCalls.length).toBe(1));
      expect(setCalls[0]).toEqual({ roomRoot: workgroupPath, enabled: false });
      await waitFor(() => expect(control.checked).toBe(false));
      // Let the post-toggle re-query land before the next interaction, so the
      // call count below is the click's alone.
      await waitFor(() =>
        expect(fake.callsFor("co_managed_effective_state")).toHaveLength(afterFirstRead + 1)
      );

      change(control, true);
      await waitFor(() => expect(setCalls.length).toBe(2));
      expect(setCalls[1]).toEqual({ roomRoot: workgroupPath, enabled: true });
      await waitFor(() =>
        expect(fake.callsFor("co_managed_effective_state")).toHaveLength(afterFirstRead + 2)
      );
      // Once per click, never on render.
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("6. a failed set_enabled leaves the control at its previous value and shows the error", async () => {
    const { rendered } = await mountCoManagedPanel({
      config: { enabled: false, catalogPath: null },
      rejectSetEnabled: "coManagedDirCreateFailed: room is not writable",
    });
    try {
      const control = toggle(rendered.root);
      await waitFor(() => expect(control.checked).toBe(false));

      change(control, true);
      await waitFor(() =>
        expect(errorLine(rendered.root)?.textContent).toContain("room is not writable")
      );

      // It never flipped: the DOM and the rendered state both stay off, and no
      // reason line is invented for a call that did not land.
      expect(control.checked).toBe(false);
      expect(control.dataset.acState).toBe("off");
      expect(reasonLine(rendered.root)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("7a. Off NotAnOrchestrator renders its line", async () => {
    const { rendered } = await mountWithReason("NotAnOrchestrator");
    try {
      expect(reasonLine(rendered.root)?.textContent).toBe(
        "Only this room's orchestrator can be co-managed."
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("7b. Off UnsupportedProvider renders its line", async () => {
    const { rendered } = await mountWithReason({ UnsupportedProvider: { agent: "antigravity" } });
    try {
      expect(reasonLine(rendered.root)?.textContent).toBe(
        "antigravity has no transcript reader, so nothing can be captured."
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("7c. Off RoomFlagOff renders no reason line", async () => {
    const { rendered } = await mountWithReason("RoomFlagOff");
    try {
      expect(reasonLine(rendered.root)).toBeNull();
      // The control is still there; only the reason is absent.
      expect(toggle(rendered.root)).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("7d. Off NoApiKey renders its line", async () => {
    const { rendered } = await mountWithReason("NoApiKey");
    try {
      expect(reasonLine(rendered.root)?.textContent).toBe("Add a Jev API key in Settings.");
    } finally {
      rendered.cleanup();
    }
  });

  it("7e. Off NoCatalogFile renders its line", async () => {
    const { rendered } = await mountWithReason("NoCatalogFile");
    try {
      expect(reasonLine(rendered.root)?.textContent).toBe(
        "Set a category catalog file for this room."
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("7f. Off CatalogUnreadable renders its line", async () => {
    const { rendered } = await mountWithReason("CatalogUnreadable");
    try {
      expect(reasonLine(rendered.root)?.textContent).toBe(
        "The category catalog could not be read."
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("8. UnsupportedProvider renders the agent name", async () => {
    const { rendered } = await mountWithReason({ UnsupportedProvider: { agent: "pi" } });
    try {
      const line = reasonLine(rendered.root)?.textContent ?? "";
      expect(line).toContain("pi");
      expect(line).toBe("pi has no transcript reader, so nothing can be captured.");
    } finally {
      rendered.cleanup();
    }
  });

  it("9. the toggle is clickable while the state is Off NoApiKey", async () => {
    const { fake, rendered } = await mountWithReason("NoApiKey");
    try {
      await waitFor(() =>
        expect(reasonLine(rendered.root)?.textContent).toBe("Add a Jev API key in Settings.")
      );
      const control = toggle(rendered.root);
      expect(control.disabled).toBe(false);

      const before = fake.callsFor("co_managed_effective_state").length;
      change(control, true);
      await waitFor(() => expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("co_managed_effective_state")).toHaveLength(before + 1)
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("10. rendering a room panel issues no write command", async () => {
    const { fake, rendered } = await mountCoManagedPanel();
    try {
      // Wait for the render-time reads to settle, then prove none of them wrote.
      await waitFor(() => expect(fake.callsFor("co_managed_get").length).toBeGreaterThan(0));
      await waitFor(() =>
        expect(fake.callsFor("co_managed_effective_state").length).toBeGreaterThan(0)
      );
      await settle();
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(0);
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
      const row = element(rendered.root, rowTestId("workgroups", ORCHESTRATOR))!;
      const dot = row.querySelector(".session-item-status")!;
      expect(dot.classList.contains("comanaged")).toBe(false);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");

      const control = toggle(rendered.root);
      await waitFor(() => expect(control.checked).toBe(false));
      change(control, true);
      await waitFor(() => expect(control.checked).toBe(true));
      await settle();

      // Epic 3.6 / phase 7 section 9.2: only a real capture cycle lights this.
      expect(dot.classList.contains("comanaged")).toBe(false);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");
      expect(sessionsStore.comanagedBySessionId[ORCHESTRATOR_SESSION] ?? false).toBe(false);
      expect(fake.callsFor("co_managed_set_enabled")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("12. a successful toggle re-queries effective_state exactly once and updates the reason", async () => {
    let state: CoManagedState = { Off: { reason: "RoomFlagOff" } };
    const { fake, rendered } = await mountCoManagedPanel({
      effective: () => state,
      setEnabled: () => ({ enabled: true, catalogPath: null }),
    });
    try {
      await waitFor(() =>
        expect(fake.callsFor("co_managed_effective_state").length).toBeGreaterThan(0)
      );
      await settle();
      expect(reasonLine(rendered.root)).toBeNull();

      const before = fake.callsFor("co_managed_effective_state").length;
      state = { Off: { reason: "NoApiKey" } };

      const control = toggle(rendered.root);
      await waitFor(() => expect(control.checked).toBe(false));
      change(control, true);
      await waitFor(() =>
        expect(reasonLine(rendered.root)?.textContent).toBe("Add a Jev API key in Settings.")
      );

      expect(fake.callsFor("co_managed_effective_state")).toHaveLength(before + 1);
    } finally {
      rendered.cleanup();
    }
  });

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
        // Phase 2 test 16 pins these literals: feeding them through the wrapper
        // must decode to exactly the value the Rust side serialized.
        const decoded = await CoManagedAPI.effectiveState(workgroupPath, ORCHESTRATOR_SESSION);
        expect(decoded).toEqual(literal);

        await waitFor(() =>
          expect(fake.callsFor("co_managed_effective_state").length).toBeGreaterThan(0)
        );
        await settle();
        if (line === null) {
          expect(reasonLine(rendered.root)).toBeNull();
        } else {
          expect(reasonLine(rendered.root)?.textContent).toBe(line);
        }
      } finally {
        rendered.cleanup();
      }
    }
  });
});
