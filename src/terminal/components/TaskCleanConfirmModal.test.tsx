// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { describeConfirmModalKeyRouting } from "../../shared/testing/confirm-modal-key-routing";
import TaskCleanConfirmModal from "./TaskCleanConfirmModal";

describeConfirmModalKeyRouting("TaskCleanConfirmModal", ({ onCancel, onConfirm }) => (
  <TaskCleanConfirmModal onCancel={onCancel} onConfirm={onConfirm} />
));

describe("TaskCleanConfirmModal content", () => {
  it("shows the Clean TASK prompt and focuses Cancel on open", () => {
    const root = document.createElement("div");
    document.body.appendChild(root);
    const dispose = render(
      () => <TaskCleanConfirmModal onCancel={() => {}} onConfirm={() => {}} />,
      root,
    );
    try {
      expect(document.body.textContent).toContain("Clean TASK?");
      expect(document.querySelector("#task-clean-body")?.textContent?.trim()).toBe("Back up the task and its history with the same timestamp, then start a new topic?");
      const buttons = document.querySelectorAll("button");
      expect(buttons[0].textContent).toContain("Cancel");
      expect(document.activeElement).toBe(buttons[0]);
    } finally {
      dispose();
    }
  });
});
