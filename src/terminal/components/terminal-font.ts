// #4244: WebKitGTK uses fontconfig's substitute (e.g. Liberation Sans) for a
// missing first family and never tries the rest, so keep only families that
// really measure as monospace here.
export const PREFERRED_TERMINAL_FONTS = ["Cascadia Code", "JetBrains Mono", "Fira Code"] as const;
export const DEFAULT_TERMINAL_FONT_FAMILY =
  "'Cascadia Code', 'JetBrains Mono', 'Fira Code', monospace";

const PROBE_CHARS = ["l", "W", "m", "i"];
const PROBE_SIZE = "14px";
const TOLERANCE_PX = 0.01;

export type TextWidth = (font: string, text: string) => number;

export const isMonospaceFamily = (family: string, width: TextWidth): boolean => {
  const widths = PROBE_CHARS.map((ch) => width(`${PROBE_SIZE} '${family}'`, ch));
  const first = widths[0] ?? 0; // PROBE_CHARS is non-empty; 0 fails the check as undefined did
  return first > 0 && widths.every((w) => Math.abs(w - first) < TOLERANCE_PX);
};

export const resolveTerminalFontFamily = (width: TextWidth | null): string => {
  if (!width) return DEFAULT_TERMINAL_FONT_FAMILY;
  const usable = PREFERRED_TERMINAL_FONTS.filter((f) => isMonospaceFamily(f, width));
  if (usable[0] === PREFERRED_TERMINAL_FONTS[0]) return DEFAULT_TERMINAL_FONT_FAMILY;
  return [...usable.map((f) => `'${f}'`), "monospace"].join(", ");
};

export const canvasTextWidth = (): TextWidth | null => {
  try {
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return null;
    return (font, text) => {
      ctx.font = font;
      return ctx.measureText(text).width;
    };
  } catch {
    return null;
  }
};
