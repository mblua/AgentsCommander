import { createSignal } from "solid-js";
import type { CodingAgentDefinition } from "../../shared/types";
import { CodingAgentsAPI } from "../../shared/ipc";
import { FALLBACK_CODING_AGENTS } from "../../shared/agent-presets";

/**
 * #769 — the coding-agent catalog consumed by OnboardingModal and SettingsModal.
 * Mirrors `workgroupGroupsStore.ensureLoaded`: the signal is seeded SYNCHRONOUSLY
 * with `FALLBACK_CODING_AGENTS`, so the onboarding/settings agent list is never
 * blank for even one frame (the "never-empty" guarantee comes from the initial
 * value, not a loading branch). `ensureLoaded()` fetches once (in-flight dedup +
 * `loaded` guard); on transport failure it KEEPS the fallback, logs, and marks
 * loaded — it never throws to the component.
 *
 * A valid `Ok([])` from the backend (the user removed every built-in) is honored
 * verbatim — the fallback is NOT resurrected on a successful empty response, only
 * on reject/pending. Onboarding stays actionable because the modal appends its
 * own "Custom Agent" entry client-side.
 */
const [catalog, setCatalog] = createSignal<CodingAgentDefinition[]>(FALLBACK_CODING_AGENTS);
const [loaded, setLoaded] = createSignal(false);
let inFlight: Promise<void> | null = null;

export const codingAgentsStore = {
  /** Reactive catalog. Starts as `FALLBACK_CODING_AGENTS`; replaced once loaded. */
  catalog,
  loaded,

  async ensureLoaded(): Promise<void> {
    if (loaded()) return;
    if (inFlight) return inFlight;

    inFlight = (async () => {
      try {
        const fetched = await CodingAgentsAPI.getCatalog();
        // Honor a valid empty list verbatim (do not fall back to the seed).
        setCatalog(fetched);
      } catch (error) {
        // Transport failure only (the command self-heals malformed files to the
        // embedded default). Keep the synchronously-seeded fallback so the list
        // is never blank; never rethrow into the mounting component.
        console.error("Failed to load coding-agent catalog; using fallback:", error);
      } finally {
        setLoaded(true);
        inFlight = null;
      }
    })();

    return inFlight;
  },

  /** Test-only: restore the pristine pre-fetch state. */
  resetForTests(): void {
    setCatalog(FALLBACK_CODING_AGENTS);
    setLoaded(false);
    inFlight = null;
  },
};
