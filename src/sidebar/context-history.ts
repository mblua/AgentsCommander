// #3532 P10 - the single sidebar owner of context-history means. Rows acquire a
// {projectPath, runtimeSessionId} scope and read an accessor; this module keeps
// the in-memory subscribers and results, batches requests per project and
// never computes a mean itself. Nothing here is written to disk.

import { createSignal, type Accessor, type Setter } from "solid-js";
import { ContextHistoryAPI } from "../shared/ipc";
import { isTauri } from "../shared/platform";
import {
  isHistoryIpcError,
  type SidebarBucket,
  type SidebarEntry,
  type SidebarScope,
  type SidebarWindows,
} from "../shared/context-history";

export const MAX_ACTIVE_KEYS = 4096;
export const MAX_IDS_PER_REQUEST = 100;
export const REFRESH_INTERVAL_MS = 30_000;
/** Backend deadline is 5s; past this the frontend frees the slot and drops the reply. */
export const REQUEST_WATCHDOG_MS = 10_000;
export const FOCUS_DEBOUNCE_MS = 1_000;

export interface SidebarHistoryScope {
  projectPath: string;
  runtimeSessionId: string | null | undefined;
  /** Root rows never carry a context history. */
  isRoot?: boolean;
}

export type SidebarHistoryState =
  | { status: "unavailable" }
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ambiguous" | "unknown"; scope: SidebarScope; generatedAtUtcMs: number; text: string }
  | { status: "ready"; scope: SidebarScope; windows: SidebarWindows; generatedAtUtcMs: number };

export interface ContextHistoryHandle {
  state: Accessor<SidebarHistoryState>;
  /** Idempotent. */
  release: () => void;
}

const UNAVAILABLE: SidebarHistoryState = { status: "unavailable" };
const NEUTRAL_TEXT: Record<"ambiguous" | "unknown", string> = {
  ambiguous: "Sin media: el agente pertenece a más de un team.",
  unknown: "Sin media: no se pudo identificar el agente o su team.",
};
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

interface KeyEntry {
  refs: number;
  state: Accessor<SidebarHistoryState>;
  setState: Setter<SidebarHistoryState>;
  fetchedAtMs: number | null;
}

interface ProjectOwner {
  projectPath: string;
  generation: number;
  keys: Map<string, KeyEntry>;
  /** Insertion-ordered and duplicate-free; always a subset of `keys`. */
  queue: Set<string>;
  inflight: number | null;
  /** Entries of the request in flight; a re-acquired key is a new entry. */
  inflightEntries: Set<KeyEntry>;
  watchdog: ReturnType<typeof setTimeout> | null;
  flushScheduled: boolean;
  timer: ReturnType<typeof setInterval> | null;
}

const owners = new Map<string, ProjectOwner>();
let activeKeys = 0;
let nextRequestId = 1;
let overflowDiagnostics = 0;
let focusTimer: ReturnType<typeof setTimeout> | null = null;
let listening = false;

const neutralHandle = (): ContextHistoryHandle => ({ state: () => UNAVAILABLE, release: () => {} });

export function acquireContextHistory(scope: SidebarHistoryScope): ContextHistoryHandle {
  const id = scope.runtimeSessionId;
  if (!isTauri || scope.isRoot || !id || !UUID.test(id) || !scope.projectPath) {
    return neutralHandle();
  }
  let owner = owners.get(scope.projectPath);
  let entry = owner?.keys.get(id);
  if (!entry) {
    if (activeKeys >= MAX_ACTIVE_KEYS) {
      if (overflowDiagnostics++ < 3) {
        console.warn(
          `[context-history] more than ${MAX_ACTIVE_KEYS} active keys; extra rows stay neutral`,
        );
      }
      return neutralHandle();
    }
    if (!owner) owner = createOwner(scope.projectPath);
    const [state, setState] = createSignal<SidebarHistoryState>({ status: "loading" });
    entry = { refs: 0, state, setState, fetchedAtMs: null };
    owner.keys.set(id, entry);
    activeKeys += 1;
    enqueue(owner, id);
  }
  entry.refs += 1;
  const held = owner!;
  const heldEntry = entry;
  let released = false;
  return {
    state: heldEntry.state,
    release: () => {
      if (released) return;
      released = true;
      releaseKey(held, id, heldEntry);
    },
  };
}

/** Hover asks for fresher data; only a key older than the refresh interval re-queues. */
export function refreshContextHistoryOnHover(scope: SidebarHistoryScope): void {
  const id = scope.runtimeSessionId;
  if (!id) return;
  const owner = owners.get(scope.projectPath);
  const entry = owner?.keys.get(id);
  if (!owner || !entry || entry.fetchedAtMs === null) return;
  if (Date.now() - entry.fetchedAtMs < REFRESH_INTERVAL_MS) return;
  enqueue(owner, id);
}

/** The bucket the saved Settings window selects; never recomputed here. */
export function bucketForDays(
  windows: SidebarWindows,
  days: 1 | 7 | 30 | undefined,
): SidebarBucket {
  if (days === 1) return windows.day;
  if (days === 7) return windows.week;
  return windows.month;
}

function createOwner(projectPath: string): ProjectOwner {
  const owner: ProjectOwner = {
    projectPath,
    generation: 1,
    keys: new Map(),
    queue: new Set(),
    inflight: null,
    inflightEntries: new Set(),
    watchdog: null,
    flushScheduled: false,
    timer: null,
  };
  owners.set(projectPath, owner);
  owner.timer = setInterval(() => refreshProject(owner), REFRESH_INTERVAL_MS);
  listen();
  return owner;
}

function releaseKey(owner: ProjectOwner, id: string, entry: KeyEntry): void {
  entry.refs -= 1;
  if (entry.refs > 0 || owner.keys.get(id) !== entry) return;
  owner.keys.delete(id);
  owner.queue.delete(id);
  activeKeys -= 1;
  if (owner.keys.size > 0) return;
  if (owner.timer !== null) clearInterval(owner.timer);
  owner.timer = null;
  owner.queue.clear();
  clearInflight(owner);
  owner.generation += 1;
  if (owners.get(owner.projectPath) === owner) owners.delete(owner.projectPath);
  if (owners.size === 0) unlisten();
}

function enqueue(owner: ProjectOwner, id: string): void {
  const entry = owner.keys.get(id);
  if (owners.get(owner.projectPath) !== owner || !entry) return;
  if (owner.inflightEntries.has(entry)) return;
  owner.queue.add(id);
  if (owner.flushScheduled || owner.inflight !== null) return;
  owner.flushScheduled = true;
  queueMicrotask(() => {
    owner.flushScheduled = false;
    flush(owner);
  });
}

/** Timer, focus and visibility refresh: skipped while a pass is still queued or in flight. */
function refreshProject(owner: ProjectOwner): void {
  if (typeof document !== "undefined" && document.hidden) return;
  if (owner.inflight !== null || owner.queue.size > 0) return;
  for (const id of owner.keys.keys()) enqueue(owner, id);
}

function flush(owner: ProjectOwner): void {
  if (owners.get(owner.projectPath) !== owner || owner.inflight !== null) return;
  const ids = Array.from(owner.queue).slice(0, MAX_IDS_PER_REQUEST);
  if (ids.length === 0) return;
  for (const id of ids) owner.queue.delete(id);
  const sent = ids.map((id) => [id, owner.keys.get(id)!] as const);
  for (const [, entry] of sent) owner.inflightEntries.add(entry);
  const requestId = nextRequestId++;
  const generation = owner.generation;
  owner.inflight = requestId;
  const current = () =>
    owners.get(owner.projectPath) === owner
    && owner.generation === generation
    && owner.inflight === requestId;
  const finish = () => {
    clearInflight(owner);
    if (owner.queue.size > 0) enqueue(owner, owner.queue.values().next().value as string);
  };
  owner.watchdog = setTimeout(() => {
    owner.watchdog = null;
    if (!current()) return;
    applyError(owner, sent, "Timeout");
    finish();
  }, REQUEST_WATCHDOG_MS);
  ContextHistoryAPI.sidebarMeans({ projectPath: owner.projectPath, runtimeSessionIds: ids }).then(
    (response) => {
      if (!current()) return;
      const byId = new Map<string, SidebarEntry>(
        response.entries.map((e) => [e.runtimeSessionId, e]),
      );
      for (const [id, entry] of sent) {
        if (owner.keys.get(id) !== entry) continue;
        const result = byId.get(id);
        entry.fetchedAtMs = Date.now();
        entry.setState(result ? toState(result, response.generatedAtUtcMs) : missing());
      }
      finish();
    },
    (error: unknown) => {
      if (!current()) return;
      applyError(owner, sent, isHistoryIpcError(error) ? error.code : String(error));
      finish();
    },
  );
}

function toState(entry: SidebarEntry, generatedAtUtcMs: number): SidebarHistoryState {
  if (entry.scope.kind === "known" && entry.windows) {
    return { status: "ready", scope: entry.scope, windows: entry.windows, generatedAtUtcMs };
  }
  const kind = entry.scope.kind === "ambiguous" ? "ambiguous" : "unknown";
  return { status: kind, scope: entry.scope, generatedAtUtcMs, text: NEUTRAL_TEXT[kind] };
}

const missing = (): SidebarHistoryState => ({ status: "error", message: "No entry in response" });

function clearInflight(owner: ProjectOwner): void {
  if (owner.watchdog !== null) clearTimeout(owner.watchdog);
  owner.watchdog = null;
  owner.inflight = null;
  owner.inflightEntries.clear();
}

/** Applies only to the entry that was sent; a re-acquired key keeps its own state. */
function applyError(
  owner: ProjectOwner,
  sent: ReadonlyArray<readonly [string, KeyEntry]>,
  message: string,
): void {
  for (const [id, entry] of sent) {
    if (owner.keys.get(id) !== entry) continue;
    entry.fetchedAtMs = Date.now();
    entry.setState({ status: "error", message });
  }
}

function onFocusOrVisible(): void {
  if (typeof document !== "undefined" && document.hidden) return;
  if (focusTimer !== null) return;
  focusTimer = setTimeout(() => {
    focusTimer = null;
    for (const owner of owners.values()) refreshProject(owner);
  }, FOCUS_DEBOUNCE_MS);
}

function listen(): void {
  if (listening || typeof window === "undefined") return;
  listening = true;
  window.addEventListener("focus", onFocusOrVisible);
  document.addEventListener("visibilitychange", onFocusOrVisible);
}

function unlisten(): void {
  if (!listening) return;
  listening = false;
  window.removeEventListener("focus", onFocusOrVisible);
  document.removeEventListener("visibilitychange", onFocusOrVisible);
  if (focusTimer !== null) clearTimeout(focusTimer);
  focusTimer = null;
}

/** Test seam: what the owner holds right now. */
export function __contextHistoryStatsForTests() {
  return {
    projects: owners.size,
    activeKeys,
    queued: Array.from(owners.values()).reduce((n, o) => n + o.queue.size, 0),
    inflight: Array.from(owners.values()).filter((o) => o.inflight !== null).length,
    timers: Array.from(owners.values()).filter((o) => o.timer !== null).length,
    watchdogs: Array.from(owners.values()).filter((o) => o.watchdog !== null).length,
    listening,
  };
}

export function __resetContextHistoryForTests(): void {
  for (const owner of owners.values()) {
    if (owner.timer !== null) clearInterval(owner.timer);
    clearInflight(owner);
    owner.generation += 1;
  }
  owners.clear();
  activeKeys = 0;
  overflowDiagnostics = 0;
  unlisten();
}
