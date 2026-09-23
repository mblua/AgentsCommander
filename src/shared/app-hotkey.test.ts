import { describe, expect, it } from "vitest";
import {
  hotkeyEventCode,
  letterFromCaptureEvent,
  matchesHotkeyEvent,
  parseAppHotkey,
} from "./app-hotkey";

// Copied verbatim from phase 4 (`settings.rs` PARITY_ACCEPTED / PARITY_REJECTED).
const PARITY_ACCEPTED = [
  "Ctrl+Shift+A", "Ctrl+Shift+B", "Ctrl+Shift+D", "Ctrl+Shift+E", "Ctrl+Shift+F", "Ctrl+Shift+G",
  "Ctrl+Shift+H", "Ctrl+Shift+I", "Ctrl+Shift+J", "Ctrl+Shift+K", "Ctrl+Shift+L", "Ctrl+Shift+M",
  "Ctrl+Shift+N", "Ctrl+Shift+O", "Ctrl+Shift+P", "Ctrl+Shift+Q", "Ctrl+Shift+S", "Ctrl+Shift+T",
  "Ctrl+Shift+U", "Ctrl+Shift+X", "Ctrl+Shift+Y", "Ctrl+Shift+Z",
  "ctrl+shift+e", "CTRL+SHIFT+E", "Control+Shift+Z", " Ctrl + Shift + E ",
];
const PARITY_REJECTED = [
  "Ctrl+Shift+W", "Ctrl+Shift+R", "Ctrl+Shift+C", "Ctrl+Shift+V", "ctrl+shift+w", "Ctrl+Shift+2",
  "Ctrl+E", "Shift+E", "Alt+Shift+E", "Ctrl+Shift+EE", "Ctrl+Shift+", "", "Ctrl+Shift+\u{212A}",
];

function keydown(key: string, code: string, init: KeyboardEventInit = {}): KeyboardEvent {
  return { key, code, ctrlKey: true, shiftKey: true, altKey: false, metaKey: false,
    repeat: false, isComposing: false, ...init } as KeyboardEvent;
}

describe("app-hotkey (#2236 phase 5)", () => {
  it("accepts all 26 PARITY_ACCEPTED entries", () => {
    expect(PARITY_ACCEPTED).toHaveLength(26);
    for (const value of PARITY_ACCEPTED) expect(parseAppHotkey(value), value).not.toBeNull();
  });

  it("rejects all 13 PARITY_REJECTED entries", () => {
    expect(PARITY_REJECTED).toHaveLength(13);
    for (const value of PARITY_REJECTED) expect(parseAppHotkey(value), value).toBeNull();
  });

  it("rejects Kelvin because the letter is not validated with toLowerCase()", () => {
    expect("\u{212A}".toLowerCase()).toBe("k");
    expect(parseAppHotkey("Ctrl+Shift+\u{212A}")).toBeNull();
  });

  it("maps a letter to its physical code", () => {
    expect(hotkeyEventCode({ letter: "e" })).toBe("KeyE");
  });

  it.each([
    ["US", "E"], ["Cyrillic", "\u0423"], ["Dvorak", ">"], ["AZERTY", "E"],
  ])("matches the physical E key on %s", (_layout, key) => {
    expect(matchesHotkeyEvent(keydown(key, "KeyE"), "Ctrl+Shift+E")).toBe(true);
  });

  it("lets a reserved key win over the code (D8b)", () => {
    expect(matchesHotkeyEvent(keydown("w", "KeyZ"), "Ctrl+Shift+Z")).toBe(false);
    expect(matchesHotkeyEvent(keydown("c", "KeyI"), "Ctrl+Shift+I")).toBe(false);
  });

  it("classifies capture events", () => {
    expect(letterFromCaptureEvent(keydown("w", "KeyZ"))).toHaveProperty("error");
    expect(letterFromCaptureEvent(keydown("c", "KeyI"))).toHaveProperty("error");
    expect(letterFromCaptureEvent(keydown("@", "Digit2"))).toBeNull();
    expect(letterFromCaptureEvent(keydown("E", "KeyE"))).toEqual({ letter: "E" });
  });

  it("ignores repeat and composition", () => {
    expect(matchesHotkeyEvent(keydown("E", "KeyE", { repeat: true }), "Ctrl+Shift+E")).toBe(false);
    expect(matchesHotkeyEvent(keydown("E", "KeyE", { isComposing: true }), "Ctrl+Shift+E")).toBe(false);
  });
});
