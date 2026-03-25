import { Component, createSignal, createEffect, For, Show, onMount, onCleanup } from "solid-js";
import type { AgentConfig, RepoMatch } from "../../shared/types";
import { ReposAPI, SessionAPI, SettingsAPI } from "../../shared/ipc";

const OpenAgentModal: Component<{ onClose: () => void }> = (props) => {
  const [query, setQuery] = createSignal("");
  const [repos, setRepos] = createSignal<RepoMatch[]>([]);
  const [agents, setAgents] = createSignal<AgentConfig[]>([]);
  const [selectedRepo, setSelectedRepo] = createSignal<RepoMatch | null>(null);
  const [highlightIndex, setHighlightIndex] = createSignal(0);
  const [loading, setLoading] = createSignal(false);
  let inputRef!: HTMLInputElement;
  let debounceTimer: number | undefined;

  onMount(async () => {
    const settings = await SettingsAPI.get();
    setAgents(settings.agents);
    // Load initial list (empty query = show all)
    const results = await ReposAPI.search("");
    setRepos(results);
    inputRef?.focus();
  });

  onCleanup(() => {
    if (debounceTimer) clearTimeout(debounceTimer);
  });

  // Debounced search
  createEffect(() => {
    const q = query();
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = window.setTimeout(async () => {
      setLoading(true);
      const results = await ReposAPI.search(q);
      setRepos(results);
      setHighlightIndex(0);
      setLoading(false);
    }, 100);
  });

  const handleKeyDown = (e: KeyboardEvent) => {
    const repo = selectedRepo();

    if (e.key === "Escape") {
      if (repo) {
        // Go back to repo list
        setSelectedRepo(null);
        setHighlightIndex(0);
        requestAnimationFrame(() => inputRef?.focus());
      } else {
        props.onClose();
      }
      return;
    }

    if (!repo) {
      // Repo list navigation
      const list = repos();
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setHighlightIndex((i) => Math.min(i + 1, list.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setHighlightIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" && list.length > 0) {
        e.preventDefault();
        setSelectedRepo(list[highlightIndex()]);
        setHighlightIndex(0);
      }
    } else {
      // Agent selection navigation
      const agentList = agents();
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setHighlightIndex((i) => Math.min(i + 1, agentList.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setHighlightIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" && agentList.length > 0) {
        e.preventDefault();
        launchAgent(repo, agentList[highlightIndex()]);
      }
    }
  };

  const launchAgent = (repo: RepoMatch, agent: AgentConfig) => {
    // Build the command: parse command string into executable + args
    const parts = agent.command.trim().split(/\s+/);
    const executable = parts[0];
    const cmdArgs = parts.slice(1);

    let shell: string;
    let shellArgs: string[];

    if (agent.gitPullBefore) {
      // Use cmd.exe /K to keep the session alive after command runs
      shell = "cmd.exe";
      shellArgs = ["/K", `git pull && ${agent.command}`];
    } else {
      shell = executable;
      shellArgs = cmdArgs;
    }

    SessionAPI.create({
      shell,
      shellArgs,
      cwd: repo.path,
      sessionName: repo.name,
      agentId: agent.id,
    });

    props.onClose();
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) {
      props.onClose();
    }
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick} onKeyDown={handleKeyDown}>
      <div class="agent-modal">
        {/* Header */}
        <Show
          when={!selectedRepo()}
          fallback={
            <div class="agent-modal-header">
              <button
                class="agent-back-btn"
                onClick={() => {
                  setSelectedRepo(null);
                  setHighlightIndex(0);
                  requestAnimationFrame(() => inputRef?.focus());
                }}
              >
                &#x2190;
              </button>
              <span class="agent-modal-title">
                Select agent for <strong>
                  {selectedRepo()!.name.includes("/") ? (
                    <>
                      <span class="name-prefix">{selectedRepo()!.name.slice(0, selectedRepo()!.name.lastIndexOf("/") + 1)}</span>
                      {selectedRepo()!.name.slice(selectedRepo()!.name.lastIndexOf("/") + 1)}
                    </>
                  ) : selectedRepo()!.name}
                </strong>
              </span>
            </div>
          }
        >
          <div class="agent-search-container">
            <input
              ref={inputRef!}
              class="agent-search-input"
              placeholder="Search repos..."
              value={query()}
              onInput={(e) => setQuery(e.currentTarget.value)}
            />
            {loading() && <div class="agent-search-spinner" />}
          </div>
        </Show>

        {/* Content */}
        <div class="agent-modal-list">
          <Show
            when={!selectedRepo()}
            fallback={
              <For each={agents()}>
                {(agent, i) => (
                  <div
                    class={`agent-modal-item agent-choice ${i() === highlightIndex() ? "highlighted" : ""}`}
                    onClick={() => launchAgent(selectedRepo()!, agent)}
                    onMouseEnter={() => setHighlightIndex(i())}
                  >
                    <div
                      class="agent-color-badge"
                      style={{ background: agent.color }}
                    />
                    <div class="agent-modal-item-info">
                      <div class="agent-modal-item-name">{agent.label}</div>
                      <div class="agent-modal-item-detail">
                        {agent.command}
                      </div>
                    </div>
                  </div>
                )}
              </For>
            }
          >
            <Show
              when={repos().length > 0}
              fallback={
                <div class="agent-modal-empty">
                  {query() ? `No repos matching "${query()}"` : "No repos found"}
                </div>
              }
            >
              <For each={repos()}>
                {(repo, i) => (
                  <div
                    class={`agent-modal-item ${i() === highlightIndex() ? "highlighted" : ""}`}
                    onClick={() => {
                      setSelectedRepo(repo);
                      setHighlightIndex(0);
                    }}
                    onMouseEnter={() => setHighlightIndex(i())}
                  >
                    <div class="agent-modal-item-icon">&#x1F4C1;</div>
                    <div class="agent-modal-item-info">
                      <div class="agent-modal-item-name">
                        {repo.name.includes("/") ? (
                          <>
                            <span class="name-prefix">{repo.name.slice(0, repo.name.lastIndexOf("/") + 1)}</span>
                            {repo.name.slice(repo.name.lastIndexOf("/") + 1)}
                          </>
                        ) : repo.name}
                      </div>
                      <div class="agent-modal-item-badges">
                        <For each={repo.agents}>
                          {(agent) => (
                            <span
                              class="agent-badge"
                              data-agent={agent}
                            >
                              {agent}
                            </span>
                          )}
                        </For>
                      </div>
                    </div>
                  </div>
                )}
              </For>
            </Show>
          </Show>
        </div>

        {/* Footer hint */}
        <div class="agent-modal-footer">
          <span>&#x2191;&#x2193; navigate</span>
          <span>&#x23CE; select</span>
          <span>esc {selectedRepo() ? "back" : "close"}</span>
        </div>
      </div>
    </div>
  );
};

export default OpenAgentModal;
