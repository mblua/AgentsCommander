import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";

export function createTaskStatusTooltip(options: {
  status: Accessor<string | null>;
  titleAnchor: Accessor<HTMLSpanElement | undefined>;
  tooltipElement: Accessor<HTMLDivElement | undefined>;
}) {
  let disposed = false;
  let titlePointer = false;
  let tooltipPointer = false;
  let titleFocused = false;
  let escaped = false;
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;
  let frame: number | undefined;
  const [tooltipOpen, setTooltipOpen] = createSignal(false);
  const [tooltipVisible, setTooltipVisible] = createSignal(false);
  const [tooltipPosition, setTooltipPosition] = createSignal({ left: 16, top: 16, width: 640, height: 384 });
  const cancelLeave = () => { clearTimeout(leaveTimer); leaveTimer = undefined; };
  const openTooltip = () => {
    cancelLeave();
    if (!disposed && !escaped && (titleFocused || titlePointer || tooltipPointer) && options.status() !== null) setTooltipOpen(true);
  };
  const enterTooltip = (region: "title" | "tooltip") => {
    if (region === "title") {
      if (!titlePointer) escaped = false;
      titlePointer = true;
    } else {
      tooltipPointer = true;
    }
    openTooltip();
  };
  const leaveTooltip = (region: "title" | "tooltip" | "focus") => {
    if (region === "title") titlePointer = false;
    if (region === "tooltip") tooltipPointer = false;
    cancelLeave();
    if (titleFocused || titlePointer || tooltipPointer) return;
    leaveTimer = setTimeout(() => { leaveTimer = undefined; setTooltipOpen(false); }, 150);
  };
  const positionTooltip = () => {
    const titleAnchor = options.titleAnchor();
    const tooltipElement = options.tooltipElement();
    if (disposed || !titleAnchor || !tooltipElement) return;
    const viewport = window.visualViewport;
    const leftEdge = viewport?.offsetLeft ?? 0;
    const topEdge = viewport?.offsetTop ?? 0;
    const width = viewport?.width ?? window.innerWidth;
    const height = viewport?.height ?? window.innerHeight;
    const anchor = titleAnchor.getBoundingClientRect();
    const visible = anchor.right > leftEdge && anchor.left < leftEdge + width &&
      anchor.bottom > topEdge && anchor.top < topEdge + height;
    setTooltipVisible(visible);
    if (!visible) return;
    const rem = Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
    const maxWidth = Math.max(0, Math.min(40 * rem, width - 32));
    const maxHeight = Math.max(0, Math.min(24 * rem, height - 32));
    tooltipElement.style.maxWidth = maxWidth + "px";
    tooltipElement.style.maxHeight = maxHeight + "px";
    const rect = tooltipElement.getBoundingClientRect();
    const below = anchor.bottom + 6;
    const preferred = below + rect.height <= topEdge + height - 16 ? below : anchor.top - 6 - rect.height;
    setTooltipPosition({
      left: Math.max(leftEdge + 16, Math.min(anchor.left, leftEdge + width - 16 - rect.width)),
      top: Math.max(topEdge + 16, Math.min(preferred, topEdge + height - 16 - rect.height)),
      width: maxWidth, height: maxHeight,
    });
  };
  const scheduleTooltipPosition = () => {
    if (frame !== undefined) cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => { frame = undefined; positionTooltip(); });
  };
  const dismissTooltip = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !(titleFocused || titlePointer || tooltipPointer)) return;
    escaped = true;
    cancelLeave();
    setTooltipOpen(false);
    if (event.target === options.titleAnchor()) {
      event.preventDefault();
      event.stopPropagation();
    }
  };
  const tooltipKeyDown = (event: KeyboardEvent, dismissAllowed: boolean) => {
    if (event.key === "Escape") { if (dismissAllowed) dismissTooltip(event); return; }
    const element = options.tooltipElement();
    if (!tooltipOpen() || !element || element.scrollHeight <= element.clientHeight) return;
    const targets: Record<string, number> = {
      ArrowUp: element.scrollTop - 32, ArrowDown: element.scrollTop + 32,
      PageUp: element.scrollTop - element.clientHeight, PageDown: element.scrollTop + element.clientHeight,
      Home: 0, End: element.scrollHeight,
    };
    if (!(event.key in targets)) return;
    element.scrollTop = targets[event.key];
    event.preventDefault();
    event.stopPropagation();
  };
  const hideUnavailable = () => {
    cancelLeave();
    tooltipPointer = false;
    setTooltipOpen(false);
  };
  const resetIdentity = () => {
    escaped = false;
    titlePointer = false;
    titleFocused = false;
    hideUnavailable();
  };
  createEffect(() => {
    if (!tooltipOpen()) return;
    scheduleTooltipPosition();
    const viewport = window.visualViewport;
    window.addEventListener("scroll", scheduleTooltipPosition, true);
    window.addEventListener("resize", scheduleTooltipPosition);
    viewport?.addEventListener("scroll", scheduleTooltipPosition);
    viewport?.addEventListener("resize", scheduleTooltipPosition);
    onCleanup(() => {
      window.removeEventListener("scroll", scheduleTooltipPosition, true);
      window.removeEventListener("resize", scheduleTooltipPosition);
      viewport?.removeEventListener("scroll", scheduleTooltipPosition);
      viewport?.removeEventListener("resize", scheduleTooltipPosition);
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = undefined;
    });
  });
  onCleanup(() => {
    disposed = true;
    cancelLeave();
    if (frame !== undefined) cancelAnimationFrame(frame);
    frame = undefined;
  });
  return {
    open: tooltipOpen, visible: tooltipVisible, position: tooltipPosition,
    enter: enterTooltip, leave: leaveTooltip,
    focus: () => {
      if (!titleFocused) escaped = false;
      titleFocused = true;
      openTooltip();
    },
    blur: () => { titleFocused = false; leaveTooltip("focus"); },
    hideUnavailable, resetIdentity,
    resetDismissal: () => { escaped = false; },
    openIfActive: openTooltip, schedulePosition: scheduleTooltipPosition,
    dismiss: dismissTooltip, keyDown: tooltipKeyDown,
  };
}
