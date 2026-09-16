// @vitest-environment jsdom
import { describeConfirmModalKeyRouting } from "../../shared/testing/confirm-modal-key-routing";
import TaskCleanConfirmModal from "./TaskCleanConfirmModal";

describeConfirmModalKeyRouting("TaskCleanConfirmModal", ({ onCancel, onConfirm }) => (
  <TaskCleanConfirmModal onCancel={onCancel} onConfirm={onConfirm} />
));
