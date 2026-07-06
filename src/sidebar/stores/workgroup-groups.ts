import { createStore } from "solid-js/store";
import type { AcWorkgroup, NonStopGroupConfig, WorkgroupGroup, WorkgroupGroupsConfig } from "../../shared/types";
import { ProjectAPI } from "../../shared/ipc";
import { normalizeProjectPathForCompare } from "./project-refresh";

export const MAX_WORKGROUP_GROUPS = 80;
export const MAX_GROUP_ID_LENGTH = 128;
export const MAX_GROUP_NAME_LENGTH = 80;
export const MAX_GROUP_REGEX_LENGTH = 1024;
export const MAX_GROUP_MATCH_ID_LENGTH = 160;

export type WorkgroupGroupSelection =
  | { kind: "all" }
  | { kind: "ungrouped" }
  | { kind: "nonstop" }
  | { kind: "group"; id: string };

// #777 Non-stop group defaults + clamp bounds (mirror the Rust side in
// project_settings.rs so both ends agree).
export const DEFAULT_NON_STOP_NAME = "Alert me!";
const LEGACY_NON_STOP_NAME = "Non-stop";
export const MATCH_NONE_REGEX = "(?!)";
const MIN_TOLERANCE_SECONDS = 1;
const MAX_TOLERANCE_SECONDS = 3600;
const DEFAULT_TOLERANCE_SECONDS = 30;
const MIN_BEEP_SECONDS = 1;
const MAX_BEEP_SECONDS = 60;
const DEFAULT_BEEP_SECONDS = 3;

interface ProjectGroupsEntry {
  config: WorkgroupGroupsConfig;
  selection: WorkgroupGroupSelection;
  loaded: boolean;
  loading: boolean;
  saving: boolean;
  error: string | null;
}

const [entries, setEntries] = createStore<Record<string, ProjectGroupsEntry | undefined>>({});
const inFlightLoads = new Map<string, Promise<void>>();
const saveVersions = new Map<string, number>();

function charLength(value: string): number {
  return Array.from(value).length;
}

function normalizeNonStopName(name: string | null | undefined): string {
  const trimmedName = (name ?? "").trim();
  if (!trimmedName || trimmedName === LEGACY_NON_STOP_NAME) return DEFAULT_NON_STOP_NAME;
  return Array.from(trimmedName).slice(0, MAX_GROUP_NAME_LENGTH).join("");
}

export function nonStopDisplayName(name: string | null | undefined): string {
  return normalizeNonStopName(name);
}

function keyFor(projectPath: string): string {
  return normalizeProjectPathForCompare(projectPath);
}

export function defaultGroupsConfig(): WorkgroupGroupsConfig {
  return {
    groups: [],
    showAll: true,
    showUngrouped: true,
    nonStop: null,
  };
}

/** #777 default Non-stop slot (D3): hidden, matches nothing, 30s tolerance, 3s beep. */
export function defaultNonStop(): NonStopGroupConfig {
  return {
    show: false,
    name: DEFAULT_NON_STOP_NAME,
    regex: MATCH_NONE_REGEX,
    toleranceSeconds: DEFAULT_TOLERANCE_SECONDS,
    telegram: { enabled: false, botId: null },
    sound: { enabled: false, seconds: DEFAULT_BEEP_SECONDS },
  };
}

function clampInt(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  const n = Math.round(value);
  if (n < min) return min;
  if (n > max) return max;
  return n;
}

/** #777 C6: repair a persisted/hand-edited `nonStop` so a bad value degrades
 *  gracefully instead of entering the fatal validate-and-reset path (which would
 *  wipe every user group). Clamps numerics, truncates the name, and resets an
 *  invalid/oversize regex to match-none. Never throws. `null`/`undefined` stay absent. */
export function normalizeNonStop(
  nonStop: NonStopGroupConfig | null | undefined
): NonStopGroupConfig | null | undefined {
  if (nonStop === null) return null;
  if (nonStop === undefined) return undefined;
  const name = normalizeNonStopName(nonStop.name);
  const regex = nonStop.regex ?? "";
  let regexOk = charLength(regex) <= MAX_GROUP_REGEX_LENGTH;
  if (regexOk) {
    try {
      new RegExp(regex);
    } catch {
      regexOk = false;
    }
  }
  return {
    show: !!nonStop.show,
    name,
    regex: regexOk ? regex : MATCH_NONE_REGEX,
    toleranceSeconds: clampInt(
      nonStop.toleranceSeconds,
      MIN_TOLERANCE_SECONDS,
      MAX_TOLERANCE_SECONDS,
      DEFAULT_TOLERANCE_SECONDS
    ),
    telegram: {
      enabled: !!nonStop.telegram?.enabled,
      botId: nonStop.telegram?.botId ?? null,
    },
    sound: {
      enabled: !!nonStop.sound?.enabled,
      seconds: clampInt(nonStop.sound?.seconds, MIN_BEEP_SECONDS, MAX_BEEP_SECONDS, DEFAULT_BEEP_SECONDS),
    },
  };
}

function cloneConfig(config: WorkgroupGroupsConfig): WorkgroupGroupsConfig {
  return {
    groups: config.groups.map((group) => ({ ...group })),
    showAll: config.showAll,
    showUngrouped: config.showUngrouped,
    nonStop: config.nonStop
      ? { ...config.nonStop, telegram: { ...config.nonStop.telegram }, sound: { ...config.nonStop.sound } }
      : (config.nonStop ?? undefined),
  };
}

export function reorderGroups(
  groups: WorkgroupGroup[],
  groupId: string,
  targetIndex: number
): WorkgroupGroup[] {
  const sourceIndex = groups.findIndex((group) => group.id === groupId);
  if (sourceIndex < 0) return groups;

  const next = groups.slice();
  const [moved] = next.splice(sourceIndex, 1);
  const safeTarget = Number.isFinite(targetIndex) ? Math.round(targetIndex) : sourceIndex;
  const clampedTarget = Math.max(0, Math.min(safeTarget, next.length));
  next.splice(clampedTarget, 0, moved);
  return next;
}

function defaultEntry(): ProjectGroupsEntry {
  return {
    config: defaultGroupsConfig(),
    selection: { kind: "all" },
    loaded: false,
    loading: false,
    saving: false,
    error: null,
  };
}

function entryFor(projectPath: string): ProjectGroupsEntry {
  return entries[keyFor(projectPath)] ?? defaultEntry();
}

function formatError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function normalizeSelection(
  selection: WorkgroupGroupSelection,
  config: WorkgroupGroupsConfig
): WorkgroupGroupSelection {
  if (selection.kind === "group") {
    if (config.groups.some((group) => group.id === selection.id)) return selection;
    return config.showAll ? { kind: "all" } : { kind: "ungrouped" };
  }
  if (selection.kind === "all" && !config.showAll) {
    return config.showUngrouped ? { kind: "ungrouped" } : { kind: "all" };
  }
  if (selection.kind === "ungrouped" && !config.showUngrouped) {
    return config.showAll ? { kind: "all" } : { kind: "ungrouped" };
  }
  // #777: selecting Non-stop while it is hidden/absent falls back like All/Ungrouped.
  if (selection.kind === "nonstop" && !config.nonStop?.show) {
    return config.showAll ? { kind: "all" } : { kind: "ungrouped" };
  }
  return selection;
}

function ensureEntry(projectPath: string): ProjectGroupsEntry {
  const key = keyFor(projectPath);
  const current = entries[key];
  if (current) return current;
  const next = defaultEntry();
  setEntries(key, next);
  return next;
}

export function groupMatchId(wg: AcWorkgroup): string {
  return wg.name;
}

export function escapeRegexLiteral(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function exactGroupRegexForWorkgroup(wgName: string): string {
  return `^(${escapeRegexLiteral(wgName)})$`;
}

export function appendExactGroupToken(regex: string, wgName: string): string | null {
  const pattern = regex.trim();
  const token = escapeRegexLiteral(wgName);
  if (!pattern || pattern === "(?!)") return `^(${token})$`;
  try {
    if (new RegExp(pattern).test(wgName)) return pattern;
  } catch {
    return null;
  }
  const generated = pattern.match(/^\^\((.*)\)\$$/);
  if (generated) return `^(${generated[1]}|${token})$`;
  return `(?:${pattern})|^(${token})$`;
}

function splitEscapedAlternatives(body: string): string[] {
  const alternatives: string[] = [];
  let current = "";
  let escaped = false;
  for (const char of body) {
    if (escaped) {
      current += char;
      escaped = false;
      continue;
    }
    if (char === "\\") {
      current += char;
      escaped = true;
      continue;
    }
    if (char === "|") {
      alternatives.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  alternatives.push(current);
  return alternatives;
}

function splitGeneratedAppend(pattern: string): { inner: string; token: string } | null {
  if (!pattern.startsWith("(?:") || !pattern.endsWith(")$")) return null;
  const markerIndex = pattern.lastIndexOf(")|^(");
  if (markerIndex < 3) return null;
  return {
    inner: pattern.slice(3, markerIndex),
    token: pattern.slice(markerIndex + 4, -2),
  };
}

function removeExactGroupTokenUnchecked(regex: string, wgName: string): string | null {
  const pattern = regex.trim();
  const token = escapeRegexLiteral(wgName);
  const generated = pattern.match(/^\^\((.*)\)\$$/);
  if (generated) {
    const alternatives = splitEscapedAlternatives(generated[1]);
    if (!alternatives.includes(token)) return null;
    const remaining = alternatives.filter((alternative) => alternative !== token);
    return remaining.length > 0 ? `^(${remaining.join("|")})$` : "(?!)";
  }

  const appended = splitGeneratedAppend(pattern);
  if (!appended) return null;
  if (appended.token === token) return appended.inner || "(?!)";

  const nextInner = removeExactGroupTokenUnchecked(appended.inner, wgName);
  if (nextInner === null) return null;
  return nextInner === "(?!)" ? `^(${appended.token})$` : `(?:${nextInner})|^(${appended.token})$`;
}

export function removeExactGroupToken(regex: string, wgName: string): string | null {
  const nextRegex = removeExactGroupTokenUnchecked(regex, wgName);
  if (nextRegex === null) return null;
  try {
    return new RegExp(nextRegex).test(wgName) ? null : nextRegex;
  } catch {
    return null;
  }
}

export function compileGroupRegex(group: WorkgroupGroup): RegExp | null {
  if (charLength(group.regex) > MAX_GROUP_REGEX_LENGTH) return null;
  try {
    return new RegExp(group.regex);
  } catch {
    return null;
  }
}

/** #777 Non-stop membership matcher. Mirrors the rail's `groupMatches` exactly
 *  (reuses `compileGroupRegex`), so the rail, the predicate, and the watchdog
 *  client resolve membership identically. Oversize/invalid regex -> false (no throw). */
export function nonStopMatchesWorkgroup(config: NonStopGroupConfig, wg: AcWorkgroup): boolean {
  const id = groupMatchId(wg);
  if (charLength(id) > MAX_GROUP_MATCH_ID_LENGTH) return false;
  const regex = compileGroupRegex({ id: "nonstop", name: "nonstop", regex: config.regex });
  return !!regex?.test(id);
}

export function validateGroupsConfig(
  config: WorkgroupGroupsConfig,
  options: { validateRegexSyntax?: boolean } = {}
): string[] {
  const errors: string[] = [];
  if (!config.showAll && !config.showUngrouped) {
    errors.push("At least one of All or Ungrouped must be visible.");
  }
  if (config.groups.length > MAX_WORKGROUP_GROUPS) {
    errors.push(`At most ${MAX_WORKGROUP_GROUPS} groups are allowed.`);
  }

  const ids = new Set<string>();
  const names = new Set<string>();
  config.groups.forEach((group, index) => {
    const label = `Group ${index + 1}`;
    const id = group.id.trim();
    const name = group.name.trim();
    if (!id) errors.push(`${label}: id cannot be blank.`);
    if (charLength(group.id) > MAX_GROUP_ID_LENGTH) {
      errors.push(`${label}: id cannot exceed ${MAX_GROUP_ID_LENGTH} characters.`);
    }
    if (id && ids.has(id)) errors.push("Duplicate group id.");
    if (id) ids.add(id);

    if (!name) errors.push(`${label}: name cannot be blank.`);
    if (charLength(group.name) > MAX_GROUP_NAME_LENGTH) {
      errors.push(`${label}: name cannot exceed ${MAX_GROUP_NAME_LENGTH} characters.`);
    }
    const normalizedName = name.toLowerCase();
    if (normalizedName && names.has(normalizedName)) errors.push("Duplicate group name.");
    if (normalizedName) names.add(normalizedName);

    if (charLength(group.regex) > MAX_GROUP_REGEX_LENGTH) {
      errors.push(`${label}: regex cannot exceed ${MAX_GROUP_REGEX_LENGTH} characters.`);
    } else if (options.validateRegexSyntax) {
      try {
        new RegExp(group.regex);
      } catch {
        errors.push(`${label}: regex is invalid.`);
      }
    }
  });

  // #777 C6: soft regex-syntax check for Non-stop, ONLY under validateRegexSyntax
  // (the save/modal path). Load uses validateRegexSyntax:false, so a bad nonStop
  // never trips the fatal reset-to-defaults path; normalizeNonStop repairs it there.
  if (config.nonStop && options.validateRegexSyntax) {
    if (charLength(config.nonStop.regex) > MAX_GROUP_REGEX_LENGTH) {
      errors.push(`Alert me!: regex cannot exceed ${MAX_GROUP_REGEX_LENGTH} characters.`);
    } else {
      try {
        new RegExp(config.nonStop.regex);
      } catch {
        errors.push("Alert me!: regex is invalid.");
      }
    }
  }

  return Array.from(new Set(errors));
}

export function createGroupId(existingIds: Set<string>): string {
  const randomUUID = globalThis.crypto?.randomUUID;
  if (typeof randomUUID === "function") {
    const id = randomUUID.call(globalThis.crypto);
    if (!existingIds.has(id)) return id;
  }
  let index = 1;
  while (existingIds.has(`group-${index}`)) index++;
  return `group-${index}`;
}

function setConfig(
  projectPath: string,
  config: WorkgroupGroupsConfig,
  patch: Partial<Pick<ProjectGroupsEntry, "loaded" | "loading" | "saving" | "error">> = {}
) {
  const key = keyFor(projectPath);
  const current = ensureEntry(projectPath);
  // #777 C6: clamp/repair nonStop on every store write (covers load + post-save),
  // so a bad persisted value degrades gracefully instead of wiping the config.
  const nextConfig = cloneConfig({ ...config, nonStop: normalizeNonStop(config.nonStop) });
  setEntries(key, {
    ...current,
    config: nextConfig,
    selection: normalizeSelection(current.selection, nextConfig),
    ...patch,
  });
}

export const workgroupGroupsStore = {
  async ensureLoaded(projectPath: string): Promise<void> {
    const key = keyFor(projectPath);
    const current = ensureEntry(projectPath);
    if (current.loaded) return;
    const inFlight = inFlightLoads.get(key);
    if (inFlight) return inFlight;
    const saveVersionAtStart = saveVersions.get(key) ?? 0;

    const promise = (async () => {
      setEntries(key, { ...entryFor(projectPath), loading: true, error: null });
      try {
        const loaded = await ProjectAPI.getGroups(projectPath);
        if ((saveVersions.get(key) ?? 0) !== saveVersionAtStart) return;
        const structuralErrors = validateGroupsConfig(loaded, { validateRegexSyntax: false });
        if (structuralErrors.length > 0) {
          setConfig(projectPath, defaultGroupsConfig(), {
            loaded: true,
            loading: false,
            error: structuralErrors.join(" "),
          });
          return;
        }
        setConfig(projectPath, loaded, { loaded: true, loading: false, error: null });
      } catch (error) {
        if ((saveVersions.get(key) ?? 0) !== saveVersionAtStart) return;
        setConfig(projectPath, defaultGroupsConfig(), {
          loaded: true,
          loading: false,
          error: formatError(error),
        });
      } finally {
        inFlightLoads.delete(key);
      }
    })();

    inFlightLoads.set(key, promise);
    return promise;
  },

  config(projectPath: string): WorkgroupGroupsConfig {
    return cloneConfig(entryFor(projectPath).config);
  },

  selection(projectPath: string): WorkgroupGroupSelection {
    return entryFor(projectPath).selection;
  },

  select(projectPath: string, selection: WorkgroupGroupSelection): void {
    const key = keyFor(projectPath);
    const current = ensureEntry(projectPath);
    setEntries(key, {
      ...current,
      selection: normalizeSelection(selection, current.config),
    });
  },

  error(projectPath: string): string | null {
    return entryFor(projectPath).error;
  },

  loading(projectPath: string): boolean {
    return entryFor(projectPath).loading;
  },

  saving(projectPath: string): boolean {
    return entryFor(projectPath).saving;
  },

  applyExternalUpdate(projectPath: string, config: WorkgroupGroupsConfig): void {
    const key = keyFor(projectPath);
    saveVersions.set(key, (saveVersions.get(key) ?? 0) + 1);

    const structuralErrors = validateGroupsConfig(config, { validateRegexSyntax: false });
    if (structuralErrors.length > 0) {
      setConfig(projectPath, defaultGroupsConfig(), {
        loaded: true,
        loading: false,
        saving: false,
        error: structuralErrors.join(" "),
      });
      return;
    }

    setConfig(projectPath, config, {
      loaded: true,
      loading: false,
      saving: false,
      error: null,
    });
  },

  async save(projectPath: string, config: WorkgroupGroupsConfig): Promise<void> {
    const key = keyFor(projectPath);
    const current = ensureEntry(projectPath);
    const errors = validateGroupsConfig(config, { validateRegexSyntax: true });
    if (errors.length > 0) {
      const message = errors.join(" ");
      setEntries(key, { ...current, error: message });
      throw new Error(message);
    }

    setEntries(key, { ...current, saving: true, error: null });
    try {
      const saved = await ProjectAPI.updateGroups(projectPath, cloneConfig(config));
      const structuralErrors = validateGroupsConfig(saved, { validateRegexSyntax: false });
      if (structuralErrors.length > 0) {
        throw new Error(structuralErrors.join(" "));
      }
      saveVersions.set(key, (saveVersions.get(key) ?? 0) + 1);
      setConfig(projectPath, saved, {
        loaded: true,
        loading: false,
        saving: false,
        error: null,
      });
    } catch (error) {
      const latest = entryFor(projectPath);
      const message = formatError(error);
      setEntries(key, { ...latest, saving: false, error: message });
      throw new Error(message);
    }
  },

  async reorderGroup(projectPath: string, groupId: string, targetIndex: number): Promise<void> {
    const config = this.config(projectPath);
    const groups = reorderGroups(config.groups, groupId, targetIndex);
    const before = config.groups.map((group) => group.id).join("\0");
    const after = groups.map((group) => group.id).join("\0");
    if (before === after) return;
    await this.save(projectPath, { ...config, groups });
  },

  async addWorkgroupToGroup(projectPath: string, groupId: string, wgName: string): Promise<void> {
    if (charLength(wgName) > MAX_GROUP_MATCH_ID_LENGTH) {
      const message = `Workgroup id cannot exceed ${MAX_GROUP_MATCH_ID_LENGTH} characters.`;
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    const config = this.config(projectPath);
    const group = config.groups.find((candidate) => candidate.id === groupId);
    if (!group) throw new Error("Group no longer exists.");
    const nextRegex = appendExactGroupToken(group.regex, wgName);
    if (nextRegex === null) {
      const message = "Fix this group's regex before adding a workgroup.";
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    await this.save(projectPath, {
      ...config,
      groups: config.groups.map((candidate) =>
        candidate.id === groupId ? { ...candidate, regex: nextRegex } : candidate
      ),
    });
  },

  async removeWorkgroupFromGroup(projectPath: string, groupId: string, wgName: string): Promise<void> {
    if (charLength(wgName) > MAX_GROUP_MATCH_ID_LENGTH) {
      const message = `Workgroup id cannot exceed ${MAX_GROUP_MATCH_ID_LENGTH} characters.`;
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    const config = this.config(projectPath);
    const group = config.groups.find((candidate) => candidate.id === groupId);
    if (!group) throw new Error("Group no longer exists.");
    const nextRegex = removeExactGroupToken(group.regex, wgName);
    if (nextRegex === null) {
      const message = "This membership comes from a custom regex. Use Edit groups to change it.";
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    await this.save(projectPath, {
      ...config,
      groups: config.groups.map((candidate) =>
        candidate.id === groupId ? { ...candidate, regex: nextRegex } : candidate
      ),
    });
  },

  async createGroupForWorkgroup(projectPath: string, name: string, wgName: string): Promise<void> {
    if (charLength(wgName) > MAX_GROUP_MATCH_ID_LENGTH) {
      const message = `Workgroup id cannot exceed ${MAX_GROUP_MATCH_ID_LENGTH} characters.`;
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    const config = this.config(projectPath);
    const existingIds = new Set(config.groups.map((group) => group.id));
    await this.save(projectPath, {
      ...config,
      groups: [
        ...config.groups,
        {
          id: createGroupId(existingIds),
          name: name.trim(),
          regex: exactGroupRegexForWorkgroup(wgName),
        },
      ],
    });
  },

  // #777 C5: membership helpers for the built-in Non-stop slot. Mirror
  // addWorkgroupToGroup/removeWorkgroupFromGroup but target `config.nonStop.regex`.
  async addWorkgroupToNonStop(projectPath: string, wgName: string): Promise<void> {
    if (charLength(wgName) > MAX_GROUP_MATCH_ID_LENGTH) {
      const message = `Workgroup id cannot exceed ${MAX_GROUP_MATCH_ID_LENGTH} characters.`;
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    const config = this.config(projectPath);
    const current = config.nonStop;
    if (!current) {
      // Materialize the slot (shown) with an exact-match regex for this workgroup.
      await this.save(projectPath, {
        ...config,
        nonStop: { ...defaultNonStop(), show: true, regex: exactGroupRegexForWorkgroup(wgName) },
      });
      return;
    }
    const nextRegex = appendExactGroupToken(current.regex, wgName);
    if (nextRegex === null) {
      const message = "Fix the Alert me! regex before adding a workgroup.";
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    await this.save(projectPath, { ...config, nonStop: { ...current, regex: nextRegex } });
  },

  async removeWorkgroupFromNonStop(projectPath: string, wgName: string): Promise<void> {
    if (charLength(wgName) > MAX_GROUP_MATCH_ID_LENGTH) {
      const message = `Workgroup id cannot exceed ${MAX_GROUP_MATCH_ID_LENGTH} characters.`;
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    const config = this.config(projectPath);
    const current = config.nonStop;
    if (!current) return;
    const nextRegex = removeExactGroupToken(current.regex, wgName);
    if (nextRegex === null) {
      const message = "This membership comes from a custom regex. Use Edit groups to change it.";
      const key = keyFor(projectPath);
      setEntries(key, { ...ensureEntry(projectPath), error: message });
      throw new Error(message);
    }
    await this.save(projectPath, { ...config, nonStop: { ...current, regex: nextRegex } });
  },

  resetForTests(): void {
    for (const key of Object.keys(entries)) {
      setEntries(key, undefined);
    }
    inFlightLoads.clear();
    saveVersions.clear();
  },
};
