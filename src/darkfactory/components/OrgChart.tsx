import { Component, createSignal, For, onMount, onCleanup } from "solid-js";
import type { DarkFactoryConfig } from "../../shared/types";
import LayerColumn from "./LayerColumn";
import ConnectionLines from "./ConnectionLines";

interface OrgChartProps {
  config: DarkFactoryConfig;
}

const OrgChart: Component<OrgChartProps> = (props) => {
  const [hoveredTeamId, setHoveredTeamId] = createSignal<string | null>(null);
  const [nodeRects, setNodeRects] = createSignal<Map<string, DOMRect>>(new Map());
  let wrapperRef: HTMLDivElement | undefined;
  let resizeObserver: ResizeObserver | undefined;

  const registerNodeRect = (teamId: string, rect: DOMRect) => {
    setNodeRects((prev) => {
      const next = new Map(prev);
      next.set(teamId, rect);
      return next;
    });
  };

  const recalculateAll = () => {
    // Force recalculation by clearing and letting nodes re-report
    setNodeRects(new Map());
    // Dispatch a custom event that TeamNodes listen for
    wrapperRef?.dispatchEvent(new CustomEvent("df-recalc"));
  };

  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  const debouncedRecalc = () => {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(recalculateAll, 100);
  };

  onMount(() => {
    if (wrapperRef) {
      resizeObserver = new ResizeObserver(debouncedRecalc);
      resizeObserver.observe(wrapperRef);
    }
  });

  onCleanup(() => {
    resizeObserver?.disconnect();
    if (debounceTimer) clearTimeout(debounceTimer);
  });

  return (
    <div class="df-orgchart-wrapper" ref={wrapperRef}>
      <ConnectionLines
        links={props.config.coordinatorLinks}
        teams={props.config.teams}
        nodeRects={nodeRects()}
        hoveredTeamId={hoveredTeamId()}
      />
      <div class="df-orgchart">
        <For each={props.config.layers}>
          {(layer) => (
            <LayerColumn
              layer={layer}
              teams={props.config.teams.filter((t) => t.layerId === layer.id)}
              hoveredTeamId={hoveredTeamId()}
              onHoverTeam={setHoveredTeamId}
              onNodeRect={registerNodeRect}
              wrapperRef={wrapperRef}
            />
          )}
        </For>
      </div>
    </div>
  );
};

export default OrgChart;
