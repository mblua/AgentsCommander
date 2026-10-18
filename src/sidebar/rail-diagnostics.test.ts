// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import { __setTransportForTests, DebugAPI } from "../shared/ipc";
import { setLogLevel } from "../shared/console-capture";
import { emitRailDiagnostic, railToken, railObjectId, railEntity, reconcileRailProjects, reconcileRailGroups, type RailFields } from "./rail-diagnostics";

describe("rail diagnostics immediate private client", () => {
  let restore: () => void;
  let fake: FakeTransport;
  beforeEach(() => {
    fake = new FakeTransport();
    fake.resolve("append_rail_diagnostic", null);
    restore = __setTransportForTests(fake);
    setLogLevel("debug");
    reconcileRailProjects([]);
  });
  afterEach(() => { restore(); setLogLevel("info"); vi.restoreAllMocks(); });

  it("starts one invoke per record synchronously while every IPC stays pending; Trace shares sequence", () => {
    fake.onInvoke("append_rail_diagnostic", () => new Promise(() => {}));
    expect(emitRailDiagnostic("sidebar", "pointerdown", { trusted: false })).toBeUndefined();
    setLogLevel("trace");
    emitRailDiagnostic("store", "reload-start");
    expect(fake.calls).toHaveLength(2);
    const records = fake.calls.map(call => call.args.record as { seq: number; windowId: string });
    expect(records[1].seq).toBe(records[0].seq + 1);
    expect(records[1].windowId).toBe(records[0].windowId);
    expect(records[0].windowId).toMatch(/^[0-9a-f-]{36}$/i);
  });

  it.each(["info", "warn", "error"] as const)("%s creates no token, serialization or IPC", level => {
    setLogLevel(level);
    const stringify = vi.spyOn(JSON, "stringify");
    const uuid = vi.spyOn(crypto, "randomUUID");
    expect(railToken()).toBeUndefined();
    expect(railObjectId({})).toBeUndefined();
    emitRailDiagnostic("sidebar", "click");
    expect(stringify).not.toHaveBeenCalled();
    expect(uuid).not.toHaveBeenCalled();
    expect(fake.calls).toHaveLength(0);
  });

  it("contains rejected promises and synchronous throws without console recursion", async () => {
    const debug = vi.spyOn(console, "debug");
    const error = vi.spyOn(console, "error");
    fake.reject("append_rail_diagnostic", "private failure");
    emitRailDiagnostic("sidebar", "click");
    await Promise.resolve(); await Promise.resolve();
    vi.spyOn(DebugAPI, "appendRailDiagnostic").mockImplementation(() => { throw Error("private"); });
    expect(() => emitRailDiagnostic("sidebar", "click")).not.toThrow();
    expect(debug).not.toHaveBeenCalled(); expect(error).not.toHaveBeenCalled();
  });

  it.each([
    { projectId: 0 }, { projectId: Number.MAX_SAFE_INTEGER + 1 }, { count: -1 }, { durationMs: NaN },
    { pointerId: 2147483648 }, { button: 6 }, { kind: "private-name" }, { trusted: 1 },
    { projectPath: "C:/private/secret" }, { targetTag: "private-tag" },
  ])("drops invalid fields without reflecting input: %j", fields => {
    emitRailDiagnostic("sidebar", "click", fields as RailFields);
    expect(fake.calls).toHaveLength(0);
  });

  it("rejects UTF8 serialization above the cap without truncating JSON", () => {
    const stringify = JSON.stringify;
    vi.spyOn(JSON, "stringify").mockImplementation(value => stringify(value) + "é".repeat(1025));
    emitRailDiagnostic("sidebar", "click");
    expect(fake.calls).toHaveLength(0);
  });

  it("shares live opaque tokens, retires config-only deletion, never recreates on lookup, purges at Info", () => {
    const path = "C:/PRIVATE/secret";
    reconcileRailProjects([path, "survivor"]);
    reconcileRailGroups(path, ["private-group"], true);
    const original = railEntity(path, "custom", "private-group");
    const project = railEntity(path);
    reconcileRailProjects([path, "survivor"]);
    reconcileRailGroups(path, ["private-group"], true);
    expect(railEntity(path, "custom", "private-group")).toEqual(original);
    reconcileRailGroups(path, [], true);
    expect(railEntity(path, "custom", "private-group").groupId).toBeUndefined();
    reconcileRailGroups(path, ["private-group"], true);
    expect(railEntity(path, "custom", "private-group").groupId).not.toBe(original.groupId);
    expect(railEntity(path).projectId).toBe(project.projectId);
    const survivor = railEntity("survivor").projectId;
    setLogLevel("info"); reconcileRailProjects(["survivor"]);
    setLogLevel("debug");
    expect(railEntity(path).projectId).toBeUndefined();
    expect(railEntity("survivor").projectId).toBe(survivor);
    emitRailDiagnostic("store", "task-start", original);
    expect(JSON.stringify(fake.calls)).not.toMatch(/PRIVATE|secret|private-group/);
  });

  it("purges config at Info without allocating tokens and preserves hidden logical entities", () => {
    reconcileRailProjects(["private"]);
    reconcileRailGroups("private", ["secret"], true);
    const old = railEntity("private", "custom", "secret").groupId;
    const all = railEntity("private", "system").groupId;
    const ungrouped = railEntity("private", "ungrouped").groupId;
    expect(old).toBeTypeOf("number");
    const before = railToken()!;
    setLogLevel("info");
    reconcileRailGroups("private", [], false);
    reconcileRailProjects(["private", "new"]);
    expect(fake.calls).toHaveLength(0);
    setLogLevel("debug");
    expect(railToken()).toBe(before + 1);
    expect(railEntity("private", "custom", "secret").groupId).toBeUndefined();
    expect(railEntity("private", "nonstop").groupId).toBeUndefined();
    expect(railEntity("new").projectId).toBeUndefined();
    reconcileRailGroups("private", ["secret"], true);
    expect(railEntity("private", "custom", "secret").groupId).not.toBe(old);
    expect(railEntity("private", "system").groupId).toBe(all);
    expect(railEntity("private", "ungrouped").groupId).toBe(ungrouped);
  });

  it("uses weak object identity and lookup does not allocate", () => {
    const object = {};
    expect(railObjectId(object, false)).toBeUndefined();
    const id = railObjectId(object);
    expect(railObjectId(object)).toBe(id);
    expect(railObjectId({})).not.toBe(id);
  });
});


describe("real browser console capture compatibility", () => {
  it("preserves both browser listeners and their Error-level callback formats", async () => {
    const methods = { log: console.log, info: console.info, warn: console.warn, error: console.error, debug: console.debug, trace: console.trace };
    vi.resetModules();
    const registrations = vi.spyOn(window, "addEventListener");
    let capture: typeof import("../shared/console-capture") | undefined;
    try {
      capture = await import("../shared/console-capture");
      capture.setLogLevel("error");
      const errors = registrations.mock.calls.filter(([type]) => type === "error");
      const rejections = registrations.mock.calls.filter(([type]) => type === "unhandledrejection");
      expect(errors).toHaveLength(1);
      expect(rejections).toHaveLength(1);
      const invoke = (callback: EventListenerOrEventListenerObject, event: Event) => {
        if (typeof callback === "function") callback.call(window, event);
        else callback.handleEvent(event);
      };
      invoke(errors[0][1], new ErrorEvent("error", { message: "browser control", filename: "control.ts", lineno: 7, colno: 9 }));
      const rejection = new Event("unhandledrejection");
      Object.defineProperty(rejection, "reason", { value: "rejection control" });
      invoke(rejections[0][1], rejection);
      expect(capture.getConsoleLogs().map(({ level, args }) => ({ level, args }))).toEqual([
        { level: "error", args: "browser control at control.ts:7:9" },
        { level: "error", args: "Unhandled rejection: rejection control" },
      ]);
    } finally {
      for (const [type, callback, options] of registrations.mock.calls) {
        if (type === "error" || type === "unhandledrejection") window.removeEventListener(type, callback, options);
      }
      capture?.setLogLevel("info");
      registrations.mockRestore();
      Object.assign(console, methods);
      vi.resetModules();
    }
  });
});
