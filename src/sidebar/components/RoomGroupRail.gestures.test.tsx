// @vitest-environment jsdom
// #4695 — pin rail gesture diagnostics edges before they move into rail-diagnostics.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RailDiagnosticRecord } from "../../shared/ipc";
import { setLogLevel } from "../../shared/console-capture";
import { renderWithFakeTransport, resetUiStoresForTests } from "../../shared/testing/ui-harness";
import RoomGroupRail from "./RoomGroupRail";
import {
  createRailProject, createRailRoom, defaultIndicatorRooms, dispatchRailPointer,
  prepareRailDiagnostics, railButtonOf, railDiagnostics,
} from "./room-group-rail-fixtures";

const tracked = ["pointerdown", "pointerup", "mousedown", "click", "window-focus", "window-blur"];

async function withRail(body: (log: () => RailDiagnosticRecord[], node: HTMLElement) => void) {
  const { fake, restore } = await prepareRailDiagnostics();
  const projects = [createRailProject(defaultIndicatorRooms(createRailRoom))];
  const view = renderWithFakeTransport(() => <RoomGroupRail projects={projects} />, fake);
  try {
    const node = await vi.waitFor(() => railButtonOf("Project", "ui"));
    fake.clearCalls();
    body(() => railDiagnostics(fake).filter(entry => tracked.includes(entry.event)), node);
  } finally {
    view.cleanup();
    restore();
  }
}

const ofEvent = (log: RailDiagnosticRecord[], event: string) =>
  log.filter(entry => entry.event === event);

describe("RoomGroupRail gesture diagnostics edges (#4695)", () => {
  afterEach(() => {
    setLogLevel("info");
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("records nothing for press and click when debug logging is off", async () => {
    await withRail((log, node) => {
      setLogLevel("info");
      dispatchRailPointer(node, "pointerdown");
      dispatchRailPointer(node, "click");
      expect(log()).toHaveLength(0);
    });
  });

  it("labels a click without an earlier rail press as no-press", async () => {
    await withRail((log, node) => {
      node.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      const [only, ...rest] = ofEvent(log(), "click");
      expect(rest).toHaveLength(0);
      expect(only.outcome).toBe("no-press");
    });
  });

  it("shares one original node between press and click, then forgets it", async () => {
    await withRail((log, node) => {
      dispatchRailPointer(node, "pointerdown", { pointerId: 4 });
      node.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      const [down] = ofEvent(log(), "pointerdown");
      const [clicked] = ofEvent(log(), "click");
      expect(down.originalNodeId).toBeTypeOf("number");
      expect(clicked.originalNodeId).toBe(down.originalNodeId);
      expect(clicked.outcome).toBeUndefined();
      dispatchRailPointer(node, "pointerup", { pointerId: 4 });
      expect(ofEvent(log(), "pointerup")[0].originalNodeId).toBeUndefined();
    });
  });

  it("emits window blur and focus records only during a held gesture", async () => {
    await withRail((log, node) => {
      const flip = () => {
        window.dispatchEvent(new Event("blur"));
        window.dispatchEvent(new Event("focus"));
      };
      flip();
      expect(log()).toEqual([]);
      dispatchRailPointer(node, "pointerdown", { pointerId: 2 });
      flip();
      const names = log().map(entry => entry.event).filter(name => name.startsWith("window-"));
      expect(names).toEqual(["window-blur", "window-focus"]);
    });
  });

  it("credits a pointerless mousedown to the most recent pointer gesture", async () => {
    await withRail((log, node) => {
      dispatchRailPointer(node, "pointerdown", { pointerId: 9 });
      node.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
      const origin = ofEvent(log(), "pointerdown")[0].originalNodeId;
      const downs = ofEvent(log(), "mousedown");
      expect(downs).toHaveLength(1);
      expect(downs[0].originalNodeId).toBe(origin);
      expect(downs[0].pointerId).toBeUndefined();
    });
  });
});
