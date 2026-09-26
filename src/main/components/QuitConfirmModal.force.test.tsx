// @vitest-environment jsdom
import { describeConfirmModalKeyRouting } from "../../shared/testing/confirm-modal-key-routing";
import QuitConfirmModal from "./QuitConfirmModal";

// Force mode routes the same keys through its own button/callback pair.
describeConfirmModalKeyRouting("QuitConfirmModal force mode", ({ onCancel, onConfirm }) => (
  <QuitConfirmModal mode="force" onKeepWaiting={onCancel} onForceQuit={onConfirm} />
));
