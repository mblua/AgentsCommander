import { createSignal } from "solid-js";
import type { CodingAgentDefinition } from "../../shared/types";
import { CodingAgentsAPI } from "../../shared/ipc";
import { FALLBACK_CODING_AGENTS } from "../../shared/agent-presets";

const [catalog, setCatalog] = createSignal<CodingAgentDefinition[]>(FALLBACK_CODING_AGENTS);
const [reseedableCommands, setReseedableCommands] = createSignal<string[]>([]);
const [loaded, setLoaded] = createSignal(false);
let inFlight: Promise<void> | null = null;

export const codingAgentsStore = {
  catalog,
  loaded,
  reseedableCommands,

  async ensureLoaded(): Promise<void> {
    if (loaded()) return;
    if (inFlight) return inFlight;

    inFlight = (async () => {
      try {
        const [catalogRes, reseedableRes] = await Promise.allSettled([
          Promise.resolve().then(() => CodingAgentsAPI.getCatalog()),
          Promise.resolve().then(() => CodingAgentsAPI.listReseedableCommands()),
        ]);

        if (catalogRes.status === "fulfilled") {
          setCatalog(catalogRes.value);
        } else {
          console.error("Failed to load coding-agent catalog; using fallback:", catalogRes.reason);
        }

        if (reseedableRes.status === "fulfilled") {
          setReseedableCommands(reseedableRes.value);
        } else {
          console.error("Failed to load reseedable commands; hiding re-seed buttons:", reseedableRes.reason);
        }
      } catch (error) {
        console.error("Unexpected error loading coding-agent catalog:", error);
      } finally {
        setLoaded(true);
        inFlight = null;
      }
    })();

    return inFlight;
  },

  resetForTests(): void {
    setCatalog(FALLBACK_CODING_AGENTS);
    setReseedableCommands([]);
    setLoaded(false);
    inFlight = null;
  },
};
