// #3532 P11 / #4091 - presentational card with the session-window means of a row.
// It only renders the state it is given: no owner, no IPC, no settings writes.
import {
  Component, For, Match, Show, Switch, createEffect, createSignal, onCleanup, onMount,
} from "solid-js";
import { Portal } from "solid-js/web";
import {
  formatRecordsPerSession,
  sessionWindowLabel,
  type SessionWindowBucket,
  type SidebarScope,
} from "../../shared/context-history";

/** What the card renders; the badge hands the sidebar owner's state as is. */
export type CardHistoryState =
  | { status: "unavailable" }
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ambiguous" | "unknown"; scope: SidebarScope; generatedAtUtcMs: number; text: string }
  | {
      status: "ready";
      scope: SidebarScope;
      sessionWindows: SessionWindowBucket[];
      generatedAtUtcMs: number;
    };

const MARGIN = 8;
/** Space between trigger and card; the badge bridges it with a delayed close. */
export const GAP = 4;

export function formatMean(mean: number): string {
  return `${mean.toFixed(1)}%`;
}

function basisLabel(sessions: number | null): string {
  return sessions === null ? "—" : sessionWindowLabel(sessions).toLowerCase();
}

/** Record count, then backend records per session: "1560 registros · 15.6 registros/sesión". */
function records(bucket: SessionWindowBucket): string {
  const count = bucket.recordsCount;
  const perSession = formatRecordsPerSession(bucket.recordsPerSession);
  return `${count} ${count === 1 ? "registro" : "registros"} · ${perSession} registros/sesión`;
}

const Bar: Component<{ bucket: SessionWindowBucket; selected: boolean }> = (props) => (
  <div
    class={`ctx-history-row${props.selected ? " selected" : ""}`}
    data-ac-sessions={props.bucket.requestedSessions}
  >
    <span class="ctx-history-period">
      {sessionWindowLabel(props.bucket.requestedSessions, props.bucket.sessionsCount)}
    </span>
    <span class="ctx-history-track">
      <Show when={props.bucket.mean}>
        {(mean) => (
          <span class="ctx-history-fill" style={{ width: `${Math.min(100, mean())}%` }} />
        )}
      </Show>
    </span>
    <span class="ctx-history-value">
      <Show
        when={props.bucket.mean !== null && props.bucket.recordsCount > 0}
        fallback={`Sin datos · ${records(props.bucket)}`}
      >
        {`${formatMean(props.bucket.mean ?? 0)} · ${records(props.bucket)}`}
      </Show>
      <Show when={props.bucket.incomplete}>
        <span class="ctx-history-partial"> · cobertura parcial</span>
      </Show>
    </span>
  </div>
);

const ContextHistoryCard: Component<{
  id: string;
  open: boolean;
  anchor: () => HTMLElement | undefined;
  state: CardHistoryState;
  percent: number | null;
  projectPath: string;
  /** requestedSessions of the bucket the badge colour uses; null when none. */
  colorSessions: number | null;
  warning: string;
  cardRef?: (el: HTMLDivElement) => void;
  onEnter?: () => void;
  onLeave?: (related: EventTarget | null) => void;
}> = (props) => {
  const [pos, setPos] = createSignal({ left: 0, top: 0 });
  let el: HTMLDivElement | undefined;

  // Fixed to the viewport so the sidebar's overflow never clips it; the CSS caps
  // the height at the viewport and scrolls, so placement only clamps.
  const place = () => {
    const anchor = props.anchor()?.getBoundingClientRect();
    if (!anchor || !el) return;
    const w = el.offsetWidth;
    const h = Math.min(el.offsetHeight, window.innerHeight - 2 * MARGIN);
    const left = Math.max(MARGIN, Math.min(anchor.left, window.innerWidth - w - MARGIN));
    const below = anchor.bottom + GAP;
    const fitsBelow = below + h + MARGIN <= window.innerHeight;
    const above = anchor.top - h - GAP;
    const top = fitsBelow
      ? below
      : Math.max(MARGIN, Math.min(above, window.innerHeight - h - MARGIN));
    setPos({ left, top });
  };
  createEffect(() => {
    if (!props.open) return;
    void props.state;
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    if (el) observer?.observe(el);
    onCleanup(() => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      observer?.disconnect();
    });
  });

  // Native listeners on the card node: it is presentational, not a control. The
  // Portal re-parents events to the badge row, so the card still stops them here.
  onMount(() => {
    const node = el;
    if (!node) return;
    const enter = () => props.onEnter?.();
    const leave = (e: MouseEvent) => props.onLeave?.(e.relatedTarget);
    const stop = (e: Event) => e.stopPropagation();
    node.addEventListener("mouseenter", enter);
    node.addEventListener("mouseleave", leave);
    node.addEventListener("pointerdown", stop);
    node.addEventListener("click", stop);
    onCleanup(() => {
      node.removeEventListener("mouseenter", enter);
      node.removeEventListener("mouseleave", leave);
      node.removeEventListener("pointerdown", stop);
      node.removeEventListener("click", stop);
    });
  });

  const current = () =>
    props.percent === null
      ? "Contexto actual de esta sesión: no disponible"
      : `Contexto actual de esta sesión: ${props.percent}%`;

  return (
    <Portal>
      <div
        ref={(node) => {
          el = node;
          props.cardRef?.(node);
        }}
        id={props.id}
        class="ctx-history-card"
        role="tooltip"
        hidden={!props.open}
        style={{ left: `${pos().left}px`, top: `${pos().top}px` }}
        data-ac-testid="context-history.card"
        data-ac-state={props.state.status}
      >
        <p class="ctx-history-current">{current()}</p>
        <Switch>
          <Match when={props.state.status === "loading"}>
            <p class="ctx-history-note">Cargando medias de contexto…</p>
          </Match>
          <Match when={props.state.status === "error"}>
            <p class="ctx-history-note">No se pudieron leer las medias de contexto.</p>
          </Match>
          <Match when={props.state.status === "ambiguous"}>
            <p class="ctx-history-note">Datos sin atribución segura a este team</p>
          </Match>
          <Match when={props.state.status === "unknown"}>
            <p class="ctx-history-note">Identidad histórica no disponible</p>
          </Match>
          <Match when={props.state.status === "ready" && props.state}>
            {(ready) => {
              // Read on every access: a ready -> ready update must reach the card.
              const s = () => ready() as Extract<CardHistoryState, { status: "ready" }>;
              return (
                <>
                  <dl class="ctx-history-identity">
                    <dt>Agente</dt>
                    <dd>{s().scope.agentKey ?? "—"}</dd>
                    <dt>Team</dt>
                    <dd>{s().scope.teamIds.join(", ") || "—"}</dd>
                    <dt>Proyecto</dt>
                    <dd>{props.projectPath}</dd>
                  </dl>
                  <For each={s().sessionWindows}>
                    {(bucket) => (
                      <Bar
                        bucket={bucket}
                        selected={bucket.requestedSessions === props.colorSessions}
                      />
                    )}
                  </For>
                  <p class="ctx-history-note">Color basado en: {basisLabel(props.colorSessions)}</p>
                  <p class="ctx-history-note">
                    Sesiones más recientes de este agente y team; sesión entera.
                  </p>
                  <p class="ctx-history-legend">
                    <span class="ctx-history-swatch low" />{" "}menor media
                    <span class="ctx-history-swatch high" />{" "}mayor media
                  </p>
                </>
              );
            }}
          </Match>
        </Switch>
        <p class="ctx-history-warning">
          {props.warning} La lectura viva puede faltar o desactualizarse; una media alta
          no ordena reiniciar. Modelo y esfuerzo no se infieren aquí.
        </p>
      </div>
    </Portal>
  );
};

export default ContextHistoryCard;
