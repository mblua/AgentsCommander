import { Component, createMemo, createSignal, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { terminalStore } from "../stores/terminal";
import { TaskAPI } from "../../shared/ipc";
import { pathHasEntityDirSegment } from "../../shared/entity-prefix";
import TaskCleanConfirmModal from "./TaskCleanConfirmModal";

interface ParsedTask {
  title: string | null;
  body: string;
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

  let title: string | null = null;
  let pos = firstNl < 0 ? detect.length : firstNl + 1;
  let bodyStart = -1;

  while (pos < detect.length) {
    const nl = detect.indexOf("\n", pos);
    const lineEnd = nl < 0 ? detect.length : nl;
    const line = detect.slice(pos, lineEnd).replace(/\s+$/, "");

    if (line === "---") {
      bodyStart = nl < 0 ? detect.length : nl + 1;
      break;
    }

    if (title === null) {
      const trimmed = detect.slice(pos, lineEnd).trim();
      if (trimmed.toLowerCase().startsWith("title:")) {
        const after = trimmed.slice(6).trim();
        if (after.startsWith("'") && after.endsWith("'") && after.length >= 2) {
          title = after.slice(1, -1).replace(/''/g, "'");
        } else if (after.startsWith('"') && after.endsWith('"') && after.length >= 2) {
          title = after.slice(1, -1);
        } else {
          title = after;
        }
      }
    }

    pos = nl < 0 ? detect.length : nl + 1;
  }

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

  const parsedTask = createMemo<ParsedTask>(() =>
    parseTask(terminalStore.activeWorkgroupTask ?? "")
  );
  const taskTitle = createMemo(() => parsedTask().title?.trim() || null);
  const sessionId = createMemo(() => terminalStore.activeSessionId);
  const cwd = createMemo(() => terminalStore.activeWorkingDirectory);
  const baseDisabled = createMemo(
    () => !sessionId() || !hasWorkgroupContext(cwd()) || busy()
  );
  const editDisabled = createMemo(() => baseDisabled() || confirmingClean());
  const cleanDisabled = createMemo(() => baseDisabled() || editing());
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
    let prefill = parseTaskTitle(terminalStore.activeWorkgroupTask) ?? "";
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
    const title = titleDraft().trim();
    if (!title) {
      setError("Title cannot be empty.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await TaskAPI.setTitle(id, title);
      terminalStore.applyLocalTask(result.workgroupRoot, result.task);
      setEditing(false);
      setTitleDraft("");
      setCapturedSessionId(null);
    } catch (err) {
      setError(String(err));
    } finally {
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
    setBusy(true);
    setError(null);
    try {
      const result = await TaskAPI.clean(id);
      terminalStore.applyLocalTask(result.workgroupRoot, result.task);
      setEditing(false);
      setTitleDraft("");
    } catch (err) {
      setError(String(err));
    } finally {
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
          <Show when={taskTitle()}>
            <span class="workgroup-task-label-sep">: </span>
            <span class="workgroup-task-title">{taskTitle()}</span>
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
