import { Component, createSignal, createEffect, For, Show, onMount, onCleanup } from "solid-js";
import type { RepoMatch } from "../../shared/types";
import { ReposAPI, SessionAPI } from "../../shared/ipc";
import { homeStore } from "../../main/stores/home";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";

const OpenAgentModal: Component<{ onClose: () => void; initialRepo?: RepoMatch }> = (props) => {
  const [query, setQuery] = createSignal("");
  const [repos, setRepos] = createSignal<RepoMatch[]>([]);
  const [selectedRepo, setSelectedRepo] = createSignal<RepoMatch | null>(props.initialRepo ?? null);
  const [highlightIndex, setHighlightIndex] = createSignal(0);
  const [loading, setLoading] = createSignal(false);
  let inputRef!: HTMLInputElement;
  let debounceTimer: number | undefined;
  let createPending = false;
  let disposed = false;

  onMount(async () => {
    if (!props.initialRepo) {
      // Load initial list (empty query = show all)
      const results = await ReposAPI.search("");
      setRepos(results);
      inputRef?.focus();
    }
  });

  onCleanup(() => {
    disposed = true;
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

  const handleEscape = () => {
    if (selectedRepo() && !props.initialRepo) {
      if (createPending) return;
      setSelectedRepo(null);
      setHighlightIndex(0);
      requestAnimationFrame(() => {
        if (!selectedRepo() && inputRef?.isConnected) inputRef.focus();
      });
    } else {
      props.onClose();
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    const repo = selectedRepo();

    if (!repo) {
      // Repo list navigation
      const list = repos();
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setHighlightIndex((i) => Math.min(i + 1, list.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setHighlightIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" && e.target === inputRef && list.length > 0) {
        e.preventDefault();
        setSelectedRepo(list[highlightIndex()] ?? null);
        setHighlightIndex(0);
      }
    } else {
      return;
    }
  };

  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    // Give the picker its own Escape before either handler changes the view.
    if (selectedRepo() && e.composedPath().some((target) =>
      target instanceof Element && target.matches('[data-ac-testid="agentPicker.overlay"]')
    )) return;

    e.preventDefault();
    e.stopPropagation();
    handleEscape();
  };

  document.addEventListener("keydown", handleDocumentKeyDown, true);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown, true));

  const launchAgent = async (repo: RepoMatch, selection: AgentPickerSelection) => {
    createPending = true;
    try {
      // Let the picker display a create rejection and restore its controls.
      await SessionAPI.create({
        cwd: repo.path,
        sessionName: repo.name,
        agentId: selection.agent.id,
        requestedProfile: selection.requestedProfile,
      });
      if (disposed) return;

      homeStore.hide();
      props.onClose();
    } finally {
      createPending = false;
    }
  };

  const repoList = () => (
    <div class="modal-overlay" role="none" onKeyDown={handleKeyDown}>
      <div class="agent-modal">
        {/* Header */}
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

        {/* Content */}
        <div class="agent-modal-list">
            <Show
              when={repos().length > 0}
              fallback={
                <div class="agent-modal-empty">
                  {query() ? `No repos matching "${query()}"` : "No repos found"}
                </div>
              }
            >
              <For each={repos()}>
                {(repo, i) => {
                  // Rows are native buttons: Enter/Space activate natively. Inline
                  // styles neutralize only the UA button styles the row CSS does not
                  // set; the UA background is cleared only outside :hover/highlight.
                  const [hovered, setHovered] = createSignal(false);
                  const highlighted = () => i() === highlightIndex();
                  return (
                  <button
                    type="button"
                    class={`agent-modal-item ${highlighted() ? "highlighted" : ""}`}
                    style={{
                      width: "100%",
                      appearance: "none",
                      font: "inherit",
                      color: "inherit",
                      "text-align": "left",
                      "border-top": "none",
                      "border-right": "none",
                      "border-bottom": "none",
                      "background-color":
                        highlighted() || hovered() ? undefined : "transparent",
                    }}
                    onClick={() => {
                      setSelectedRepo(repo);
                      setHighlightIndex(0);
                    }}
                    onMouseEnter={() => {
                      setHovered(true);
                      setHighlightIndex(i());
                    }}
                    onMouseLeave={() => setHovered(false)}
                  >
                    <span class="agent-modal-item-icon">&#x1F4C1;</span>
                    <span class="agent-modal-item-info">
                      <span class="agent-modal-item-name" style={{ display: "block" }}>
                        {repo.name.includes("/") ? (
                          <>
                            <span class="name-prefix">{repo.name.slice(0, repo.name.lastIndexOf("/") + 1)}</span>
                            {repo.name.slice(repo.name.lastIndexOf("/") + 1)}
                          </>
                        ) : repo.name}
                      </span>
                      <span class="agent-modal-item-badges">
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
                      </span>
                    </span>
                  </button>
                  );
                }}
              </For>
            </Show>
        </div>

        {/* Footer hint */}
        <div class="agent-modal-footer">
          <span>&#x2191;&#x2193; navigate</span>
          <span>&#x23CE; select</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );

  return (
    <Show when={selectedRepo()} keyed fallback={repoList()}>
      {(repo) => (
        <AgentPickerModal
          sessionName={repo.name}
          agentPath={repo.path}
          onSelect={(selection) => launchAgent(repo, selection)}
          onClose={handleEscape}
        />
      )}
    </Show>
  );
};

export default OpenAgentModal;
