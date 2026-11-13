import { createSignal, type Accessor } from "solid-js";
import { ContextHistoryAPI } from "../shared/ipc";
import {
  type AgentDetailResponse,
  type AgentSummary,
  type ChangesPageResponse,
  type HistoryRecord,
  type LaunchFilter,
  type ReportQuery,
  type ReportResponse,
  type SessionSummary,
} from "../shared/context-history";

// #3532 P06 - per-instance state of the history view. Every request is
// stamped with a generation; changing project, filters or selection bumps it
// so a late response of an older request is ignored (there is no IPC cancel;
// the backend bounds each read). Nothing is aggregated or persisted here: the
// view shows the backend's counts and means as served.
// #4091 Count mode: the newest `sessionCount` sessions per scope before a
// cutoff pinned at refresh. Counts come only from the loaded settings list.

export interface HistoryFilters {
  teamId: string | null;
  /** A member of the loaded session-window list; null until one is loaded. */
  sessionCount: number | null;
  codingAgentId: LaunchFilter;
  model: LaunchFilter;
  effort: LaunchFilter;
}

export const DEFAULT_FILTERS: HistoryFilters = {
  teamId: null,
  sessionCount: null,
  codingAgentId: { kind: "any" },
  model: { kind: "any" },
  effort: { kind: "any" },
};

/** One loadable section: its latest result, loading flag and error. */
export interface Section<T> {
  loading: boolean;
  error: boolean;
  data: T | null;
}

const idle = <T>(): Section<T> => ({ loading: false, error: false, data: null });

export interface HistoryState {
  projectPath: Accessor<string>;
  filters: Accessor<HistoryFilters>;
  /** Loaded session-window counts; null when none is configured. */
  windows: Accessor<readonly number[] | null>;
  report: Accessor<Section<ReportResponse>>;
  detail: Accessor<Section<AgentDetailResponse>>;
  changes: Accessor<Section<ChangesPageResponse>>;
  selectedAgent: Accessor<string | null>;
  selectedSession: Accessor<string | null>;
  setProject: (projectPath: string) => void;
  setFilters: (patch: Partial<HistoryFilters>) => void;
  /** Loads the configured counts; a changed list resets pages and selection. */
  setWindows: (counts: readonly number[] | null) => void;
  refresh: () => Promise<void>;
  loadMoreAgents: () => Promise<void>;
  selectAgent: (agentKey: string | null) => Promise<void>;
  loadMoreSessions: () => Promise<void>;
  selectSession: (logicalSessionId: string | null) => Promise<void>;
  loadMoreRecords: () => Promise<void>;
  dispose: () => void;
}

export function createHistoryState(
  initialProject: string,
  now: () => number = Date.now,
): HistoryState {
  const [projectPath, setProjectPath] = createSignal(initialProject);
  const [filters, setFiltersSignal] = createSignal<HistoryFilters>({ ...DEFAULT_FILTERS });
  const [windows, setWindowsSignal] = createSignal<readonly number[] | null>(null);
  const [report, setReport] = createSignal<Section<ReportResponse>>(idle());
  const [detail, setDetail] = createSignal<Section<AgentDetailResponse>>(idle());
  const [changes, setChanges] = createSignal<Section<ChangesPageResponse>>(idle());
  const [selectedAgent, setSelectedAgent] = createSignal<string | null>(null);
  const [selectedSession, setSelectedSession] = createSignal<string | null>(null);

  // Generations: report covers project/filters/period; detail and changes
  // also move with their own selection.
  let reportGen = 0;
  let detailGen = 0;
  let changesGen = 0;
  let disposed = false;
  let cutoffUtcMs = now();

  const configured = (count: number | null): count is number =>
    count !== null && (windows()?.includes(count) ?? false);

  // fromUtcMs 0 and the pinned cutoff are compatibility fields, not a date range.
  const baseQuery = (cursor: string | null, agentKey: string | null): ReportQuery | null => {
    const f = filters();
    if (!configured(f.sessionCount)) return null;
    return {
      projectPath: projectPath(),
      fromUtcMs: 0,
      toUtcMs: cutoffUtcMs,
      agentKey,
      teamId: f.teamId,
      codingAgentId: f.codingAgentId,
      model: f.model,
      effort: f.effort,
      cursor,
      sessionCount: f.sessionCount,
    };
  };

  const resetAll = () => {
    reportGen += 1;
    setReport(idle<ReportResponse>());
    clearDetail();
  };

  const clearChanges = () => {
    changesGen += 1;
    setSelectedSession(null);
    setChanges(idle<ChangesPageResponse>());
  };

  const clearDetail = () => {
    detailGen += 1;
    setSelectedAgent(null);
    setDetail(idle<AgentDetailResponse>());
    clearChanges();
  };

  const loadReport = async (append: boolean) => {
    const current = report().data;
    const cursor = append ? current?.nextAgentCursor ?? null : null;
    if (append && cursor === null) return;
    const query = baseQuery(cursor, null);
    if (query === null) return;
    const gen = ++reportGen;
    setReport({ loading: true, error: false, data: append ? current : null });
    try {
      const page = await ContextHistoryAPI.report(query);
      if (disposed || gen !== reportGen) return;
      const data =
        append && current
          ? { ...page, agentsPage: [...current.agentsPage, ...page.agentsPage] }
          : page;
      setReport({ loading: false, error: false, data });
    } catch {
      if (disposed || gen !== reportGen) return;
      setReport({ loading: false, error: true, data: append ? current : null });
    }
  };

  const loadDetail = async (agentKey: string, append: boolean) => {
    const current = detail().data;
    const cursor = append ? current?.nextSessionCursor ?? null : null;
    if (append && cursor === null) return;
    const query = baseQuery(cursor, agentKey);
    if (query === null) return;
    const gen = ++detailGen;
    setDetail({ loading: true, error: false, data: append ? current : null });
    try {
      const page = await ContextHistoryAPI.agentDetail(query);
      if (disposed || gen !== detailGen) return;
      const data =
        append && current
          ? { ...page, sessionsPage: [...current.sessionsPage, ...page.sessionsPage] }
          : page;
      setDetail({ loading: false, error: false, data });
    } catch {
      if (disposed || gen !== detailGen) return;
      setDetail({ loading: false, error: true, data: append ? current : null });
    }
  };

  const loadChanges = async (agentKey: string, sessionId: string, append: boolean) => {
    const current = changes().data;
    const cursor = append ? current?.nextRecordCursor ?? null : null;
    if (append && cursor === null) return;
    const query = baseQuery(cursor, agentKey);
    if (query === null) return;
    const gen = ++changesGen;
    setChanges({ loading: true, error: false, data: append ? current : null });
    try {
      const page = await ContextHistoryAPI.changes({
        query,
        logicalSessionId: sessionId,
      });
      if (disposed || gen !== changesGen) return;
      const data =
        append && current
          ? { ...page, recordsPage: [...current.recordsPage, ...page.recordsPage] }
          : page;
      setChanges({ loading: false, error: false, data });
    } catch {
      if (disposed || gen !== changesGen) return;
      setChanges({ loading: false, error: true, data: append ? current : null });
    }
  };

  const refresh = async () => {
    if (!configured(filters().sessionCount)) return;
    cutoffUtcMs = now();
    const agent = selectedAgent();
    const session = selectedSession();
    const work = [loadReport(false)];
    if (agent !== null) {
      work.push(loadDetail(agent, false));
      if (session !== null) work.push(loadChanges(agent, session, false));
    }
    await Promise.all(work);
  };

  return {
    projectPath,
    filters,
    windows,
    report,
    detail,
    changes,
    selectedAgent,
    selectedSession,
    setProject: (next) => {
      if (next === projectPath()) return;
      setProjectPath(next);
      resetAll();
    },
    setFilters: (patch) => {
      const next = { ...filters(), ...patch };
      // An unconfigured count never reaches a query.
      if (!configured(next.sessionCount)) return;
      setFiltersSignal(next);
      resetAll();
    },
    setWindows: (counts) => {
      const list = counts && counts.length > 0 ? [...counts] : null;
      const current = windows();
      const same =
        current !== null && list !== null &&
        current.length === list.length && current.every((count, i) => count === list[i]);
      if (same) return;
      setWindowsSignal(list);
      const selected = filters().sessionCount;
      let sessionCount: number | null = null;
      if (list !== null) {
        sessionCount = selected !== null && list.includes(selected)
          ? selected
          : list[list.length - 1] ?? null;
      }
      setFiltersSignal({ ...filters(), sessionCount });
      resetAll();
    },
    refresh,
    loadMoreAgents: () => loadReport(true),
    selectAgent: async (agentKey) => {
      clearDetail();
      if (agentKey === null) return;
      setSelectedAgent(agentKey);
      await loadDetail(agentKey, false);
    },
    loadMoreSessions: async () => {
      const agent = selectedAgent();
      if (agent !== null) await loadDetail(agent, true);
    },
    selectSession: async (sessionId) => {
      clearChanges();
      const agent = selectedAgent();
      if (sessionId === null || agent === null) return;
      setSelectedSession(sessionId);
      await loadChanges(agent, sessionId, false);
    },
    loadMoreRecords: async () => {
      const agent = selectedAgent();
      const session = selectedSession();
      if (agent !== null && session !== null) await loadChanges(agent, session, true);
    },
    dispose: () => {
      disposed = true;
      reportGen += 1;
      detailGen += 1;
      changesGen += 1;
    },
  };
}

/** Sessions of the selected agent, in the backend's order. */
export function sessionsOf(section: Section<AgentDetailResponse>): SessionSummary[] {
  return section.data?.sessionsPage ?? [];
}

/** Agents of the current report, in the backend's order. */
export function agentsOf(section: Section<ReportResponse>): AgentSummary[] {
  return section.data?.agentsPage ?? [];
}

/** Records of the selected session, in the backend's order. */
export function recordsOf(section: Section<ChangesPageResponse>): HistoryRecord[] {
  return section.data?.recordsPage ?? [];
}
