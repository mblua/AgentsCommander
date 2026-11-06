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
    incomplete: false,
  };
}

function report(agents: AgentSummary[], next: string | null, total: number): ReportResponse {
  return {
    schemaVersion: 2,
    requestedPeriod: { fromUtcMs: 0, toUtcMs: NOW, selection: "session_started" },
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

describe("createHistoryState", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
  });

  afterEach(() => restore());

  const sentQuery = (index: number) => fake.calls[index].args.request as ReportQuery;

  it("fixes a half-open period from the refresh clock and sends filters as given", async () => {
    fake.resolve("get_context_history_report", report([], null, 0));
    const state = createHistoryState("C:/p", () => NOW);
    state.setFilters({ days: 1, teamId: null, model: { kind: "unknown" } });
    await state.refresh();
    const query = sentQuery(0);
    expect(query).toMatchObject({
      projectPath: "C:/p",
      fromUtcMs: NOW - 86_400_000,
      toUtcMs: NOW,
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
    const state = createHistoryState("C:/a", () => NOW);
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
    const state = createHistoryState("C:/p", () => NOW);
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
    const state = createHistoryState("C:/p", () => NOW);
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
    const state = createHistoryState("C:/p", () => NOW);
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
    const state = createHistoryState("C:/p", () => NOW);
    await state.selectAgent("dev");
    state.setFilters({ days: 30 });
    expect(state.selectedAgent()).toBeNull();
    expect(state.detail().data).toBeNull();
  });

  it("ignores responses that arrive after dispose", async () => {
    const pending = deferred<ReportResponse>();
    fake.onInvoke("get_context_history_report", () => pending.promise);
    const state = createHistoryState("C:/p", () => NOW);
    const work = state.refresh();
    state.dispose();
    pending.resolve(report([agent("a", 1, 1)], null, 1));
    await work;
    expect(state.report().data).toBeNull();
  });
});
