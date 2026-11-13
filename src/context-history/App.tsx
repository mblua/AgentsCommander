import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  on,
  onCleanup,
  onMount,
  type Component,
} from "solid-js";
import { isTauri } from "../shared/platform";
import { SettingsAPI } from "../shared/ipc";
import {
  formatRecordsPerSession,
  sessionWindowLabel,
  type AgentSummary,
  type BoundaryReason,
  type HistoryRecord,
  type LaunchFilter,
  type MetadataBucket,
  type ScopeWindowCoverage,
  type SessionSummary,
} from "../shared/context-history";
import {
  agentsOf,
  createHistoryState,
  recordsOf,
  sessionsOf,
  type HistoryFilters,
} from "./state";
import "./styles.css";

// #3532 P06 - "Contexto por agente": agents on the left, the selected agent's
// sessions and one session's timeline on the right. Every number shown comes
// from the backend; refresh is manual and there is no polling.

export interface ContextHistoryAppProps {
  /** Canonical path of the registered project to read. */
  projectPath: string;
  onClose: () => void;
  /** Clock seam for tests; defaults to Date.now. */
  now?: () => number;
}

export const TEXT = {
  title: "Contexto por agente",
  loading: "Cargando historial…",
  empty: "No hay registros para estos filtros.",
  error: "No se pudo leer el historial.",
  retry: "Reintentar",
  refresh: "Actualizar",
  close: "Cerrar",
  back: "Volver a agentes",
  periodNote: "Últimas sesiones de cada agente y team; métricas de la sesión entera",
  launchNote: "Modelo y esfuerzo del comando de lanzamiento. Pueden ser desconocidos.",
  coverageNote:
    "Observación cada 5 s; puede haber huecos de observación y registros " +
    "retirados por retención. Root no se lista.",
  meanLabel: "Contexto medio por registro",
  perSession: "registros/sesión",
  ambiguous: "Ambiguo",
  settingsError: "No se pudo leer la configuración de ventanas de sesiones.",
  unknown: "Desconocido",
  all: "Todos",
  incomplete: "Datos conservados de sesión incompleta",
  desktopOnly: "El historial de contexto solo está disponible en la app de escritorio.",
  moreAgents: "Más agentes",
  moreSessions: "Más sesiones",
  moreRecords: "Más registros",
} as const;

export const BOUNDARY_TEXT: Record<BoundaryReason, string> = {
  observed_zero: "Reinicio observado",
  lifecycle: "Intento de reinicio de AC",
  observation_gap: "Observación interrumpida",
};

export function formatMean(mean: number | null): string {
  return mean === null ? "—" : mean.toFixed(1);
}

export function formatCount(count: number): string {
  return String(Math.trunc(count));
}

const TIME_ZONE = Intl.DateTimeFormat().resolvedOptions().timeZone;

export function formatTime(utcMs: number): string {
  return `${new Date(utcMs).toLocaleString()} (${TIME_ZONE})`;
}

function encodeFilter(filter: LaunchFilter): string {
  return filter.kind === "exact" ? `exact:${filter.value}` : filter.kind;
}

function decodeFilter(value: string): LaunchFilter {
  if (value === "unknown") return { kind: "unknown" };
  if (value.startsWith("exact:")) return { kind: "exact", value: value.slice(6) };
  return { kind: "any" };
}

type MetadataField = "codingAgentId" | "model" | "effort";

/** Plain UTF-16 code-unit order: no locale, collation, normalization or numbers. */
function byCodeUnit(a: string, b: string): number {
  if (a === b) return 0;
  return a < b ? -1 : 1;
}

/** Exact values offered by a launch filter: those seen plus the selected one. */
function exactValues(
  agents: AgentSummary[],
  field: MetadataField,
  selected: LaunchFilter,
): string[] {
  const values = new Set<string>();
  for (const agent of agents) {
    for (const bucket of agent.metadataBuckets as MetadataBucket[]) {
      const value = bucket[field];
      if (value !== null) values.add(value);
    }
  }
  if (selected.kind === "exact") values.add(selected.value);
  return [...values].sort(byCodeUnit);
}

function teamValues(agents: AgentSummary[], selected: string | null): string[] {
  const values = new Set<string>();
  for (const agent of agents) {
    for (const bucket of agent.metadataBuckets) {
      for (const team of bucket.teamIds) values.add(team);
    }
  }
  if (selected !== null) values.add(selected);
  return [...values].sort(byCodeUnit);
}

/** #4091 Team attribution of one count-mode scope, never a merged bucket. */
function scopeTeams(scope: ScopeWindowCoverage): string {
  if (scope.teamAttribution === "unknown") return TEXT.unknown;
  if (scope.teamAttribution === "ambiguous") return TEXT.ambiguous;
  return scope.teamIds.length > 0 ? scope.teamIds.join(", ") : TEXT.unknown;
}

const ScopeWindows = (scopes: ScopeWindowCoverage[] | null) => (
  <Show when={scopes && scopes.length > 0}>
    <span data-ac-testid="contextHistory.scopeWindows">
      <For each={scopes ?? []}>
        {(scope) => (
          <span
            class="ch-metric"
            data-metric="scope-window"
            data-team-attribution={scope.teamAttribution}
          >
            {scopeTeams(scope)}:{" "}
            {sessionWindowLabel(scope.requestedSessions, scope.availableSessions)}
          </span>
        )}
      </For>
    </span>
  </Show>
);

function meanBarWidth(mean: number | null): string {
  const value = mean === null ? 0 : Math.max(0, Math.min(100, mean));
  return `${value}%`;
}

const ContextHistoryApp: Component<ContextHistoryAppProps> = (props) => {
  const state = createHistoryState(props.projectPath, props.now);
  let rootRef: HTMLDivElement | undefined;
  let detailRef: HTMLElement | undefined;
  let listRef: HTMLElement | undefined;

  // #4091 Settings load before the first query and every manual refresh/retry;
  // a missing or failed list shows the error and sends no history request.
  const [settingsError, setSettingsError] = createSignal(false);
  let loadGen = 0;
  let disposed = false;
  const loadAsync = async () => {
    const gen = ++loadGen;
    const project = props.projectPath;
    const current = () => !disposed && gen === loadGen && project === props.projectPath;
    let counts: number[] | undefined;
    try {
      counts = (await SettingsAPI.get()).contextHistorySessionWindows;
    } catch {
      counts = undefined;
    }
    if (!current()) return;
    if (!counts || counts.length === 0) {
      setSettingsError(true);
      return;
    }
    setSettingsError(false);
    state.setWindows(counts);
    await state.refresh();
  };
  // Desktop-only: outside Tauri the view never reaches Settings or the history IPC.
  const load = () => {
    if (isTauri) void loadAsync();
  };

  onMount(() => {
    load();
    rootRef?.focus();
  });
  // Native keydown on the region node itself: the region is not a control, so the
  // Escape handler is registered by ref instead of a JSX listener.
  onMount(() => {
    const node = rootRef;
    if (!node) return;
    node.addEventListener("keydown", onKeyDown);
    onCleanup(() => node.removeEventListener("keydown", onKeyDown));
  });
  createEffect(
    on(
      () => props.projectPath,
      (projectPath) => {
        state.setProject(projectPath);
        load();
      },
      { defer: true },
    ),
  );
  onCleanup(() => {
    disposed = true;
    state.dispose();
  });

  const agents = createMemo(() => agentsOf(state.report()));
  const sessions = createMemo(() => sessionsOf(state.detail()));
  const records = createMemo(() => recordsOf(state.changes()));

  const applyFilter = (patch: Partial<HistoryFilters>) => {
    state.setFilters(patch);
    if (isTauri) void state.refresh();
  };

  // Narrow layout hides the list while an agent is open, so focus follows
  // the visible pane: into the detail on select, back to the row on return.
  const narrow = () =>
    typeof window.matchMedia === "function" &&
    window.matchMedia("(max-width: 720px)").matches;

  const openAgent = async (agentKey: string) => {
    const moveFocus = narrow();
    const pending = state.selectAgent(agentKey);
    if (moveFocus) queueMicrotask(() => detailRef?.focus());
    await pending;
  };

  const backToAgents = () => {
    const agentKey = state.selectedAgent();
    void state.selectAgent(null);
    queueMicrotask(() => {
      const rows = listRef?.querySelectorAll<HTMLElement>("[data-agent-key]") ?? [];
      const row = [...rows].find((candidate) => candidate.dataset.agentKey === agentKey);
      (row ?? listRef)?.focus();
    });
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      props.onClose();
    }
  };

  const status = createMemo(() => {
    if (settingsError()) return TEXT.settingsError;
    const report = state.report();
    if (report.loading) return TEXT.loading;
    if (report.error) return TEXT.error;
    if (report.data?.agentsPage.length === 0) return TEXT.empty;
    return "";
  });

  const launchSelect = (label: string, field: MetadataField) => (
    <label class="ch-filter">
      <span>{label}</span>
      <select
        value={encodeFilter(state.filters()[field])}
        onChange={(event) => applyFilter({ [field]: decodeFilter(event.currentTarget.value) })}
      >
        <option value="any">{TEXT.all}</option>
        <option value="unknown">{TEXT.unknown}</option>
        <For each={exactValues(agents(), field, state.filters()[field])}>
          {(value) => <option value={`exact:${value}`}>{value}</option>}
        </For>
      </select>
    </label>
  );

  const AgentRow = (agent: AgentSummary) => (
    <li>
      <button
        type="button"
        class="ch-agent-row"
        classList={{ "ch-selected": state.selectedAgent() === agent.agentKey }}
        aria-pressed={state.selectedAgent() === agent.agentKey}
        data-agent-key={agent.agentKey}
        onClick={() => void openAgent(agent.agentKey)}
      >
        <span class="ch-agent-name">{agent.agentName}</span>
        <span class="ch-metric" data-metric="count">
          {formatCount(agent.recordsCount)} registros
        </span>
        <span class="ch-metric" data-metric="records-per-session">
          {formatRecordsPerSession(agent.recordsPerSession)} {TEXT.perSession}
        </span>
        <span class="ch-metric" data-metric="mean">
          {TEXT.meanLabel}: {formatMean(agent.mean)}
        </span>
        {ScopeWindows(agent.scopeWindows)}
        <span class="ch-bar" aria-hidden="true">
          <span class="ch-bar-fill" style={{ width: meanBarWidth(agent.mean) }} />
        </span>
        <Show when={agent.incomplete}>
          <span class="ch-note">{TEXT.incomplete}</span>
        </Show>
      </button>
    </li>
  );

  const SessionRow = (session: SessionSummary) => (
    <li>
      <button
        type="button"
        class="ch-session-row"
        classList={{ "ch-selected": state.selectedSession() === session.logicalSessionId }}
        aria-pressed={state.selectedSession() === session.logicalSessionId}
        data-session-id={session.logicalSessionId}
        onClick={() => void state.selectSession(session.logicalSessionId)}
      >
        <span>{formatTime(session.logicalStartedAtUtcMs)}</span>
        <span class="ch-provenance">{session.room ?? session.instanceFqn}</span>
        <span class="ch-metric" data-metric="count">
          {formatCount(session.recordsCount)} registros
        </span>
        <span class="ch-metric" data-metric="mean">
          {TEXT.meanLabel}: {formatMean(session.mean)}
        </span>
        <span class="ch-metric" data-metric="initial">
          Inicial: {session.initialValue === null ? TEXT.unknown : `${session.initialValue}%`}
        </span>
        <span class="ch-metric" data-metric="max">
          Máximo: {session.max === null ? TEXT.unknown : `${session.max}%`}
        </span>
        <span class="ch-metric" data-metric="launch">
          {session.codingAgentId ?? TEXT.unknown} · {session.model ?? TEXT.unknown} ·{" "}
          {session.effort ?? TEXT.unknown}
        </span>
        <Show when={session.partialStart || session.partialEnd || session.retentionClipped}>
          <span class="ch-note">{session.coverageLabel ?? TEXT.incomplete}</span>
        </Show>
        <Show when={session.boundaryReason}>
          {(reason) => <span class="ch-note">{BOUNDARY_TEXT[reason()]}</span>}
        </Show>
      </button>
    </li>
  );

  const RecordRow = (record: HistoryRecord) => (
    <li class="ch-record" data-kind={record.kind}>
      <time datetime={new Date(record.observedAtUtcMs).toISOString()}>
        {formatTime(record.observedAtUtcMs)}
      </time>
      <Show
        when={record.kind === "boundary" && record.boundaryReason}
        fallback={<span class="ch-metric">{record.percent ?? 0}%</span>}
      >
        {(reason) => <span class="ch-boundary">{BOUNDARY_TEXT[reason()]}</span>}
      </Show>
    </li>
  );

  return (
    <div
      ref={rootRef}
      class="ch-overlay"
      role="region"
      aria-labelledby="ch-title"
      tabIndex={-1}
    >
      <div class="ch-panel" classList={{ "ch-has-selection": state.selectedAgent() !== null }}>
        <header class="ch-header">
          <h2 id="ch-title">{TEXT.title}</h2>
          <div class="ch-actions">
            <Show when={isTauri}>
              <button type="button" onClick={load}>
                {TEXT.refresh}
              </button>
            </Show>
            <button type="button" onClick={() => props.onClose()}>
              {TEXT.close}
            </button>
          </div>
        </header>
        <Show when={isTauri} fallback={<p class="ch-status">{TEXT.desktopOnly}</p>}>
          <div class="ch-filters">
            <label class="ch-filter">
              <span>Team</span>
              <select
                value={state.filters().teamId ?? ""}
                onChange={(event) =>
                  applyFilter({ teamId: event.currentTarget.value || null })
                }
              >
                <option value="">{TEXT.all}</option>
                <For each={teamValues(agents(), state.filters().teamId)}>
                  {(team) => <option value={team}>{team}</option>}
                </For>
              </select>
            </label>
            <label class="ch-filter">
              <span>Sesiones</span>
              <select
                value={String(state.filters().sessionCount ?? "")}
                onChange={(event) =>
                  applyFilter({ sessionCount: Number(event.currentTarget.value) })
                }
                data-ac-testid="contextHistory.sessionCount"
              >
                <For each={state.windows() ?? []}>
                  {(count) => <option value={String(count)}>{sessionWindowLabel(count)}</option>}
                </For>
              </select>
            </label>
            {launchSelect("Agente de programación", "codingAgentId")}
            {launchSelect("Modelo al iniciar", "model")}
            {launchSelect("Esfuerzo al iniciar", "effort")}
          </div>
          <div class="ch-notes">
            <p>{TEXT.periodNote}</p>
            <p>{TEXT.launchNote}</p>
            <p>{TEXT.coverageNote}</p>
          </div>
          <p class="ch-status" role="status" aria-live="polite">
            {status()}
          </p>
          <Show when={settingsError() || state.report().error}>
            <button type="button" data-ac-testid="contextHistory.retry" onClick={load}>
              {TEXT.retry}
            </button>
          </Show>
          <div class="ch-body">
            <section ref={listRef} class="ch-list" aria-label="Agentes" tabIndex={-1}>
              <Show when={state.report().data}>
                {(report) => (
                  <p data-metric="total-agents">
                    {formatCount(report().totalAgents)} agentes
                  </p>
                )}
              </Show>
              <ul>
                <For each={agents()}>{AgentRow}</For>
              </ul>
              <Show when={state.report().data?.nextAgentCursor}>
                <button type="button" onClick={() => void state.loadMoreAgents()}>
                  {TEXT.moreAgents}
                </button>
              </Show>
            </section>
            <Show when={state.selectedAgent()}>
              <section
                ref={detailRef}
                class="ch-detail"
                aria-label="Detalle del agente"
                tabIndex={-1}
              >
                <button
                  type="button"
                  class="ch-back"
                  onClick={backToAgents}
                >
                  {TEXT.back}
                </button>
                <Show when={state.detail().loading}>
                  <p class="ch-status">{TEXT.loading}</p>
                </Show>
                <Show when={state.detail().error}>
                  <p class="ch-status">{TEXT.error}</p>
                </Show>
                <Show when={state.detail().data}>
                  {(detail) => (
                    <div data-ac-testid="contextHistory.detailTotals">
                      <span data-metric="count">
                        {formatCount(detail().recordsCount)} registros
                      </span>
                      {" · "}
                      <span data-metric="records-per-session">
                        {formatRecordsPerSession(detail().recordsPerSession)} {TEXT.perSession}
                      </span>
                      {" · "}
                      <span data-metric="mean">
                        {TEXT.meanLabel}: {formatMean(detail().mean)}
                      </span>
                      {ScopeWindows(detail().scopeWindows)}
                    </div>
                  )}
                </Show>
                <ul>
                  <For each={sessions()}>{SessionRow}</For>
                </ul>
                <Show when={state.detail().data?.nextSessionCursor}>
                  <button type="button" onClick={() => void state.loadMoreSessions()}>
                    {TEXT.moreSessions}
                  </button>
                </Show>
                <Show when={state.selectedSession()}>
                  <section aria-label="Línea de tiempo">
                    <Show when={state.changes().loading}>
                      <p class="ch-status">{TEXT.loading}</p>
                    </Show>
                    <Show when={state.changes().error}>
                      <p class="ch-status">{TEXT.error}</p>
                    </Show>
                    <ol>
                      <For each={records()}>{RecordRow}</For>
                    </ol>
                    <Show when={state.changes().data?.nextRecordCursor}>
                      <button type="button" onClick={() => void state.loadMoreRecords()}>
                        {TEXT.moreRecords}
                      </button>
                    </Show>
                  </section>
                </Show>
              </section>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
};

export default ContextHistoryApp;
