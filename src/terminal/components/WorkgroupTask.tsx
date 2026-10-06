import { Component, createEffect, createMemo, createSignal, createUniqueId, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { terminalStore } from "../stores/terminal";
import { TaskAPI } from "../../shared/ipc";
import { pathHasEntityDirSegment } from "../../shared/entity-prefix";
import TaskCleanConfirmModal from "./TaskCleanConfirmModal";

interface ParsedTask {
  title: string | null;
  body: string;
}

function unquoteTitle(after: string): string {
  if (after.startsWith("'") && after.endsWith("'") && after.length >= 2) {
    return after.slice(1, -1).replace(/''/g, "'");
  } else if (after.startsWith('"') && after.endsWith('"') && after.length >= 2) {
    return after.slice(1, -1);
  } else {
    return after;
  }
}

function readTitleLine(rawLine: string): string | null {
  const trimmed = rawLine.trim();
  if (trimmed.toLowerCase().startsWith("title:")) return unquoteTitle(trimmed.slice(6).trim());
  return null;
}

function scanFrontmatter(detect: string, start: number): { title: string | null; bodyStart: number } {
  let title: string | null = null;
  let pos = start;
  let bodyStart = -1;

  while (pos < detect.length) {
    const nl = detect.indexOf("\n", pos);
    const lineEnd = nl < 0 ? detect.length : nl;
    const line = detect.slice(pos, lineEnd).replace(/\s+$/, "");

    if (line === "---") {
      bodyStart = nl < 0 ? detect.length : nl + 1;
      break;
    }

    if (title === null) title = readTitleLine(detect.slice(pos, lineEnd));

    pos = nl < 0 ? detect.length : nl + 1;
  }

  return { title, bodyStart };
}

// Splits TASK.md content into a YAML-frontmatter title and the body that
// follows the closing `---`. Delimiters must be a line containing exactly
// `---` (trailing whitespace tolerated). If the input lacks a valid
// frontmatter block, the entire original content is returned as the body
// so we never hide useful text behind a malformed delimiter.
function parseTask(content: string | null): ParsedTask {
  const raw = content ?? "";
  // BOM is stripped only for delimiter/title detection; the fallback body
  // returns the original content unchanged.
  const detect = raw.startsWith("\uFEFF") ? raw.slice(1) : raw;

  const firstNl = detect.indexOf("\n");
  const firstLineEnd = firstNl < 0 ? detect.length : firstNl;
  const firstLine = detect.slice(0, firstLineEnd).replace(/\s+$/, "");
  // Reject prefixed openers like `---not`, `--- body`, or `----`.
  if (firstLine !== "---") return { title: null, body: raw };

  const { title, bodyStart } = scanFrontmatter(detect, firstNl < 0 ? detect.length : firstNl + 1);

  // Missing closer means malformed frontmatter — fall back to original.
  if (bodyStart < 0) return { title: null, body: raw };

  return { title, body: detect.slice(bodyStart) };
}

function parseTaskTitle(content: string | null): string | null {
  return parseTask(content).title;
}

// Backend is byte-exact (`has_entity_prefix`); `pathHasEntityDirSegment` mirrors it.
// Keep this regex case-sensitive so the UX gate matches; matching `WG-19`
// here would render the buttons enabled but every click would fail.
function hasWorkgroupContext(cwd: string): boolean {
  return pathHasEntityDirSegment(cwd);
}

const WorkgroupTask: Component = () => {
  const [editing, setEditing] = createSignal(false);
  const [titleDraft, setTitleDraft] = createSignal("");
  const [confirmingClean, setConfirmingClean] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [capturedSessionId, setCapturedSessionId] = createSignal<string | null>(null);

  const snapshot = createMemo(() => terminalStore.activeTaskSnapshot);
  const taskTitle = createMemo(() => snapshot()?.taskTitle?.trim() || "--No title specified--");
  const readable = createMemo(() => snapshot() !== null && !terminalStore.cleanPending);
  const tooltipStatus = createMemo(() => terminalStore.taskReadState === "ready" ? snapshot()?.status ?? null : null);
  const readMessage = createMemo(() => {
    if (terminalStore.cleanPending) return "Clean was saved, but the updated task could not be read. Clean is disabled until the task can be read.";
    if (terminalStore.taskReadState === "loading") return "Loading task…";
    if (terminalStore.taskReadState === "refreshing") return "Refreshing task…";
    if (terminalStore.taskReadState === "error") return readable() ? "Could not refresh the task." : "Could not read the task.";
    return null;
  });
  const tooltipId = createUniqueId();
  let titleAnchor: HTMLSpanElement | undefined;
  let tooltipElement: HTMLDivElement | undefined;
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
    if (!escaped && tooltipStatus() !== null) setTooltipOpen(true);
  };
  const enterTooltip = (region: "title" | "tooltip") => {
    if (region === "title") { if (!titlePointer) escaped = false; titlePointer = true; }
    else tooltipPointer = true;
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
    if (!titleAnchor || !tooltipElement) return;
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
    const maxWidth = Math.max(0, Math.min(640, width - 32));
    const maxHeight = Math.max(0, Math.min(384, height - 32));
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
    event.preventDefault();
    event.stopPropagation();
  };
  const tooltipKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") { dismissTooltip(event); return; }
    const element = tooltipElement;
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
  createEffect(() => {
    // Invalidation hides the overlay immediately, retaining Escape's latch.
    if (tooltipStatus() === null) { cancelLeave(); tooltipPointer = false; setTooltipOpen(false); }
  });
  createEffect(() => {
    const session = terminalStore.activeSessionId;
    void session;
    escaped = false;
    titlePointer = false;
    tooltipPointer = false;
    titleFocused = false;
    cancelLeave();
    setTooltipOpen(false);
  });
  createEffect(() => {
    if (!tooltipOpen()) return;
    scheduleTooltipPosition();
    window.addEventListener("scroll", scheduleTooltipPosition, true);
    window.addEventListener("resize", scheduleTooltipPosition);
    document.addEventListener("keydown", dismissTooltip, true);
    window.visualViewport?.addEventListener("scroll", scheduleTooltipPosition);
    window.visualViewport?.addEventListener("resize", scheduleTooltipPosition);
    onCleanup(() => {
      window.removeEventListener("scroll", scheduleTooltipPosition, true);
      window.removeEventListener("resize", scheduleTooltipPosition);
      document.removeEventListener("keydown", dismissTooltip, true);
      window.visualViewport?.removeEventListener("scroll", scheduleTooltipPosition);
      window.visualViewport?.removeEventListener("resize", scheduleTooltipPosition);
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = undefined;
    });
  });
  onCleanup(() => { cancelLeave(); if (frame !== undefined) cancelAnimationFrame(frame); });

  const mutationRoot = () => {
    const root = snapshot()?.workgroupRoot;
    if (root) return root;
    const parts = cwd().split(String.fromCharCode(92)).join("/").split("/");
    const index = parts.findIndex(part => part.startsWith("room-") || part.startsWith("wg-"));
    return parts.slice(0, index < 0 ? parts.length : index + 1).join("/");
  };
  const sessionId = createMemo(() => terminalStore.activeSessionId);
  const cwd = createMemo(() => terminalStore.activeWorkingDirectory);
  const baseDisabled = createMemo(
    () => !sessionId() || !hasWorkgroupContext(cwd()) || busy()
  );
  const editDisabled = createMemo(() => baseDisabled() || confirmingClean());
  const cleanDisabled = createMemo(() => baseDisabled() || editing() || terminalStore.cleanPending);
  const startEditing = async () => {
    if (editDisabled()) return;
    setError(null);
    const id = sessionId();
    if (!id) {
      setError("Session no longer available.");
      return;
    }
    // Lock out Clean while we await getTitle; otherwise the clean modal
    // could open in parallel with the editor (NB-1 race).
    setCapturedSessionId(id);
    setBusy(true);
    let prefill = snapshot()?.taskTitle ?? parseTaskTitle(terminalStore.activeWorkgroupTask) ?? "";
    try {
      const fromBackend = await TaskAPI.getTitle(id);
      if (fromBackend !== null && fromBackend !== undefined) {
        prefill = fromBackend;
      }
    } catch (err) {
      setError(String(err));
      setCapturedSessionId(null);
      setBusy(false);
      return;
    }
    if (sessionId() !== id) {
      setCapturedSessionId(null);
      setBusy(false);
      setError("Session changed; please retry.");
      return;
    }
    setTitleDraft(prefill);
    setEditing(true);
    setBusy(false);
  };

  const cancelEditing = () => {
    setEditing(false);
    setTitleDraft("");
    setCapturedSessionId(null);
    setError(null);
  };

  const saveTitle = async () => {
    const id = sessionId();
    if (!id) {
      setError("Session no longer available.");
      return;
    }
    if (capturedSessionId() !== id) {
      setError("Session changed; cancel and retry.");
      return;
    }
    const workgroupRoot = mutationRoot();
    const title = titleDraft().trim();
    if (!title) {
      setError("Title cannot be empty.");
      return;
    }
    setBusy(true);
    setError(null);
    terminalStore.beginTaskMutation(workgroupRoot);
    try {
      await TaskAPI.setTitle(id, title);
      setEditing(false);
      setTitleDraft("");
      setCapturedSessionId(null);
    } catch (err) {
      setError(String(err));
    } finally {
      terminalStore.finishTaskMutation(workgroupRoot, false);
      setBusy(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      saveTitle();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelEditing();
    }
  };


  const requestClean = () => {
    if (cleanDisabled()) return;
    const id = sessionId();
    if (!id) return;
    setError(null);
    setCapturedSessionId(id);
    setConfirmingClean(true);
  };

  const performClean = async () => {
    const id = sessionId();
    setConfirmingClean(false);
    if (!id) {
      setCapturedSessionId(null);
      setError("Session no longer available.");
      return;
    }
    if (capturedSessionId() !== id) {
      setCapturedSessionId(null);
      setError("Session changed; cancel and retry.");
      return;
    }
    const workgroupRoot = mutationRoot();
    let committed = false;
    setBusy(true);
    setError(null);
    terminalStore.beginTaskMutation(workgroupRoot);
    try {
      await TaskAPI.clean(id);
      committed = true;
      setEditing(false);
      setTitleDraft("");
    } catch (err) {
      committed = String(err).includes("task mutation already committed");
      setError(String(err));
    } finally {
      terminalStore.finishTaskMutation(workgroupRoot, committed);
      setCapturedSessionId(null);
      setBusy(false);
    }
  };

  const onInputRef = (el: HTMLInputElement) => {
    requestAnimationFrame(() => {
      el.focus();
      el.select();
    });
  };


  return (
    <div class="workgroup-task-panel">
      <div class="workgroup-task-header">
        <div class="workgroup-task-label">
          TASK
          <Show when={readable()}>
            <span>: </span>
            <span ref={titleAnchor} class="workgroup-task-title" tabIndex={0}
              aria-describedby={tooltipStatus() !== null ? tooltipId : undefined}
              onPointerEnter={() => enterTooltip("title")} onPointerLeave={() => leaveTooltip("title")}
              onFocus={() => { titleFocused = true; escaped = false; openTooltip(); }}
              onBlur={() => { titleFocused = false; leaveTooltip("focus"); }}
              onKeyDown={tooltipKeyDown}>{taskTitle()}</span>
          </Show>
        </div>
        <div class="workgroup-task-actions">
          <button
            class="workgroup-task-action"
            onClick={startEditing}
            disabled={editDisabled()}
            title="Edit TASK title"
            type="button"
          >
            &#x270E;
          </button>
          <button
            class="workgroup-task-action"
            onClick={requestClean}
            disabled={cleanDisabled()}
            title="Clean TASK (reset for new topic)"
            type="button"
          >
            &#x1F9F9;
          </button>
        </div>
      </div>
      <Show when={readMessage()}>
        <div class="workgroup-task-error" classList={{ "workgroup-task-loading": terminalStore.taskReadState === "loading" || terminalStore.taskReadState === "refreshing" }}>{readMessage()}</div>
      </Show>
      <Show when={readable()}><div class="workgroup-task-text">{snapshot()?.description}</div></Show>
      <Show when={snapshot()?.tailIncomplete && readable()}><div class="workgroup-task-error">Task history is incomplete.</div></Show>
      <Show when={tooltipStatus() !== null}>
        <Portal><div id={tooltipId} ref={tooltipElement} role="tooltip" class="workgroup-task-tooltip"
          style={{ display: tooltipOpen() && tooltipVisible() ? "block" : "none", left: tooltipPosition().left + "px", top: tooltipPosition().top + "px",
            "max-width": tooltipPosition().width + "px", "max-height": tooltipPosition().height + "px" }}
          onPointerEnter={() => enterTooltip("tooltip")} onPointerLeave={() => leaveTooltip("tooltip")}>{tooltipStatus()}</div></Portal>
      </Show>
      <Show when={editing()}>
        <div class="workgroup-task-title-edit">
          <input
            ref={onInputRef}
            class="workgroup-task-title-input"
            value={titleDraft()}
            onInput={(e) => setTitleDraft(e.currentTarget.value)}
            onKeyDown={handleKeyDown}
            placeholder="Title"
            disabled={busy()}
          />
          <button
            class="workgroup-task-title-btn save"
            onClick={saveTitle}
            disabled={busy() || !titleDraft().trim()}
            type="button"
          >
            Save
          </button>
          <button
            class="workgroup-task-title-btn cancel"
            onClick={cancelEditing}
            disabled={busy()}
            type="button"
          >
            Cancel
          </button>
        </div>
      </Show>
      <Show when={error()}>
        <div class="workgroup-task-error">{error()}</div>
      </Show>
      <Show when={confirmingClean()}>
        <Portal>
          <TaskCleanConfirmModal
            onCancel={() => {
              setConfirmingClean(false);
              setCapturedSessionId(null);
            }}
            onConfirm={performClean}
          />
        </Portal>
      </Show>
    </div>
  );
};

export default WorkgroupTask;
