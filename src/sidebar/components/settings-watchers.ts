import { commandExecutableBasename } from "../../shared/profile-utils";
import type {
  AgentConfig,
  WatcherAgentDraftEntry,
  WatcherConfig,
  WatcherDedupe,
  WatcherDraftEntry,
  WatcherEntry,
  WatcherMode,
  WatcherReachEntry,
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
 * must not offer to edit, must not delete on save, and must not send to
 * `preview_watcher_reach` -- a row this predicate calls valid and `serde` does not would be
 * counted against an agent's budget, could push a real watcher out of it, and would then be
 * skipped by the engine after Save.
 *
 * **The predicate mirrors what the serializer emits and rejects everything the decoder would
 * reject.** That is one-directional on purpose and it is not "the exact mirror of the
 * decoder": `serde` accepts `enabled`, `dedupe` and `dedupeWindowMs` absent through
 * `#[serde(default)]`, while this requires them present. Being stricter is the safe
 * direction -- the worst it does is leave a row out of the request, which under-reports the
 * budget -- and it is unreachable in practice, because the frontend never sees the file's
 * bytes but what Rust re-serialized, and those three fields carry no `skip_serializing_if`.
 */
export function isWatcherConfig(entry: unknown): entry is WatcherConfig {
  // `unknown` and not `WatcherEntry`: the value really is a `serde_json::Value`, so it can be
  // a string, a number, `null` or an array as easily as an object, and a decoder that only
  // accepts what it already believes is not a decoder.
  if (typeof entry !== "object" || entry === null || Array.isArray(entry)) return false;
  const candidate = entry as Partial<WatcherConfig>;
  if (typeof candidate.enabled !== "boolean") return false;
  if (!WATCHER_MODES.includes(candidate.mode as WatcherMode)) return false;
  if (typeof candidate.pattern !== "string") return false;
  if (!WATCHER_DEDUPES.includes(candidate.dedupe as WatcherDedupe)) return false;
  if (!isDedupeWindowMs(candidate.dedupeWindowMs)) return false;
  if (!isCommandsSelector(candidate.commands)) return false;
  return (
    candidate.capturedAgainst === undefined ||
    candidate.capturedAgainst === null ||
    typeof candidate.capturedAgainst === "string"
  );
}

/**
 * `Vec<String>`, and not merely "an array".
 *
 * `commands: [1]` is an array, so a bare `Array.isArray` admits it while `serde` classifies
 * the whole entry as `Invalid`. Absent and `null` both mean "every configured agent"; `[]`
 * means "nobody". All three are legal and they are three different things.
 */
function isCommandsSelector(value: unknown): boolean {
  if (value === undefined || value === null) return true;
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

/**
 * A `u64` this editor can actually hold.
 *
 * `typeof n === "number" && n >= 0` is the naive correction and it is not sufficient: it
 * still admits `1.5` and `1e30`, both of which `serde` rejects for a `u64`, so both would
 * travel and consume a budget slot. Safe-integer is deliberately NARROWER than `u64` as
 * well: JavaScript cannot represent every `u64` exactly, so a hand-written value above 2^53
 * is classified unrecognised rather than silently rounded. Such a row still runs in the
 * engine, clamped, and the editor lists it instead of offering to edit a number it cannot
 * hold.
 */
function isDedupeWindowMs(value: unknown): boolean {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
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

/**
 * The shape a brand-new row starts from.
 *
 * **Born disabled**, which is the one place this deviates from the Rust serde defaults, and
 * deliberately. An empty pattern is a valid regex that matches every row, so a row born
 * enabled turns Add plus an accidental Save into a watcher that matches everything on every
 * agent: it fills the per-tick caps, turns the ring over, goes degraded and can displace a
 * useful watcher out of an agent's budget, all without the user having written a pattern or
 * looked at the preview. The editor also refuses to enable a row whose pattern is empty
 * (`canEnableWatcher`).
 *
 * The serde default for a HAND-WRITTEN file stays `true`: an omitted `enabled` in a file
 * someone wrote deliberately means on. Only the editor's new-row shape changes.
 */
export function newWatcherConfig(): WatcherConfig {
  return {
    enabled: false,
    mode: "occurrence",
    pattern: "",
    dedupe: "row",
    dedupeWindowMs: 2000,
  };
}

/**
 * Whether the editor will let this row be enabled.
 *
 * The Rust compiler is NOT changed to reject an empty pattern: it is a legal regex, a
 * hand-written one is bounded by the caps and the suspension rule, and the editor is where
 * the user is.
 */
export function canEnableWatcher(config: WatcherConfig): boolean {
  return config.pattern !== "";
}

/**
 * The editor's one invariant, applied to every write: an enabled watcher has a pattern.
 *
 * Gating the checkbox alone is not enough, because `enabled` is not the only field that can
 * break the pair. Type a pattern, enable the row, then delete the pattern, and the row is
 * left `enabled: true` with `pattern: ""` -- the global regex that matches every row on every
 * agent, which is the flood the whole rule exists to prevent. Save does not validate
 * watchers, so it would be persisted.
 *
 * Auto-disabling rather than blocking the edit is deliberate: clearing a pattern to rewrite
 * it is ordinary work, and refusing the keystroke would fight the user over a field they are
 * in the middle of. The row says why it turned off, in the sentence
 * `watcherReachSummary` gives it.
 */
export function withWatcherInvariant(config: WatcherConfig): WatcherConfig {
  if (config.enabled && !canEnableWatcher(config)) return { ...config, enabled: false };
  return config;
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
 *
 * **And because it is a create, the row it produces goes through `withWatcherInvariant` like
 * every other row the editor creates.** Without that, Rename is a way out of a state that
 * editing has no way out of: a hand-written `{ enabled: true, pattern: "" }` is deliberately
 * left as written until the editor touches it, but the id is exactly what the 8-per-agent
 * budget resolves in, so a `zzz` sitting outside the first eight, renamed to `aaa`, walks
 * into budget, displaces a useful watcher and runs the global regex -- with nobody having
 * gone near the pattern.
 *
 * An entry this build could not read still moves byte for byte: there is no config to hold
 * an invariant over, and preserving what it cannot read is the contract.
 */
export function renameWatcherEntry(
  watchers: Readonly<Record<string, WatcherEntry>>,
  fromId: string,
  toId: string
): Record<string, WatcherEntry> {
  const renamed: Record<string, WatcherEntry> = {};
  for (const [id, entry] of Object.entries(watchers)) {
    if (id !== fromId) {
      renamed[id] = entry;
      continue;
    }
    renamed[toId] = isWatcherConfig(entry) ? withWatcherInvariant(entry) : entry;
  }
  return renamed;
}

/** Map entries in key order, which is the order the Rust `BTreeMap` resolves the budget in. */
export function sortedWatcherIds(
  watchers: Readonly<Record<string, WatcherEntry>> | undefined
): string[] {
  return Object.keys(watchers ?? {}).sort();
}

/**
 * What a row's reach reads as, which depends on whether the row is enabled.
 *
 * The two fields answer different questions and are therefore worded differently. `entries`
 * is what the row's SELECTOR reaches, whatever its `enabled`; `allocated` is whether the row
 * holds a slot on that agent after Save.
 *
 * A disabled row gets "would reach ... when enabled" rather than the present tense, because
 * "reaches" on a watcher that is doing nothing is a false statement, and it is the
 * over-reporting direction this feature refuses everywhere else. A disabled row with an empty
 * pattern gets a second sentence naming the missing condition first: it is the state every
 * user sees the instant they press Add Watcher, and "when enabled" alone offers a condition
 * the editor refuses to let them meet. The reach is kept after it rather than hidden, because
 * configuring the selector before the pattern is a legitimate order of work and it is the
 * reason a disabled row's reach is reported at all.
 */
export function watcherReachSummary(
  config: WatcherConfig,
  entries: readonly WatcherReachEntry[]
): string {
  const count = `${entries.length} agent${entries.length === 1 ? "" : "s"}`;
  if (config.enabled) return `Reaches ${count}.`;
  const would = `Would reach ${count} when enabled.`;
  return canEnableWatcher(config) ? would : `${would} Add a pattern to enable it.`;
}

/**
 * The budget badge, which only an ENABLED row can carry.
 *
 * An enabled row that reaches an agent and holds no slot can only be out of budget, so the
 * badge names the one real reason. A disabled row holds no slot BECAUSE it is disabled, and
 * presenting that as a budget outcome would name the wrong cause, so it gets no badge at all.
 *
 * `allocated` is slot assignment and not a promise of output: a resolved watcher whose regex
 * does not compile is allocated a slot and is inert. That dimension is answered on the same
 * row by the pattern preview and is deliberately not restated here.
 */
export function watcherBudgetNotice(
  config: WatcherConfig,
  entries: readonly WatcherReachEntry[]
): string {
  if (!config.enabled) return "";
  const displaced = entries.filter((entry) => !entry.allocated);
  if (displaced.length === 0) return "";
  const names = displaced.map((entry) => entry.agentLabel || entry.agentId).join(", ");
  return ` Not running on ${names} (budget).`;
}

/** Both halves of one `preview_watcher_reach` call. */
export interface WatcherReachRequest {
  watchers: WatcherDraftEntry[];
  agents: WatcherAgentDraftEntry[];
}

/**
 * The exact request the Watchers section would send for this draft.
 *
 * Both halves come from the draft and nothing comes from disk. Whether a watcher holds one of
 * an agent's 8 slots is a property of the whole set, and the modal edits agents and watchers
 * in one store that one Save writes together, so an answer resolved against either saved half
 * would describe a state the user has already left.
 *
 * Unrecognised entries are left out: resolution skips them before any budget is counted, so
 * they consume no slot and sending them would only produce notices. They are still preserved
 * verbatim for the save.
 *
 * `undefined` and `null` `commands` both mean "every configured agent" and are normalized to
 * `null`, so two drafts that differ only in which of the two they hold are one request and
 * not two.
 */
export function watcherReachRequest(
  watchers: Readonly<Record<string, WatcherEntry>> | undefined,
  agents: readonly AgentConfig[]
): WatcherReachRequest {
  const map = watchers ?? {};
  const rows: WatcherDraftEntry[] = [];
  for (const id of sortedWatcherIds(map)) {
    const entry = map[id];
    if (!isWatcherConfig(entry)) continue;
    rows.push({ id, enabled: entry.enabled, commands: entry.commands ?? null });
  }
  return {
    watchers: rows,
    agents: agents.map((agent) => ({
      id: agent.id,
      label: agent.label,
      command: agent.command,
    })),
  };
}

/**
 * The identity of a request, which is what the displayed reach answer belongs to.
 *
 * Keying the guard on the REQUEST rather than on "any change to the draft" is what makes the
 * rule consistent with itself: a `pattern` keystroke changes the draft but not the request,
 * so under a clear-on-any-change rule the answer would be cleared and no call would ever
 * replace it, leaving the row pending forever. Under this key a `pattern` keystroke changes
 * nothing at all.
 */
export function reachRequestFingerprint(request: WatcherReachRequest): string {
  return JSON.stringify(request);
}
