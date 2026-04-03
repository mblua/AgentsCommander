import { Component, For, Show, createSignal } from "solid-js";
import type { AcWorkgroup, AcAgentReplica, Session } from "../../shared/types";
import { SessionAPI, WindowAPI } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";

/** Build the session name used to link a replica to its session */
function replicaSessionName(wg: AcWorkgroup, replica: AcAgentReplica): string {
  return `${wg.name}/${replica.name}`;
}

/** Find existing session for a replica, if any */
function replicaSession(wg: AcWorkgroup, replica: AcAgentReplica): Session | undefined {
  return sessionsStore.findSessionByName(replicaSessionName(wg, replica));
}

/** Compute CSS class for replica status dot */
function replicaDotClass(wg: AcWorkgroup, replica: AcAgentReplica): string {
  const session = replicaSession(wg, replica);
  if (!session) return "offline";
  if (session.pendingReview) return "pending";
  if (session.waitingForInput) return "waiting";
  if (typeof session.status === "string") return session.status;
  return "exited";
}

const ProjectPanel: Component = () => {
  const [collapsed, setCollapsed] = createSignal(false);

  const handleReplicaClick = async (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const existing = replicaSession(wg, replica);
    if (existing) {
      // Already instantiated — just switch to it
      await SessionAPI.switch(existing.id);
      if (isTauri) {
        const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
        const detachedLabel = `terminal-${existing.id.replace(/-/g, "")}`;
        const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
        if (!detachedWin) {
          await WindowAPI.ensureTerminal();
        }
      }
      return;
    }

    // Not instantiated — create session in-place
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
      sessionName: replicaSessionName(wg, replica),
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
                {(wg) => {
                  const [wgCollapsed, setWgCollapsed] = createSignal(false);
                  return (
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        title={wg.path}
                        onClick={() => setWgCollapsed((c) => !c)}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: wgCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">{wg.name}</span>
                          <Show when={wg.brief}>
                            <span class="ac-wg-brief">{wg.brief}</span>
                          </Show>
                        </div>
                      </div>
                      <Show when={!wgCollapsed()}>
                        <For each={wg.agents}>
                          {(replica) => {
                            const repoCount = () => replica.repoPaths.length;
                            const branchLabel = () => {
                              if (repoCount() === 1) return replica.repoBranch ?? "1 repo";
                              if (repoCount() > 1) return "multi-repo";
                              return null;
                            };
                            const dotClass = () => replicaDotClass(wg, replica);
                            return (
                              <div
                                class="ac-discovery-item"
                                onClick={() => handleReplicaClick(replica, wg)}
                                title={replica.path}
                              >
                                <div class={`session-item-status ${dotClass()}`} />
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
                      </Show>
                    </div>
                  );
                }}
              </For>
              {/* Agent Matrix */}
              <Show when={proj().agents.length > 0}>
                {(() => {
                  const [matrixCollapsed, setMatrixCollapsed] = createSignal(false);
                  return (
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setMatrixCollapsed((c) => !c)}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: matrixCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Agent Matrix</span>
                        </div>
                      </div>
                      <Show when={!matrixCollapsed()}>
                        <For each={proj().agents}>
                          {(agent) => (
                            <div
                              class="ac-discovery-item"
                              onClick={() => handleAgentClick(agent)}
                              title={agent.path}
                            >
                              <div class="ac-discovery-item-info">
                                <span class="ac-discovery-item-name">
                                  {agent.name.slice(agent.name.lastIndexOf("/") + 1)}
                                </span>
                                <div class="ac-discovery-badges">
                                  <span class="ac-discovery-badge team">matrix</span>
                                </div>
                              </div>
                            </div>
                          )}
                        </For>
                      </Show>
                    </div>
                  );
                })()}
              </Show>
            </div>
          </Show>
        </div>
      )}
    </Show>
  );
};

export default ProjectPanel;
