import { Component, createSignal, For, Show, onMount } from "solid-js";
import type { AcAgentMatrix, AcTeam, AcWorkgroup, AcAgentReplica } from "../../shared/types";
import { AcDiscoveryAPI, SessionAPI } from "../../shared/ipc";

const AcDiscoveryPanel: Component = () => {
  const [agents, setAgents] = createSignal<AcAgentMatrix[]>([]);
  const [teams, setTeams] = createSignal<AcTeam[]>([]);
  const [workgroups, setWorkgroups] = createSignal<AcWorkgroup[]>([]);
  const [collapsed, setCollapsed] = createSignal(false);
  const [wgCollapsed, setWgCollapsed] = createSignal(false);
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

  const handleReplicaClick = (replica: AcAgentReplica, wg: AcWorkgroup) => {
    // Use the repo path as CWD when exactly 1 repo is assigned,
    // so the agent launches directly in its working repo
    const cwd = replica.repoPaths.length === 1
      ? replica.repoPaths[0]
      : replica.path;
    SessionAPI.create({
      cwd,
      sessionName: `${wg.name}/${replica.name}`,
      agentId: replica.preferredAgentId,
    });
  };

  onMount(async () => {
    try {
      const result = await AcDiscoveryAPI.discover();
      setAgents(result.agents);
      setTeams(result.teams);
      setWorkgroups(result.workgroups);
    } catch (e) {
      console.error("AC discovery failed:", e);
    } finally {
      setLoading(false);
    }
  });

  return (
    <Show when={!loading() && (agents().length > 0 || workgroups().length > 0)}>
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
        <Show when={workgroups().length > 0}>
          <button
            class="ac-discovery-header"
            onClick={() => setWgCollapsed((c) => !c)}
          >
            <span class="ac-discovery-chevron" classList={{ collapsed: wgCollapsed() }}>
              &#x25BE;
            </span>
            <span class="ac-discovery-title">Workgroups</span>
            <span class="ac-discovery-count">{workgroups().length}</span>
          </button>
          <Show when={!wgCollapsed()}>
            <div class="ac-discovery-list">
              <For each={workgroups()}>
                {(wg) => (
                  <div class="ac-wg-group">
                    <div class="ac-wg-header" title={wg.path}>
                      <span class="ac-wg-name">{wg.name}</span>
                      <Show when={wg.brief}>
                        <span class="ac-wg-brief">{wg.brief}</span>
                      </Show>
                    </div>
                    <For each={wg.agents}>
                      {(replica) => {
                        const repoCount = () => replica.repoPaths.length;
                        const branchLabel = () => {
                          if (repoCount() === 1) return replica.repoBranch ?? "1 repo";
                          if (repoCount() > 1) return "multi-repo";
                          return null;
                        };
                        return (
                          <div
                            class="ac-discovery-item"
                            onClick={() => handleReplicaClick(replica, wg)}
                            title={replica.path}
                          >
                            <div class="ac-discovery-item-info">
                              <span class="ac-discovery-item-name">{replica.name}</span>
                              <div class="ac-discovery-badges">
                                <Show when={branchLabel()}>
                                  <span class="ac-discovery-badge branch">{branchLabel()}</span>
                                </Show>
                                <span class="ac-discovery-badge team">replica</span>
                              </div>
                            </div>
                          </div>
                        );
                      }}
                    </For>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Show>
      </div>
    </Show>
  );
};

export default AcDiscoveryPanel;
