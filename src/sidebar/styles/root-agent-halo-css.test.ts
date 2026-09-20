import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #2277 (epic #2236) — the V3 halo on the selected Root Agent banner, and the
// rail width token. jsdom never applies this stylesheet, so the bytes on disk
// are the only place the declarations can be pinned. Both stylesheets are CRLF
// on disk: line-based reads below split on /\r?\n/ and never on a bare \n.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const VARS = readFileSync(new URL("./variables.css", import.meta.url), "utf8");

interface CssRule {
  selectors: string[];
  body: string;
}

/**
 * Every `selector-list { body }` pair in a sheet, comments removed first. A
 * rule inside an @media wrapper is found on its own; the wrapper itself never
 * matches because its body contains braces.
 */
const scan = (text: string): CssRule[] => {
  const found = [...text.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(
    (m) => ({ selectors: m[1].split(",").map((s) => s.trim()), body: m[2] })
  );
  if (found.length === 0) throw new Error("no CSS rules found in the stylesheet");
  return found;
};

/** Rules carrying `selector` verbatim — exact equality, never a substring. */
const rulesFor = (text: string, selector: string): CssRule[] =>
  scan(text).filter((rule) => rule.selectors.includes(selector));

/** Parsed occurrences, so a duplicated entry in one selector list cannot hide. */
const hits = (text: string, selector: string): number =>
  scan(text).reduce((n, rule) => n + rule.selectors.filter((s) => s === selector).length, 0);

/** The only rule carrying `selector` verbatim; throws on any other count. */
function one(text: string, selector: string): CssRule {
  const rules = rulesFor(text, selector);
  if (rules.length !== 1 || hits(text, selector) !== 1) {
    throw new Error(`expected exactly one rule carrying ${selector}`);
  }
  return rules[0];
}

/** Bodies of every block carrying `selector` verbatim, in file order. */
function merged(text: string, selector: string): string {
  const rules = rulesFor(text, selector);
  if (rules.length === 0) throw new Error(`no rule with selector ${selector}`);
  return rules.map((rule) => rule.body).join(";");
}

/** `prop: value` pairs of a rule body, in source order, whitespace-normalized. */
const pairs = (body: string): Array<[string, string]> =>
  [...body.matchAll(/(?:^|;)\s*([\w-]+)\s*:\s*([^;]*)/g)].map((m) => [
    m[1],
    m[2].trim().replace(/\s+/g, " "),
  ]);

const propsOf = (body: string): string[] => pairs(body).map(([prop]) => prop);

/** Last declaration of `prop`, mirroring the within-rule cascade. Throws on a miss. */
const valueOf = (body: string, prop: string): string => {
  const hit = pairs(body).filter(([name]) => name === prop).pop();
  if (hit === undefined) throw new Error(`missing declaration: ${prop}`);
  return hit[1];
};

/** Every effective declaration of a body; a later duplicate replaces an earlier one. */
const valuesOf = (body: string): Map<string, string> => new Map(pairs(body));

/** Statement-level scan: trimmed lines that declare `token`. */
const declaredLines = (text: string, token: string): string[] =>
  text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith(`${token}:`));

const squeeze = (value: string): string => value.replace(/\s+/g, "");

const BANNER_ACTIVE = ".root-agent-banner.active";
const BANNER_ACTIVE_BEFORE = ".root-agent-banner.active::before";
const banner = (style: string): string => `[data-sidebar-style="${style}"] .root-agent-banner`;
const bannerBefore = (style: string): string =>
  `[data-sidebar-style="${style}"] .root-agent-banner.active::before`;

// ---------------------------------------------------------------------------
// Border / margin arithmetic for assertion 8. The expected offsets are
// computed from the declarations each style's own banner rule carries, so a
// future change to a border or a margin turns into a failing offset here.
// ---------------------------------------------------------------------------

interface BoxWidths {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

function parsePx(value: string): number | null {
  const m = value.match(/^(-?\d+(?:\.\d+)?)px$/);
  return m ? Number(m[1]) : null;
}

function lengthOrThrow(value: string, what: string): number {
  const px = parsePx(value);
  if (px === null) throw new Error(`unsupported ${what}: ${value}`);
  return px;
}

/** The width a single border declaration contributes; `none` contributes 0. */
function borderWidth(value: string): number {
  const first = value.trim().split(/\s+/)[0];
  if (first === "none" || first === "0") return 0;
  return lengthOrThrow(first, "border width");
}

function borderWidths(body: string): BoxWidths {
  const out: BoxWidths = { top: 0, right: 0, bottom: 0, left: 0 };
  for (const [prop, value] of pairs(body)) {
    if (prop === "border") {
      const width = borderWidth(value);
      out.top = width;
      out.right = width;
      out.bottom = width;
      out.left = width;
    } else if (prop === "border-left") out.left = borderWidth(value);
    else if (prop === "border-right") out.right = borderWidth(value);
    else if (prop === "border-top") out.top = borderWidth(value);
    else if (prop === "border-bottom") out.bottom = borderWidth(value);
  }
  return out;
}

function marginWidths(body: string): BoxWidths {
  const out: BoxWidths = { top: 0, right: 0, bottom: 0, left: 0 };
  for (const [prop, value] of pairs(body)) {
    if (prop === "margin") {
      const parts = value.split(/\s+/).map((p) => lengthOrThrow(p, "margin width"));
      if (parts.length === 1) {
        out.top = out.right = out.bottom = out.left = parts[0];
      } else if (parts.length === 2) {
        out.top = out.bottom = parts[0];
        out.right = out.left = parts[1];
      } else if (parts.length === 3) {
        out.top = parts[0];
        out.right = out.left = parts[1];
        out.bottom = parts[2];
      } else if (parts.length === 4) {
        out.top = parts[0];
        out.right = parts[1];
        out.bottom = parts[2];
        out.left = parts[3];
      } else {
        throw new Error(`unsupported margin: ${value}`);
      }
    } else if (prop === "margin-left") out.left = lengthOrThrow(value, "margin width");
    else if (prop === "margin-right") out.right = lengthOrThrow(value, "margin width");
    else if (prop === "margin-top") out.top = lengthOrThrow(value, "margin width");
    else if (prop === "margin-bottom") out.bottom = lengthOrThrow(value, "margin width");
  }
  return out;
}

describe("#2277 root agent halo CSS bytes", () => {
  it("1. .root-agent-banner.active is one rule adding position and isolation", () => {
    expect(hits(CSS, BANNER_ACTIVE)).toBe(1);
    const body = one(CSS, BANNER_ACTIVE).body;
    expect(valueOf(body, "position")).toBe("relative");
    expect(valueOf(body, "isolation")).toBe("isolate");
  });

  it("2. the ::before overlay is one rule declaring the out-of-flow overlay", () => {
    expect(hits(CSS, BANNER_ACTIVE_BEFORE)).toBe(1);
    const body = one(CSS, BANNER_ACTIVE_BEFORE).body;
    expect(valueOf(body, "position")).toBe("absolute");
    expect(valueOf(body, "inset")).toBe("0");
    expect(valueOf(body, "z-index")).toBe("-1");
    expect(valueOf(body, "pointer-events")).toBe("none");
  });

  it("3. the overlay is the V3 triple: white line, white glow, blue gradient", () => {
    const body = one(CSS, BANNER_ACTIVE_BEFORE).body;
    const layers = valueOf(body, "background-image").split("linear-gradient(");
    expect(layers).toHaveLength(4); // the prefix plus exactly three layers
    const [line, glow, blue] = layers.slice(1).map(squeeze);
    expect(line.match(/var\(--ac-selected-rail-color\)/g) ?? []).toHaveLength(2);
    expect(line).not.toContain("rgba(");
    expect(glow).toContain("rgba(255,255,255,0.28)");
    expect(glow).toContain("rgba(255,255,255,0)");
    expect(blue).toContain("rgba(128,166,255,0.34)");
    expect(blue).toContain("rgba(128,166,255,0.10)");
    expect(blue).toContain("rgba(128,166,255,0.05)");
  });

  it("4. the line and the glow are sized and positioned by the rail token", () => {
    const body = one(CSS, BANNER_ACTIVE_BEFORE).body;
    const sizes = squeeze(valueOf(body, "background-size")).split(",");
    expect(sizes).toHaveLength(3);
    expect(sizes[0]).toBe("var(--ac-selected-rail-width)100%");
    expect(sizes[1]).toBe("calc(var(--ac-selected-rail-width)*1.9)100%");
    const positions = squeeze(valueOf(body, "background-position")).split(",");
    expect(positions).toHaveLength(3);
    expect(positions[1]).toBe("var(--ac-selected-rail-width)top");
  });

  it("5. the halo reuses the selected-row rail tokens without redeclaring them", () => {
    for (const token of ["--ac-selected-rail-width", "--ac-selected-rail-color"]) {
      expect(declaredLines(VARS, token), `${token} in variables.css`).toHaveLength(1);
      expect(declaredLines(CSS, token), `${token} in sidebar.css`).toHaveLength(0);
    }
    expect(valueOf(one(VARS, ":root").body, "--ac-rail-delta")).toBe(
      "calc(var(--ac-selected-rail-width) - 3px)"
    );
  });

  it("6. no hover rule can reach the pseudo-element overlay", () => {
    const hoverRules = scan(CSS).filter((rule) =>
      rule.selectors.some((selector) => /\.root-agent-banner:hover\b/.test(selector))
    );
    expect(hoverRules).toHaveLength(7);
    for (const rule of hoverRules) {
      for (const selector of rule.selectors) {
        expect(selector, `hover selector reaches a pseudo-element: ${selector}`).not.toContain(
          "::before"
        );
        expect(selector, `hover selector reaches a pseudo-element: ${selector}`).not.toContain(
          "::after"
        );
      }
    }
  });

  it("7. the three bordered styles carry their planned offsets", () => {
    const noir = one(CSS, bannerBefore("noir-minimal"));
    expect(valueOf(noir.body, "left")).toBe("-3px");
    const command = one(CSS, bannerBefore("command-center"));
    expect(valueOf(command.body, "left")).toBe("-4px");
    const card = one(CSS, bannerBefore("card-sections"));
    expect(valueOf(card.body, "inset-block")).toBe("-1px");
    expect(valueOf(card.body, "left")).toBe("-9px");
    expect(valueOf(card.body, "right")).toBe("7px");
    expect(propsOf(card.body)).not.toContain("inset");
    for (const rule of [noir, command]) {
      const values = pairs(rule.body).map(([, value]) => value);
      expect(values).not.toContain("-9px");
      expect(values).not.toContain("7px");
    }
  });

  it("8. each offset is computed from that style's own border and margin", () => {
    const check = (style: string, border: BoxWidths, margin: BoxWidths): void => {
      const body = one(CSS, bannerBefore(style)).body;
      const decls = valuesOf(body);
      expect(decls.get("left"), `${style} left`).toBe(`${-(border.left + margin.left)}px`);
      if (style === "card-sections") {
        expect(decls.get("right"), `${style} right`).toBe(`${margin.left - border.right}px`);
        expect(decls.get("inset-block"), `${style} inset-block`).toBe(`${-border.top}px`);
      } else {
        // These two styles add an inline-start border and no margin, so there
        // is no end-side rule to compute: -3px and -4px are the whole box.
        expect(decls.has("right")).toBe(false);
        expect(decls.has("inset-block")).toBe(false);
        expect(decls.has("inset")).toBe(false);
      }
    };

    for (const style of ["noir-minimal", "command-center", "card-sections"]) {
      const bannerBody = merged(CSS, banner(style));
      check(style, borderWidths(bannerBody), marginWidths(bannerBody));
    }
  });

  it("9. the halo blocks add no !important and no border-left", () => {
    for (const selector of [BANNER_ACTIVE, BANNER_ACTIVE_BEFORE]) {
      const body = one(CSS, selector).body;
      expect(propsOf(body)).not.toContain("border-left");
      expect(body).not.toContain("!important");
    }
  });

  it("10. --ac-rail-width is declared once, in variables.css", () => {
    const inVars = declaredLines(VARS, "--ac-rail-width");
    expect(inVars).toHaveLength(1);
    expect(declaredLines(CSS, "--ac-rail-width")).toHaveLength(0);
    const value = inVars[0].slice(inVars[0].indexOf(":") + 1).split(";")[0].trim();
    expect(value).toBe("68px");
  });

  it("11. the group rail consumes the token and no literal 68px survives", () => {
    const rail = one(CSS, ".workgroup-group-rail");
    expect(valueOf(rail.body, "flex")).toBe("0 0 var(--ac-rail-width)");
    expect(valueOf(rail.body, "min-width")).toBe("var(--ac-rail-width)");
    expect(valueOf(rail.body, "max-width")).toBe("var(--ac-rail-width)");
    expect(CSS.match(/(^|[^0-9])68px/gm)).toBeNull();
  });
});
