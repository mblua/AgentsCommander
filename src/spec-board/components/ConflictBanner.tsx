import { Component, Show } from "solid-js";
import { specBoardStore, setSpecBoardStore } from "../stores/spec-board";
import { SpecBoardAPI } from "../../shared/ipc";

const ConflictBanner: Component = () => {
  const handleApply = async () => {
    if (!specBoardStore.docId) return;
    const doc = await SpecBoardAPI.applyExternal(specBoardStore.docId);
    if (doc) setSpecBoardStore(doc);
  };

  const handleKeepMine = async () => {
    if (!specBoardStore.docId) return;
    const doc = await SpecBoardAPI.keepMine(specBoardStore.docId);
    if (doc) setSpecBoardStore(doc);
  };

  const handleSaveAs = async () => {
    if (!specBoardStore.docId) return;
    const doc = await SpecBoardAPI.pickSave(specBoardStore.docId, specBoardStore.content, specBoardStore.repoRoot);
    if (doc) setSpecBoardStore(doc);
  };

  return (
    <Show when={specBoardStore.conflict}>
      <div class="spec-board-conflict-banner">
        <span>File changed externally.</span>
        <div class="spec-board-conflict-banner-actions">
          <button onClick={handleApply}>Apply External</button>
          <button onClick={handleKeepMine}>Keep Mine</button>
          <button onClick={handleSaveAs}>Save As</button>
        </div>
      </div>
    </Show>
  );
};

export default ConflictBanner;
