// #2143 D-D - the one-item right-click menu over selected text. The text is
// captured by the caller when the menu opens: pressing the item collapses the
// selection, so reading it at click time would copy the empty string.
import type { Component } from "solid-js";
import ContextMenuSurface from "./ContextMenuSurface";

export interface SelectionCopyMenuProps {
  open: boolean;
  x: number;
  y: number;
  text: string;
  onDismiss: () => void;
}

const SelectionCopyMenu: Component<SelectionCopyMenuProps> = (props) => {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.text);
    } catch (err) {
      console.error("[selection-copy-menu] clipboard write failed:", err);
    }
    props.onDismiss();
  };

  return (
    <ContextMenuSurface
      open={props.open}
      x={props.x}
      y={props.y}
      testId="agentHelpTips.copyMenu"
      onDismiss={props.onDismiss}
    >
      {() => (
        <button
          type="button"
          class="session-context-option"
          data-ac-testid="agentHelpTips.copyMenuItem"
          onClick={() => void copy()}
        >
          Copy
        </button>
      )}
    </ContextMenuSurface>
  );
};

export default SelectionCopyMenu;
