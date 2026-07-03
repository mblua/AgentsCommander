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
// #769 Phase 2 — command basenames that ship a re-seedable default config master.
// Init EMPTY and stay empty on any fetch failure: the re-seed button is a
// destructive action, so we never offer it for a set we could not validate (no
// hardcoded fallback here, unlike the catalog).
const [reseedableCommands, setReseedableCommands] = createSignal<string[]>([]);
const [loaded, setLoaded] = createSignal(false);
let inFlight: Promise<void> | null = null;

export const codingAgentsStore = {
  /** Reactive catalog. Starts as `FALLBACK_CODING_AGENTS`; replaced once loaded. */
  catalog,
  loaded,
  /** Reactive re-seedable command-basename set. Empty until loaded, empty on failure. */
  reseedableCommands,

  async ensureLoaded(): Promise<void> {
    if (loaded()) return;
    if (inFlight) return inFlight;

    inFlight = (async () => {
      try {
        // Independent failure domains: a catalog failure keeps the fallback list,
        // a reseedable failure leaves the set empty (no buttons). Neither blocks
        // the other. `allSettled` never rejects; the outer try also contains any
        // synchronous throw so ensureLoaded can never reject into its caller.
        const [catalogRes, reseedableRes] = await Promise.allSettled([
          Promise.resolve().then(() => CodingAgentsAPI.getCatalog()),
          Promise.resolve().then(() => CodingAgentsAPI.listReseedableCommands()),
        ]);

        if (catalogRes.status === "fulfilled") {
          // Honor a valid empty list verbatim (do not fall back to the seed).
          setCatalog(catalogRes.value);
        } else {
          // Keep the synchronously-seeded fallback so the list is never blank.
          console.error("Failed to load coding-agent catalog; using fallback:", catalogRes.reason);
        }

        if (reseedableRes.status === "fulfilled") {
          setReseedableCommands(reseedableRes.value);
        } else {
          // Leave the set empty → no re-seed buttons (failure-safe).
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

  /** Test-only: restore the pristine pre-fetch state. */
  resetForTests(): void {
    setCatalog(FALLBACK_CODING_AGENTS);
    setReseedableCommands([]);
    setLoaded(false);
    inFlight = null;
  },
};
