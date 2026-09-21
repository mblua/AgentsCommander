import { Component, onCleanup } from "solid-js";
import { specBoardStore, setSpecBoardStore } from "../stores/spec-board";
import { SpecBoardAPI } from "../../shared/ipc";

interface SpecBoardEditorProps {
  /** Second guard (the wrapper's `inert` is the first): true until quit-gate
   *  registration succeeds. */
  editingLocked?: boolean;
}

const SpecBoardEditor: Component<SpecBoardEditorProps> = (props) => {
  let debounceTimer: any;

  const handleInput = (e: Event) => {
    const value = (e.currentTarget as HTMLTextAreaElement).value;

    if (debounceTimer) clearTimeout(debounceTimer);
    
    // Sync store immediately so other paths (save/close) see the latest content
    setSpecBoardStore("content", value);
    setSpecBoardStore("dirty", true);

    const currentDocId = specBoardStore.docId;
    
    debounceTimer = setTimeout(async () => {
      if (currentDocId) {
        try {
          const doc = await SpecBoardAPI.updateContent(currentDocId, value);
          if (specBoardStore.docId === currentDocId) {
            if (specBoardStore.content === value) {
              setSpecBoardStore(doc);
            }
          }
        } catch (err) {
          console.error("Failed to update content", err);
        }
      }
    }, 400); // Wait a bit before pushing to backend
  };

  onCleanup(() => {
    if (debounceTimer) clearTimeout(debounceTimer);
  });

  return (
    <div class="spec-board-editor">
      <textarea
        data-ac-testid="specBoard.editor.textarea"
        value={specBoardStore.content}
        onInput={handleInput}
        spellcheck={false}
        disabled={props.editingLocked === true}
      />
    </div>
  );
};

export default SpecBoardEditor;
