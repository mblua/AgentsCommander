// @vitest-environment jsdom
//
// #2236 phase 5 — what a REAL xterm Terminal can prove. It does not assert
// "no bytes for Ctrl+Shift+E": an unvetoed build produces no bytes either.
import { readFileSync } from "node:fs";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import { parseAppHotkey } from "../../shared/app-hotkey";

const moduleUrl = import.meta.url;

beforeAll(() => {
  window.matchMedia ??= ((query: string) => ({
    matches: false, media: query, onchange: null,
    addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
});

let terminal: Terminal | null = null;
afterEach(() => {
  terminal?.dispose();
  terminal = null;
  document.body.innerHTML = "";
});

function bytesFor(init: KeyboardEventInit): number[] {
  const host = document.createElement("div");
  document.body.append(host);
  terminal = new Terminal();
  terminal.open(host);
  const bytes: number[] = [];
  terminal.onData((data) => bytes.push(...Array.from(data, (c) => c.charCodeAt(0))));
  const textarea = host.querySelector("textarea");
  if (!textarea) throw new Error("real xterm rendered no textarea");
  textarea.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
  return bytes;
}

describe("real xterm keyboard facts (#2236 D8)", () => {
  it("positive control: Ctrl+E writes 0x05", () => {
    expect(bytesFor({ key: "e", code: "KeyE", keyCode: 69, ctrlKey: true })).toEqual([0x05]);
  });

  it("digit hazard: the US Ctrl+Shift+2 triple writes NUL with no veto", () => {
    expect(
      bytesFor({ key: "@", code: "Digit2", keyCode: 50, ctrlKey: true, shiftKey: true }),
    ).toEqual([0x00]);
  });

  it("mitigation: Ctrl+Shift+2 can never be configured", () => {
    expect(parseAppHotkey("Ctrl+Shift+2")).toBeNull();
  });

  it("rot tripwire: installed @xterm/xterm matches package-lock.json", () => {
    const lock = JSON.parse(readFileSync(new URL("../../../package-lock.json", moduleUrl), "utf8"));
    const installed = JSON.parse(readFileSync(new URL("../../../node_modules/@xterm/xterm/package.json", moduleUrl), "utf8"));
    expect(installed.version).toBe(lock.packages["node_modules/@xterm/xterm"].version);
  });
});
