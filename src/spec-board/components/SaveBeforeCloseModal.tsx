
import { Component } from "solid-js";

interface Props {
  onSaveAndClose: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}

const SaveBeforeCloseModal: Component<Props> = (props) => {
  return (
    <div class="spec-board-modal-overlay">
      <div class="spec-board-modal">
        <div>You have unsaved changes. Do you want to save before closing?</div>
        <div class="spec-board-modal-buttons">
          <button data-ac-testid="specBoard.saveBeforeClose.save" onClick={props.onSaveAndClose}>Save</button>
          <button data-ac-testid="specBoard.saveBeforeClose.discard" onClick={props.onDiscard}>Discard</button>
          <button data-ac-testid="specBoard.saveBeforeClose.cancel" onClick={props.onCancel}>Cancel</button>
        </div>
      </div>
    </div>
  );
};

export default SaveBeforeCloseModal;
