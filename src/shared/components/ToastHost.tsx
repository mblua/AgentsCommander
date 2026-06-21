import { Component, For } from "solid-js";
import { Portal } from "solid-js/web";
import { toastStore, type ToastKind } from "../stores/toasts";

const ROLE_BY_KIND: Record<ToastKind, "alert" | "status"> = {
  error: "alert",
  success: "status",
  info: "status",
};
const ARIA_LIVE_BY_KIND: Record<ToastKind, "assertive" | "polite"> = {
  error: "assertive",
  success: "polite",
  info: "polite",
};

const ToastHost: Component = () => {
  // §15.6: clicking the toast body must NOT raise the terminal. The App's
  // handleRaiseTerminal (App.tsx:82-95) is a NATIVE document `mousedown` listener
  // that only excludes BUTTON, so the message span / item div would otherwise
  // raise + focus the terminal on the standalone desktop sidebar. We use Solid's
  // `on:mousedown` (a native listener attached directly to .toast-item) rather
  // than the delegated `onMouseDown`: Solid delegates `mousedown` to `document`
  // (web.cjs delegateEvents -> document.addEventListener), the SAME target as
  // handleRaiseTerminal, and stopPropagation() does NOT stop sibling listeners on
  // the same target. A native element-level listener stops the event below
  // document, so neither Solid's delegation nor handleRaiseTerminal sees it. The
  // dismiss button's separate `click` event is unaffected.
  return (
    <Portal>
      <div class="toast-host" data-ac-testid="toast.host" data-ac-role="surface">
        <For each={toastStore.items}>
          {(toast) => (
            <div
              class={`toast-item toast-item--${toast.kind}`}
              data-ac-testid="toast.item"
              data-ac-kind={toast.kind}
              data-ac-toast-id={toast.id}
              role={ROLE_BY_KIND[toast.kind]}
              aria-live={ARIA_LIVE_BY_KIND[toast.kind]}
              on:mousedown={(e) => e.stopPropagation()}
            >
              <span class="toast-item__message">{toast.message}</span>
              <button
                class="toast-item__dismiss"
                type="button"
                aria-label="Dismiss notification"
                data-ac-testid="toast.item.dismiss"
                onClick={() => toastStore.dismiss(toast.id)}
              >
                &#xD7;
              </button>
            </div>
          )}
        </For>
      </div>
    </Portal>
  );
};

export default ToastHost;
