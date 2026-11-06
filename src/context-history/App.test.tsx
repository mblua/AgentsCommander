// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import { click, renderWithFakeTransport, waitFor } from "../shared/testing/ui-harness";
import type {
  AgentDetailResponse,
  AgentSummary,
  ChangesPageResponse,
  HistoryRecord,
  ReportResponse,
  SessionSummary,
} from "../shared/context-history";
import ContextHistoryApp, { BOUNDARY_TEXT, TEXT } from "./App";

vi.mock("../shared/platform", () => ({ isTauri: true, isBrowser: false, isWindows: false }));

// #3532 P06 - the history view renders backend numbers as served, separates
// loading, empty and error, and keeps keyboard and narrow-layout affordances.

const NOW = 30 * 86_400_000;
const L = "11111111-1111-4111-8111-111111111111";

const header = {
  schemaVersion: 2,
  requestedPeriod: { fromUtcMs: 0, toUtcMs: NOW, selection: "session_started" as const },
  generatedAtUtcMs: NOW,
  coverage: {
    retentionDays: 30,
    retainedEarliestUtcMs: null,
    retainedLatestUtcMs: null,
    incomplete: false,
    unknownStartSessionsCount: 0,
  },
};

const agent: AgentSummary = {
  agentKey: "dev",
  agentName: "dev-rust",
  metadataBuckets: [
    {
      codingAgentId: "claude",
      model: "opus",
      effort: null,
      metadataSource: "launch_command",
      teamAttribution: "known",
      teamIds: ["team-a"],
      sessionsCount: 2,
    },
  ],
  recordsCount: 7,
  positiveSum: 0,
  mean: 42.25,
  sessionsCount: 2,
  incomplete: false,
};

const reportResponse = (agents: AgentSummary[]): ReportResponse => ({
  ...header,
  agentsPage: agents,
  nextAgentCursor: null,
  totalAgents: agents.length,
});

const session: SessionSummary = {
  logicalSessionId: L,
  appRunId: "22222222-2222-4222-8222-222222222222",
  runtimeSessionId: "33333333-3333-4333-8333-333333333333",
  ordinal: 0,
  instanceFqn: "p:room-1/dev",
  room: "room-1",
  matrixType: "replica",
  logicalStartedAtUtcMs: NOW - 1000,
  firstObservedAtUtcMs: NOW - 1000,
  lastObservedAtUtcMs: NOW + 5000,
  initialKind: "unknown",
  boundaryReason: "observed_zero",
  recordsCount: 3,
  positiveSum: 0,
  mean: 10,
  max: 20,
  maxPartial: true,
  initialValue: null,
  partialStart: true,
  partialEnd: false,
  retentionClipped: false,
  coverageLabel: TEXT.incomplete,
  codingAgentId: null,
  model: null,
  effort: null,
  metadataSource: "unavailable",
  teamAttribution: "unknown",
  teamIds: [],
};

const detailResponse: AgentDetailResponse = {
  ...header,
  agentKey: "dev",
  recordsCount: 7,
  positiveSum: 0,
  mean: 42.25,
  sessionsCount: 1,
  incomplete: false,
  sessionsPage: [session],
  nextSessionCursor: null,
};

const boundary = { kind: "boundary", percent: null, boundaryReason: "lifecycle" };
const changesResponse = {
  ...header,
  agentKey: "dev",
  logicalSessionId: L,
  recordsPage: [
    { observedAtUtcMs: NOW, kind: "change", percent: 15, boundaryReason: null },
    { observedAtUtcMs: NOW + 5000, ...boundary },
  ] as unknown as HistoryRecord[],
  nextRecordCursor: null,
} as ChangesPageResponse;

function mount(fake: FakeTransport, onClose = () => {}) {
  return renderWithFakeTransport(
    () => <ContextHistoryApp projectPath="C:/p" onClose={onClose} now={() => NOW} />,
    fake,
  );
}

const must = (root: Element, selector: string): HTMLElement => {
  const el = root.querySelector<HTMLElement>(selector);
  if (!el) throw new Error(`missing ${selector}`);
  return el;
};

describe("ContextHistoryApp", () => {
  let cleanup: (() => void) | undefined;
  afterEach(() => cleanup?.());

  it("renders the title, notes and backend count and mean", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    expect(view.root.textContent).toContain(TEXT.title);
    expect(view.root.textContent).toContain(TEXT.periodNote);
    expect(view.root.textContent).toContain(TEXT.launchNote);
    const row = must(view.root, "[data-agent-key='dev']");
    expect(must(row, "[data-metric='count']").textContent).toBe("7 registros");
    expect(must(row, "[data-metric='mean']").textContent).toBe(`${TEXT.meanLabel}: 42.3`);
    expect(must(view.root, "[data-metric='total-agents']").textContent).toBe("1 agentes");
  });

  it("shows empty and error distinctly and retries", async () => {
    const fake = new FakeTransport();
    fake.reject("get_context_history_report", "boom");
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => expect(must(view.root, "[role='status']").textContent).toBe(TEXT.error));
    fake.resolve("get_context_history_report", reportResponse([]));
    click(must(view.root, ".ch-retry"));
    await waitFor(() => expect(must(view.root, "[role='status']").textContent).toBe(TEXT.empty));
    expect(fake.calls.filter((c) => c.cmd === "get_context_history_report")).toHaveLength(2);
  });

  it("offers Todos and Desconocido distinctly and sends the chosen filter", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const selects = view.root.querySelectorAll<HTMLSelectElement>("select");
    const model = selects[3];
    expect([...model.options].map((o) => o.value)).toEqual(["any", "unknown", "exact:opus"]);
    model.value = "unknown";
    model.dispatchEvent(new Event("change", { bubbles: true }));
    await waitFor(() => expect(fake.calls).toHaveLength(2));
    expect(fake.calls[1].args.request).toMatchObject({ model: { kind: "unknown" } });
  });

  it("opens sessions and the timeline with backend labels", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    fake.resolve("get_context_history_agent_detail", detailResponse);
    fake.resolve("get_context_history_changes", changesResponse);
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    click(must(view.root, "[data-agent-key='dev']"));
    await waitFor(() => void must(view.root, `[data-session-id='${L}']`));
    const row = must(view.root, `[data-session-id='${L}']`);
    expect(must(row, "[data-metric='initial']").textContent).toBe(`Inicial: ${TEXT.unknown}`);
    expect(must(row, "[data-metric='max']").textContent).toBe("Máximo: 20%");
    expect(row.textContent).toContain(TEXT.incomplete);
    expect(row.textContent).toContain(BOUNDARY_TEXT.observed_zero);
    expect(row.textContent).toContain("room-1");
    click(row);
    await waitFor(() => expect(view.root.querySelectorAll(".ch-record")).toHaveLength(2));
    expect(view.root.textContent).toContain(BOUNDARY_TEXT.lifecycle);
    expect(must(view.root, ".ch-panel").classList.contains("ch-has-selection")).toBe(true);
    click(must(view.root, ".ch-back"));
    await waitFor(() => expect(view.root.querySelector(".ch-detail")).toBeNull());
  });

  it("closes on Escape", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([]));
    const onClose = vi.fn();
    const view = mount(fake, onClose);
    cleanup = view.cleanup;
    must(view.root, ".ch-overlay").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
