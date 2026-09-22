import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #2271 - the Co-managed dot. jsdom never applies sidebar.css and resolves the
// cascade by order rather than by specificity (see
// ProjectPanel.working-tint.test.tsx:583 and working-tint-css.test.ts:6-9), so
// the bytes on disk are the only place the token values, the declarations and
// the cascade position can be pinned. A computed-style test would pass while
// the real webview showed `waiting`.
//
// Every extraction below throws when it does not find what it looks for. There
// is no `?? ""` fallback anywhere: without that, the absence assertions would
// be vacuous and the cascade-position loop would run zero times and still pass.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const VARS = readFileSync(new URL("./variables.css", import.meta.url), "utf8");

// Blank out comment spans, preserving length and line breaks so byte indices
// stay comparable between the raw and the scanned copy. Required for
// correctness: the #2271 block's own comment names `.comanaged`, and a comment
// can never be allowed to satisfy a rule lookup.
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
}

const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

interface ScannedRule {
  selectors: string[];
  body: string;
  index: number;
}

/** Every innermost `selector-list { ... }` in the sheet, byte-indexed. */
function scanRules(css: string): ScannedRule[] {
  const out: ScannedRule[] = [];
  const re = /([^{}]*)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null = re.exec(css);
  while (m !== null) {
    const lead = m[1];
    const raw = lead.trim();
    if (raw !== "") {
      out.push({
        selectors: raw.split(",").map((s) => s.trim()).filter((s) => s !== ""),
        body: m[2],
        index: m.index + (lead.length - lead.trimStart().length),
      });
    }
    m = re.exec(css);
  }
  if (out.length === 0) throw new Error("scanRules found no rules at all");
  return out;
}

const RULES = scanRules(stripComments(CSS));

/** The rule whose selector list contains `selector` exactly. Throws on a miss. */
function ruleFor(selector: string): ScannedRule {
  const found = RULES.filter((rule) => rule.selectors.includes(selector));
  if (found.length === 0) throw new Error(`no rule carries the selector: ${selector}`);
  return found[0];
}

function varsBlock(selector: string): string {
  const re = new RegExp(`^${escapeRe(selector)} \\{([^}]*)\\}`, "m");
  const match = stripComments(VARS).match(re);
  if (!match) throw new Error(`missing variables.css block: ${selector}`);
  return match[1];
}

function declarations(body: string): Array<[string, string]> {
  return body
    .split(";")
    .map((d) => d.trim())
    .filter((d) => d.includes(":"))
    .map((d) => [
      d.slice(0, d.indexOf(":")).trim(),
      d.slice(d.indexOf(":") + 1).trim().replace(/\s+/g, " "),
    ]);
}

const declProps = (body: string): string[] => declarations(body).map(([prop]) => prop);

/** Last declaration of `prop` in `body`, mirroring the within-rule cascade. */
function declValue(body: string, prop: string): string {
  let found: string | undefined;
  for (const [p, v] of declarations(body)) if (p === prop) found = v;
  if (found === undefined) throw new Error(`missing declaration: ${prop}`);
  return found;
}

/**
 * [ids, classes+attributes+pseudo-classes, element types], the standard
 * specificity triple. Adequate for the plain compound selectors this file uses;
 * a selector it cannot classify throws rather than silently counting zero.
 */
function specificity(selector: string): [number, number, number] {
  const ids = (selector.match(/#[\w-]+/g) ?? []).length;
  const classLike =
    (selector.match(/\.[\w-]+/g) ?? []).length +
    (selector.match(/\[[^\]]*\]/g) ?? []).length +
    (selector.match(/(?<!:):[\w-]+/g) ?? []).length;
  const compounds = selector.split(/[\s>+~]+/).filter((part) => part !== "");
  if (compounds.length === 0) throw new Error(`empty selector: ${selector}`);
  const types = compounds
    .map((part) =>
      part
        .replace(/#[\w-]+/g, "")
        .replace(/\.[\w-]+/g, "")
        .replace(/\[[^\]]*\]/g, "")
        .replace(/(?<!:):[\w-]+/g, ""),
    )
    .filter((part) => part !== "" && part !== "*").length;
  return [ids, classLike, types];
}

function specificityAtLeast(
  candidate: [number, number, number],
  baseline: [number, number, number],
): boolean {
  for (let i = 0; i < candidate.length; i += 1) {
    if (candidate[i] !== baseline[i]) return candidate[i] > baseline[i];
  }
  return true;
}

const CONTAINERS = [".session-item", ".replica-item", ".root-agent-banner"];

describe("Co-managed CSS (#2271)", () => {
  it("defines --status-comanaged in both theme blocks with the exact exited hex values and no glow", () => {
    // #2271 test 10: the token exists in both themes and the rule has no
    // box-shadow. The absence is asserted explicitly, so a rule that gains a
    // glow later must fail this test.
    const dark = varsBlock(":root");
    const light = varsBlock("html.light-theme");

    expect(declValue(dark, "--status-comanaged")).toBe("#ff3b5c");
    expect(declValue(light, "--status-comanaged")).toBe("#dc2626");
    // Section 6: those are exactly the exited values in each theme.
    expect(declValue(dark, "--status-comanaged")).toBe(declValue(dark, "--status-exited"));
    expect(declValue(light, "--status-comanaged")).toBe(declValue(light, "--status-exited"));

    for (const container of CONTAINERS) {
      const rule = ruleFor(`${container} .session-item-status.comanaged`);
      expect(declProps(rule.body)).not.toContain("box-shadow");
      expect(declProps(rule.body)).not.toContain("transition");
      expect(declProps(rule.body)).not.toContain("animation");
      expect(rule.body).not.toContain("box-shadow");
      expect(rule.body).not.toContain("transition");
      expect(rule.body).not.toContain("animation");
    }
  });

  it("declares the .comanaged rules after their .waiting siblings, at >= specificity (#2271 test 11)", () => {
    for (const container of CONTAINERS) {
      const comanagedSelector = `${container} .session-item-status.comanaged`;
      const waitingSelector = `${container} .session-item-status.waiting`;

      const comanaged = ruleFor(comanagedSelector);
      const waiting = ruleFor(waitingSelector);

      // The byte position is the pin: after .waiting, so the idle edge's
      // waiting declaration cannot win the real cascade.
      expect(comanaged.index).toBeGreaterThan(waiting.index);
      expect(
        specificityAtLeast(specificity(comanagedSelector), specificity(waitingSelector)),
      ).toBe(true);

      expect(declValue(comanaged.body, "background")).toBe("var(--status-comanaged)");
    }
  });

  it("keeps the corrected .waiting comment, which now yields to .comanaged", () => {
    // Criterion 8: the old comment claimed .waiting overrides ALL other status
    // colors, which is no longer true. The file must stop asserting it.
    expect(CSS).not.toContain("waiting must override all other status colors");
    expect(CSS).toContain("it yields to .comanaged");
  });
});
