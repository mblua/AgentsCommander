import { DebugAPI, type RailDiagnosticRecord } from "../shared/ipc";
import { isDebugEnabled } from "../shared/console-capture";

export type RailFields = Omit<RailDiagnosticRecord, "version" | "windowId" | "seq" | "atMs" | "surface" | "event">;
type Kind = NonNullable<RailFields["kind"]>;
let windowId: string | undefined;
let sequence = 0;
let token = 0;
const objects = new WeakMap<object, number>();
const entities = new Map<string, { id?: number; groups: Map<string, number | undefined> }>();
const kinds = ["custom", "system", "nonstop", "ungrouped", "project", "favorites"];
const tags = ["button", "span", "svg", "path", "aside", "body", "input", "textarea", "other", "none"];
const events = ("pointerdown pointerup pointercancel mousedown mouseup click gotpointercapture lostpointercapture " +
  "focusin focusout window-focus window-blur handler-enter handler-end selection-change rail-mount rail-dispose " +
  "section-mount section-dispose node-mount node-dispose project-list project-buttons favorites-entries reload-start " +
  "reload-end discover-start discover-end snapshot-start snapshot-end snapshots-start snapshots-end task-start task-end loop-start loop-end").split(" ");
const positive = new Set("railId operationId projectId groupId entityId nodeId originalNodeId objectId selectionProjectId selectionGroupId".split(" "));
const booleans = new Set("primary trusted connected originalConnected captured documentFocused".split(" "));
const enums: { [key: string]: readonly string[] } = {
  kind: kinds, selectionKind: kinds, targetTag: tags, activeTag: tags,
  phase: ["arming", "dragging", "saving", "idle"],
  outcome: "applied unchanged stale error disconnected unloaded queued discarded suppressed overflow no-press".split(" "),
};

function validField(key: string, value: unknown): boolean {
  if (positive.has(key)) return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
  if (booleans.has(key)) return typeof value === "boolean";
  if (enums[key]) return typeof value === "string" && enums[key].includes(value);
  if (key === "count" || key === "changedCount") return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
  if (key === "durationMs") return typeof value === "number" && Number.isFinite(value) && value >= 0;
  if (key === "pointerId") return typeof value === "number" && Number.isInteger(value) && value >= -2147483648 && value <= 2147483647;
  if (key === "button") return typeof value === "number" && Number.isInteger(value) && value >= -1 && value <= 5;
  return false;
}

/** Dispatch starts here, synchronously. ACK is not a durable file receipt. */
export function emitRailDiagnostic(surface: RailDiagnosticRecord["surface"], event: RailDiagnosticRecord["event"], fields: RailFields = {}): void {
  if (!isDebugEnabled()) return;
  try {
    if (!["embedded", "sidebar", "store"].includes(surface) || !events.includes(event)) return;
    const projected: RailFields = {};
    for (const [key, value] of Object.entries(fields)) {
      if (value === undefined) continue;
      if (!validField(key, value)) return;
      Object.assign(projected, { [key]: value });
    }
    if (!windowId) windowId = crypto.randomUUID();
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(windowId)) return;
    if (sequence >= Number.MAX_SAFE_INTEGER) return;
    const atMs = performance.now();
    if (!Number.isFinite(atMs) || atMs < 0) return;
    const record: RailDiagnosticRecord = { version: 1, windowId, seq: sequence + 1, atMs, surface, event, ...projected };
    if (new TextEncoder().encode(JSON.stringify(record)).length > 2048) return;
    sequence++;
    void DebugAPI.appendRailDiagnostic(record).catch(() => {});
  } catch { /* Diagnostics must not reach the product or the console logger. */ }
}

export function railToken(): number | undefined {
  if (!isDebugEnabled() || token >= Number.MAX_SAFE_INTEGER) return undefined;
  return ++token;
}

export function railObjectId(object: object | null | undefined, assign = true): number | undefined {
  if (!isDebugEnabled() || !object) return undefined;
  let id = objects.get(object);
  if (id === undefined && assign) {
    id = railToken();
    if (id !== undefined) objects.set(object, id);
  }
  return id;
}

/** Only the loaded-project store may retire projects. Rails reconcile groups only. */
export function reconcileRailProjects(paths: readonly string[]): void {
  const live = new Set(paths);
  for (const path of entities.keys()) if (!live.has(path)) entities.delete(path);
  for (const path of paths) {
    let entry = entities.get(path);
    if (!entry) {
      entry = { id: railToken(), groups: new Map() };
      entities.set(path, entry);
    }
    if (entry && entry.id === undefined) entry.id = railToken();
  }
}

function groupKey(kind: Kind, id: string | null): string { return `${kind}:${id === null ? "n" : `s${id}`}`; }

export function reconcileRailGroups(path: string, groups: readonly string[], nonstop: boolean): void {
  const entry = entities.get(path);
  if (!entry) return;
  if (entry.id === undefined) entry.id = railToken();
  const live = new Set([groupKey("system", null), groupKey("ungrouped", null), ...groups.map(id => groupKey("custom", id))]);
  if (nonstop) live.add(groupKey("nonstop", null));
  for (const key of entry.groups.keys()) if (!live.has(key)) entry.groups.delete(key);
  if (!isDebugEnabled()) return;
  for (const key of live) if (entry.groups.get(key) === undefined) entry.groups.set(key, railToken());
}

/** Lookups never recreate a removed entity, including release after deletion. */
export function railEntity(path: string, kind: Kind = "project", id: string | null = null): RailFields {
  if (!isDebugEnabled()) return {};
  const entry = entities.get(path);
  const groupId = entry?.groups.get(groupKey(kind, id));
  return { projectId: entry?.id, groupId, entityId: kind === "project" ? entry?.id : groupId, kind };
}

export function railTag(target: EventTarget | null): NonNullable<RailFields["targetTag"]> {
  if (!target) return "none";
  const tag = target instanceof Element ? target.tagName.toLowerCase() : "other";
  return (tags.includes(tag) ? tag : "other") as NonNullable<RailFields["targetTag"]>;
}
