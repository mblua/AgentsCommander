import { Component, Show, createMemo, createSignal, onCleanup } from "solid-js";
import type { JSX } from "solid-js";
import { settingsStore } from "../../shared/stores/settings";
import { rootMessageStore } from "../stores/root-message-tutorial";
import {
  COPIED, COPY, COPY_FAILED, RM_IDS, ROOT_MESSAGE, rootMessageView,
} from "../tutorials/root-agent-message";
import { bubblePos } from "./TutorialOverlay";

// #5287 - ring on the Root Agent banner (or the Settings gear) and the message
// bubble. Copy only fills the clipboard: no IPC, no session write, no keys.
const COPIED_MS = 2000;
const find = (id: string) => document.querySelector(`[data-ac-testid="${id}"]`);

const RootMessageOverlay: Component<{ tick: number }> = (props) => {
  let bubble: HTMLDivElement | undefined;
  const v = createMemo(() => {
    void props.tick; // the banner state lives in the DOM, so re-read per frame
    const banner = find(RM_IDS.banner)?.getAttribute("data-ac-state") ?? null;
    const s = settingsStore.current;
    return rootMessageView(banner, s ? s.agents.length : null);
  });
  const rect = createMemo(() => {
    void props.tick;
    const target = v().target;
    const el = target ? find(target) : null;
    return el ? el.getBoundingClientRect() : null;
  });
  const [copied, setCopied] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(ROOT_MESSAGE);
      setFailed(false);
      setCopied(true);
      clearTimeout(timer);
      timer = setTimeout(() => setCopied(false), COPIED_MS);
    } catch {
      setCopied(false);
      setFailed(true);
    }
  };
  const bubbleStyle = (): JSX.CSSProperties => {
    const r = rect();
    if (!r) return { right: "24px", bottom: "24px" };
    const p = bubblePos(r, bubble?.offsetHeight ?? 0, window.innerWidth, window.innerHeight);
    return { left: `${p.left}px`, top: `${p.top}px` };
  };
  return (
    <>
      <Show when={rect()}>
        {(r) => (
          <div
            class="tutorial-ring"
            data-ac-testid="rootMessage.ring"
            style={{
              top: `${r().top - 4}px`, left: `${r().left - 4}px`,
              width: `${r().width + 8}px`, height: `${r().height + 8}px`,
            }}
          />
        )}
      </Show>
      <div
        ref={bubble}
        class="tutorial-bubble"
        data-ac-testid="rootMessage.bubble"
        data-ac-state={v().warn ? "warn" : undefined}
        style={bubbleStyle()}
      >
        <p data-ac-testid="rootMessage.hint">{v().text}</p>
        <pre class="tutorial-message" data-ac-testid="rootMessage.text">{ROOT_MESSAGE}</pre>
        <Show when={failed()}>
          <p data-ac-testid="rootMessage.copyFailed">{COPY_FAILED}</p>
        </Show>
        <div class="tutorial-bubble-actions">
          <button type="button" data-ac-testid="rootMessage.copy" onClick={() => void copy()}>
            {copied() ? COPIED : COPY}
          </button>
          <button type="button" data-ac-testid="rootMessage.close"
            onClick={() => rootMessageStore.exit()}>Close tutorial</button>
        </div>
      </div>
    </>
  );
};

export default RootMessageOverlay;
