import {
  Component,
  For,
  Show,
  createResource,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import { ProjectAPI } from "../../shared/ipc";
import type { ArchivedProject } from "../../shared/types";
import { projectStore } from "../stores/project";
import { automationIdPart } from "./replica-repo-badges";

interface ArchivedProjectsModalProps {
  onClose: () => void;
}

function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
}

const archiveRowId = (path: string) => automationIdPart(path);

const ArchivedProjectsModal: Component<ArchivedProjectsModalProps> = (props) => {
  const [rows, { refetch }] = createResource(() => ProjectAPI.listArchived());
  const [busyPath, setBusyPath] = createSignal<string | null>(null);
  const [rowError, setRowError] = createSignal<{ path: string; message: string } | null>(null);
  let modalRef: HTMLDivElement | undefined;
  let closeButtonRef: HTMLButtonElement | undefined;
  let previouslyFocused: HTMLElement | null = null;

  const refreshRows = async () => {
    try {
      await refetch();
    } catch {
      // createResource exposes refetch failures through rows.error.
    }
  };

  onMount(() => {
    previouslyFocused = document.activeElement as HTMLElement | null;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        props.onClose();
        return;
      }
      if (event.key !== "Tab") return;

      const focusables = Array.from(
        modalRef?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
        ) ?? []
      );
      if (focusables.length === 0) return;

      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (!active || !modalRef?.contains(active)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
        return;
      }
      if (event.shiftKey && active === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    queueMicrotask(() => closeButtonRef?.focus());
    onCleanup(() => {
      document.removeEventListener("keydown", onKeyDown);
      try {
        previouslyFocused?.focus();
      } catch {
        // Best effort only: the original target may have been removed.
      }
    });
  });

  const unarchive = async (row: ArchivedProject) => {
    setBusyPath(row.path);
    setRowError(null);
    try {
      await projectStore.unarchiveProject(row.path);
    } catch (error) {
      setRowError({ path: row.path, message: errorMessage(error) });
      return;
    } finally {
      setBusyPath(null);
    }
    await refreshRows();
  };

  const forget = async (row: ArchivedProject) => {
    setBusyPath(row.path);
    setRowError(null);
    try {
      await ProjectAPI.remove(row.path);
    } catch (error) {
      setRowError({ path: row.path, message: errorMessage(error) });
      return;
    } finally {
      setBusyPath(null);
    }
    await refreshRows();
  };

  return (
    <div class="modal-overlay" data-ac-testid="archivedProjects.modal">
      <div
        class="agent-modal archived-projects-modal"
        ref={modalRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="archived-projects-title"
      >
        <div class="agent-modal-header">
          <span class="agent-modal-title" id="archived-projects-title">
            Archived projects
          </span>
          <button
            ref={closeButtonRef}
            class="modal-close"
            onClick={props.onClose}
            aria-label="Close archived projects"
          >
            &times;
          </button>
        </div>

        <div class="archived-projects-list">
          <Show when={rows.loading}>
            <div class="workgroup-groups-empty">Loading...</div>
          </Show>

          <Show when={!rows.loading && rows.error}>
            <div
              class="workgroup-groups-errors"
              role="alert"
              data-ac-testid="archivedProjects.error"
            >
              {errorMessage(rows.error)}
            </div>
          </Show>

          <Show when={!rows.loading && !rows.error && (rows()?.length ?? 0) === 0}>
            <div class="workgroup-groups-empty" data-ac-testid="archivedProjects.empty">
              No archived projects
            </div>
          </Show>

          <Show when={!rows.loading && !rows.error && (rows()?.length ?? 0) > 0}>
            <For each={rows() ?? []}>
              {(row) => {
                const rowId = () => archiveRowId(row.path);
                const busy = () => busyPath() === row.path;
                const error = () => {
                  const current = rowError();
                  return current?.path === row.path ? current.message : null;
                };
                return (
                  <div
                    class="archived-projects-row"
                    classList={{
                      "archived-projects-row-warning": !row.exists || !row.hasAcRoot,
                    }}
                    data-ac-testid={`archivedProjects.row.${rowId()}`}
                  >
                    <div class="archived-projects-main">
                      <div class="archived-projects-name" title={row.path}>
                        {row.folderName}
                      </div>
                      <div class="archived-projects-path" title={row.path}>
                        {row.path}
                      </div>
                      <Show when={!row.exists || !row.hasAcRoot}>
                        <div class="archived-projects-warning">
                          {!row.exists ? "Folder missing" : "Workspace missing"}
                        </div>
                      </Show>
                    </div>
                    <div class="archived-projects-actions">
                      <button
                        class="modal-btn modal-btn-save"
                        disabled={busy()}
                        onClick={() => void unarchive(row)}
                        data-ac-testid={`archivedProjects.unarchive.${rowId()}`}
                      >
                        {busy() ? "Unarchiving..." : "Unarchive"}
                      </button>
                      <Show when={!row.exists || !row.hasAcRoot}>
                        <button
                          class="modal-btn modal-btn-cancel"
                          disabled={busy()}
                          onClick={() => void forget(row)}
                          data-ac-testid={`archivedProjects.forget.${rowId()}`}
                        >
                          Remove from list
                        </button>
                      </Show>
                    </div>
                    <Show when={error()}>
                      {(message) => (
                        <div
                          class="workgroup-groups-errors archived-projects-row-error"
                          role="alert"
                          data-ac-testid={`archivedProjects.rowError.${rowId()}`}
                        >
                          {message()}
                        </div>
                      )}
                    </Show>
                  </div>
                );
              }}
            </For>
          </Show>
        </div>

        <div class="agent-modal-footer">
          <button class="modal-btn modal-btn-cancel" onClick={props.onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
};

export default ArchivedProjectsModal;
