import { Component, Show, onMount, onCleanup } from "solid-js";
import type { Team } from "../../shared/types";

interface TeamNodeProps {
  team: Team;
  highlighted: boolean;
  onHover: (hovering: boolean) => void;
  onNodeRect: (teamId: string, rect: DOMRect) => void;
  wrapperRef: HTMLDivElement | undefined;
}

const TeamNode: Component<TeamNodeProps> = (props) => {
  let nodeRef: HTMLDivElement | undefined;

  const reportRect = () => {
    if (nodeRef && props.wrapperRef) {
      const wrapperRect = props.wrapperRef.getBoundingClientRect();
      const nodeRect = nodeRef.getBoundingClientRect();
      // Store rect relative to wrapper
      const relativeRect = new DOMRect(
        nodeRect.x - wrapperRect.x,
        nodeRect.y - wrapperRect.y,
        nodeRect.width,
        nodeRect.height,
      );
      props.onNodeRect(props.team.id, relativeRect);
    }
  };

  const handleRecalc = () => reportRect();

  onMount(() => {
    // Report initial position after a frame (let layout settle)
    requestAnimationFrame(reportRect);
    // Listen for recalculation events
    props.wrapperRef?.addEventListener("df-recalc", handleRecalc);
  });

  onCleanup(() => {
    props.wrapperRef?.removeEventListener("df-recalc", handleRecalc);
  });

  const noMembers = () => props.team.members.length === 0;

  return (
    <div
      ref={nodeRef}
      class={`df-team-node${props.highlighted ? " highlighted" : ""}${noMembers() ? " no-members" : ""}`}
      onMouseEnter={() => props.onHover(true)}
      onMouseLeave={() => props.onHover(false)}
    >
      <Show when={props.team.coordinatorName}>
        <div class="df-team-coordinator">
          <span class="df-team-coordinator-star">&#x2605;</span>
          {props.team.coordinatorName}
        </div>
        <div class="df-team-separator" />
      </Show>
      <div class="df-team-name">{props.team.name}</div>
      <div class="df-team-members">
        {props.team.members.length} member{props.team.members.length !== 1 ? "s" : ""}
      </div>
    </div>
  );
};

export default TeamNode;
