import { Component } from "solid-js";
import { SpecBoardAPI } from "../../shared/ipc";
import { specBoardStore, setSpecBoardStore } from "../stores/spec-board";

const SpecBoardToolbar: Component = () => {
  const handleNew = async () => {
    try {
      const oldDocId = specBoardStore.docId;
      const doc = await SpecBoardAPI.new(specBoardStore.repoRoot);
      if (doc) {
        if (oldDocId && oldDocId !== doc.docId) await SpecBoardAPI.close(oldDocId);
        setSpecBoardStore(doc);
      }
    } catch (e) {
      console.error(e);
    }
  };

  const handleOpen = async () => {
    try {
      const oldDocId = specBoardStore.docId;
      const doc = await SpecBoardAPI.pickOpen(specBoardStore.repoRoot);
      if (doc) {
        if (oldDocId && oldDocId !== doc.docId) await SpecBoardAPI.close(oldDocId);
        setSpecBoardStore(doc);
      }
    } catch (e) {
      console.error(e);
    }
  };

  const handleSave = async () => {
    if (!specBoardStore.docId) return;
    try {
      if (specBoardStore.path) {
        const doc = await SpecBoardAPI.save(specBoardStore.docId, specBoardStore.content);
        if (doc) setSpecBoardStore(doc);
      } else {
        const doc = await SpecBoardAPI.pickSave(specBoardStore.docId, specBoardStore.content, specBoardStore.repoRoot);
        if (doc) setSpecBoardStore(doc);
      }
    } catch (e) {
      console.error(e);
    }
  };

  const handleSaveAs = async () => {
    if (!specBoardStore.docId) return;
    try {
      const doc = await SpecBoardAPI.pickSave(specBoardStore.docId, specBoardStore.content, specBoardStore.repoRoot);
      if (doc) setSpecBoardStore(doc);
    } catch (e) {
      console.error(e);
    }
  };

  const handleZoomOut = () => setSpecBoardStore("previewZoom", z => Math.max(0.1, z - 0.1));
  const handleZoomIn = () => setSpecBoardStore("previewZoom", z => z + 0.1);
  const handleZoomReset = () => {
    setSpecBoardStore("previewZoom", 1);
    setSpecBoardStore("previewOffset", { x: 0, y: 0 });
  };

  const toggleAskAgent = () => {
    setSpecBoardStore("showAskAgent", !specBoardStore.showAskAgent);
  };

  const canAskAgent = () => specBoardStore.path !== null;

  return (
    <div class="spec-board-toolbar">
      <button onClick={handleNew}>New</button>
      <button onClick={handleOpen}>Open</button>
      <button onClick={handleSave} disabled={!specBoardStore.dirty && !!specBoardStore.path}>Save</button>
      <button onClick={handleSaveAs}>Save As</button>
      <button onClick={toggleAskAgent} disabled={!canAskAgent()}>Ask Agent</button>

      <div style={{ flex: 1 }} />

      <button onClick={handleZoomOut}>-</button>
      <button onClick={handleZoomReset}>Reset Zoom</button>
      <button onClick={handleZoomIn}>+</button>
    </div>
  );
};

export default SpecBoardToolbar;
