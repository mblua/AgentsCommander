// @vitest-environment jsdom
import { afterAll, describe, expect, it } from "vitest";
import { setLogLevel, getConsoleLogs } from "./console-capture";
import type { LogLevel } from "./types";

// The module monkey-patches the global console + keeps ONE shared ring buffer,
// so assertions probe `getConsoleLogs()` with a unique marker per call rather
// than depending on cross-test ordering / accumulation.
let seq = 0;
function marker(tag: string): string {
  seq += 1;
  return `__cc_test_${tag}_${seq}__`;
}
function buffered(m: string): boolean {
  return getConsoleLogs().some((e) => e.args.includes(m));
}

afterAll(() => {
  setLogLevel("info"); // restore the module default for any later consumer
});

describe("console-capture severity gate (#612)", () => {
  it("Info: suppresses debug, keeps warn + error", () => {
    setLogLevel("info");
    const d = marker("debug");
    const w = marker("warn");
    const e = marker("error");
    console.debug(d);
    console.warn(w);
    console.error(e);
    expect(buffered(d)).toBe(false);
    expect(buffered(w)).toBe(true);
    expect(buffered(e)).toBe(true);
  });

  it("Info: keeps info + log (log is treated as info severity)", () => {
    setLogLevel("info");
    const i = marker("info");
    const lg = marker("log");
    console.info(i);
    console.log(lg);
    expect(buffered(i)).toBe(true);
    expect(buffered(lg)).toBe(true);
  });

  it("Trace: buffers debug (the widest gate admits everything below)", () => {
    setLogLevel("trace");
    const d = marker("debug");
    console.debug(d);
    expect(buffered(d)).toBe(true);
  });

  it("Error: drops warn + info + log, keeps error", () => {
    setLogLevel("error");
    const w = marker("warn");
    const i = marker("info");
    const lg = marker("log");
    const e = marker("error");
    console.warn(w);
    console.info(i);
    console.log(lg);
    console.error(e);
    expect(buffered(w)).toBe(false);
    expect(buffered(i)).toBe(false);
    expect(buffered(lg)).toBe(false);
    expect(buffered(e)).toBe(true);
  });

  it("flipping the level changes the verdict for the same method (runtime adjustability)", () => {
    setLogLevel("info");
    const suppressed = marker("debug_suppressed");
    console.debug(suppressed);
    expect(buffered(suppressed)).toBe(false);

    setLogLevel("debug");
    const admitted = marker("debug_admitted");
    console.debug(admitted);
    expect(buffered(admitted)).toBe(true);
  });

  it("an unknown level falls back to Info", () => {
    setLogLevel("totally-bogus" as LogLevel);
    const d = marker("debug");
    const i = marker("info");
    console.debug(d);
    console.info(i);
    expect(buffered(d)).toBe(false); // Info gate suppresses debug
    expect(buffered(i)).toBe(true);
  });
});
