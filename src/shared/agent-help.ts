import embeddedAgentHelp from "../../src-tauri/resources/agent-help/agent-help.json";
import { AgentHelpAPI } from "./ipc";
import { executableBasename } from "./profile-utils";

export interface AgentHelpTip {
  title: string;
  body: string;
  link?: { label: string; url: string };
}

export interface AgentHelpEntry {
  label?: string;
  paramsExample?: string;
  docsUrl?: string;
  tips?: AgentHelpTip[];
}

export interface AgentHelpFile {
  schemaVersion: number;
  note?: string;
  general?: AgentHelpEntry;
  byCommand?: Record<string, AgentHelpEntry>;
  byAgent?: Record<string, AgentHelpEntry>;
}

export interface AgentHelpOverlay {
  local: AgentHelpFile | null;
  remote: AgentHelpFile | null;
  localError: string | null;
}

export const AGENT_HELP_SCHEMA_VERSION = 1;
export const GENERIC_PARAMS_EXAMPLE = "--help";

export const EMBEDDED_AGENT_HELP: AgentHelpFile = embeddedAgentHelp;

export const EMPTY_AGENT_HELP_OVERLAY: AgentHelpOverlay = Object.freeze({
  local: null,
  remote: null,
  localError: null,
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function entryIn(map: unknown, key: string): AgentHelpEntry | null {
  if (!isRecord(map) || !Object.prototype.hasOwnProperty.call(map, key)) return null;
  const entry = map[key];
  return isRecord(entry) ? (entry as AgentHelpEntry) : null;
}

function fileOrNull(value: unknown): AgentHelpFile | null {
  return isRecord(value) ? (value as unknown as AgentHelpFile) : null;
}

function layerOf(overlay: unknown, layer: "local" | "remote"): AgentHelpFile | null {
  return isRecord(overlay) ? fileOrNull(overlay[layer]) : null;
}

function normalizeOverlay(payload: unknown): AgentHelpOverlay {
  if (!isRecord(payload)) return EMPTY_AGENT_HELP_OVERLAY;
  return {
    local: fileOrNull(payload.local),
    remote: fileOrNull(payload.remote),
    localError: typeof payload.localError === "string" ? payload.localError : null,
  };
}

/**
 * Four layers, whole entry, first match wins: local byAgent, local byCommand,
 * remote byCommand, embedded byCommand. Without a usable command stem only the
 * local byAgent row can match.
 */
export function resolveAgentHelpEntry(
  overlay: AgentHelpOverlay,
  agentId: string,
  command: string
): AgentHelpEntry | null {
  const local = layerOf(overlay, "local");
  const byAgent = entryIn(local?.byAgent, agentId);
  if (byAgent) return byAgent;
  const stem = typeof command === "string" && command.trim() ? executableBasename(command) : "";
  if (!stem) return null;
  return (
    entryIn(local?.byCommand, stem) ??
    entryIn(layerOf(overlay, "remote")?.byCommand, stem) ??
    entryIn(EMBEDDED_AGENT_HELP.byCommand, stem)
  );
}

export function resolveAgentHelpGeneral(overlay: AgentHelpOverlay): AgentHelpEntry | null {
  for (const file of [layerOf(overlay, "local"), layerOf(overlay, "remote"), EMBEDDED_AGENT_HELP]) {
    if (file && isRecord(file.general)) return file.general;
  }
  return null;
}

export function paramsExampleFor(
  overlay: AgentHelpOverlay,
  agentId: string,
  command: string
): string {
  const value = resolveAgentHelpEntry(overlay, agentId, command)?.paramsExample;
  return typeof value === "string" && value ? value : GENERIC_PARAMS_EXAMPLE;
}

export function docsUrlFor(
  overlay: AgentHelpOverlay,
  agentId: string,
  command: string
): string | null {
  const value = resolveAgentHelpEntry(overlay, agentId, command)?.docsUrl;
  if (typeof value !== "string" || !value) return null;
  try {
    return new URL(value).protocol === "https:" ? value : null;
  } catch {
    return null;
  }
}

let warned = false;
export async function loadAgentHelpOverlay(): Promise<AgentHelpOverlay> {
  try {
    return normalizeOverlay(await AgentHelpAPI.get());
  } catch (err) {
    if (!warned) {
      warned = true;
      console.debug("[agent-help] overlay unavailable; using the embedded layer:", err);
    }
    return EMPTY_AGENT_HELP_OVERLAY;
  }
}
