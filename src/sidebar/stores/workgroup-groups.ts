import { createStore } from "solid-js/store";
import type { AcWorkgroup, WorkgroupGroup, WorkgroupGroupsConfig } from "../../shared/types";
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
  | { kind: "group"; id: string };

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

function keyFor(projectPath: string): string {
  return normalizeProjectPathForCompare(projectPath);
}

export function defaultGroupsConfig(): WorkgroupGroupsConfig {
  return {
    groups: [],
    showAll: true,
    showUngrouped: true,
  };
}

function cloneConfig(config: WorkgroupGroupsConfig): WorkgroupGroupsConfig {
  return {
    groups: config.groups.map((group) => ({ ...group })),
    showAll: config.showAll,
    showUngrouped: config.showUngrouped,
  };
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

export function compileGroupRegex(group: WorkgroupGroup): RegExp | null {
  if (charLength(group.regex) > MAX_GROUP_REGEX_LENGTH) return null;
  try {
    return new RegExp(group.regex);
  } catch {
    return null;
  }
}

export function validateGroupsConfig(
  config: WorkgroupGroupsConfig,
  options: { validateRegexSyntax?: boolean } = {}
): string[] {
  const errors: string[] = [];
  if (!config.showAll && !config.showUngrouped) {
    errors.push("At least one of Todos or Sin Grupo must be visible.");
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
  const nextConfig = cloneConfig(config);
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

  resetForTests(): void {
    for (const key of Object.keys(entries)) {
      setEntries(key, undefined);
    }
    inFlightLoads.clear();
    saveVersions.clear();
  },
};
