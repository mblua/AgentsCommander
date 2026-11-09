import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";
import type { AgentSummary, ReportQuery, ReportResponse } from "../shared/context-history";
import { createHistoryState } from "./state";

// #3532 P06 - per-instance history state: generations drop late responses,
// pagination appends backend pages without recomputing totals, and errors
// never keep an older result on screen as the answer of a newer query.

const NOW = 30 * 86_400_000;

function agent(agentKey: string, mean: number | null, recordsCount: number): AgentSummary {
  return {
    agentKey,
    agentName: agentKey,
    metadataBuckets: [],
    recordsCount,
    positiveSum: 0,
    mean,
    sessionsCount: 1,
    recordsPerSession: recordsCount,
    incomplete: false,
    scopeWindows: null,
  };
}

function report(agents: AgentSummary[], next: string | null, total: number): ReportResponse {
  return {
    schemaVersion: 2,
    requestedPeriod: {
      fromUtcMs: 0, toUtcMs: NOW, sessionCount: 100, selection: "recent_sessions",
    },
    generatedAtUtcMs: NOW,
    coverage: {
      retentionDays: 30,
      retainedEarliestUtcMs: null,
      retainedLatestUtcMs: null,
      incomplete: false,
      unknownStartSessionsCount: 0,
    },
    agentsPage: agents,
    nextAgentCursor: next,
    totalAgents: total,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const WINDOWS = [25, 100, 300];

/** #4091 A state with the loaded settings list, as the App provides it. */
function configured(project: string, now: () => number = () => NOW) {
  const state = createHistoryState(project, now);
  state.setWindows(WINDOWS);
  return state;
}

describe("createHistoryState", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
  });

  afterEach(() => restore());

  const sentQuery = (index: number) => fake.calls[index].args.request as ReportQuery;

  it("pins the cutoff from the refresh clock and sends the count and filters", async () => {
    fake.resolve("get_context_history_report", report([], null, 0));
    const state = configured("C:/p");
    state.setFilters({ sessionCount: 25, teamId: null, model: { kind: "unknown" } });
    await state.refresh();
    const query = sentQuery(0);
    expect(query).toMatchObject({
      projectPath: "C:/p",
      fromUtcMs: 0,
      toUtcMs: NOW,
      sessionCount: 25,
      agentKey: null,
      teamId: null,
      model: { kind: "unknown" },
      codingAgentId: { kind: "any" },
      cursor: null,
    });
  });

  it("ignores a late response of an older project", async () => {
    const first = deferred<ReportResponse>();
    let calls = 0;
    fake.onInvoke("get_context_history_report", () => {
      calls += 1;
      return calls === 1 ? first.promise : report([agent("b", 2, 2)], null, 1);
    });
    const state = configured("C:/a");
    const stale = state.refresh();
    state.setProject("C:/b");
    await state.refresh();
    first.resolve(report([agent("a", 1, 1)], null, 1));
    await stale;
    expect(state.report().data?.agentsPage.map((a) => a.agentKey)).toEqual(["b"]);
    expect(sentQuery(1).projectPath).toBe("C:/b");
  });

  it("appends pages with the cursor and keeps the backend total", async () => {
    fake.onInvoke("get_context_history_report", (args) => {
      const query = args.request as ReportQuery;
      return query.cursor === null
        ? report([agent("a", 1.25, 3)], "c1", 2)
        : report([agent("b", 4, 5)], null, 2);
    });
    const state = configured("C:/p");
    await state.refresh();
    await state.loadMoreAgents();
    expect(sentQuery(1).cursor).toBe("c1");
    expect(state.report().data?.agentsPage.map((a) => a.mean)).toEqual([1.25, 4]);
    expect(state.report().data?.totalAgents).toBe(2);
    await state.loadMoreAgents();
    expect(fake.calls).toHaveLength(2);
  });

  it("reports an error without the previous result and retries with the same filters", async () => {
    let fail = false;
    fake.onInvoke("get_context_history_report", () => {
      if (fail) throw { code: "Busy", message: "history is busy; retry" };
      return report([agent("a", 1, 1)], null, 1);
    });
    const state = configured("C:/p");
    state.setFilters({ teamId: "t1" });
    await state.refresh();
    fail = true;
    await state.refresh();
    expect(state.report()).toEqual({ loading: false, error: true, data: null });
    fail = false;
    await state.refresh();
    expect(sentQuery(2).teamId).toBe("t1");
    expect(state.report().error).toBe(false);
  });

  it("loads detail and nested changes for the selected agent and session", async () => {
    fake.resolve("get_context_history_agent_detail", { sessionsPage: [] });
    fake.resolve("get_context_history_changes", { recordsPage: [] });
    const state = configured("C:/p");
    await state.selectAgent("dev");
    await state.selectSession("11111111-1111-4111-8111-111111111111");
    expect(fake.calls[0].args.request).toMatchObject({ agentKey: "dev" });
    expect(fake.calls[1].args.request).toMatchObject({
      query: { agentKey: "dev", cursor: null },
      logicalSessionId: "11111111-1111-4111-8111-111111111111",
    });
  });

  it("drops the selection when filters change", async () => {
    fake.resolve("get_context_history_agent_detail", { sessionsPage: [] });
    const state = configured("C:/p");
    await state.selectAgent("dev");
    state.setFilters({ sessionCount: 300 });
    expect(state.selectedAgent()).toBeNull();
    expect(state.detail().data).toBeNull();
  });

  it("ignores responses that arrive after dispose", async () => {
    const pending = deferred<ReportResponse>();
    fake.onInvoke("get_context_history_report", () => pending.promise);
    const state = configured("C:/p");
    const work = state.refresh();
    state.dispose();
    pending.resolve(report([agent("a", 1, 1)], null, 1));
    await work;
    expect(state.report().data).toBeNull();
  });

  it("starts on the last configured count and sends nothing without a list", async () => {
    fake.resolve("get_context_history_report", report([], null, 0));
    const bare = createHistoryState("C:/p", () => NOW);
    await bare.refresh();
    await bare.selectAgent("dev");
    expect(fake.calls).toEqual([]);
    bare.setWindows([]);
    await bare.refresh();
    expect(fake.calls).toEqual([]);
    expect(bare.filters().sessionCount).toBeNull();
    const state = configured("C:/p");
    expect(state.windows()).toEqual(WINDOWS);
    expect(state.filters().sessionCount).toBe(300);
    await state.refresh();
    expect(sentQuery(0).sessionCount).toBe(300);
  });

  it("ignores a filter patch with an unconfigured count", async () => {
    fake.resolve("get_context_history_report", report([], null, 0));
    const state = configured("C:/p");
    state.setFilters({ sessionCount: 7, teamId: "t1" });
    expect(state.filters().sessionCount).toBe(300);
    expect(state.filters().teamId).toBeNull();
    await state.refresh();
    expect(sentQuery(0)).toMatchObject({ sessionCount: 300, teamId: null });
  });

  it("a changed list keeps a present count, else the last; drops pages and selection", async () => {
    fake.resolve("get_context_history_report", report([agent("a", 1, 1)], "c1", 2));
    fake.resolve("get_context_history_agent_detail", { sessionsPage: [] });
    const state = configured("C:/p");
    state.setFilters({ sessionCount: 100 });
    await state.refresh();
    await state.selectAgent("a");
    state.setWindows(WINDOWS);
    expect(state.selectedAgent()).toBe("a");
    state.setWindows([50, 100]);
    expect(state.filters().sessionCount).toBe(100);
    expect(state.report().data).toBeNull();
    expect(state.selectedAgent()).toBeNull();
    state.setWindows([5, 10]);
    expect(state.filters().sessionCount).toBe(10);
    await state.refresh();
    expect(sentQuery(fake.calls.length - 1)).toMatchObject({ sessionCount: 10, cursor: null });
  });

  it("drops a late page of the old list after the list changes", async () => {
    const first = deferred<ReportResponse>();
    let calls = 0;
    fake.onInvoke("get_context_history_report", () => {
      calls += 1;
      return calls === 1 ? first.promise : report([agent("new", 2, 2)], null, 1);
    });
    const state = configured("C:/p");
    const stale = state.refresh();
    state.setWindows([10]);
    await state.refresh();
    first.resolve(report([agent("old", 1, 1)], null, 1));
    await stale;
    expect(state.report().data?.agentsPage.map((a) => a.agentKey)).toEqual(["new"]);
  });

  it("keeps the count and cutoff across pagination, detail and changes", async () => {
    let clock = NOW;
    fake.onInvoke("get_context_history_report", (args) =>
      (args.request as ReportQuery).cursor === null
        ? report([agent("a", 1, 3)], "c1", 2)
        : report([agent("b", 4, 5)], null, 2),
    );
    fake.resolve("get_context_history_agent_detail", { sessionsPage: [], nextSessionCursor: "s1" });
    fake.resolve("get_context_history_changes", { recordsPage: [], nextRecordCursor: "r1" });
    const state = configured("C:/p", () => clock);
    state.setFilters({ sessionCount: 25 });
    await state.refresh();
    clock = NOW + 60_000;
    await state.loadMoreAgents();
    await state.selectAgent("a");
    await state.loadMoreSessions();
    await state.selectSession("11111111-1111-4111-8111-111111111111");
    await state.loadMoreRecords();
    const queries = fake.calls.map((c) => {
      const request = c.args.request as ReportQuery | { query: ReportQuery };
      return "query" in request ? request.query : request;
    });
    expect(queries).toHaveLength(6);
    for (const query of queries) {
      expect(query).toMatchObject({ fromUtcMs: 0, toUtcMs: NOW, sessionCount: 25 });
    }
    expect(queries.map((q) => q.cursor)).toEqual([null, "c1", null, "s1", null, "r1"]);
    clock = NOW + 120_000;
    await state.refresh();
    const last = fake.calls[fake.calls.length - 1].args.request as { query: ReportQuery };
    expect(last.query).toMatchObject({ toUtcMs: NOW + 120_000, sessionCount: 25, cursor: null });
  });

  it("keeps the backend records-per-session of each appended agent", async () => {
    fake.onInvoke("get_context_history_report", (args) =>
      (args.request as ReportQuery).cursor === null
        ? report([{ ...agent("a", 1, 1560), sessionsCount: 100, recordsPerSession: 15.6 }], "c1", 2)
        : report([{ ...agent("b", 1, 21), sessionsCount: 2, recordsPerSession: 9.9 }], null, 2),
    );
    const state = configured("C:/p");
    await state.refresh();
    await state.loadMoreAgents();
    expect(state.report().data?.agentsPage.map((a) => a.recordsPerSession)).toEqual([15.6, 9.9]);
  });
});
