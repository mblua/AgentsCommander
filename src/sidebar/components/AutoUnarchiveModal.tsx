import { Component, For, Show, createEffect, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { autoUnarchiveStore } from "../stores/auto-unarchive";
import { automationIdPart } from "./replica-repo-badges";

const AutoUnarchiveModal: Component = () => {
  let messageRef: HTMLDivElement | undefined;
  let acknowledgeRef: HTMLButtonElement | undefined;
  let previouslyFocused: HTMLElement | null = null;
  let wasOpen = false;

  const entries = () => autoUnarchiveStore.entries;

  onMount(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!autoUnarchiveStore.open) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        return;
      }
      if (event.key === "Tab") {
        event.stopImmediatePropagation();
        const focusables = [messageRef, acknowledgeRef].filter(Boolean) as HTMLElement[];
        if (focusables.length < 2) return;
        const index = focusables.indexOf(document.activeElement as HTMLElement);
        if (index === -1) {
          event.preventDefault();
          (event.shiftKey ? focusables[focusables.length - 1] : focusables[0]).focus();
          return;
        }
        if (event.shiftKey) {
          if (index <= 0) {
            event.preventDefault();
            focusables[focusables.length - 1].focus();
          }
        } else if (index === focusables.length - 1) {
          event.preventDefault();
          focusables[0].focus();
        }
        return;
      }
      event.stopImmediatePropagation();
    };

    document.addEventListener("keydown", onKeyDown, true);
    onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
  });

  createEffect(() => {
    const open = autoUnarchiveStore.open;
    if (open === wasOpen) return;
    wasOpen = open;
    if (open) {
      previouslyFocused = document.activeElement as HTMLElement | null;
      queueMicrotask(() => acknowledgeRef?.focus());
    } else {
      try {
        previouslyFocused?.focus();
      } catch {
        // Best effort only: the original target may have been removed.
      }
      previouslyFocused = null;
    }
  });

  return (
    <Show when={autoUnarchiveStore.open}>
      <Portal>
        <div class="modal-overlay auto-unarchive-overlay" data-ac-testid="autoUnarchive.modal">
          <div
            class="agent-modal auto-unarchive-modal"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="auto-unarchive-title"
            aria-describedby="auto-unarchive-description"
          >
            <div class="agent-modal-header">
              <span class="agent-modal-title" id="auto-unarchive-title">
                {entries().length === 1 ? "Project un-archived" : "Projects un-archived"}
              </span>
            </div>

            <div
              class="auto-unarchive-body"
              id="auto-unarchive-description"
              ref={messageRef}
              tabindex="0"
            >
              <p class="auto-unarchive-copy">
                {entries().length === 1
                  ? `${entries()[0].folderName} was restored to your project list. A session started inside it, and an archived project cannot hold a running session.`
                  : "These projects were restored to your project list because sessions started inside them, and archived projects cannot hold running sessions."}
              </p>
              <div class="auto-unarchive-list">
                <For each={entries()}>
                  {(entry) => (
                    <div
                      class="auto-unarchive-row"
                      title={entry.path}
                      data-ac-testid={`autoUnarchive.row.${automationIdPart(entry.path)}`}
                      data-ac-testid-private="true"
                    >
                      <strong>{entry.folderName}</strong>
                      <Show when={entry.sessionName}>
                        {(sessionName) => (
                          <span class="auto-unarchive-session">{sessionName()}</span>
                        )}
                      </Show>
                    </div>
                  )}
                </For>
              </div>
            </div>

            <div class="agent-modal-footer">
              <button
                ref={acknowledgeRef}
                class="modal-btn modal-btn-save"
                onClick={() => autoUnarchiveStore.acknowledge()}
                data-ac-testid="autoUnarchive.acknowledge"
              >
                Got it
              </button>
            </div>
          </div>
        </div>
      </Portal>
    </Show>
  );
};

export default AutoUnarchiveModal;
