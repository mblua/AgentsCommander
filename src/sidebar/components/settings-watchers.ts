import { commandExecutableBasename } from "../../shared/profile-utils";
import type {
  AgentConfig,
  WatcherConfig,
  WatcherDedupe,
  WatcherEntry,
  WatcherMode,
} from "../../shared/types";

/**
 * #1171 - the pure half of the Watchers settings tab, kept out of the TSX so it can be
 * tested without rendering, following `settings-save.ts`.
 */

/** Ids are user-written, so they are constrained to what reads well as a chip and a map key. */
export const WATCHER_ID_PATTERN = /^[a-z0-9][a-z0-9-]{0,39}$/;

const WATCHER_MODES: readonly WatcherMode[] = ["state", "occurrence"];
const WATCHER_DEDUPES: readonly WatcherDedupe[] = ["row", "capture", "none"];

/**
 * Whether an entry of the root `watchers` map is one this build understands.
 *
 * The Rust side keeps an unrecognized entry verbatim rather than dropping it, so that a
 * hand-written `"mode": "State"` costs one skipped watcher instead of the whole
 * `AppSettings` parse. The editor has to honour the same contract: what it cannot read, it
 * must not offer to edit, and must not delete on save.
 *
 * Every field required here is written unconditionally by the Rust serializer, so a valid
 * entry that made the round trip always carries all five, whatever the file omitted.
 */
export function isWatcherConfig(entry: WatcherEntry): entry is WatcherConfig {
  if (typeof entry !== "object" || entry === null) return false;
  const candidate = entry as Partial<WatcherConfig>;
  if (typeof candidate.enabled !== "boolean") return false;
  if (!WATCHER_MODES.includes(candidate.mode as WatcherMode)) return false;
  if (typeof candidate.pattern !== "string") return false;
  if (!WATCHER_DEDUPES.includes(candidate.dedupe as WatcherDedupe)) return false;
  if (typeof candidate.dedupeWindowMs !== "number") return false;
  if (
    candidate.commands !== undefined &&
    candidate.commands !== null &&
    !Array.isArray(candidate.commands)
  ) {
    return false;
  }
  return (
    candidate.capturedAgainst === undefined ||
    candidate.capturedAgainst === null ||
    typeof candidate.capturedAgainst === "string"
  );
}

/** A watcher id is invalid, taken, or fine. Returns the message to show, or null. */
export function validateWatcherId(id: string, takenIds: readonly string[]): string | null {
  if (!id) return "An id is required.";
  if (!WATCHER_ID_PATTERN.test(id)) {
    return "Lowercase letters, digits and dashes, starting with a letter or digit, up to 40 characters.";
  }
  if (takenIds.includes(id)) return `"${id}" already exists.`;
  return null;
}

/** The shape a brand-new row starts from. Mirrors the Rust serde defaults. */
export function newWatcherConfig(): WatcherConfig {
  return {
    enabled: true,
    mode: "occurrence",
    pattern: "",
    dedupe: "row",
    dedupeWindowMs: 2000,
  };
}

/** An id no existing watcher holds, so "Add watcher" never collides. */
export function nextWatcherId(takenIds: readonly string[]): string {
  for (let n = 1; ; n += 1) {
    const candidate = `watcher-${n}`;
    if (!takenIds.includes(candidate)) return candidate;
  }
}

/**
 * Which of the two selector states a config is in.
 *
 * Absent or null is "every configured agent"; a present list is "only these", and an empty
 * present list is the valid, deliberate "nobody". A plain multiselect cannot express the
 * first and the last as different things, which is why the UI carries this as a mode.
 */
export function selectorMode(config: WatcherConfig): "all" | "selected" {
  return config.commands === undefined || config.commands === null ? "all" : "selected";
}

/** Switch selector state without losing the list the user already picked. */
export function withSelectorMode(
  config: WatcherConfig,
  mode: "all" | "selected"
): WatcherConfig {
  if (mode === "all") return { ...config, commands: null };
  return { ...config, commands: config.commands ?? [] };
}

/** Add or drop one stem from the `Selected` list, leaving `All agents` alone. */
export function toggleCommandStem(config: WatcherConfig, stem: string): WatcherConfig {
  const current = config.commands ?? [];
  const next = current.includes(stem)
    ? current.filter((entry) => entry !== stem)
    : [...current, stem];
  return { ...config, commands: next };
}

/**
 * The distinct executable stems of the configured agents, for the `Selected` options.
 *
 * This populates a picker; it does **not** decide reach. Reach and budget come from
 * `preview_watcher_reach`, because the one stem rule lives in Rust and the frontend's own
 * `starts_with` rule in `suggestedContextRegex` must not be ported: the catalog rejects
 * prefix matching in writing, `pi` and `agent` being the false-match risk.
 */
export function distinctCommandStems(agents: readonly AgentConfig[]): string[] {
  const stems = new Set<string>();
  for (const agent of agents) {
    const stem = commandExecutableBasename(agent.command);
    if (stem) stems.add(stem);
  }
  return [...stems].sort();
}

/**
 * Rename a key of the watcher map, preserving every other entry including the ones this
 * build could not read.
 *
 * This really is delete plus create: the id is the map key and the same grouping key the
 * activity window and the history counters use, so activations already recorded under the
 * old id keep it. The UI says so next to the control.
 */
export function renameWatcherEntry(
  watchers: Readonly<Record<string, WatcherEntry>>,
  fromId: string,
  toId: string
): Record<string, WatcherEntry> {
  const renamed: Record<string, WatcherEntry> = {};
  for (const [id, entry] of Object.entries(watchers)) {
    if (id === fromId) renamed[toId] = entry;
    else renamed[id] = entry;
  }
  return renamed;
}

/** Map entries in key order, which is the order the Rust `BTreeMap` resolves the budget in. */
export function sortedWatcherIds(
  watchers: Readonly<Record<string, WatcherEntry>> | undefined
): string[] {
  return Object.keys(watchers ?? {}).sort();
}
