import { describe, it, expect } from "vitest";
import { validateScreenshotHotkeySyntax } from "./screenshot-hotkey";

describe("validateScreenshotHotkeySyntax", () => {
  it("accepts one Ctrl/Control modifier plus one alphanumeric key", () => {
    for (const value of ["Ctrl+Q", "ctrl+q", "Control+A", "control+z", "Ctrl+1"]) {
      expect(validateScreenshotHotkeySyntax(value)).toBeNull();
    }
  });

  it("trims surrounding whitespace before validating", () => {
    expect(validateScreenshotHotkeySyntax("  Ctrl+Q  ")).toBeNull();
  });

  it("reports empty / whitespace-only as required", () => {
    expect(validateScreenshotHotkeySyntax("")).toBe("Screenshot hotkey is required");
    expect(validateScreenshotHotkeySyntax("   ")).toBe(
      "Screenshot hotkey is required"
    );
  });

  it("rejects a bare key with no modifier", () => {
    expect(validateScreenshotHotkeySyntax("Q")).toBe(
      "Screenshot hotkey must look like Ctrl+Q"
    );
  });

  it("rejects a second modifier (matches the backend MVP)", () => {
    expect(validateScreenshotHotkeySyntax("Ctrl+Shift+Q")).toBe(
      "Screenshot hotkey must look like Ctrl+Q"
    );
  });

  it("rejects unsupported modifiers and multi-char / non-alphanumeric keys", () => {
    for (const value of ["Alt+Q", "Shift+Q", "Ctrl+", "Ctrl+F1", "Ctrl+QQ", "Ctrl++"]) {
      expect(validateScreenshotHotkeySyntax(value)).toBe(
        "Screenshot hotkey must look like Ctrl+Q"
      );
    }
  });
});
