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
    let cancelled = 0;
    let confirmed = 0;
    const dispose = render(
      () => <TaskCleanConfirmModal onCancel={() => { cancelled++; }} onConfirm={() => { confirmed++; }} />,
      root,
    );
    try {
      expect(document.body.textContent).toContain("Clean TASK?");
      expect(document.querySelector("#task-clean-body")?.textContent?.trim()).toBe("Back up the task and its history with the same timestamp, then start a new topic?");
      for (const [id, role] of [["root", "dialog"], ["title", "surface"], ["body", "surface"], ["cancel", "button"], ["confirm", "button"]]) {
        const nodes = document.querySelectorAll(`[data-ac-testid="taskCleanConfirm.${id}"]`);
        expect(nodes).toHaveLength(1); expect(nodes[0].getAttribute("data-ac-role")).toBe(role);
      }
      const buttons = ["cancel", "confirm"].map(id => document.querySelector<HTMLButtonElement>(`[data-ac-testid="taskCleanConfirm.${id}"]`)!);
      expect(buttons[0].textContent).toContain("Cancel");
      expect(document.activeElement).toBe(buttons[0]);
      buttons[0].click(); expect(cancelled).toBe(1); expect(confirmed).toBe(0);
      buttons[1].click(); expect(confirmed).toBe(1); expect(cancelled).toBe(1);
    } finally {
      dispose();
      expect(document.querySelectorAll('[data-ac-testid^="taskCleanConfirm."]')).toHaveLength(0);
      root.remove();
    }
  });
});
