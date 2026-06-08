
import { createStore } from "solid-js/store";
import { SpecBoardConflict, SpecBoardSnapshot } from "../../shared/types";

export interface SpecBoardState {
  docId: string | null;
  repoRoot: string | null;
  path: string | null;
  fileKind: "mermaid" | "markdown";
  content: string;
  diagramSource: string;
  dirty: boolean;
  conflict: SpecBoardConflict | null;
  renderError: string | null;
  snapshots: SpecBoardSnapshot[];
  versionIndex: number;
  versionCount: number;
  previewZoom: number;
  previewOffset: { x: number; y: number };
  lastExternalUpdateAt: number | null;
  showAskAgent: boolean;
}

export const [specBoardStore, setSpecBoardStore] = createStore<SpecBoardState>({
  docId: null,
  repoRoot: null,
  path: null,
  fileKind: "mermaid",
  content: "",
  diagramSource: "",
  dirty: false,
  conflict: null,
  renderError: null,
  snapshots: [],
  versionIndex: 0,
  versionCount: 0,
  previewZoom: 1.0,
  previewOffset: { x: 0, y: 0 },
  lastExternalUpdateAt: null,
  showAskAgent: false,
});
