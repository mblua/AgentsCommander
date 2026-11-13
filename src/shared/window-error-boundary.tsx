import { ErrorBoundary, createSignal, onCleanup, type JSX } from "solid-js";
import { isTauri } from "./platform";

export function WindowErrorBoundary(props: { label: string; children: JSX.Element }): JSX.Element {
  return (
    <ErrorBoundary fallback={(error, reset) => (
      <WindowErrorFallback label={props.label} error={error} reset={reset} />
    )}>
      {props.children}
    </ErrorBoundary>
  );
}

function WindowErrorFallback(props: { label: string; error: unknown; reset: () => void }) {
  console.error(
    "[window-error-boundary]",
    props.label,
    String(props.error instanceof Error ? props.error.stack ?? props.error.message : props.error),
  );
  const [confirming, setConfirming] = createSignal(false);
  let card!: HTMLDivElement;
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let win: { destroy: () => Promise<void> } | undefined;

  if (props.label === "main" && isTauri) {
    void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      const w = getCurrentWindow();
      const off = await w.onCloseRequested((event) => {
        event.preventDefault();
        setConfirming(true);
      });
      if (disposed) {
        off();
        return;
      }
      win = w;
      unlisten = off;
    }).catch((error) => console.error(
      "[window-error-boundary]", props.label, "close guard: " + String(error),
    ));
  }
  onCleanup(() => {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
  });

  return (
    <div style={{ display: "grid", "place-items": "center", height: "100%", overflow: "auto" }}>
      <div ref={card} tabIndex={-1} style={{
        color: "#eeeeee", background: "#242424", padding: "24px", "max-width": "480px",
      }}>
        <h2>Something went wrong in this window.</h2>
        <p>{String(props.error instanceof Error ? props.error.message : props.error)}</p>
        <button type="button" onClick={() => props.reset()}>Try again</button>
        <button type="button" onClick={() => location.reload()}>Reload</button>
        {confirming() && (
          <div>
            <p>Exit AgentsCommander and stop all sessions?</p>
            <button type="button" onClick={() => { void win?.destroy(); }}>Exit</button>
            <button type="button" onClick={() => {
              setConfirming(false);
              card.focus();
            }}>Cancel</button>
          </div>
        )}
      </div>
    </div>
  );
}
