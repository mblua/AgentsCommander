// @vitest-environment jsdom
import { describeConfirmModalKeyRouting } from "../../shared/testing/confirm-modal-key-routing";
import QuitConfirmModal from "./QuitConfirmModal";

describeConfirmModalKeyRouting("QuitConfirmModal", ({ onCancel, onConfirm }) => (
  <QuitConfirmModal detachedCount={1} onCancel={onCancel} onQuit={onConfirm} />
));
