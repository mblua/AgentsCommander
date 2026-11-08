// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import { readFileSync } from "node:fs";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  click,
  renderWithFakeTransport,
  settingsSnapshot,
  waitFor,
} from "../shared/testing/ui-harness";
import type {
  AgentDetailResponse,
  AgentSummary,
  ChangesPageResponse,
  HistoryRecord,
  ReportResponse,
  ScopeWindowCoverage,
  SessionSummary,
  TeamAttribution,
} from "../shared/context-history";
import ContextHistoryApp, { BOUNDARY_TEXT, TEXT } from "./App";

const platform = vi.hoisted(() => ({ isTauri: true }));
vi.mock("../shared/platform", () => ({
  get isTauri() {
    return platform.isTauri;
  },
  get isBrowser() {
    return !platform.isTauri;
  },
  isWindows: false,
}));

// #3532 P06 - the history view renders backend numbers as served, separates
// loading, empty and error, and keeps keyboard and narrow-layout affordances.

const NOW = 30 * 86_400_000;
const L = "11111111-1111-4111-8111-111111111111";

const header = {
  schemaVersion: 2,
  requestedPeriod: {
    fromUtcMs: 0,
    toUtcMs: NOW,
    sessionCount: 300,
    selection: "recent_sessions" as const,
  },
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
  recordsPerSession: 3.5,
  incomplete: false,
  scopeWindows: null,
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
  recordsPerSession: 7,
  incomplete: false,
  scopeWindows: null,
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

const WINDOWS = [25, 100, 300];

/** #4091 The App reads the configured session windows before any history query. */
function serveSettings(fake: FakeTransport, windows: number[] | null = WINDOWS) {
  const snapshot = settingsSnapshot(windows ? { contextHistorySessionWindows: windows } : {});
  if (!windows) delete snapshot.contextHistorySessionWindows;
  fake.resolve("get_settings", snapshot);
}

const historyCalls = (fake: FakeTransport) =>
  fake.calls.filter((c) => c.cmd.startsWith("get_context_history_"));

function mount(fake: FakeTransport, onClose = () => {}, windows: number[] | null = WINDOWS) {
  if (windows !== null && !fake.calls.length) serveSettings(fake, windows);
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

function stubNarrow(matches: boolean): void {
  window.matchMedia = ((query: string) =>
    ({ matches, media: query }) as MediaQueryList) as typeof window.matchMedia;
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("ContextHistoryApp", () => {
  let cleanup: (() => void) | undefined;
  afterEach(() => {
    cleanup?.();
    cleanup = undefined;
    platform.isTauri = true;
  });

  it("never invokes the history IPC outside Tauri, even when the project changes", async () => {
    platform.isTauri = false;
    const fake = new FakeTransport();
    const [project, setProject] = createSignal("C:/a");
    const view = renderWithFakeTransport(
      () => <ContextHistoryApp projectPath={project()} onClose={() => {}} now={() => NOW} />,
      fake,
    );
    cleanup = view.cleanup;
    setProject("C:/b");
    await flush();
    expect(fake.calls).toEqual([]);
    expect(view.root.textContent).toContain(TEXT.desktopOnly);
  });

  it("is a central-pane region, not a global modal", () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([]));
    const view = mount(fake);
    cleanup = view.cleanup;
    const host = must(view.root, ".ch-overlay");
    expect(host.getAttribute("role")).toBe("region");
    expect(host.hasAttribute("aria-modal")).toBe(false);
    // Variable base keeps the file: URL under jsdom (see browser/App.compact.test).
    const moduleUrl = import.meta.url;
    const css = readFileSync(new URL("./styles.css", moduleUrl), "utf8")
      .split("\r\n")
      .join("\n");
    expect(css.length).toBeGreaterThan(0);
    const rule = /\.ch-overlay \{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("position: absolute;");
    expect(css).not.toMatch(/position:\s*fixed/);
    expect(css).not.toMatch(/\d+v[wh]\b/);
  });

  it("moves focus to the detail on select and back to the row in a narrow layout", async () => {
    stubNarrow(true);
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    fake.resolve("get_context_history_agent_detail", detailResponse);
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const row = must(view.root, "[data-agent-key='dev']");
    row.focus();
    row.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    click(row);
    await waitFor(() => expect(document.activeElement).toBe(must(view.root, ".ch-detail")));
    click(must(view.root, ".ch-back"));
    await waitFor(() =>
      expect(document.activeElement).toBe(must(view.root, "[data-agent-key='dev']")),
    );
  });

  it("keeps focus on the list row when selecting in a wide layout", async () => {
    stubNarrow(false);
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    fake.resolve("get_context_history_agent_detail", detailResponse);
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const row = must(view.root, "[data-agent-key='dev']");
    row.focus();
    click(row);
    await waitFor(() => void must(view.root, `[data-session-id='${L}']`));
    expect(document.activeElement).toBe(row);
  });

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
    click(must(view.root, '[data-ac-testid="contextHistory.retry"]'));
    await waitFor(() => expect(must(view.root, "[role='status']").textContent).toBe(TEXT.empty));
    expect(fake.calls.filter((c) => c.cmd === "get_context_history_report")).toHaveLength(2);
    expect(fake.calls.filter((c) => c.cmd === "get_settings")).toHaveLength(2);
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
    await waitFor(() => expect(historyCalls(fake)).toHaveLength(2));
    expect(historyCalls(fake)[1].args.request).toMatchObject({ model: { kind: "unknown" } });
    expect(fake.calls.filter((c) => c.cmd === "get_settings")).toHaveLength(1);
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

  it("reads settings first and queries the last configured count", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([]));
    const view = mount(fake, () => {}, [3, 40]);
    cleanup = view.cleanup;
    await waitFor(() => expect(historyCalls(fake)).toHaveLength(1));
    expect(fake.calls[0].cmd).toBe("get_settings");
    expect(historyCalls(fake)[0].args.request).toMatchObject({
      fromUtcMs: 0,
      toUtcMs: NOW,
      sessionCount: 40,
    });
    const select = must(view.root, '[data-ac-testid="contextHistory.sessionCount"]');
    const options = [...(select as HTMLSelectElement).options];
    expect(options.map((o) => o.value)).toEqual(["3", "40"]);
    expect(options.map((o) => o.textContent))
      .toEqual(["Últimas 3 sesiones", "Últimas 40 sesiones"]);
    expect((select as HTMLSelectElement).value).toBe("40");
    (select as HTMLSelectElement).value = "3";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await waitFor(() => expect(historyCalls(fake)).toHaveLength(2));
    expect(historyCalls(fake)[1].args.request).toMatchObject({ sessionCount: 3, cursor: null });
    expect(fake.calls.filter((c) => c.cmd === "get_settings")).toHaveLength(1);
  });

  for (const failure of ["reject", "missing", "empty"] as const) {
    it(`shows an error and sends no history request when settings are ${failure}`, async () => {
      const fake = new FakeTransport();
      if (failure === "reject") fake.reject("get_settings", "settings unavailable");
      else serveSettings(fake, failure === "empty" ? [] : null);
      fake.resolve("get_context_history_report", reportResponse([agent]));
      const view = mount(fake, () => {}, null);
      cleanup = view.cleanup;
      await waitFor(() =>
        expect(must(view.root, "[role='status']").textContent).toBe(TEXT.settingsError),
      );
      expect(historyCalls(fake)).toEqual([]);
      serveSettings(fake, [10]);
      click(must(view.root, '[data-ac-testid="contextHistory.retry"]'));
      await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
      expect(historyCalls(fake)[0].args.request).toMatchObject({ sessionCount: 10 });
    });
  }

  it("refresh re-reads settings and follows a changed list", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    fake.resolve("get_context_history_agent_detail", detailResponse);
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    click(must(view.root, "[data-agent-key='dev']"));
    await waitFor(() => void must(view.root, ".ch-detail"));
    serveSettings(fake, [100, 200]);
    const refresh = [...view.root.querySelectorAll("button")]
      .find((b) => b.textContent === TEXT.refresh)!;
    click(refresh);
    await waitFor(() => expect(fake.calls.filter((c) => c.cmd === "get_settings")).toHaveLength(2));
    await waitFor(() => expect(historyCalls(fake).length).toBeGreaterThan(2));
    const last = historyCalls(fake)[historyCalls(fake).length - 1];
    expect(last.cmd).toBe("get_context_history_report");
    expect(last.args.request).toMatchObject({ sessionCount: 200, cursor: null });
    expect(view.root.querySelector(".ch-detail")).toBeNull();
  });

  it("ignores a late settings reply after the project changes", async () => {
    const fake = new FakeTransport();
    let release!: (value: unknown) => void;
    let settingsCalls = 0;
    fake.onInvoke("get_settings", () => {
      settingsCalls += 1;
      if (settingsCalls === 1) return new Promise((resolve) => { release = resolve; });
      return settingsSnapshot({ contextHistorySessionWindows: [9] });
    });
    fake.resolve("get_context_history_report", reportResponse([]));
    const [project, setProject] = createSignal("C:/a");
    const view = renderWithFakeTransport(
      () => <ContextHistoryApp projectPath={project()} onClose={() => {}} now={() => NOW} />,
      fake,
    );
    cleanup = view.cleanup;
    setProject("C:/b");
    await waitFor(() => expect(historyCalls(fake)).toHaveLength(1));
    release(settingsSnapshot({ contextHistorySessionWindows: [1, 2] }));
    await flush();
    expect(historyCalls(fake)).toHaveLength(1);
    expect(historyCalls(fake)[0].args.request).toMatchObject({
      projectPath: "C:/b",
      sessionCount: 9,
    });
  });

  it("renders backend records per session for agents and detail, never derived", async () => {
    const fake = new FakeTransport();
    const rows: AgentSummary[] = [
      { ...agent, agentKey: "a", recordsCount: 1560, sessionsCount: 100, recordsPerSession: 15.6 },
      { ...agent, agentKey: "b", recordsCount: 21, sessionsCount: 2, recordsPerSession: 10.5 },
      { ...agent, agentKey: "c", recordsCount: 0, sessionsCount: 0, recordsPerSession: null },
      {
        ...agent, agentKey: "d", recordsCount: 0, sessionsCount: 3, recordsPerSession: 0,
        mean: null,
      },
      { ...agent, agentKey: "e", recordsCount: 10, sessionsCount: 3, recordsPerSession: 9.9 },
    ];
    fake.resolve("get_context_history_report", reportResponse(rows));
    fake.resolve("get_context_history_agent_detail", {
      ...detailResponse,
      recordsCount: 1560,
      sessionsCount: 100,
      recordsPerSession: 15.6,
    });
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='e']"));
    const perSession = (key: string) =>
      must(must(view.root, `[data-agent-key='${key}']`), "[data-metric='records-per-session']")
        .textContent;
    expect(perSession("a")).toBe("15.6 registros/sesión");
    expect(perSession("b")).toBe("10.5 registros/sesión");
    expect(perSession("c")).toBe("— registros/sesión");
    expect(perSession("d")).toBe("0.0 registros/sesión");
    expect(perSession("e")).toBe("9.9 registros/sesión");
    const metrics = [...must(view.root, "[data-agent-key='a']").querySelectorAll("[data-metric]")]
      .map((el) => el.getAttribute("data-metric"));
    expect(metrics.slice(0, 3)).toEqual(["count", "records-per-session", "mean"]);
    click(must(view.root, "[data-agent-key='a']"));
    await waitFor(() => void must(view.root, '[data-ac-testid="contextHistory.detailTotals"]'));
    const totals = must(view.root, '[data-ac-testid="contextHistory.detailTotals"]');
    expect(must(totals, "[data-metric='records-per-session']").textContent)
      .toBe("15.6 registros/sesión");
    expect(must(totals, "[data-metric='mean']").textContent).toBe(`${TEXT.meanLabel}: 42.3`);
  });

  it("shows each scope's requested and available sessions with its team", async () => {
    const fake = new FakeTransport();
    const scope = (
      teamAttribution: TeamAttribution,
      teamIds: string[],
      availableSessions: number,
    ): ScopeWindowCoverage =>
      ({ teamAttribution, teamIds, requestedSessions: 300, availableSessions });
    const scoped: AgentSummary = {
      ...agent,
      scopeWindows: [
        scope("known", ["team-a"], 300),
        scope("known", ["team-b"], 2),
        scope("unknown", [], 1),
        scope("ambiguous", [], 0),
      ],
    };
    fake.resolve("get_context_history_report", reportResponse([scoped]));
    fake.resolve("get_context_history_agent_detail", {
      ...detailResponse,
      scopeWindows: scoped.scopeWindows,
    });
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const texts = (root: Element) =>
      [...root.querySelectorAll("[data-metric='scope-window']")].map((el) => el.textContent);
    const expected = [
      "team-a: Últimas 300 sesiones",
      "team-b: Últimas 300 sesiones (2 disponibles)",
      `${TEXT.unknown}: Últimas 300 sesiones (1 disponibles)`,
      `${TEXT.ambiguous}: Últimas 300 sesiones (0 disponibles)`,
    ];
    expect(texts(must(view.root, "[data-agent-key='dev']"))).toEqual(expected);
    click(must(view.root, "[data-agent-key='dev']"));
    await waitFor(() => void must(view.root, '[data-ac-testid="contextHistory.detailTotals"]'));
    expect(texts(must(view.root, '[data-ac-testid="contextHistory.detailTotals"]'))).toEqual(expected);
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

// #3532 P18 - options keep a plain UTF-16 code-unit order (Sonar S2871), and
// the region's Escape listener is a native one with a clean lifecycle (S6847).
describe("ContextHistoryApp P18 order and Escape lifecycle", () => {
  let cleanup: (() => void) | undefined;
  afterEach(() => {
    cleanup?.();
    cleanup = undefined;
    platform.isTauri = true;
  });

  // Written out by hand, not computed with the helper under test.
  const MIXED = ["a2", "", "a", "\u{1F600}", "A", "ä", "a10", "a"];
  const EXPECTED = ["A", "a", "a10", "a2", "ä", "\u{1F600}", ""];
  const mixedAgent = (values: string[]): AgentSummary => ({
    ...agent,
    metadataBuckets: values.map((value) => ({
      ...agent.metadataBuckets[0],
      model: value,
      teamIds: [value],
    })),
  });
  const optionValues = (select: HTMLSelectElement) => [...select.options].map((o) => o.value);
  const selectsOf = (root: Element) => {
    const all = [...root.querySelectorAll<HTMLSelectElement>("select")];
    const team = all.find((s) => s.options[0]?.value === "")!;
    return { team, model: all[3] };
  };

  it("orders team and metadata options by code unit, deduplicated", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([mixedAgent(MIXED)]));
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const { team, model } = selectsOf(view.root);
    expect(optionValues(model)).toEqual(["any", "unknown", ...EXPECTED.map((v) => `exact:${v}`)]);
    expect(optionValues(team)).toEqual(["", ...EXPECTED]);
  });

  it("keeps a selected value that the next report no longer has, in order", async () => {
    const fake = new FakeTransport();
    let calls = 0;
    fake.onInvoke("get_context_history_report", () =>
      reportResponse([mixedAgent(calls++ === 0 ? MIXED : ["A", ""])]),
    );
    const view = mount(fake);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const { model } = selectsOf(view.root);
    model.value = "exact:a2";
    model.dispatchEvent(new Event("change", { bubbles: true }));
    await waitFor(() => expect(calls).toBe(2));
    await waitFor(() =>
      expect(optionValues(selectsOf(view.root).model)).toEqual(
        ["any", "unknown", "exact:A", "exact:a2", "exact:"],
      ),
    );
  });

  it("closes once on Escape from the region or a descendant; other keys do nothing", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([agent]));
    const onClose = vi.fn();
    const view = mount(fake, onClose);
    cleanup = view.cleanup;
    await waitFor(() => void must(view.root, "[data-agent-key='dev']"));
    const region = must(view.root, ".ch-overlay");
    expect(region.getAttribute("role")).toBe("region");
    expect(region.getAttribute("tabindex")).toBe("-1");
    expect(document.activeElement).toBe(region);

    const outer = vi.fn();
    document.body.addEventListener("keydown", outer);
    const other = new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true });
    region.dispatchEvent(other);
    expect(onClose).not.toHaveBeenCalled();
    expect(other.defaultPrevented).toBe(false);

    const fromChild = new KeyboardEvent("keydown", {
      key: "Escape", bubbles: true, cancelable: true,
    });
    must(view.root, "[data-agent-key='dev']").dispatchEvent(fromChild);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(fromChild.defaultPrevented).toBe(true);
    // No stopPropagation: the event still reaches outer listeners.
    expect(outer).toHaveBeenCalledTimes(2);
    document.body.removeEventListener("keydown", outer);
  });

  it("leaves no listener on the retained node after dispose and works after remount", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_context_history_report", reportResponse([]));
    const first = vi.fn();
    const view = mount(fake, first);
    const region = must(view.root, ".ch-overlay");
    view.cleanup();
    region.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(first).not.toHaveBeenCalled();

    const second = vi.fn();
    const again = mount(fake, second);
    cleanup = again.cleanup;
    must(again.root, ".ch-overlay").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();
  });
});
