import type { LogLevel } from "./types";

const MAX_ENTRIES = 500;

interface LogEntry {
  ts: string;
  level: "log" | "warn" | "error" | "info" | "debug" | "trace";
  args: string;
}

const captured: LogEntry[] = [];

// #612 Severity ranking for the runtime log-level gate. NOTE: these integers are
// a FRONTEND-LOCAL ordering (error=0 .. trace=4) and are intentionally NOT the
// backend's `log::Level` u8 (Error=1 .. Trace=5). A console.* call is suppressed
// (neither buffered nor forwarded to the real console) when its severity is
// greater than the current level. console.log is treated as `info`.
const SEVERITY: Record<LogLevel, number> = {
  error: 0,
  warn: 1,
  info: 2,
  debug: 3,
  trace: 4,
};

let currentLevel = SEVERITY.info; // default Info

/** #612 Set the active console verbosity. Called by `initLogLevelForWindow`
 *  from the persisted setting and the live `log_level_changed` event. An
 *  unknown value falls back to Info. */
export function setLogLevel(level: LogLevel): void {
  currentLevel = SEVERITY[level] ?? SEVERITY.info;
}

function timestamp(): string {
  const d = new Date();
  return d.toLocaleTimeString("en-GB", { hour12: false }) + "." + String(d.getMilliseconds()).padStart(3, "0");
}

function serialize(args: unknown[]): string {
  return args
    .map((a) => {
      if (typeof a === "string") return a;
      try {
        return JSON.stringify(a, null, 2);
      } catch {
        return String(a);
      }
    })
    .join(" ");
}

function capture(
  level: LogEntry["level"],
  severity: number,
  origFn: (...args: unknown[]) => void,
  args: unknown[]
) {
  // #612 Gate BOTH the ring buffer AND the real-console passthrough. At the
  // default Info level the downgraded [idle-fe] debug chatter is dropped, so it
  // neither floods the 500-entry buffer nor evicts real errors from "copy logs".
  // Errors (severity 0) survive every selectable level. To capture debug/trace,
  // raise the level and reproduce (consistent with the backend gate).
  if (severity > currentLevel) return;
  const entry: LogEntry = { ts: timestamp(), level, args: serialize(args) };
  captured.push(entry);
  if (captured.length > MAX_ENTRIES) captured.shift();
  origFn.apply(console, args);
}

const origLog = console.log;
const origWarn = console.warn;
const origError = console.error;
const origInfo = console.info;
const origDebug = console.debug;
const origTrace = console.trace;

console.log = (...args: unknown[]) => capture("log", SEVERITY.info, origLog, args);
console.info = (...args: unknown[]) => capture("info", SEVERITY.info, origInfo, args);
console.warn = (...args: unknown[]) => capture("warn", SEVERITY.warn, origWarn, args);
console.error = (...args: unknown[]) => capture("error", SEVERITY.error, origError, args);
console.debug = (...args: unknown[]) => capture("debug", SEVERITY.debug, origDebug, args);
console.trace = (...args: unknown[]) => capture("trace", SEVERITY.trace, origTrace, args);

// Also capture unhandled errors and promise rejections. These are ERROR-level
// signals and bypass the #612 severity gate entirely (always captured).
window.addEventListener("error", (e) => {
  const msg = `${e.message} at ${e.filename}:${e.lineno}:${e.colno}`;
  captured.push({ ts: timestamp(), level: "error", args: msg });
});

window.addEventListener("unhandledrejection", (e) => {
  const msg = `Unhandled rejection: ${e.reason}`;
  captured.push({ ts: timestamp(), level: "error", args: msg });
});

export function getConsoleLogs(): LogEntry[] {
  return [...captured];
}

export function getConsoleText(): string {
  return captured
    .map((e) => `[${e.ts}] [${e.level.toUpperCase()}] ${e.args}`)
    .join("\n");
}

export function getErrorsOnly(): string {
  return captured
    .filter((e) => e.level === "error" || e.level === "warn")
    .map((e) => `[${e.ts}] [${e.level.toUpperCase()}] ${e.args}`)
    .join("\n");
}

export async function copyConsoleLogs(): Promise<number> {
  const text = getConsoleText();
  await navigator.clipboard.writeText(text);
  return captured.length;
}

export async function copyErrors(): Promise<number> {
  const errors = captured.filter((e) => e.level === "error" || e.level === "warn");
  const text = getErrorsOnly();
  await navigator.clipboard.writeText(text);
  return errors.length;
}
