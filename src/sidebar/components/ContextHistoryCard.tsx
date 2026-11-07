// #3532 P11 - presentational card with the three context means of a row.
// It only renders the state it is given: no owner, no IPC, no settings writes.
import { Component, For, Match, Show, Switch, createEffect, createSignal } from "solid-js";
import { Portal } from "solid-js/web";
import type { SidebarBucket } from "../../shared/context-history";
import type { SidebarHistoryState } from "../context-history";

export type HistoryDays = 1 | 7 | 30;

const MARGIN = 8;

export function formatMean(mean: number): string {
  return `${mean.toFixed(1)}%`;
}

function periodLabel(days: number): string {
  return days === 1 ? "Último día" : `Últimos ${days} días`;
}

function basisLabel(days: HistoryDays): string {
  return days === 1 ? "último día" : `últimos ${days} días`;
}

function records(count: number): string {
  return `${count} ${count === 1 ? "registro" : "registros"}`;
}

const Bar: Component<{ bucket: SidebarBucket; selected: boolean }> = (props) => (
  <div
    class={`ctx-history-row${props.selected ? " selected" : ""}`}
    data-ac-days={props.bucket.days}
  >
    <span class="ctx-history-period">{periodLabel(props.bucket.days)}</span>
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
        fallback={`Sin datos · ${records(props.bucket.recordsCount)}`}
      >
        {`${formatMean(props.bucket.mean ?? 0)} · ${records(props.bucket.recordsCount)}`}
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
  state: SidebarHistoryState;
  percent: number | null;
  projectPath: string;
  colorDays: HistoryDays;
  warning: string;
  cardRef?: (el: HTMLDivElement) => void;
  onLeave?: (related: EventTarget | null) => void;
}> = (props) => {
  const [pos, setPos] = createSignal({ left: 0, top: 0 });
  let el: HTMLDivElement | undefined;

  // Fixed to the viewport so the sidebar's overflow never clips it.
  createEffect(() => {
    if (!props.open) return;
    const anchor = props.anchor()?.getBoundingClientRect();
    if (!anchor || !el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.max(MARGIN, Math.min(anchor.left, window.innerWidth - w - MARGIN));
    const below = anchor.bottom + 4;
    const top = below + h + MARGIN > window.innerHeight
      ? Math.max(MARGIN, anchor.top - h - 4)
      : below;
    setPos({ left, top });
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
        onMouseLeave={(e) => props.onLeave?.(e.relatedTarget)}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
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
              const s = ready() as Extract<SidebarHistoryState, { status: "ready" }>;
              return (
                <>
                  <dl class="ctx-history-identity">
                    <dt>Agente</dt>
                    <dd>{s.scope.agentKey ?? "—"}</dd>
                    <dt>Team</dt>
                    <dd>{s.scope.teamIds.join(", ") || "—"}</dd>
                    <dt>Proyecto</dt>
                    <dd>{props.projectPath}</dd>
                  </dl>
                  <For each={[s.windows.day, s.windows.week, s.windows.month]}>
                    {(bucket) => <Bar bucket={bucket} selected={bucket.days === props.colorDays} />}
                  </For>
                  <p class="ctx-history-note">Color basado en: {basisLabel(props.colorDays)}</p>
                  <p class="ctx-history-note">
                    Sesiones iniciadas en cada período; sesión entera.
                  </p>
                  <p class="ctx-history-legend">
                    <span class="ctx-history-swatch low" /> menor media
                    <span class="ctx-history-swatch high" /> mayor media
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
