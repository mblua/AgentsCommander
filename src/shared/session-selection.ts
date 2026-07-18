import type {
  SessionSelection,
  SessionSelectionCause,
  SessionSelectionMode,
} from "./types";

const SELECTION_KEYS = [
  "epoch",
  "source",
  "userInitiated",
  "revision",
  "mode",
  "id",
  "status",
  "hasPty",
  "detached",
  "displayable",
] as const;

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const I32_MIN = -2_147_483_648;
const I32_MAX = 2_147_483_647;

function fail(path: string, detail: string): never {
  throw new Error(`Invalid session selection at ${path}: ${detail}`);
}

function requirePlainDataRecord(
  value: unknown,
  path: string,
  exactKeys: readonly string[],
): object {
  if (typeof value !== "object" || value === null) {
    return fail(path, "expected a plain object");
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    return fail(path, "expected a plain object prototype");
  }
  const keys = Reflect.ownKeys(value);
  if (
    keys.length !== exactKeys.length ||
    keys.some((key) => typeof key !== "string") ||
    exactKeys.some((key) => !Object.prototype.hasOwnProperty.call(value, key))
  ) {
    return fail(path, `expected exact keys ${exactKeys.join(",")}`);
  }
  for (const key of exactKeys) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor || !("value" in descriptor)) {
      return fail(`${path}.${key}`, "accessor-backed properties are not allowed");
    }
  }
  return value;
}

function dataValue(record: object, key: string): unknown {
  const descriptor = Object.getOwnPropertyDescriptor(record, key);
  if (!descriptor || !("value" in descriptor)) {
    return fail(key, "missing own data property");
  }
  return descriptor.value;
}

function requireUuid(value: unknown, path: string): string {
  if (typeof value !== "string" || !UUID_PATTERN.test(value)) {
    return fail(path, "expected a canonical lowercase UUID");
  }
  return value;
}

function requireBoolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") return fail(path, "expected a boolean");
  return value;
}

function requireMode(value: unknown): SessionSelectionMode {
  if (value === "none" || value === "live" || value === "dormant") return value;
  return fail("mode", "expected none, live, or dormant");
}

function requireRevision(value: unknown): number {
  if (!Number.isSafeInteger(value) || typeof value !== "number" || value < 0) {
    return fail("revision", "expected a nonnegative safe integer");
  }
  return value;
}

function requireLiteral<T extends string | boolean | null>(
  value: unknown,
  expected: T,
  path: string,
): T {
  if (value !== expected) return fail(path, `expected literal ${String(expected)}`);
  return expected;
}

function decodeCause(
  sourceValue: unknown,
  userValue: unknown,
  mode: SessionSelectionMode,
  revision: number,
): SessionSelectionCause {
  const userInitiated = requireBoolean(userValue, "userInitiated");
  if (sourceValue !== "initialHydration" && revision === 0) {
    return fail("revision", "revision 0 is reserved for initialHydration");
  }
  switch (sourceValue) {
    case "initialHydration":
      if (mode !== "none" || userInitiated || revision !== 0) {
        return fail("source", "initialHydration requires none/false/revision 0");
      }
      return { source: "initialHydration", userInitiated: false, mode: "none" };
    case "sessionCreated":
      if (mode !== "live") return fail("source", "sessionCreated requires live");
      return { source: "sessionCreated", userInitiated, mode: "live" };
    case "userSwitch":
      if (!userInitiated || (mode !== "live" && mode !== "dormant")) {
        return fail("source", "userSwitch requires live|dormant and true");
      }
      return mode === "live"
        ? { source: "userSwitch", userInitiated: true, mode: "live" }
        : { source: "userSwitch", userInitiated: true, mode: "dormant" };
    case "manualClose":
      if (!userInitiated || (mode !== "live" && mode !== "none")) {
        return fail("source", "manualClose requires live|none and true");
      }
      return mode === "live"
        ? { source: "manualClose", userInitiated: true, mode: "live" }
        : { source: "manualClose", userInitiated: true, mode: "none" };
    case "autoClose":
      if (userInitiated || mode !== "none") return fail("source", "autoClose requires none/false");
      return { source: "autoClose", userInitiated: false, mode: "none" };
    case "restart":
      if (mode !== "live" && mode !== "none") return fail("source", "restart requires live|none");
      return mode === "live"
        ? { source: "restart", userInitiated, mode: "live" }
        : { source: "restart", userInitiated, mode: "none" };
    case "restore":
      if (userInitiated) return fail("source", "restore requires false");
      if (mode === "live") return { source: "restore", userInitiated: false, mode: "live" };
      if (mode === "dormant") return { source: "restore", userInitiated: false, mode: "dormant" };
      return { source: "restore", userInitiated: false, mode: "none" };
    case "detach":
      if (!userInitiated || (mode !== "live" && mode !== "none")) {
        return fail("source", "detach requires live|none and true");
      }
      return mode === "live"
        ? { source: "detach", userInitiated: true, mode: "live" }
        : { source: "detach", userInitiated: true, mode: "none" };
    case "attach":
      if (!userInitiated || (mode !== "live" && mode !== "dormant")) {
        return fail("source", "attach requires live|dormant and true");
      }
      return mode === "live"
        ? { source: "attach", userInitiated: true, mode: "live" }
        : { source: "attach", userInitiated: true, mode: "dormant" };
    case "spawnRollback":
      if (userInitiated || mode !== "none") return fail("source", "spawnRollback requires none/false");
      return { source: "spawnRollback", userInitiated: false, mode: "none" };
    case "resourceMonitor":
      if (mode !== "none") return fail("source", "resourceMonitor requires none");
      return { source: "resourceMonitor", userInitiated, mode: "none" };
    case "backgroundCleanup":
      if (userInitiated || mode !== "none") return fail("source", "backgroundCleanup requires none/false");
      return { source: "backgroundCleanup", userInitiated: false, mode: "none" };
    case "livenessReconcile":
      if (userInitiated || (mode !== "dormant" && mode !== "none")) {
        return fail("source", "livenessReconcile requires dormant|none and false");
      }
      return mode === "dormant"
        ? { source: "livenessReconcile", userInitiated: false, mode: "dormant" }
        : { source: "livenessReconcile", userInitiated: false, mode: "none" };
    default:
      return fail("source", "unknown selection source");
  }
}

function decodeExitedStatus(value: unknown): { exited: number } {
  const record = requirePlainDataRecord(value, "status", ["exited"]);
  const exited = dataValue(record, "exited");
  if (
    typeof exited !== "number" ||
    !Number.isInteger(exited) ||
    exited < I32_MIN ||
    exited > I32_MAX
  ) {
    return fail("status.exited", "expected a signed 32-bit integer");
  }
  return { exited };
}

/** Decode and normalize the sole untrusted selection transport contract. */
export function decodeSessionSelection(value: unknown): SessionSelection {
  const record = requirePlainDataRecord(value, "selection", SELECTION_KEYS);
  const epoch = requireUuid(dataValue(record, "epoch"), "epoch");
  const revision = requireRevision(dataValue(record, "revision"));
  const mode = requireMode(dataValue(record, "mode"));
  const cause = decodeCause(
    dataValue(record, "source"),
    dataValue(record, "userInitiated"),
    mode,
    revision,
  );

  if (cause.mode === "none") {
    const normalized: SessionSelection = {
      epoch,
      revision,
      ...cause,
      id: requireLiteral(dataValue(record, "id"), null, "id"),
      status: requireLiteral(dataValue(record, "status"), null, "status"),
      hasPty: requireLiteral(dataValue(record, "hasPty"), false, "hasPty"),
      detached: requireLiteral(dataValue(record, "detached"), false, "detached"),
      displayable: requireLiteral(dataValue(record, "displayable"), false, "displayable"),
    };
    return normalized;
  }

  const id = requireUuid(dataValue(record, "id"), "id");
  if (cause.mode === "live") {
    const normalized: SessionSelection = {
      epoch,
      revision,
      ...cause,
      id,
      status: requireLiteral(dataValue(record, "status"), "active", "status"),
      hasPty: requireLiteral(dataValue(record, "hasPty"), true, "hasPty"),
      detached: requireLiteral(dataValue(record, "detached"), false, "detached"),
      displayable: requireLiteral(dataValue(record, "displayable"), true, "displayable"),
    };
    return normalized;
  }

  const normalized: SessionSelection = {
    epoch,
    revision,
    ...cause,
    id,
    status: decodeExitedStatus(dataValue(record, "status")),
    hasPty: requireBoolean(dataValue(record, "hasPty"), "hasPty"),
    detached: requireLiteral(dataValue(record, "detached"), false, "detached"),
    displayable: requireLiteral(dataValue(record, "displayable"), false, "displayable"),
  };
  return normalized;
}
