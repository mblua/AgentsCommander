import { readFileSync } from "node:fs";

// Test-only helpers shared by the stylesheet byte tests (dup gate).
// Every extraction throws on a miss: the absence and count assertions in those
// suites would otherwise pass vacuously on an empty or partial match.

export const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, String.raw`\$&`);

export interface ScannedRule {
  selectors: string[];
  body: string;
  index: number;
}

/**
 * Every innermost `selector-list { ... }` in the sheet, with the UTF-16 index of
 * the first character of the selector list. Rules nested in an @media block are
 * found too; the wrapper itself never yields a match because its body contains
 * braces.
 */
export function scanRules(css: string): ScannedRule[] {
  const out: ScannedRule[] = [];
  let leadStart = 0;
  let open = -1;
  for (let i = 0; i < css.length; i++) {
    if (css[i] === "{") {
      if (open >= 0) leadStart = open + 1;
      open = i;
    } else if (css[i] === "}") {
      if (open >= 0) {
        const lead = css.slice(leadStart, open);
        const trimmedStart = lead.trimStart();
        const raw = trimmedStart.trimEnd();
        if (raw !== "") {
          out.push({
            selectors: raw.split(",").map((s) => s.trim()).filter((s) => s !== ""),
            body: css.slice(open + 1, i),
            index: leadStart + lead.length - trimmedStart.length,
          });
        }
      }
      leadStart = i + 1;
      open = -1;
    }
  }
  if (out.length === 0) throw new Error("scanRules found no rules at all");
  return out;
}

export function declarations(body: string): Array<[string, string]> {
  return body
    .split(";")
    .map((d) => d.trim())
    .filter((d) => d.includes(":"))
    .map((d) => [
      d.slice(0, d.indexOf(":")).trim(),
      d.slice(d.indexOf(":") + 1).trim().replace(/\s+/g, " "),
    ]);
}

export const declProps = (body: string): string[] => declarations(body).map(([prop]) => prop);

/** Last declaration of `prop` in `body`, mirroring the within-rule cascade. Throws on a miss. */
export function declValue(body: string, prop: string): string {
  let found: string | undefined;
  for (const [p, v] of declarations(body)) if (p === prop) found = v;
  if (found === undefined) throw new Error(`missing declaration: ${prop}`);
  return found;
}

/** Rules of a stylesheet with comments blanked (offsets kept), so commented-out CSS never matches. */
export function scanSheet(url: URL): ScannedRule[] {
  const css = readFileSync(url, "utf8");
  return scanRules(css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " ")));
}

/** Body of the single rule whose selector list contains `selector`. Throws unless exactly one. */
export function soleRuleBody(rules: ScannedRule[], selector: string): string {
  const hits = rules.filter((r) => r.selectors.includes(selector));
  const hit = hits[0];
  if (hits.length !== 1 || hit === undefined) throw new Error(`expected 1 rule for ${selector}, got ${hits.length}`);
  return hit.body;
}
