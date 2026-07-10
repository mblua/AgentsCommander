// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ProjectArchiveChanged } from "../../shared/types";
import { click, waitFor } from "../../shared/testing/ui-harness";
import { autoUnarchiveStore } from "../stores/auto-unarchive";
import AutoUnarchiveModal from "./AutoUnarchiveModal";

function event(
  path: string,
  folderName: string,
  reason: ProjectArchiveChanged["reason"] = "autoUnarchive",
): ProjectArchiveChanged {
  return {
    path,
    folderName,
    archived: false,
    reason,
    sessionName: `${folderName}/dev-webpage-ui`,
  };
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

describe("AutoUnarchiveModal (#881)", () => {
  let dispose: (() => void) | null = null;

  beforeEach(() => {
    autoUnarchiveStore.acknowledge();
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(() => <AutoUnarchiveModal />, root);
  });

  afterEach(() => {
    dispose?.();
    dispose = null;
    autoUnarchiveStore.acknowledge();
    document.body.replaceChildren();
  });

  it("coalesces auto-unarchive events, dedupes by project path, and requires acknowledgement", async () => {
    autoUnarchiveStore.push(event("C:\\ProjectA", "ProjectA"));

    await waitFor(() => expect(byTestId("autoUnarchive.modal")).toBeTruthy());
    expect(byTestId("autoUnarchive.modal").textContent).toContain("ProjectA");
    expect(byTestId("autoUnarchive.modal").textContent).toContain("ProjectA/dev-webpage-ui");

    autoUnarchiveStore.push(event("C:\\ProjectB", "ProjectB"));
    await waitFor(() =>
      expect(document.querySelectorAll('[data-ac-testid^="autoUnarchive.row."]')).toHaveLength(2),
    );
    expect(document.querySelectorAll('[data-ac-testid="autoUnarchive.modal"]')).toHaveLength(1);

    autoUnarchiveStore.push(event("c:/projecta/", "ProjectA duplicate"));
    await Promise.resolve();
    expect(document.querySelectorAll('[data-ac-testid^="autoUnarchive.row."]')).toHaveLength(2);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(byTestId("autoUnarchive.modal")).toBeTruthy();

    click(byTestId("autoUnarchive.acknowledge"));
    await waitFor(() =>
      expect(document.querySelector('[data-ac-testid="autoUnarchive.modal"]')).toBeNull(),
    );
  });

  it("ignores non-auto-unarchive reasons", async () => {
    autoUnarchiveStore.push(event("C:\\ProjectA", "ProjectA", "unarchive"));
    await Promise.resolve();

    expect(document.querySelector('[data-ac-testid="autoUnarchive.modal"]')).toBeNull();
  });

  it("coalesces ten simultaneous auto-unarchives into one modal with ten rows", async () => {
    for (let index = 0; index < 10; index++) {
      autoUnarchiveStore.push(event(`C:\\Project${index}`, `Project${index}`));
    }

    await waitFor(() =>
      expect(document.querySelectorAll('[data-ac-testid^="autoUnarchive.row."]')).toHaveLength(10),
    );
    expect(document.querySelectorAll('[data-ac-testid="autoUnarchive.modal"]')).toHaveLength(1);
  });
});
