// #4316 - frontend `invoke("<cmd>", args)` calls must match a registered Rust
// `#[tauri::command]` by name and by argument keys (Tauri 2 lowerCamelCase keys).
// Registered commands with no caller are printed, never failed.
//
// D3: commands served only by the browser/WS dispatcher (src-tauri/src/web/) are
// checked by NAME only, through the exact two-way WS_ONLY_COMMANDS list. Their keys
// are read by hand-written helpers, so their argument keys are not checked.
// F4 limit: for wrapper calls such as `withDraft({...}, draft)` only the inline
// object literal is analysed; keys added by the wrapper are not seen.
import { beforeAll, describe, expect, it } from "vitest";
import {
  checkContract,
  type ContractOptions,
  type ContractReport,
  type Sources,
} from "./testing/ipc-contract-scan";

const WS_ONLY_COMMANDS = ["broadcast_event", "get_pty_size", "subscribe_session"];
const FORWARDER_SITES: Record<string, number> = { "src/shared/ipc.ts": 1 };
const OPTS: ContractOptions = { wsOnly: WS_ONLY_COMMANDS, forwarders: FORWARDER_SITES };

const ROOT = new URL("../../", import.meta.url);

function normalize(glob: Record<string, string>): Sources {
  const out: Sources = {};
  for (const [key, text] of Object.entries(glob)) {
    out[new URL(key, import.meta.url).href.slice(ROOT.href.length)] = text;
  }
  return out;
}

const REAL_RUST = normalize(
  import.meta.glob<string>("../../src-tauri/src/**/*.rs", {
    query: "?raw",
    import: "default",
    eager: true,
  }),
);
const REAL_FRONTEND = normalize(
  import.meta.glob<string>("../**/*.{ts,tsx}", {
    query: "?raw",
    import: "default",
    eager: true,
  }),
);

describe("IPC contract (real tree)", () => {
  let report: ContractReport;

  beforeAll(() => {
    report = checkContract(REAL_RUST, REAL_FRONTEND, OPTS);
  }, 30000);

  it("has no frontend/backend command mismatches", () => {
    expect(report.failures).toEqual([]);
  });

  it("reports registered commands without a frontend caller", () => {
    console.info(
      `[ipc-contract] ${report.uncalled.length} uncalled registered commands: ` +
        report.uncalled.join(", "),
    );
    expect(Array.isArray(report.uncalled)).toBe(true);
  });

  it("scans enough of both sides to be meaningful", () => {
    const rustOnly = checkContract(REAL_RUST, {}, OPTS);
    expect(rustOnly.uncalled.length).toBeGreaterThanOrEqual(150);
    expect(report.uncalled.length).toBeLessThan(20);
  });
});

const RUST_FIXTURE = [
  "#[tauri::command]",
  "pub async fn pty_write(",
  "    state: State<'_, X>,",
  "    session_id: String,",
  "    rows: Option<u16>,",
  ") -> Result<(), String> {",
  "    Ok(())",
  "}",
  "fn run() { tauri::generate_handler![commands::pty::pty_write]; }",
].join("\n");
const RUST = { "src-tauri/src/commands/pty.rs": RUST_FIXTURE };
const NO_OPTS: ContractOptions = { wsOnly: [], forwarders: {} };

function fixture(ts: string, rust: Sources = RUST, opts = NO_OPTS): string[] {
  return checkContract(rust, { "src/fake.ts": ts }, opts).failures;
}

describe("IPC contract (negative controls)", () => {
  it("N1 fails on a command name mismatch", () => {
    expect(fixture('invoke("pty_wrte", { sessionId });')).toContain(
      'unknown command "pty_wrte" at src/fake.ts:1',
    );
  });

  it("N2 fails on an extra argument key", () => {
    expect(fixture('invoke("pty_write", { sessionId, session_id });')).toContain(
      'extra arg key "session_id" for "pty_write" at src/fake.ts:1',
    );
  });

  it("N3 fails on a missing required argument key", () => {
    expect(fixture('invoke("pty_write", { rows: 1 });')).toContain(
      'missing required arg key "sessionId" for "pty_write" at src/fake.ts:1',
    );
  });

  it("N4 accepts an omitted optional key", () => {
    expect(fixture('invoke("pty_write", { sessionId });')).toEqual([]);
  });

  it("N5 checks keys inside conditional spreads", () => {
    expect(fixture('invoke("pty_write", { sessionId, ...(c ? { bogus } : {}) });')).toContain(
      'extra arg key "bogus" for "pty_write" at src/fake.ts:1',
    );
    expect(fixture('invoke("pty_write", { sessionId, ...(c ? { rows } : {}) });')).toEqual([]);
  });

  it("N6 fails on arguments it cannot analyse", () => {
    expect(fixture('invoke("pty_write", args);')).toContain(
      'unanalyzable args for "pty_write" at src/fake.ts:1',
    );
  });

  it("N7 fails on a non-literal command name outside the forwarders", () => {
    expect(fixture("invoke(name, { sessionId });")).toContain(
      "non-literal command sites in src/fake.ts: found 1, allowed 0 (src/fake.ts:1)",
    );
  });

  it("N8 fails on an unsupported command attribute", () => {
    const rust = {
      "src-tauri/src/commands/pty.rs": RUST_FIXTURE.replace(
        "#[tauri::command]",
        '#[tauri::command(rename_all = "snake_case")]',
      ),
    };
    expect(fixture('invoke("pty_write", { sessionId });', rust)).toContain(
      "unsupported command attribute at src-tauri/src/commands/pty.rs:1",
    );
  });

  it("N9 fails on a ws-only entry with no dispatcher arm", () => {
    const opts: ContractOptions = { wsOnly: ["get_pty_size"], forwarders: {} };
    expect(fixture('invoke("get_pty_size", { sessionId });', RUST, opts)).toContain(
      'ws-only command "get_pty_size" has no web dispatcher arm',
    );
  });

  it("N10a fails on a real-source command name mutation", () => {
    const file = "src/shared/ipc.ts";
    const original = REAL_FRONTEND[file];
    const mutated = original.replace('"pty_write"', '"pty_wrte"');
    expect(mutated).not.toBe(original);
    const failures = checkContract(REAL_RUST, { ...REAL_FRONTEND, [file]: mutated }, OPTS)
      .failures;
    expect(failures.some((f) => f.startsWith('unknown command "pty_wrte"'))).toBe(true);
  });

  it("N10b fails on a real-source argument key mutation", () => {
    const file = "src-tauri/src/commands/pty.rs";
    const original = REAL_RUST[file];
    const start = original.indexOf("fn pty_write(");
    expect(start).toBeGreaterThanOrEqual(0);
    const mutated =
      original.slice(0, start) +
      original.slice(start).replace("session_id: String", "sid: String");
    expect(mutated).not.toBe(original);
    const failures = checkContract({ ...REAL_RUST, [file]: mutated }, REAL_FRONTEND, OPTS)
      .failures;
    const has = (text: string): boolean => failures.some((f) => f.includes(text));
    expect(has('missing required arg key "sid" for "pty_write"')).toBe(true);
    expect(has('extra arg key "sessionId" for "pty_write"')).toBe(true);
  });
});
