// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { describeConfirmModalKeyRouting } from "../../shared/testing/confirm-modal-key-routing";
import QuitConfirmModal from "./QuitConfirmModal";

// Force mode routes the same keys through its own button/callback pair.
describeConfirmModalKeyRouting("QuitConfirmModal force mode", ({ onCancel, onConfirm }) => (
  <QuitConfirmModal mode="force" onKeepWaiting={onCancel} onForceQuit={onConfirm} />
));

describe("QuitConfirmModal force content", () => {
  it("shows the force warning and focuses Keep waiting on open", () => {
    const root = document.createElement("div");
    document.body.appendChild(root);
    const dispose = render(
      () => <QuitConfirmModal mode="force" onKeepWaiting={() => {}} onForceQuit={() => {}} />,
      root,
    );
    try {
      expect(document.body.textContent).toContain("Force quit?");
      expect(document.activeElement).toBe(
        document.querySelector('[data-ac-testid="quit.keepWaiting"]'),
      );
    } finally {
      dispose();
    }
  });
});
