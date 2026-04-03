import { Component, createSignal, For, Show, onMount } from "solid-js";
import type { AcAgentMatrix, AcTeam } from "../../shared/types";
import { AcDiscoveryAPI, SessionAPI } from "../../shared/ipc";

const AcDiscoveryPanel: Component = () => {
  const [agents, setAgents] = createSignal<AcAgentMatrix[]>([]);
  const [teams, setTeams] = createSignal<AcTeam[]>([]);
  const [collapsed, setCollapsed] = createSignal(false);
  const [loading, setLoading] = createSignal(true);

  /** Find which teams an agent belongs to */
  const teamsForAgent = (agentName: string): string[] => {
    return teams()
      .filter((t) => t.agents.includes(agentName))
      .map((t) => t.name);
  };

  /** Check if agent is a coordinator of any team */
  const isCoordinator = (agentName: string): boolean => {
    return teams().some((t) => t.coordinator === agentName);
  };

  const handleAgentClick = (agent: AcAgentMatrix) => {
    SessionAPI.create({
      cwd: agent.path,
      sessionName: agent.name,
      agentId: agent.preferredAgentId,
    });
  };

  onMount(async () => {
    try {
      const result = await AcDiscoveryAPI.discover();
      setAgents(result.agents);
      setTeams(result.teams);
    } catch (e) {
      console.error("AC discovery failed:", e);
    } finally {
      setLoading(false);
    }
  });

  return (
    <Show when={!loading() && agents().length > 0}>
      <div class="ac-discovery-panel">
        <button
          class="ac-discovery-header"
          onClick={() => setCollapsed((c) => !c)}
        >
          <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
            &#x25BE;
          </span>
          <span class="ac-discovery-title">AC Agents</span>
          <span class="ac-discovery-count">{agents().length}</span>
        </button>
        <Show when={!collapsed()}>
          <div class="ac-discovery-list">
            <For each={agents()}>
              {(agent) => {
                const agentTeams = () => teamsForAgent(agent.name);
                const coord = () => isCoordinator(agent.name);
                return (
                  <div
                    class="ac-discovery-item"
                    onClick={() => handleAgentClick(agent)}
                    title={agent.path}
                  >
                    <div class="ac-discovery-item-info">
                      <span class="ac-discovery-item-name">
                        <span class="ac-discovery-prefix">
                          {agent.name.slice(0, agent.name.lastIndexOf("/") + 1)}
                        </span>
                        {agent.name.slice(agent.name.lastIndexOf("/") + 1)}
                      </span>
                      <div class="ac-discovery-badges">
                        <Show when={coord()}>
                          <span class="ac-discovery-badge coord">C</span>
                        </Show>
                        <For each={agentTeams()}>
                          {(teamName) => (
                            <span class="ac-discovery-badge team">{teamName}</span>
                          )}
                        </For>
                        <Show when={!agent.roleExists}>
                          <span class="ac-discovery-badge no-role">no role</span>
                        </Show>
                      </div>
                    </div>
                  </div>
                );
              }}
            </For>
          </div>
        </Show>
      </div>
    </Show>
  );
};

export default AcDiscoveryPanel;
