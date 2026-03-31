import { Component } from "solid-js";
import type { Session, Team } from "../../shared/types";

function statusClass(session: Session | null): string {
  if (!session) return "offline";
  const s = session.status;
  if (typeof s === "string") return s;
  return "exited";
}

const TeamGroupHeader: Component<{
  team: Team;
  coordinator: Session | null;
  collapsed: boolean;
  onToggle: () => void;
}> = (props) => {
  return (
    <div class="team-group-header" onClick={() => props.onToggle()}>
      <span class={`team-group-chevron ${props.collapsed ? "" : "expanded"}`}>&#9654;</span>
      <span class={`team-group-status ${statusClass(props.coordinator)}`} />
      <span class="team-group-name">{props.team.name}</span>
      <span class="team-group-count">
        {props.team.members.length - (props.team.coordinatorName ? 1 : 0)}
      </span>
    </div>
  );
};

export default TeamGroupHeader;
