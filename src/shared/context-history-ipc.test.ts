import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ContextHistoryAPI, __setTransportForTests } from "./ipc";
import { FakeTransport } from "./testing/fake-transport";
import {
  CONTEXT_HISTORY_SCHEMA_VERSION,
  formatRecordsPerSession,
  isHistoryIpcError,
  periodEndingAt,
  selectSessionWindow,
  sessionWindowLabel,
  type AgentDetailResponse,
  type ChangesRequest,
  type SessionWindowBucket,
  type SidebarMeansResponse,
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

  it("sends sidebar means nested as {request:{projectPath,runtimeSessionIds}}", async () => {
    const reply: SidebarMeansResponse = {
      schemaVersion: 2,
      generatedAtUtcMs: 5,
      entries: [
        {
          runtimeSessionId: L,
          scope: { kind: "unknown", agentKey: null, teamIds: [] },
          windows: null,
        },
      ],
    };
    fake.resolve("get_context_history_sidebar_means", reply);
    const got = await ContextHistoryAPI.sidebarMeans({
      projectPath: "__PROJECT__",
      runtimeSessionIds: [L],
    });
    expect(fake.calls[0]).toEqual({
      cmd: "get_context_history_sidebar_means",
      args: { request: { projectPath: "__PROJECT__", runtimeSessionIds: [L] } },
    });
    expect(got.entries[0].windows).toBeNull();
  });

  it("sends a count-mode query nested and keeps it in the date representation", async () => {
    fake.resolve("get_context_history_agent_detail", {});
    const counted: ReportQuery = { ...Q, fromUtcMs: 0, sessionCount: 100 };
    await ContextHistoryAPI.agentDetail(counted);
    expect(fake.calls).toEqual([
      { cmd: "get_context_history_agent_detail", args: { request: counted } },
    ]);
    expect((fake.calls[0].args.request as ReportQuery).sessionCount).toBe(100);
  });

  it("passes schema-3 detail and sidebar DTOs through unchanged, nulls included", async () => {
    const detail: AgentDetailResponse = {
      schemaVersion: 3,
      requestedPeriod: { fromUtcMs: 0, toUtcMs: 5, sessionCount: 100, selection: "recent_sessions" },
      generatedAtUtcMs: 5,
      coverage: {
        retentionDays: 30,
        retainedEarliestUtcMs: null,
        retainedLatestUtcMs: null,
        incomplete: false,
        unknownStartSessionsCount: 0,
      },
      agentKey: "dev",
      scopeWindows: [
        { teamAttribution: "known", teamIds: ["t"], requestedSessions: 100, availableSessions: 42 },
      ],
      recordsCount: 0,
      positiveSum: 0,
      mean: null,
      sessionsCount: 0,
      recordsPerSession: null,
      incomplete: false,
      sessionsPage: [],
      nextSessionCursor: null,
    };
    const bucket: SessionWindowBucket = {
      requestedSessions: 25,
      recordsCount: 390,
      positiveSum: 7800,
      mean: 20,
      sessionsCount: 25,
      recordsPerSession: 15.6,
      incomplete: false,
    };
    const sidebar: SidebarMeansResponse = {
      schemaVersion: 3,
      generatedAtUtcMs: 5,
      entries: [
        {
          runtimeSessionId: L,
          scope: { kind: "known", agentKey: "dev", teamIds: ["t"] },
          windows: null,
          sessionWindows: [bucket],
        },
        {
          runtimeSessionId: L,
          scope: { kind: "ambiguous", agentKey: null, teamIds: [] },
          windows: null,
          sessionWindows: null,
        },
      ],
    };
    fake.resolve("get_context_history_agent_detail", detail);
    fake.resolve("get_context_history_sidebar_means", sidebar);
    const gotDetail = await ContextHistoryAPI.agentDetail({ ...Q, sessionCount: 100 });
    const gotSidebar = await ContextHistoryAPI.sidebarMeans({
      projectPath: "__PROJECT__",
      runtimeSessionIds: [L],
    });
    expect(gotDetail).toEqual(detail);
    expect(gotDetail.recordsPerSession).toBeNull();
    expect(gotDetail.scopeWindows?.[0].availableSessions).toBe(42);
    expect(gotSidebar.entries[0].sessionWindows?.[0].recordsPerSession).toBe(15.6);
    expect(gotSidebar.entries[1].sessionWindows).toBeNull();
    expect(CONTEXT_HISTORY_SCHEMA_VERSION).toBe(3);
  });
});

describe("context-history helpers", () => {
  it("builds a half-open UTC period ending at the refresh time", () => {
    expect(periodEndingAt(7 * 86400000, 7)).toEqual({ fromUtcMs: 0, toUtcMs: 7 * 86400000 });
  });

  it("labels session windows and notes fewer available sessions", () => {
    expect(sessionWindowLabel(1)).toBe("Última sesión");
    expect(sessionWindowLabel(25)).toBe("Últimas 25 sesiones");
    expect(sessionWindowLabel(100, 42)).toBe("Últimas 100 sesiones (42 disponibles)");
    expect(sessionWindowLabel(100, 100)).toBe("Últimas 100 sesiones");
    expect(sessionWindowLabel(100, null)).toBe("Últimas 100 sesiones");
  });

  it("selects the configured colour window, falling back to the last member", () => {
    const counts = [2, 5];
    const two: SessionWindowBucket = {
      requestedSessions: 2,
      recordsCount: 21,
      positiveSum: 210,
      mean: 10,
      sessionsCount: 2,
      recordsPerSession: 10.5,
      incomplete: false,
    };
    const five: SessionWindowBucket = { ...two, requestedSessions: 5 };
    expect(selectSessionWindow(counts, 2, [two, five])).toEqual({ requestedSessions: 2, bucket: two });
    expect(selectSessionWindow(counts, 300, [two, five])).toEqual({ requestedSessions: 5, bucket: five });
    expect(selectSessionWindow(counts, undefined, null)).toEqual({ requestedSessions: 5, bucket: null });
    expect(selectSessionWindow([], 2, [two])).toBeNull();
    expect(sessionWindowLabel(5)).toBe("Últimas 5 sesiones");
  });

  it("formats backend records per session with one decimal or an em dash", () => {
    expect(formatRecordsPerSession(15.6)).toBe("15.6");
    expect(formatRecordsPerSession(10.5)).toBe("10.5");
    expect(formatRecordsPerSession(0)).toBe("0.0");
    expect(formatRecordsPerSession(null)).toBe("—");
    expect(formatRecordsPerSession(undefined)).toBe("—");
    expect(formatRecordsPerSession(47 / 3)).toBe("15.7");
  });

  it("recognizes only {code,message} as a backend error", () => {
    expect(isHistoryIpcError({ code: "Busy", message: "retry" })).toBe(true);
    expect(isHistoryIpcError("Busy")).toBe(false);
    expect(isHistoryIpcError(null)).toBe(false);
  });
});
