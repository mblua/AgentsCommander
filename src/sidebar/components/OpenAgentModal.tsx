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

  onMount(async () => {
    if (!props.initialRepo) {
      // Load initial list (empty query = show all)
      const results = await ReposAPI.search("");
      setRepos(results);
      inputRef?.focus();
    }
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

  const handleEscape = () => {
    const repo = selectedRepo();

    if (repo && !props.initialRepo) {
      // Go back to repo list (only if we navigated there ourselves)
      setSelectedRepo(null);
      setHighlightIndex(0);
      requestAnimationFrame(() => inputRef?.focus());
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
      } else if (e.key === "Enter" && list.length > 0) {
        e.preventDefault();
        setSelectedRepo(list[highlightIndex()]);
        setHighlightIndex(0);
      }
    } else {
      return;
    }
  };

  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") handleEscape();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  const launchAgent = async (repo: RepoMatch, selection: AgentPickerSelection) => {
    // #516 — create first; only hide Home / close the modal once the session
    // exists. A rejection (e.g. the Resource Monitor cap) propagates to
    // AgentPickerModal.apply()'s catch, which keeps this modal open with the
    // error shown instead of hiding Home behind a silent hang.
    await SessionAPI.create({
      cwd: repo.path,
      sessionName: repo.name,
      agentId: selection.agent.id,
      requestedProfile: selection.requestedProfile,
    });

    homeStore.hide();
    props.onClose();
  };

  if (selectedRepo()) {
    return (
      <AgentPickerModal
        sessionName={selectedRepo()!.name}
        agentPath={selectedRepo()!.path}
        onSelect={(selection) => launchAgent(selectedRepo()!, selection)}
        onClose={handleEscape}
      />
    );
  }

  return (
    <div class="modal-overlay" onKeyDown={handleKeyDown}>
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
        </div>

        {/* Footer hint */}
        <div class="agent-modal-footer">
          <span>&#x2191;&#x2193; navigate</span>
          <span>&#x23CE; select</span>
          <span>esc {selectedRepo() && !props.initialRepo ? "back" : "close"}</span>
        </div>
      </div>
    </div>
  );
};

export default OpenAgentModal;
