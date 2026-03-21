import { createStore } from "solid-js/store";
import type { BridgeInfo } from "../../shared/types";

interface BridgesState {
  bridges: BridgeInfo[];
}

const [state, setState] = createStore<BridgesState>({
  bridges: [],
});

export const bridgesStore = {
  get bridges() {
    return state.bridges;
  },

  setBridges(bridges: BridgeInfo[]) {
    setState("bridges", bridges);
  },

  addBridge(bridge: BridgeInfo) {
    setState("bridges", (prev) => [...prev, bridge]);
  },

  removeBridge(sessionId: string) {
    setState("bridges", (prev) =>
      prev.filter((b) => b.sessionId !== sessionId)
    );
  },

  getBridge(sessionId: string): BridgeInfo | undefined {
    return state.bridges.find((b) => b.sessionId === sessionId);
  },
};
