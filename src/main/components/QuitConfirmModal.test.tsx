// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { describeConfirmModalKeyRouting } from "../../shared/testing/confirm-modal-key-routing";
import QuitConfirmModal from "./QuitConfirmModal";

describeConfirmModalKeyRouting("QuitConfirmModal", ({ onCancel, onConfirm }) => (
  <QuitConfirmModal detachedCount={1} onCancel={onCancel} onQuit={onConfirm} />
));

describe("QuitConfirmModal detached content", () => {
  it("shows the detached count and focuses Cancel on open", () => {
    const root = document.createElement("div");
    document.body.appendChild(root);
    const dispose = render(
      () => <QuitConfirmModal detachedCount={2} onCancel={() => {}} onQuit={() => {}} />,
      root,
    );
    try {
      expect(document.body.textContent).toContain("You have 2 detached sessions open.");
      expect(document.activeElement).toBe(
        document.querySelector('[data-ac-testid="quit.cancel"]'),
      );
    } finally {
      dispose();
    }
  });
});
