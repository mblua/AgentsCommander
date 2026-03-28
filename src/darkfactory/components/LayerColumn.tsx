import { Component, For } from "solid-js";
import type { DarkFactoryLayer, Team } from "../../shared/types";
import TeamNode from "./TeamNode";

interface LayerColumnProps {
  layer: DarkFactoryLayer;
  teams: Team[];
  hoveredTeamId: string | null;
  onHoverTeam: (id: string | null) => void;
  onNodeRect: (teamId: string, rect: DOMRect) => void;
  wrapperRef: HTMLDivElement | undefined;
}

const LayerColumn: Component<LayerColumnProps> = (props) => {
  return (
    <div class="df-layer">
      <div class="df-layer-header">{props.layer.name}</div>
      <div class="df-layer-teams">
        <For each={props.teams}>
          {(team) => (
            <TeamNode
              team={team}
              highlighted={props.hoveredTeamId === team.id}
              onHover={(hovering) => props.onHoverTeam(hovering ? team.id : null)}
              onNodeRect={props.onNodeRect}
              wrapperRef={props.wrapperRef}
            />
          )}
        </For>
      </div>
    </div>
  );
};

export default LayerColumn;
