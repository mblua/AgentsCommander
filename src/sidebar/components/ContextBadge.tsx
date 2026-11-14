import {
  Component,
  Show,
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  on,
  onCleanup,
  type Accessor,
  type JSX,
} from "solid-js";
import { contextBadgeText } from "./session-context";
import ContextHistoryCard from "./ContextHistoryCard";
import {
  acquireContextHistory,
  bucketForSessions,
  refreshContextHistoryOnHover,
  type SidebarHistoryState,
} from "../context-history";
import "../styles/context-history.css";

export const CONTEXT_BADGE_TOOLTIP =
  "Context window in use, read from what the agent draws in its terminal. " +
  "Best-effort: it can be unavailable, stale or absent. A high reading does not " +
  "mean this session must be restarted.";

/** #3532 P11 - the live row this badge belongs to; Root rows never pass one. */
export interface ContextHistoryScope {
  projectPath: string;
  runtimeSessionId: string | null | undefined;
}

type Rgb = [number, number, number];
const DARK_RAMP: [Rgb, Rgb, Rgb] = [[0x22, 0xc5, 0x5e], [0xea, 0xb3, 0x08], [0xef, 0x44, 0x44]];
const LIGHT_RAMP: [Rgb, Rgb, Rgb] = [[0x15, 0x80, 0x3d], [0x8a, 0x64, 0x00], [0xb9, 0x1c, 0x1c]];

/** Linear RGB per segment [0,50] and [50,100] on the unrounded mean. */
export function rampColor(mean: number, theme: "dark" | "light"): string {
  const [low, mid, high] = theme === "dark" ? DARK_RAMP : LIGHT_RAMP;
  const m = Math.min(100, Math.max(0, mean));
  const [from, to, t] = m <= 50 ? [low, mid, m / 50] : [mid, high, (m - 50) / 50];
  const hex = from.map((c, i) => Math.round(c + ((to[i] ?? c) - c) * t).toString(16).padStart(2, "0"));
  return `#${hex.join("")}`;
}

/** Grace period to move the pointer from the badge to the card. */
export const CLOSE_DELAY_MS = 150;

const UNAVAILABLE: SidebarHistoryState = { status: "unavailable" };

const ContextBadge: Component<{
  percent: number | null | undefined;
  testId?: string;
  historyScope?: ContextHistoryScope;
  /** #4091 Saved colour session count; the returned buckets decide the fallback. */
  colorSessions?: number;
}> = (props) => {
  const reading = (): { value: number } | undefined => {
    const percent = props.percent;
    return percent === null || percent === undefined ? undefined : { value: percent };
  };

  const [history, setHistory] = createSignal<Accessor<SidebarHistoryState>>(() => UNAVAILABLE);
  const scopeKey = createMemo(() => {
    const s = props.historyScope;
    return s ? `${s.projectPath}\u0000${s.runtimeSessionId ?? ""}` : null;
  });
  createEffect(on(scopeKey, (key) => {
    const s = props.historyScope;
    if (key === null || !s) {
      setHistory(() => () => UNAVAILABLE);
      return;
    }
    const handle = acquireContextHistory({
      projectPath: s.projectPath,
      runtimeSessionId: s.runtimeSessionId,
    });
    setHistory(() => handle.state);
    onCleanup(handle.release);
  }));

  const state = () => history()();
  const historical = () => state().status !== "unavailable";
  // One selected bucket drives both the tone and the card basis label.
  const colorBucket = () => {
    const s = state();
    return s.status === "ready" ? bucketForSessions(s.sessionWindows, props.colorSessions) : null;
  };
  const toneMean = (): number | null => colorBucket()?.mean ?? null;

  const [open, setOpen] = createSignal(false);
  let trigger: HTMLSpanElement | undefined;
  let card: HTMLDivElement | undefined;
  const cardId = `ctx-history-${createUniqueId()}`;

  // Crossing the gap between badge and card must not close it: leaving either
  // area closes after a short delay that entering the other one cancels.
  let cardHovered = false;
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  const cancelClose = () => {
    if (closeTimer !== undefined) clearTimeout(closeTimer);
    closeTimer = undefined;
  };
  const close = () => {
    cancelClose();
    cardHovered = false;
    setOpen(false);
  };
  const scheduleClose = () => {
    cancelClose();
    closeTimer = setTimeout(close, CLOSE_DELAY_MS);
  };
  onCleanup(cancelClose);

  const show = () => {
    cancelClose();
    setOpen(true);
    const s = props.historyScope;
    if (s) refreshContextHistoryOnHover(s);
  };
  const inside = (node: EventTarget | null) =>
    node instanceof Node && (trigger?.contains(node) || card?.contains(node));
  // Escape closes wherever focus is while the card is open.
  const onEscapeAnywhere = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
  };
  const onPointerOutside = (e: PointerEvent) => {
    if (!inside(e.target)) close();
  };
  createEffect(() => {
    if (!open()) return;
    document.addEventListener("pointerdown", onPointerOutside, true);
    document.addEventListener("keydown", onEscapeAnywhere, true);
    onCleanup(() => {
      document.removeEventListener("pointerdown", onPointerOutside, true);
      document.removeEventListener("keydown", onEscapeAnywhere, true);
    });
  });
  createEffect(() => {
    if (!historical()) close();
  });

  // Native (not delegated) listeners so the row never sees these events.
  const historyHandlers = (): Record<string, unknown> => ({
    tabIndex: 0,
    "aria-describedby": cardId,
    onMouseEnter: show,
    onMouseLeave: (e: MouseEvent) => {
      if (!inside(e.relatedTarget)) scheduleClose();
    },
    onFocus: show,
    // Pressing on the card (e.g. its scrollbar) blurs the badge; keep it open then.
    // Focus moving to a real control always closes; a null target with the pointer
    // on the card is a press on the card itself (e.g. its scrollbar).
    onBlur: (e: FocusEvent) => {
      if (e.relatedTarget !== null || !cardHovered) close();
    },
    "on:keydown": (e: KeyboardEvent) => {
      // The row selects on Enter/Space; nothing on the badge may switch the session.
      e.stopPropagation();
      if (e.key === "Escape") close();
    },
    "on:pointerdown": (e: PointerEvent) => {
      e.stopPropagation();
      if (e.pointerType === "touch") {
        if (open()) setOpen(false);
        else show();
      }
    },
    "on:click": (e: MouseEvent) => e.stopPropagation(),
  });

  const baseAttrs = (state: "reading" | "unavailable") => ({
    "data-ac-testid": props.testId,
    "data-ac-role": "status",
    "data-ac-state": state,
  });

  const historyClasses = () => ({
    "ctx-history": historical(),
    toned: historical() && toneMean() !== null,
  });
  const toneStyle = (): JSX.CSSProperties | undefined => {
    const mean = toneMean();
    if (!historical() || mean === null) return undefined;
    return {
      "--ctx-history-tone-dark": rampColor(mean, "dark"),
      "--ctx-history-tone-light": rampColor(mean, "light"),
    };
  };
  const label = (text: string) =>
    historical() ? <span class="ctx-history-live">{text}</span> : text;

  const badge = (
    <Show
      when={reading()}
      fallback={
        <span
          ref={trigger}
          class="ctx-badge unavailable"
          classList={historyClasses()}
          style={toneStyle()}
          title={historical() ? undefined : CONTEXT_BADGE_TOOLTIP}
          {...baseAttrs("unavailable")}
          {...(historical() ? historyHandlers() : {})}
        >
          {label(contextBadgeText(props.percent))}
        </span>
      }
    >
      {(value) => (
        <span
          ref={trigger}
          class="ctx-badge"
          classList={historyClasses()}
          style={toneStyle()}
          role="meter"
          aria-label="Context window used"
          aria-valuenow={value().value}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuetext={`Context ${value().value}% used`}
          title={historical() ? undefined : CONTEXT_BADGE_TOOLTIP}
          {...baseAttrs("reading")}
          {...(historical() ? historyHandlers() : {})}
        >
          {label(contextBadgeText(value().value))}
        </span>
      )}
    </Show>
  );

  return (
    <>
      {badge}
      <Show when={historical()}>
        <ContextHistoryCard
          id={cardId}
          cardRef={(el) => (card = el)}
          open={open()}
          anchor={() => trigger}
          state={state()}
          percent={reading()?.value ?? null}
          projectPath={props.historyScope?.projectPath ?? ""}
          colorSessions={colorBucket()?.requestedSessions ?? null}
          warning={CONTEXT_BADGE_TOOLTIP}
          onEnter={() => {
            cardHovered = true;
            cancelClose();
          }}
          onLeave={(related) => {
            cardHovered = false;
            if (!inside(related)) scheduleClose();
          }}
        />
      </Show>
    </>
  );
};

export default ContextBadge;
