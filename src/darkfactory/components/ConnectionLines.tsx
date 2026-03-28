import { Component, For, Show } from "solid-js";
import type { CoordinatorLink, Team } from "../../shared/types";

interface ConnectionLinesProps {
  links: CoordinatorLink[];
  teams: Team[];
  nodeRects: Map<string, DOMRect>;
  hoveredTeamId: string | null;
}

const ConnectionLines: Component<ConnectionLinesProps> = (props) => {
  const isHighlighted = (link: CoordinatorLink) => {
    return (
      props.hoveredTeamId === link.supervisorTeamId ||
      props.hoveredTeamId === link.subordinateTeamId
    );
  };

  const getPath = (link: CoordinatorLink): string | null => {
    const fromRect = props.nodeRects.get(link.supervisorTeamId);
    const toRect = props.nodeRects.get(link.subordinateTeamId);
    if (!fromRect || !toRect) return null;

    // From right edge center of supervisor to left edge center of subordinate
    const x1 = fromRect.x + fromRect.width;
    const y1 = fromRect.y + fromRect.height / 2;
    const x2 = toRect.x;
    const y2 = toRect.y + toRect.height / 2;

    // Horizontal bezier curve
    const dx = Math.abs(x2 - x1) * 0.5;
    return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
  };

  return (
    <svg class="df-connections">
      <For each={props.links}>
        {(link) => {
          const path = () => getPath(link);
          return (
            <Show when={path()}>
              <path
                class={`df-connection-path${isHighlighted(link) ? " highlighted" : ""}`}
                d={path()!}
              />
            </Show>
          );
        }}
      </For>
    </svg>
  );
};

export default ConnectionLines;
