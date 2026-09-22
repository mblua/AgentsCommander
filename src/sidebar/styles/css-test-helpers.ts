// Test-only helpers shared by the stylesheet byte tests (#2271 dup gate).
// Every extraction throws on a miss: the absence and count assertions in those
// suites would otherwise pass vacuously on an empty or partial match.

export const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export interface ScannedRule {
  selectors: string[];
  body: string;
  index: number;
}

/**
 * Every innermost `selector-list { ... }` in the sheet, with the byte index of
 * the first character of the selector list. Rules nested in an @media block are
 * found too; the wrapper itself never yields a match because its body contains
 * braces.
 */
export function scanRules(css: string): ScannedRule[] {
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
