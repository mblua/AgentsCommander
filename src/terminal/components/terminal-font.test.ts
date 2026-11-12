import { describe, expect, it } from "vitest";
import {
  DEFAULT_TERMINAL_FONT_FAMILY,
  isMonospaceFamily,
  resolveTerminalFontFamily,
  type TextWidth,
} from "./terminal-font";

const fake =
  (installed: string[]): TextWidth =>
  (font, text) => {
    if (installed.some((family) => font.includes(`'${family}'`))) return 8.4;
    return text === "l" || text === "i" ? 3.11 : 13.21;
  };

describe("resolveTerminalFontFamily", () => {
  it("T1: keeps today's string when all preferred fonts are installed", () => {
    const width = fake(["Cascadia Code", "JetBrains Mono", "Fira Code"]);
    expect(resolveTerminalFontFamily(width)).toBe(DEFAULT_TERMINAL_FONT_FAMILY);
  });

  it("T2: only JetBrains Mono installed", () => {
    expect(resolveTerminalFontFamily(fake(["JetBrains Mono"]))).toBe(
      "'JetBrains Mono', monospace"
    );
  });

  it("T3: does not lead with a missing Cascadia Code", () => {
    const result = resolveTerminalFontFamily(fake(["JetBrains Mono", "Fira Code"]));
    expect(result.startsWith("'Cascadia Code'")).toBe(false);
  });

  it("T4: none installed gives monospace; no measurer gives today's string", () => {
    expect(resolveTerminalFontFamily(fake([]))).toBe("monospace");
    expect(resolveTerminalFontFamily(null)).toBe(DEFAULT_TERMINAL_FONT_FAMILY);
  });
});

describe("isMonospaceFamily", () => {
  it("T5: one char with a different width is not monospace", () => {
    const width: TextWidth = (_font, text) => (text === "m" ? 9.0 : 8.4);
    expect(isMonospaceFamily("Any", width)).toBe(false);
  });

  it("T6: zero widths are not monospace", () => {
    expect(isMonospaceFamily("Any", () => 0)).toBe(false);
  });
});
