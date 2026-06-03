
import { Component, createEffect, onMount, onCleanup, createSignal, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { specBoardStore, setSpecBoardStore } from "./stores/spec-board";
import { SpecBoardAPI, onSpecBoardChanged, onSpecBoardConflict, onSpecBoardFileMissing } from "../shared/ipc";
import type { UnlistenFn } from "../shared/transport";

import SpecBoardTitlebar from "./components/SpecBoardTitlebar";
import SpecBoardToolbar from "./components/SpecBoardToolbar";
import SpecBoardEditor from "./components/SpecBoardEditor";
import MermaidPreview from "./components/MermaidPreview";
import ConflictBanner from "./components/ConflictBanner";
import AskAgentPanel from "./components/AskAgentPanel";
import SaveBeforeCloseModal from "./components/SaveBeforeCloseModal";

import "./styles/spec-board.css";

const SpecBoardApp: Component = () => {
  const [showCloseModal, setShowCloseModal] = createSignal(false);
  let unlistenClose: (() => void) | undefined;
  let unlistens: UnlistenFn[] = [];

  onMount(async () => {
    // Initial fetch/setup here if needed, or rely on command to open window and set state
    
    // Setup listeners
    unlistens.push(await onSpecBoardChanged((payload) => {
      setSpecBoardStore({
        docId: payload.docId,
        path: payload.path,
        content: payload.content,
        diagramSource: payload.diagramSource,
        versionIndex: payload.versionIndex,
        versionCount: payload.versionCount,
        dirty: false,
        conflict: null,
      });
    }));

    unlistens.push(await onSpecBoardConflict((payload) => {
      if (specBoardStore.docId === payload.docId) {
        // Backend emits the full SpecBoardDocument on conflict
        setSpecBoardStore(payload);
      }
    }));

    unlistens.push(await onSpecBoardFileMissing((payload) => {
      if (specBoardStore.docId === payload.docId) {
        // Just mark as dirty so they can save it again
        setSpecBoardStore("dirty", true);
      }
    }));

    const appWindow = getCurrentWindow();
    unlistenClose = await appWindow.onCloseRequested(async (event) => {
      if (specBoardStore.dirty || (!specBoardStore.path && specBoardStore.content.trim().length > 0)) {
        event.preventDefault();
        setShowCloseModal(true);
      } else {
        if (specBoardStore.docId) {
          await SpecBoardAPI.close(specBoardStore.docId);
        }
      }
    });
  });

  onCleanup(() => {
    if (unlistenClose) unlistenClose();
    unlistens.forEach(u => u());
  });

  const handleSaveAndClose = async () => {
    if (specBoardStore.docId) {
      try {
        if (specBoardStore.path) {
          await SpecBoardAPI.save(specBoardStore.docId, specBoardStore.content);
        } else {
          const doc = await SpecBoardAPI.pickSave(specBoardStore.docId, specBoardStore.content, specBoardStore.repoRoot);
          if (!doc) return; // Cancelled
        }
        await SpecBoardAPI.close(specBoardStore.docId);
        if (unlistenClose) unlistenClose();
        getCurrentWindow().close();
      } catch (err) {
        console.error("Save failed", err);
        setSpecBoardStore("renderError", String(err));
      }
    } else {
      if (unlistenClose) unlistenClose();
      getCurrentWindow().close();
    }
  };

  const handleDiscardClose = async () => {
    if (specBoardStore.docId) {
      await SpecBoardAPI.close(specBoardStore.docId);
    }
    if (unlistenClose) unlistenClose();
    getCurrentWindow().close();
  };

  const handleCancelClose = () => {
    setShowCloseModal(false);
  };

  return (
    <div class="spec-board-container">
      <SpecBoardTitlebar />
      <SpecBoardToolbar />
      <ConflictBanner />
      
      <div class="spec-board-main">
        <SpecBoardEditor />
        <MermaidPreview />
        
        <Show when={specBoardStore.showAskAgent}>
          <AskAgentPanel />
        </Show>
        <Show when={specBoardStore.renderError}>
          <div class="spec-board-error">{specBoardStore.renderError}</div>
        </Show>
      </div>
      
      <div class="spec-board-footer">
        <div>{specBoardStore.path || "Unsaved"} ({specBoardStore.fileKind})</div>
        <div>
          {specBoardStore.dirty ? "Dirty " : ""}
          Version {specBoardStore.versionIndex} of {specBoardStore.versionCount}
        </div>
      </div>

      <Show when={showCloseModal()}>
        <SaveBeforeCloseModal
          onSaveAndClose={handleSaveAndClose}
          onDiscard={handleDiscardClose}
          onCancel={handleCancelClose}
        />
      </Show>
    </div>
  );
};

export default SpecBoardApp;
