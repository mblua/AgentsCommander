// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import ContextTemplateUpdateModal from "./ContextTemplateUpdateModal";
import type { ContextTemplateUpdate } from "../../shared/types";

function target<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing test target: ${testId}`);
  return element;
}

function makeUpdate(
  overrides: Partial<ContextTemplateUpdate> = {},
): ContextTemplateUpdate {
  return {
    projectPath: "C:\\Project",
    acRootPath: "C:\\Project\\.ac",
    filePath: "C:\\Project\\.ac\\Context.coordinator.md",
    filename: "Context.coordinator.md",
    label: "Coordinator context",
    currentFileSha256: "file-a",
    currentDefaultSha256: "default-2",
    currentDefaultVersion: 2,
    ...overrides,
  };
}

function renderModal(
  overrides: Partial<{
    update: ContextTemplateUpdate;
    busy: boolean;
    error: string | null;
    onKeep: () => void;
    onOverwrite: () => void;
  }> = {},
) {
  const root = document.createElement("div");
  const onKeep = vi.fn();
  const onOverwrite = vi.fn();
  document.body.append(root);
  const dispose = render(
    () => (
      <ContextTemplateUpdateModal
        update={overrides.update ?? makeUpdate()}
        busy={overrides.busy ?? false}
        error={overrides.error ?? null}
        onKeep={overrides.onKeep ?? onKeep}
        onOverwrite={overrides.onOverwrite ?? onOverwrite}
      />
    ),
    root,
  );
  return { dispose, onKeep, onOverwrite };
}

describe("ContextTemplateUpdateModal (#695)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("renders the template label, file path, and both action buttons", () => {
    const { dispose } = renderModal({
      update: makeUpdate({
        label: "Coordinator context",
        filePath: "C:\\P\\.ac\\Context.coordinator.md",
      }),
    });
    const body = target("contextTemplateUpdate.modal").textContent ?? "";
    expect(body).toContain("Coordinator context");
    expect(body).toContain("C:\\P\\.ac\\Context.coordinator.md");
    expect(
      target<HTMLButtonElement>("contextTemplateUpdate.keep").textContent?.trim(),
    ).toBe("Keep my version");
    expect(
      target<HTMLButtonElement>("contextTemplateUpdate.overwrite").textContent?.trim(),
    ).toBe("Overwrite with default");
    dispose();
  });

  it("explains that overwrite preserves the old file as a .bak backup", () => {
    const { dispose } = renderModal();
    const body = (target("contextTemplateUpdate.modal").textContent ?? "").toLowerCase();
    expect(body).toContain(".bak");
    expect(body).toContain("customized");
    dispose();
  });

  it("Keep my version triggers only onKeep", () => {
    const { dispose, onKeep, onOverwrite } = renderModal();
    target<HTMLButtonElement>("contextTemplateUpdate.keep").click();
    expect(onKeep).toHaveBeenCalledTimes(1);
    expect(onOverwrite).not.toHaveBeenCalled();
    dispose();
  });

  it("Overwrite with default triggers only onOverwrite", () => {
    const { dispose, onKeep, onOverwrite } = renderModal();
    target<HTMLButtonElement>("contextTemplateUpdate.overwrite").click();
    expect(onOverwrite).toHaveBeenCalledTimes(1);
    expect(onKeep).not.toHaveBeenCalled();
    dispose();
  });

  it("disables both buttons while busy and fires no callback when clicked", () => {
    const { dispose, onKeep, onOverwrite } = renderModal({ busy: true });
    const keep = target<HTMLButtonElement>("contextTemplateUpdate.keep");
    const overwrite = target<HTMLButtonElement>("contextTemplateUpdate.overwrite");
    expect(keep.disabled).toBe(true);
    expect(overwrite.disabled).toBe(true);
    keep.click();
    overwrite.click();
    expect(onKeep).not.toHaveBeenCalled();
    expect(onOverwrite).not.toHaveBeenCalled();
    dispose();
  });

  it("renders no error chip by default", () => {
    const { dispose } = renderModal();
    expect(
      document.querySelector('[data-ac-testid="contextTemplateUpdate.error"]'),
    ).toBeNull();
    dispose();
  });

  it("surfaces the error via the error chip when provided", () => {
    const { dispose } = renderModal({
      error: "Context template changed on disk; reload the project before overwriting.",
    });
    expect(target("contextTemplateUpdate.error").textContent ?? "").toContain(
      "changed on disk",
    );
    dispose();
  });

  // grinch fix #6 / plan §15: the only resolution actions are the two buttons.
  // A backdrop click must NOT dismiss the modal (closing without a backend
  // keep/overwrite decision would re-show the same notice on next discovery).
  it("does not resolve or unmount on a backdrop (overlay) click", () => {
    const { dispose, onKeep, onOverwrite } = renderModal();
    target("contextTemplateUpdate.overlay").click();
    expect(onKeep).not.toHaveBeenCalled();
    expect(onOverwrite).not.toHaveBeenCalled();
    expect(
      document.querySelector('[data-ac-testid="contextTemplateUpdate.modal"]'),
    ).not.toBeNull();
    dispose();
  });

  it("does not resolve or unmount on Escape", () => {
    const { dispose, onKeep, onOverwrite } = renderModal();
    target("contextTemplateUpdate.overlay").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    expect(onKeep).not.toHaveBeenCalled();
    expect(onOverwrite).not.toHaveBeenCalled();
    expect(
      document.querySelector('[data-ac-testid="contextTemplateUpdate.modal"]'),
    ).not.toBeNull();
    dispose();
  });
});
