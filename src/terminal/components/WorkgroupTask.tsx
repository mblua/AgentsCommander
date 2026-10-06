import { createTaskStatusTooltip } from "../../shared/task-status-tooltip";
import { Component, createEffect, createMemo, createSignal, createUniqueId, on, onCleanup, untrack, Show } from "solid-js";
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
    if (terminalStore.cleanPending) return terminalStore.cleanReadFailed
      ? "Clean was saved, but the updated task could not be read. Clean is disabled until the task can be read."
      : "Loading task…";
    if (terminalStore.taskReadState === "loading") return "Loading task…";
    if (terminalStore.taskReadState === "refreshing") return "Refreshing task…";
    if (terminalStore.taskReadState === "error") return readable() ? "Could not refresh the task." : "Could not read the task.";
    return null;
  });
  const tooltipId = createUniqueId();
  let titleAnchor: HTMLSpanElement | undefined;
  let tooltipElement: HTMLDivElement | undefined;
  const tooltip = createTaskStatusTooltip({
    status: tooltipStatus, titleAnchor: () => titleAnchor, tooltipElement: () => tooltipElement,
  });
  const { open: tooltipOpen, visible: tooltipVisible, position: tooltipPosition,
    enter: enterTooltip, leave: leaveTooltip } = tooltip;
  const dismissTooltip = (event: KeyboardEvent) => {
    if (tooltipOpen()) tooltip.dismiss(event);
  };
  const tooltipKeyDown = (event: KeyboardEvent) => tooltip.keyDown(event, tooltipOpen());
  createEffect(() => {
    if (tooltipStatus() === null) untrack(tooltip.hideUnavailable);
  });
  createEffect(on(() => terminalStore.activeSessionId, () => tooltip.resetIdentity()));
  createEffect(() => {
    if (!tooltipOpen()) return;
    document.addEventListener("keydown", dismissTooltip, true);
    onCleanup(() => document.removeEventListener("keydown", dismissTooltip, true));
  });

  const mutationRoot = () => {
    const root = snapshot()?.workgroupRoot;
    if (root) return root;
    const parts = cwd().split(String.fromCharCode(92)).join("/").split("/");
    const index = parts.map(part => part.startsWith("room-") || part.startsWith("wg-")).lastIndexOf(true);
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
    let workgroupRoot = mutationRoot();
    const title = titleDraft().trim();
    if (!title) {
      setError("Title cannot be empty.");
      return;
    }
    setBusy(true);
    setError(null);
    terminalStore.beginTaskMutation(workgroupRoot);
    try {
      const result = await TaskAPI.setTitle(id, title);
      workgroupRoot = result.workgroupRoot;
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
    let workgroupRoot = mutationRoot();
    let committed = false;
    setBusy(true);
    setError(null);
    terminalStore.beginTaskMutation(workgroupRoot);
    try {
      const result = await TaskAPI.clean(id);
      workgroupRoot = result.workgroupRoot;
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
    <div data-ac-testid="workgroupTask.root" data-ac-role="surface" class="workgroup-task-panel">
      <div class="workgroup-task-header">
        <div class="workgroup-task-label">
          TASK
          <Show when={readable()}>
            <span>: </span>
            <span data-ac-testid="workgroupTask.title" data-ac-role="surface" ref={titleAnchor} class="workgroup-task-title" tabIndex={0}
              aria-describedby={tooltipStatus() !== null ? tooltipId : undefined}
              onPointerEnter={() => enterTooltip("title")} onPointerLeave={() => leaveTooltip("title")}
              onFocus={tooltip.focus}
              onBlur={tooltip.blur}
              onKeyDown={tooltipKeyDown}>{taskTitle()}</span>
          </Show>
        </div>
        <div class="workgroup-task-actions">
          <button
            class="workgroup-task-action"
            onClick={startEditing}
            disabled={editDisabled()}
            data-ac-testid="workgroupTask.edit" data-ac-role="button"
            title="Edit TASK title"
            type="button"
          >
            &#x270E;
          </button>
          <button
            class="workgroup-task-action"
            onClick={requestClean}
            disabled={cleanDisabled()}
            data-ac-testid="workgroupTask.clean" data-ac-role="button"
            title="Clean TASK (reset for new topic)"
            type="button"
          >
            &#x1F9F9;
          </button>
        </div>
      </div>
      <Show when={readMessage()}>
        <div data-ac-testid="workgroupTask.readMessage" data-ac-role="status" class="workgroup-task-error" classList={{ "workgroup-task-loading": terminalStore.taskReadState === "loading" || terminalStore.taskReadState === "refreshing" || (terminalStore.cleanPending && !terminalStore.cleanReadFailed) }}>{readMessage()}</div>
      </Show>
      <Show when={readable()}><div data-ac-testid="workgroupTask.description" data-ac-role="surface" class="workgroup-task-text">{snapshot()?.description}</div></Show>
      <Show when={snapshot()?.tailIncomplete && readable()}><div data-ac-testid="workgroupTask.historyIncomplete" data-ac-role="status" class="workgroup-task-error">Task history is incomplete.</div></Show>
      <Show when={tooltipStatus() !== null}>
        <Portal><div data-ac-testid="workgroupTask.tooltip" data-ac-role="overlay" id={tooltipId} ref={tooltipElement} role="tooltip" class="workgroup-task-tooltip"
          style={{ display: tooltipOpen() && tooltipVisible() ? "block" : "none", left: tooltipPosition().left + "px", top: tooltipPosition().top + "px",
            "max-width": tooltipPosition().width + "px", "max-height": tooltipPosition().height + "px" }}
          onPointerEnter={() => enterTooltip("tooltip")} onPointerLeave={() => leaveTooltip("tooltip")}>{tooltipStatus()}</div></Portal>
      </Show>
      <Show when={editing()}>
        <div class="workgroup-task-title-edit">
          <input
            data-ac-testid="workgroupTask.titleInput" data-ac-role="textbox"
            ref={onInputRef}
            class="workgroup-task-title-input"
            value={titleDraft()}
            onInput={(e) => setTitleDraft(e.currentTarget.value)}
            onKeyDown={handleKeyDown}
            placeholder="Title"
            disabled={busy()}
          />
          <button
            data-ac-testid="workgroupTask.save" data-ac-role="button"
            class="workgroup-task-title-btn save"
            onClick={saveTitle}
            disabled={busy() || !titleDraft().trim()}
            type="button"
          >
            Save
          </button>
          <button
            data-ac-testid="workgroupTask.cancel" data-ac-role="button"
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
        <div data-ac-testid="workgroupTask.mutationError" data-ac-role="status" class="workgroup-task-error">{error()}</div>
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
