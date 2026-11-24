// Settings > Coding Agents Export/Import: the transfer file format and its IPC.
// Pure functions own the format; the API factory takes an injected transport.
import type { Transport } from "./transport";
import type {
  AgentConfig,
  CodingAgentProfilesConfig,
  ProfileCellConfig,
  ProfileSlotConfig,
  SessionBackendKind,
} from "./types";
import { PROFILE_LETTERS } from "./profile-utils";

export const TRANSFER_FORMAT = "agentscommander.coding-agents-export";
export const TRANSFER_FORMAT_VERSION = 1;
const MAX_TIMEOUT_SECONDS = 4_294_967_295;
const AGENT_FIELDS = [
  "label", "command", "color", "envs", "isolatedHome", "instructionsFilename",
  "configSeed", "contextRegex", "backend", "preflight", "preflightTimeoutSeconds",
] as const;
const CELL_FIELDS = [
  "enabled", "command", "env", "notes", "preflight", "preflightTimeoutSeconds",
] as const;

export type TransferAgent = Pick<AgentConfig, (typeof AGENT_FIELDS)[number]>;
export type TransferCell = Pick<ProfileCellConfig, (typeof CELL_FIELDS)[number]>;
export interface TransferProfile { label?: string; cell?: TransferCell }
export interface TransferEntry { agent: TransferAgent; profiles: Record<string, TransferProfile> }
export interface TransferFile {
  format: typeof TRANSFER_FORMAT;
  formatVersion: typeof TRANSFER_FORMAT_VERSION;
  exportedAt: string;
  profileSlots: Record<string, ProfileSlotConfig>;
  agents: TransferEntry[];
}
export interface TransferMerge {
  added: AgentConfig[];
  profiles: CodingAgentProfilesConfig;
}

/** The two native dialog commands; Rust owns the dialog and the disk. */
export function createCodingAgentsTransferAPI(transport: Pick<Transport, "invoke">) {
  return {
    pickSaveExport: (content: string, defaultFileName: string) =>
      transport.invoke<string | null>("coding_agents_export_pick_save", {
        content,
        defaultFileName,
      }),
    pickOpenImport: () => transport.invoke<string | null>("coding_agents_import_pick_open"),
  };
}

function pick<T extends object, K extends keyof T>(source: T, keys: readonly K[]): Pick<T, K> {
  const out = {} as Pick<T, K>;
  for (const key of keys) if (source[key] !== undefined) out[key] = source[key];
  return out;
}

const pad = (n: number) => String(n).padStart(2, "0");

export function exportFileName(now: Date): string {
  const date = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`;
  const time = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `ac-coding-agents-exports_${date}_${time}.json`;
}

export function buildTransferFile(
  agents: AgentConfig[],
  profiles: CodingAgentProfilesConfig,
  now: Date,
): TransferFile {
  const profileSlots: Record<string, ProfileSlotConfig> = {};
  const entries = agents.map((agent) => {
    const cells = profiles.profilesByAgent[agent.id] ?? {};
    const labels = profiles.profileLabelsByAgent[agent.id] ?? {};
    const out: Record<string, TransferProfile> = {};
    for (const letter of PROFILE_LETTERS) {
      const cell = cells[letter];
      const label = labels[letter];
      if (cell === undefined && label === undefined) continue;
      out[letter] = {
        ...(label === undefined ? {} : { label }),
        ...(cell === undefined ? {} : { cell: pick(cell, CELL_FIELDS) }),
      };
      profileSlots[letter] = { label: profiles.profileSlots[letter]?.label ?? "" };
    }
    return { agent: pick(agent, AGENT_FIELDS), profiles: out };
  });
  return {
    format: TRANSFER_FORMAT,
    formatVersion: TRANSFER_FORMAT_VERSION,
    exportedAt: now.toISOString(),
    profileSlots,
    agents: entries,
  };
}

export function transferHasEnvValues(file: TransferFile): boolean {
  return file.agents.some(
    ({ agent, profiles }) =>
      agent.envs.some((env) => env.value !== "") ||
      Object.values(profiles).some((p) => Object.values(p.cell?.env ?? {}).some((v) => v !== "")),
  );
}

type Json = Record<string, unknown>;
const isObject = (v: unknown): v is Json =>
  typeof v === "object" && v !== null && !Array.isArray(v);

function fail(where: string, what: string): never {
  throw new Error(`Invalid import file: ${where}${what}`);
}

function req<T>(o: Json, key: string, where: string, ok: (v: unknown) => v is T): T {
  const v = o[key];
  if (!ok(v)) fail(where, `${key} is missing or has the wrong type`);
  return v;
}
function opt<T>(o: Json, key: string, where: string, ok: (v: unknown) => v is T): T | undefined {
  return o[key] === undefined ? undefined : req(o, key, where, ok);
}
const isString = (v: unknown): v is string => typeof v === "string";
const isBool = (v: unknown): v is boolean => typeof v === "boolean";
const isTimeout = (v: unknown): v is number =>
  Number.isInteger(v) && (v as number) >= 1 && (v as number) <= MAX_TIMEOUT_SECONDS;
const isBackendKind = (v: unknown): v is SessionBackendKind =>
  v === "localProcess" || v === "containerTransport";
const isLetter = (k: string) => PROFILE_LETTERS.includes(k);
const defined = <T extends Json>(o: T): T =>
  Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as T;

function readEnvs(o: Json, where: string): AgentConfig["envs"] {
  const list = req(o, "envs", where, Array.isArray);
  return list.map((raw: unknown, i) => {
    const at = `${where}envs[${i}].`;
    if (!isObject(raw)) fail(at, "entry is not an object");
    const source = req(raw, "source", at, isString);
    if (source !== "user" && source !== "system") fail(at, "source is not user or system");
    return {
      key: req(raw, "key", at, isString),
      value: req(raw, "value", at, isString),
      source,
      enabled: req(raw, "enabled", at, isBool),
    };
  });
}

function readAgent(o: Json, where: string): TransferAgent {
  const seed = opt(o, "configSeed", where, isObject);
  const backend = opt(o, "backend", where, isObject);
  const at = `${where}backend.`;
  const kind = backend === undefined ? undefined : opt(backend, "kind", at, isBackendKind);
  return defined({
    label: req(o, "label", where, isString),
    command: req(o, "command", where, isString),
    color: req(o, "color", where, isString),
    envs: readEnvs(o, where),
    isolatedHome: req(o, "isolatedHome", where, isBool),
    instructionsFilename: opt(o, "instructionsFilename", where, isString),
    configSeed: seed && {
      enabled: req(seed, "enabled", `${where}configSeed.`, isBool),
      dest: req(seed, "dest", `${where}configSeed.`, isString),
    },
    contextRegex: opt(o, "contextRegex", where, isString),
    backend: backend && defined({ kind, image: opt(backend, "image", at, isString) }),
    preflight: opt(o, "preflight", where, isString),
    preflightTimeoutSeconds: opt(o, "preflightTimeoutSeconds", where, isTimeout),
  });
}

function readCell(o: Json, where: string): TransferCell {
  const env = req(o, "env", where, isObject);
  for (const [k, v] of Object.entries(env)) {
    if (!isString(v)) fail(where, `env.${k} is not a string`);
  }
  return defined({
    enabled: req(o, "enabled", where, isBool),
    command: req(o, "command", where, isString),
    env: { ...(env as Record<string, string>) },
    notes: req(o, "notes", where, isString),
    preflight: opt(o, "preflight", where, isString),
    preflightTimeoutSeconds: opt(o, "preflightTimeoutSeconds", where, isTimeout),
  });
}

function readProfiles(o: Json, where: string): Record<string, TransferProfile> {
  const raw = opt(o, "profiles", where, isObject) ?? {};
  const out: Record<string, TransferProfile> = {};
  for (const [letter, value] of Object.entries(raw)) {
    const at = `${where}profiles.${letter}.`;
    if (!isLetter(letter)) fail(at, "letter is not A-Z");
    if (!isObject(value)) fail(at, "entry is not an object");
    const cell = opt(value, "cell", at, isObject);
    out[letter] = defined({
      label: opt(value, "label", at, isString),
      cell: cell && readCell(cell, `${at}cell.`),
    });
  }
  return out;
}

function readSlots(o: Json): Record<string, ProfileSlotConfig> {
  const raw = opt(o, "profileSlots", "", isObject) ?? {};
  const out: Record<string, ProfileSlotConfig> = {};
  for (const [letter, value] of Object.entries(raw)) {
    const at = `profileSlots.${letter}.`;
    if (!isLetter(letter)) fail(at, "letter is not A-Z");
    if (!isObject(value)) fail(at, "entry is not an object");
    out[letter] = { label: req(value, "label", at, isString) };
  }
  return out;
}

/** All-or-nothing: returns a file holding only known fields, or throws. */
export function parseTransferFile(text: string): TransferFile {
  let root: unknown;
  try {
    root = JSON.parse(text.replace(/^﻿/, ""));
  } catch {
    throw new Error("Import file is not valid JSON");
  }
  if (!isObject(root)) fail("", "root is not an object");
  if (root.format !== TRANSFER_FORMAT) fail("", "format is not " + TRANSFER_FORMAT);
  if (root.formatVersion !== TRANSFER_FORMAT_VERSION) fail("", "formatVersion is not 1");
  const list = req(root, "agents", "", Array.isArray);
  if (list.length === 0) throw new Error("Import file contains no coding agents");
  const agents = list.map((raw: unknown, i) => {
    const where = `Agent ${i + 1}: `;
    if (!isObject(raw)) fail(where, "entry is not an object");
    const agent = req(raw, "agent", where, isObject);
    return { agent: readAgent(agent, where), profiles: readProfiles(raw, where) };
  });
  return {
    format: TRANSFER_FORMAT,
    formatVersion: TRANSFER_FORMAT_VERSION,
    exportedAt: isString(root.exportedAt) ? root.exportedAt : "",
    profileSlots: readSlots(root),
    agents,
  };
}

const labelKey = (label: string) => label.trim().toLowerCase();

export function uniqueAgentLabel(label: string, taken: Set<string>): string {
  const base = label.trim();
  if (base === "" || !taken.has(labelKey(base))) return label;
  for (let n = 1; ; n++) {
    const next = `${base} (${n})`;
    if (!taken.has(labelKey(next))) return next;
  }
}

/** Adds every file agent with a new id and a free label; never mutates inputs. */
export function mergeTransferFile(
  agents: AgentConfig[],
  profiles: CodingAgentProfilesConfig,
  file: TransferFile,
  newId: () => string,
): TransferMerge {
  const taken = new Set(agents.map((a) => labelKey(a.label)));
  const slots = { ...profiles.profileSlots };
  const byAgent = { ...profiles.profilesByAgent };
  const labelsByAgent = { ...profiles.profileLabelsByAgent };
  const added = file.agents.map(({ agent, profiles: entry }) => {
    const label = uniqueAgentLabel(agent.label, taken);
    taken.add(labelKey(label));
    const id = newId();
    const cells: Record<string, ProfileCellConfig> = {};
    const labels: Record<string, string> = {};
    for (const [letter, p] of Object.entries(entry)) {
      if (p.cell) cells[letter] = { ...p.cell, env: { ...p.cell.env } };
      if (p.label !== undefined) labels[letter] = p.label;
      slots[letter] ??= { label: file.profileSlots[letter]?.label ?? "" };
    }
    if (Object.keys(cells).length > 0) byAgent[id] = cells;
    if (Object.keys(labels).length > 0) labelsByAgent[id] = labels;
    return structuredClone({ ...agent, id, label });
  });
  return {
    added,
    profiles: { ...profiles, profileSlots: slots, profilesByAgent: byAgent,
      profileLabelsByAgent: labelsByAgent },
  };
}
