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
const events = new Set(("pointerdown pointerup pointercancel mousedown mouseup click gotpointercapture lostpointercapture " +
  "focusin focusout window-focus window-blur handler-enter handler-end selection-change rail-mount rail-dispose " +
  "section-mount section-dispose node-mount node-dispose project-list project-buttons favorites-entries reload-start " +
  "reload-end discover-start discover-end snapshot-start snapshot-end snapshots-start snapshots-end task-start task-end loop-start loop-end").split(" "));
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
    if (!["embedded", "sidebar", "store"].includes(surface) || !events.has(event)) return;
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

function groupKey(kind: Kind, id: string | null): string {
  const suffix = id === null ? "n" : `s${id}`;
  return `${kind}:${suffix}`;
}

export function reconcileRailGroups(path: string, groups: readonly string[], nonstop: boolean): void {
  const entry = entities.get(path);
  if (!entry) return;
  entry.id ??= railToken();
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

export interface RailGestureHost {
  aside: HTMLElement;
  metadata: WeakMap<Element, () => RailFields>;
  emit: (event: RailDiagnosticRecord["event"], fields?: RailFields) => void;
  selection: () => RailFields;
}
type Gesture = { node: Element; fields: RailFields; pointerId?: number };

function knownNode(host: RailGestureHost, target: EventTarget | null): Element | undefined {
  let node = target instanceof Element ? target : null;
  while (node && node !== host.aside) {
    if (host.metadata.has(node)) return node;
    node = node.parentElement;
  }
  return undefined;
}

function currentGesture(
  gestures: Map<number, Gesture>, key: number, pointerId: number | undefined,
): Gesture | undefined {
  const gesture = gestures.get(key);
  if (gesture || pointerId !== undefined) return gesture;
  const values = [...gestures.values()];
  return values[values.length - 1];
}

function startsPress(event: Event, gesture: Gesture | undefined): boolean {
  return event.type === "pointerdown" || (event.type === "mousedown" && !gesture);
}

function forgetGestures(
  gestures: Map<number, Gesture>, match: (id: number, candidate: Gesture) => boolean,
): void {
  for (const [id, candidate] of gestures) if (match(id, candidate)) gestures.delete(id);
}

function startGesture(
  host: RailGestureHost, gestures: Map<number, Gesture>, node: Element, key: number,
  pointerId: number | undefined,
): Gesture {
  const fields = { ...host.metadata.get(node)?.(), nodeId: railObjectId(node) };
  if (gestures.size >= 8 && !gestures.has(key)) {
    const oldest = gestures.keys().next().value!;
    gestures.delete(oldest);
    host.emit("pointercancel", { outcome: "overflow" });
  }
  const gesture = { node, fields, pointerId };
  gestures.set(key, gesture);
  return gesture;
}

function reportGesture(
  host: RailGestureHost, event: Event, pointerId: number | undefined,
  gesture: Gesture | undefined, node: Element | undefined,
): void {
  const pointer = event as PointerEvent;
  let captured = false;
  try {
    captured = gesture?.pointerId !== undefined
      && !!gesture.node.hasPointerCapture?.(gesture.pointerId);
  } catch {}
  host.emit(event.type as Parameters<typeof emitRailDiagnostic>[1], {
    ...(gesture?.fields ?? (node ? host.metadata.get(node)?.() : {})), ...host.selection(),
    pointerId, button: typeof pointer.button === "number" ? pointer.button : undefined,
    primary: typeof pointer.isPrimary === "boolean" ? pointer.isPrimary : undefined,
    trusted: event.isTrusted, targetTag: railTag(event.target),
    activeTag: railTag(document.activeElement),
    nodeId: railObjectId(node, false), originalNodeId: gesture?.fields.nodeId,
    connected: node?.isConnected, originalConnected: gesture?.node.isConnected, captured,
    documentFocused: document.hasFocus(),
    outcome: event.type === "click" && !gesture ? "no-press" : undefined,
  });
}

function observeGesture(host: RailGestureHost, gestures: Map<number, Gesture>, event: Event): void {
  if (!isDebugEnabled()) { gestures.clear(); return; }
  const pointer = event as PointerEvent;
  const pointerId = typeof pointer.pointerId === "number" ? pointer.pointerId : undefined;
  const key = pointerId ?? -1;
  const inside = event.target instanceof Node && host.aside.contains(event.target);
  let gesture = currentGesture(gestures, key, pointerId);
  if (!inside && (event.type === "pointerdown" || event.type === "mousedown")) {
    // A new outside press ends attribution to the prior rail gesture.
    const prior = gesture;
    forgetGestures(gestures, (id, candidate) => id === key || candidate === prior);
    return;
  }
  if (!inside && !gesture) return;
  const node = knownNode(host, event.target);
  if (startsPress(event, gesture)) {
    if (!inside) return;
    if (node) gesture = startGesture(host, gestures, node, key, pointerId);
  }
  reportGesture(host, event, pointerId, gesture, node);
  if (event.type === "click") {
    const ended = gesture;
    forgetGestures(gestures, (_id, candidate) => candidate === ended);
  }
}

export function createRailGestureTracker(host: RailGestureHost) {
  const gestures = new Map<number, Gesture>();
  return {
    observe: (event: Event) => observeGesture(host, gestures, event),
    size: () => gestures.size,
    clear: () => gestures.clear(),
  };
}
