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

interface CauseRule {
  /** Allowed modes; null = any mode. */
  modes: readonly SessionSelectionMode[] | null;
  /** Required userInitiated value (also the value returned); null = any value. */
  userInitiated: boolean | null;
  /** Only initialHydration also requires revision 0. */
  revisionZero: boolean;
  message: string;
}

/** One rule per selection source; mirrors the SessionSelectionCause union. */
const CAUSE_RULES: ReadonlyMap<string, CauseRule> = new Map<string, CauseRule>([
  ["initialHydration", { modes: ["none"], userInitiated: false, revisionZero: true, message: "initialHydration requires none/false/revision 0" }],
  ["sessionCreated", { modes: ["live"], userInitiated: null, revisionZero: false, message: "sessionCreated requires live" }],
  ["userSwitch", { modes: ["live", "dormant"], userInitiated: true, revisionZero: false, message: "userSwitch requires live|dormant and true" }],
  ["manualClose", { modes: ["live", "none"], userInitiated: true, revisionZero: false, message: "manualClose requires live|none and true" }],
  ["autoClose", { modes: ["none"], userInitiated: false, revisionZero: false, message: "autoClose requires none/false" }],
  ["restart", { modes: ["live", "none"], userInitiated: null, revisionZero: false, message: "restart requires live|none" }],
  ["restore", { modes: null, userInitiated: false, revisionZero: false, message: "restore requires false" }],
  ["detach", { modes: ["live", "none"], userInitiated: true, revisionZero: false, message: "detach requires live|none and true" }],
  ["attach", { modes: ["live", "dormant"], userInitiated: true, revisionZero: false, message: "attach requires live|dormant and true" }],
  ["spawnRollback", { modes: ["none"], userInitiated: false, revisionZero: false, message: "spawnRollback requires none/false" }],
  ["resourceMonitor", { modes: ["none"], userInitiated: null, revisionZero: false, message: "resourceMonitor requires none" }],
  ["backgroundCleanup", { modes: ["none"], userInitiated: false, revisionZero: false, message: "backgroundCleanup requires none/false" }],
  ["livenessReconcile", { modes: ["dormant", "none"], userInitiated: false, revisionZero: false, message: "livenessReconcile requires dormant|none and false" }],
]);

function violatesCauseRule(
  rule: CauseRule,
  userInitiated: boolean,
  mode: SessionSelectionMode,
  revision: number,
): boolean {
  if (rule.userInitiated !== null && userInitiated !== rule.userInitiated) return true;
  if (rule.modes !== null && !rule.modes.includes(mode)) return true;
  return rule.revisionZero && revision !== 0;
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
  const rule = typeof sourceValue === "string" ? CAUSE_RULES.get(sourceValue) : undefined;
  if (!rule) return fail("source", "unknown selection source");
  if (violatesCauseRule(rule, userInitiated, mode, revision)) return fail("source", rule.message);
  // CAUSE_RULES admits exactly the source/userInitiated/mode combinations of the union.
  return {
    source: sourceValue,
    userInitiated: rule.userInitiated ?? userInitiated,
    mode,
  } as SessionSelectionCause;
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
