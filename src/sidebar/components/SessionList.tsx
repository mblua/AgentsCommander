import { Component, For, Show } from "solid-js";
import { sessionsStore } from "../stores/sessions";
import SessionItem from "./SessionItem";
import TeamGroupHeader from "./TeamGroupHeader";

const SessionList: Component = () => {
  const groups = () => sessionsStore.groupedSessions.groups;
  const ungrouped = () => sessionsStore.groupedSessions.ungrouped;
  const hasGroups = () => groups().length > 0;
  const isEmpty = () => sessionsStore.filteredSessions.length === 0;

  return (
    <div class="session-list-container">
      <Show
        when={!isEmpty()}
        fallback={
          <div class="empty-state">
            <span>{sessionsStore.teamFilter ? "No sessions in this team" : "No sessions"}</span>
            <span>Click + to create one</span>
          </div>
        }
      >
        <Show when={hasGroups()} fallback={
          <For each={sessionsStore.filteredSessions}>
            {(session) => (
              <SessionItem
                session={session}
                isActive={session.id === sessionsStore.activeId}
              />
            )}
          </For>
        }>
          <For each={groups()}>
            {(group) => {
              const collapsed = () => !!sessionsStore.collapsedTeams[group.team.id];
              return (
                <div class="team-group">
                  <TeamGroupHeader
                    team={group.team}
                    coordinator={group.coordinator}
                    collapsed={collapsed()}
                    onToggle={() => sessionsStore.toggleTeamCollapsed(group.team.id)}
                  />
                  <Show when={group.coordinator}>
                    <div class="team-group-coordinator">
                      <SessionItem
                        session={group.coordinator!}
                        isActive={group.coordinator!.id === sessionsStore.activeId}
                      />
                    </div>
                  </Show>
                  <Show when={!collapsed()}>
                    <div class="team-group-members">
                      <For each={group.members}>
                        {(session) => (
                          <SessionItem
                            session={session}
                            isActive={session.id === sessionsStore.activeId}
                          />
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              );
            }}
          </For>
          <Show when={ungrouped().length > 0}>
            <div class="team-group-ungrouped">
              <For each={ungrouped()}>
                {(session) => (
                  <SessionItem
                    session={session}
                    isActive={session.id === sessionsStore.activeId}
                  />
                )}
              </For>
            </div>
          </Show>
        </Show>
      </Show>
    </div>
  );
};

export default SessionList;
