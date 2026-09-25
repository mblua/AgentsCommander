import { Component, For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import type { AgentHelpEntry, AgentHelpTip } from "../../shared/agent-help";
import SelectionCopyMenu from "./context-menu/SelectionCopyMenu";

interface AgentHelpTipsModalProps {
  title: string;
  entry: AgentHelpEntry | null;
  localError: string | null;
  onClose: () => void;
}

function isTip(value: unknown): value is AgentHelpTip {
  if (typeof value !== "object" || value === null) return false;
  const tip = value as Record<string, unknown>;
  return typeof tip.title === "string" && typeof tip.body === "string";
}

function tipLink(tip: AgentHelpTip): { label: string; url: string } | null {
  const link: unknown = tip.link;
  if (typeof link !== "object" || link === null) return null;
  const { label, url } = link as Record<string, unknown>;
  if (typeof url !== "string") return null;
  return { label: typeof label === "string" && label ? label : url, url };
}

const AgentHelpTipsModal: Component<AgentHelpTipsModalProps> = (props) => {
  let dialogRef: HTMLDivElement | undefined;
  let closeRef: HTMLButtonElement | undefined;
  const previouslyFocused = document.activeElement as HTMLElement | null;
  const [copyMenu, setCopyMenu] = createSignal<{ x: number; y: number; text: string } | null>(null);

  // Read defensively here: this is the first reader of `tips`.
  const tips = (): AgentHelpTip[] => {
    const raw: unknown = props.entry?.tips;
    return Array.isArray(raw) ? raw.filter(isTip) : [];
  };

  // Capture the selection now: pressing the menu item collapses it.
  const openCopyMenu = (event: MouseEvent) => {
    const text = window.getSelection()?.toString() ?? "";
    // Nothing selected: leave the event to the sidebar's blocker, as everywhere else.
    if (!text) return;
    event.preventDefault();
    setCopyMenu({ x: event.clientX, y: event.clientY, text });
  };

  onMount(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        // An open copy menu takes the first Escape; the window takes the next.
        if (copyMenu()) {
          setCopyMenu(null);
          return;
        }
        props.onClose();
        return;
      }
      if (event.key === "Tab") {
        event.stopImmediatePropagation();
        // Every focusable control in DOM order, tip links included.
        const focusables = Array.from(
          dialogRef?.querySelectorAll<HTMLElement>("a[href], button:not([disabled])") ?? [],
        );
        const index = focusables.indexOf(document.activeElement as HTMLElement);
        if (index === -1) {
          event.preventDefault();
          (event.shiftKey ? focusables[focusables.length - 1] : focusables[0])?.focus();
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
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    queueMicrotask(() => closeRef?.focus());
    onCleanup(() => {
      document.removeEventListener("keydown", onKeyDown, true);
      try {
        previouslyFocused?.focus();
      } catch {
        // Best effort only: the original target may have been removed.
      }
    });
  });

  return (
    <Portal>
      <div class="modal-overlay" data-ac-testid="agentHelpTips.modal">
        <div
          ref={dialogRef}
          class="agent-modal agent-help-tips-modal"
          role="dialog"
          aria-modal="true"
          aria-labelledby="agent-help-tips-title"
        >
          <div class="agent-modal-header">
            <span class="agent-modal-title" id="agent-help-tips-title">
              {props.title}
            </span>
          </div>

          <div class="agent-help-tips-body" onContextMenu={openCopyMenu}>
            <Show when={props.localError}>
              {(reason) => (
                <div
                  class="agent-help-tips-error"
                  data-ac-testid="agentHelpTips.localError"
                  role="alert"
                >
                  Your agent-help.local.json was ignored: {reason()}
                </div>
              )}
            </Show>
            <For each={tips()}>
              {(tip) => (
                <section class="agent-help-tips-tip">
                  <div class="agent-help-tips-tip-title">{tip.title}</div>
                  <div class="agent-help-tips-tip-body">{tip.body}</div>
                  <Show when={tipLink(tip)}>
                    {(link) => (
                      <a
                        class="agent-help-tips-tip-link"
                        href={link().url}
                        target="_blank"
                        rel="noopener noreferrer"
                      >
                        {link().label}
                      </a>
                    )}
                  </Show>
                </section>
              )}
            </For>
            <Show when={tips().length === 0 && !props.localError}>
              <div class="agent-help-tips-empty">No tips here yet.</div>
            </Show>
          </div>

          <div class="agent-modal-footer">
            <button
              ref={closeRef}
              type="button"
              class="modal-btn modal-btn-save"
              onClick={() => props.onClose()}
              data-ac-testid="agentHelpTips.close"
            >
              Close
            </button>
          </div>
        </div>
      </div>
      <SelectionCopyMenu
        open={copyMenu() !== null}
        x={copyMenu()?.x ?? 0}
        y={copyMenu()?.y ?? 0}
        text={copyMenu()?.text ?? ""}
        onDismiss={() => setCopyMenu(null)}
      />
    </Portal>
  );
};

export default AgentHelpTipsModal;
