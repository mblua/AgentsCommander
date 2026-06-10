import { Component, createSignal, createMemo, For, Show, onMount, onCleanup } from "solid-js";
import { EntityAPI, RoleTemplateAPI } from "../../shared/ipc";
import type { AgencyTemplatesStatus, RoleTemplateMeta } from "../../shared/types";
import {
  applyTemplatePrefill,
  filterRoleTemplates,
  sourceLabel,
} from "../../shared/role-templates";
import { projectStore } from "../stores/project";

const NewEntityAgentModal: Component<{
  projectPath: string;
  onClose: () => void;
}> = (props) => {
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);

  // #278 — distinguish user-edited fields from template-populated ones so a
  // second template click can overwrite the first template's values without
  // clobbering anything the user typed by hand.
  const [nameDirty, setNameDirty] = createSignal(false);
  const [descriptionDirty, setDescriptionDirty] = createSignal(false);

  // #271 — template picker state.
  const [templates, setTemplates] = createSignal<RoleTemplateMeta[]>([]);
  const [templateQuery, setTemplateQuery] = createSignal("");
  const [selectedTemplateId, setSelectedTemplateId] = createSignal<string | null>(null);
  const [agencyStatus, setAgencyStatus] = createSignal<AgencyTemplatesStatus | null>(null);
  const [agencyInstalling, setAgencyInstalling] = createSignal(false);
  const [agencyInstallError, setAgencyInstallError] = createSignal("");
  let nameRef!: HTMLInputElement;
  let listRef: HTMLDivElement | undefined;

  let mounted = true;
  onCleanup(() => {
    mounted = false;
  });

  const reloadTemplatesAndAgencyStatus = async () => {
    const [templatesResult, statusResult] = await Promise.allSettled([
      RoleTemplateAPI.list(),
      RoleTemplateAPI.status(),
    ]);
    if (!mounted) return;
    if (templatesResult.status === "fulfilled") {
      setTemplates(templatesResult.value);
    } else {
      console.error("list_role_templates failed:", templatesResult.reason);
    }
    if (statusResult.status === "fulfilled") {
      setAgencyStatus(statusResult.value);
    } else {
      console.error("get_agency_templates_status failed:", statusResult.reason);
    }
  };

  onMount(async () => {
    await reloadTemplatesAndAgencyStatus();
  });

  const filteredTemplates = createMemo(() =>
    filterRoleTemplates(templates(), templateQuery()),
  );
  const shouldShowAgencyInstall = createMemo(() => agencyStatus()?.available === false);

  const selectTemplate = (t: RoleTemplateMeta | null) => {
    setSelectedTemplateId(t ? t.id : null);
    if (t) {
      const next = applyTemplatePrefill(t, {
        name: name(),
        description: description(),
        nameDirty: nameDirty(),
        descriptionDirty: descriptionDirty(),
      });
      setName(next.name);
      setDescription(next.description);
      setError("");
    }
  };

  const errorMessage = (err: unknown, fallback: string) =>
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : fallback;

  const handleInstallAgencyTemplates = async (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (agencyInstalling()) return;
    setAgencyInstalling(true);
    setAgencyInstallError("");
    try {
      await RoleTemplateAPI.updateAgencyTemplates();
      await reloadTemplatesAndAgencyStatus();
    } catch (err: unknown) {
      console.error("update_agency_templates failed:", err);
      if (mounted) {
        setAgencyInstallError(errorMessage(err, "Failed to install Agents Agency templates"));
      }
    } finally {
      if (mounted) setAgencyInstalling(false);
    }
  };

  const canCreate = createMemo(() => {
    const n = name().trim();
    return n.length > 0 && !n.includes("/") && !n.includes("\\") && !n.includes(" ");
  });

  const handleCreate = async () => {
    if (!canCreate() || creating()) return;
    setCreating(true);
    setError("");
    try {
      await EntityAPI.createAgentMatrix(
        props.projectPath,
        name().trim(),
        description().trim(),
        selectedTemplateId(),
      );
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
      // intentionally do NOT clear creating() — modal unmounts; any in-flight
      // keydown event in the close transition stays guarded.
    } catch (e: any) {
      console.error("create_agent_matrix failed:", e);
      setError(typeof e === "string" ? e : e.message || "Failed to create agent");
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      handleCreate();
    }
  };

  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  // Roving Tab/Arrow navigation across picker rows. Enter activates the focused
  // row via the browser default; the list container then swallows Enter so it
  // never bubbles up to the overlay's create-on-Enter handler.
  const focusRowByOffset = (e: KeyboardEvent, offset: number) => {
    if (!listRef) return;
    const rows = Array.from(
      listRef.querySelectorAll<HTMLButtonElement>("button.tpl-picker-item"),
    );
    if (rows.length === 0) return;
    const active = document.activeElement as HTMLElement | null;
    const currentIndex = active ? rows.indexOf(active as HTMLButtonElement) : -1;
    const nextIndex =
      currentIndex < 0
        ? offset > 0
          ? 0
          : rows.length - 1
        : (currentIndex + offset + rows.length) % rows.length;
    e.preventDefault();
    rows[nextIndex].focus();
  };

  const handleListKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      // Prevent the modal's create-on-Enter from firing when the user is
      // navigating the picker with the keyboard.
      e.stopPropagation();
      return;
    }
    if (e.key === "ArrowDown") {
      focusRowByOffset(e, 1);
      return;
    }
    if (e.key === "ArrowUp") {
      focusRowByOffset(e, -1);
      return;
    }
  };

  const handleSearchKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      // Pressing Enter while typing in the search input must not also trigger
      // create — the user is still picking a template.
      e.stopPropagation();
      e.preventDefault();
      return;
    }
    if (e.key === "ArrowDown") {
      // Jump from the search input down into the list.
      focusRowByOffset(e, 1);
      return;
    }
  };

  return (
    <div class="modal-overlay" onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Agent</span>
        </div>

        <div class="new-agent-form new-agent-form-scroll">
          {/* #271 — optional role-template picker */}
          <div class="new-agent-field tpl-picker">
            <label class="new-agent-label">Template (optional)</label>
            <div class="tpl-picker-search">
              <svg
                class="tpl-picker-search-icon"
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <circle cx="11" cy="11" r="8" />
                <line x1="21" y1="21" x2="16.65" y2="16.65" />
              </svg>
              <input
                class="tpl-picker-search-input"
                value={templateQuery()}
                onInput={(e) => setTemplateQuery(e.currentTarget.value)}
                onKeyDown={handleSearchKeyDown}
                placeholder="Search templates..."
                aria-label="Search role templates"
              />
            </div>
            <Show when={shouldShowAgencyInstall()}>
              <div class="tpl-agency-install">
                <div class="tpl-agency-install-copy">
                  <div class="tpl-agency-install-title">Agents Agency templates are not installed</div>
                  <div class="tpl-agency-install-desc">
                    Install tested specialist role templates for new agents. You can still create a blank agent.
                  </div>
                  <Show when={agencyInstallError()}>
                    <div class="tpl-agency-install-error" role="status" aria-live="polite">
                      {agencyInstallError()}
                    </div>
                  </Show>
                </div>
                <button
                  type="button"
                  class="tpl-agency-install-btn"
                  disabled={agencyInstalling()}
                  onClick={handleInstallAgencyTemplates}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") e.stopPropagation();
                  }}
                >
                  <span aria-live="polite">
                    {agencyInstalling() ? "Installing..." : "Install templates"}
                  </span>
                </button>
              </div>
            </Show>
            <div
              class="tpl-picker-list"
              ref={listRef}
              onKeyDown={handleListKeyDown}
              role="listbox"
              aria-label="Role templates"
            >
              {/* "No template" row is always present, regardless of the filter.
                  Rows are real <button>s so they are keyboard-focusable;
                  type="button" stops the default form submit, and the list's
                  onKeyDown above stops a row Enter from bubbling to the
                  overlay's create-on-Enter handler. */}
              <button
                type="button"
                class="tpl-picker-item"
                classList={{ selected: selectedTemplateId() === null }}
                onClick={() => selectTemplate(null)}
                role="option"
                aria-selected={selectedTemplateId() === null}
              >
                <div class="tpl-picker-item-name">No template</div>
                <div class="tpl-picker-item-desc">Blank role — current default behavior</div>
              </button>
              <For each={filteredTemplates()}>
                {(t) => (
                  <button
                    type="button"
                    class="tpl-picker-item"
                    classList={{ selected: selectedTemplateId() === t.id }}
                    onClick={() => selectTemplate(t)}
                    title={t.description}
                    role="option"
                    aria-selected={selectedTemplateId() === t.id}
                  >
                    <div class="tpl-picker-item-head">
                      <Show when={t.color}>
                        <span class="tpl-picker-color-dot" style={{ background: t.color! }} />
                      </Show>
                      <span class="tpl-picker-item-name">{t.name}</span>
                      <span class="tpl-picker-item-cat">{t.category}</span>
                      <Show when={t.hasSkills}>
                        <span class="tpl-picker-item-cat">skills</span>
                      </Show>
                      <span
                        class="tpl-picker-item-src"
                        classList={{
                          "src-agency": t.source === "agency",
                          "src-local": t.source === "local",
                        }}
                        title={sourceLabel(t)}
                      >
                        {sourceLabel(t)}
                      </span>
                    </div>
                    <Show when={t.description}>
                      <div class="tpl-picker-item-desc">{t.description}</div>
                    </Show>
                  </button>
                )}
              </For>
              <Show when={templateQuery().trim() !== "" && filteredTemplates().length === 0}>
                <div class="tpl-picker-empty">No templates match "{templateQuery()}"</div>
              </Show>
            </div>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Name</label>
            <input
              ref={nameRef!}
              class="agent-search-input"
              value={name()}
              onInput={(e) => {
                setName(e.currentTarget.value);
                setNameDirty(true);
                setError("");
              }}
              placeholder="my-agent"
              autofocus
            />
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Description</label>
            <textarea
              class="entity-textarea"
              value={description()}
              onInput={(e) => {
                setDescription(e.currentTarget.value);
                setDescriptionDirty(true);
              }}
              placeholder="What does this agent do? (optional, max 250 chars)"
              maxLength={250}
              rows={3}
              aria-describedby="description-keyhint"
            />
            <div class="entity-textarea-meta">
              <span id="description-keyhint" class="entity-textarea-hint">
                Enter to create · Shift+Enter for newline
              </span>
              <Show when={description().length > 0}>
                <span class="entity-char-count">{description().length}/250</span>
              </Show>
            </div>
          </div>

          <Show when={error()}>
            <div class="new-agent-error">{error()}</div>
          </Show>
        </div>

        <div class="new-agent-footer">
          <button type="button" class="new-agent-cancel-btn" onClick={() => props.onClose()}>
            Cancel
          </button>
          <button
            class="new-agent-create-btn"
            disabled={!canCreate() || creating()}
            onClick={handleCreate}
          >
            {creating() ? "Creating..." : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default NewEntityAgentModal;
