import { Component, For, Show, createSignal } from "solid-js";
import type { AcWorkgroup, AcAgentReplica } from "../../shared/types";
import { SessionAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";

const ProjectPanel: Component = () => {
  const [collapsed, setCollapsed] = createSignal(false);

  const handleReplicaClick = (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const repoPaths = replica.repoPaths ?? [];
    let gitBranchSource: string | undefined;
    let gitBranchPrefix: string | undefined;

    if (repoPaths.length === 1) {
      gitBranchSource = repoPaths[0];
      const dirName = repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
      gitBranchPrefix = dirName.startsWith("repo-") ? dirName.slice(5) : dirName;
    } else if (repoPaths.length > 1) {
      gitBranchPrefix = "multi-repo";
    }

    SessionAPI.create({
      cwd: replica.path,
      sessionName: `${wg.name}/${replica.name}`,
      agentId: replica.preferredAgentId,
      gitBranchSource,
      gitBranchPrefix,
    });
  };

  const handleAgentClick = (agent: { name: string; path: string; preferredAgentId?: string }) => {
    SessionAPI.create({
      cwd: agent.path,
      sessionName: agent.name,
      agentId: agent.preferredAgentId,
    });
  };

  return (
    <Show when={projectStore.current}>
      {(proj) => (
        <div class="project-panel">
          <button
            class="project-header"
            onClick={() => setCollapsed((c) => !c)}
          >
            <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
              &#x25BE;
            </span>
            <span class="project-title">Project: {proj().folderName}</span>
          </button>
          <Show when={!collapsed()}>
            <div class="project-content">
              {/* Workgroups */}
              <For each={proj().workgroups}>
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
              {/* Agents (non-workgroup) */}
              <Show when={proj().agents.length > 0}>
                <For each={proj().agents}>
                  {(agent) => (
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
                      </div>
                    </div>
                  )}
                </For>
              </Show>
            </div>
          </Show>
        </div>
      )}
    </Show>
  );
};

export default ProjectPanel;
