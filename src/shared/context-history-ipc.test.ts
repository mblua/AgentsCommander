import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ContextHistoryAPI, __setTransportForTests } from "./ipc";
import { FakeTransport } from "./testing/fake-transport";
import {
  isHistoryIpcError,
  periodEndingAt,
  type ChangesRequest,
  type ReportQuery,
} from "./context-history";

// #3532 P06 - wire contract of the four context-history commands. The common
// changes fixture (Q, L) is the same one the Rust adapter tests pin: the
// request travels NESTED as {request:{query:Q,logicalSessionId:L}}; the flat
// spelling is refused by the backend with InvalidQuery.

const Q: ReportQuery = {
  projectPath: "__PROJECT__",
  fromUtcMs: 0,
  toUtcMs: 86400000,
  agentKey: "dev",
  teamId: null,
  codingAgentId: { kind: "any" },
  model: { kind: "any" },
  effort: { kind: "any" },
  cursor: null,
};
const L = "11111111-1111-4111-8111-111111111111";

describe("ContextHistoryAPI wire contract", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
  });

  afterEach(() => restore());

  it("sends changes nested as {request:{query,logicalSessionId}}", async () => {
    fake.resolve("get_context_history_changes", { recordsPage: [] });
    const request: ChangesRequest = { query: Q, logicalSessionId: L };
    await ContextHistoryAPI.changes(request);
    expect(fake.calls).toEqual([
      {
        cmd: "get_context_history_changes",
        args: { request: { query: { ...Q }, logicalSessionId: L } },
      },
    ]);
    const sent = fake.calls[0].args.request as Record<string, unknown>;
    expect(Object.keys(sent).sort()).toEqual(["logicalSessionId", "query"]);
  });

  it("surfaces the backend InvalidQuery refusal of the flat spelling", async () => {
    fake.onInvoke("get_context_history_changes", (args) => {
      const request = args.request as Record<string, unknown>;
      if (!("query" in request)) {
        throw { code: "InvalidQuery", message: "malformed request" };
      }
      return {};
    });
    const flat = { ...Q, logicalSessionId: L } as unknown as ChangesRequest;
    const error = await ContextHistoryAPI.changes(flat).catch((reason: unknown) => reason);
    expect(isHistoryIpcError(error)).toBe(true);
    expect(error).toEqual({ code: "InvalidQuery", message: "malformed request" });
  });

  it("sends report and detail as a ReportQuery request", async () => {
    fake.resolve("get_context_history_report", {});
    fake.resolve("get_context_history_agent_detail", {});
    await ContextHistoryAPI.report({ ...Q, agentKey: null });
    await ContextHistoryAPI.agentDetail(Q);
    expect(fake.calls).toEqual([
      { cmd: "get_context_history_report", args: { request: { ...Q, agentKey: null } } },
      { cmd: "get_context_history_agent_detail", args: { request: Q } },
    ]);
  });

  it("sends sidebar means with project and runtime ids only", async () => {
    fake.resolve("get_context_history_sidebar_means", { entries: [] });
    const request = { projectPath: "__PROJECT__", runtimeSessionIds: [L] };
    await ContextHistoryAPI.sidebarMeans(request);
    expect(fake.calls).toEqual([
      { cmd: "get_context_history_sidebar_means", args: { request } },
    ]);
  });

  it("keeps launch filters distinct: any, unknown (null) and exact", async () => {
    fake.resolve("get_context_history_report", {});
    await ContextHistoryAPI.report({
      ...Q,
      codingAgentId: { kind: "unknown" },
      model: { kind: "exact", value: "opus" },
    });
    const sent = fake.calls[0].args.request as ReportQuery;
    expect(sent.codingAgentId).toEqual({ kind: "unknown" });
    expect(sent.model).toEqual({ kind: "exact", value: "opus" });
    expect(sent.effort).toEqual({ kind: "any" });
  });
});

describe("context-history helpers", () => {
  it("builds a half-open UTC period ending at the refresh time", () => {
    expect(periodEndingAt(7 * 86400000, 7)).toEqual({ fromUtcMs: 0, toUtcMs: 7 * 86400000 });
  });

  it("recognizes only {code,message} as a backend error", () => {
    expect(isHistoryIpcError({ code: "Busy", message: "retry" })).toBe(true);
    expect(isHistoryIpcError("Busy")).toBe(false);
    expect(isHistoryIpcError(null)).toBe(false);
  });
});
