import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./resource-monitor.css", import.meta.url), "utf8");

interface Declaration {
  property: string;
  value: string;
}

interface StyleRule {
  selector: string;
  selectors: string[];
  body: string;
  start: number;
  end: number;
}

interface AtRule {
  name: string;
  condition: string;
  body: string;
  start: number;
  end: number;
}

interface ParsedStylesheet {
  source: string;
  imports: string[];
  atRules: AtRule[];
  rules: StyleRule[];
  conditionalRules: StyleRule[];
  baseRules: StyleRule[];
}

const stripComments = (source: string): string =>
  source.replace(/\/\*[\s\S]*?\*\//g, "");

const splitSelectors = (selector: string): string[] => {
  const selectors: string[] = [];
  let depth = 0;
  let current = "";
  for (const char of selector) {
    if (char === "(") depth += 1;
    if (char === ")") depth -= 1;
    if (char === "," && depth === 0) {
      selectors.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  if (current.trim().length > 0) selectors.push(current);
  return selectors.map((value) => value.replace(/\s+/g, " ").trim());
};

const parseStylesheet = (raw: string): ParsedStylesheet => {
  const source = stripComments(raw);
  let work = source;
  const imports: string[] = [];
  for (const match of source.matchAll(/@(import|charset|namespace)\b[^;]*;/g)) {
    imports.push(match[0].trim());
    work =
      work.slice(0, match.index) +
      " ".repeat(match[0].length) +
      work.slice(match.index + match[0].length);
  }

  const atRules: AtRule[] = [];
  const rules: StyleRule[] = [];
  let cursor = 0;
  let headerFloor = 0;
  while (cursor < work.length) {
    const open = work.indexOf("{", cursor);
    if (open === -1) break;
    const headerStart =
      Math.max(
        work.lastIndexOf("}", open),
        work.lastIndexOf(";", open),
        headerFloor - 1
      ) + 1;
    const header = work.slice(headerStart, open).trim();
    if (header.length === 0) {
      cursor = open + 1;
      continue;
    }
    let depth = 1;
    let close = open + 1;
    while (close < work.length && depth > 0) {
      if (work[close] === "{") depth += 1;
      if (work[close] === "}") depth -= 1;
      close += 1;
    }
    const body = work.slice(open + 1, close - 1);
    const atName = /^@([a-z-]+)/i.exec(header);
    if (header.startsWith("@") && atName) {
      atRules.push({
        name: atName[1].toLowerCase(),
        condition: header,
        body,
        start: headerStart,
        end: close,
      });
      headerFloor = open + 1;
      cursor = open + 1;
      continue;
    }
    rules.push({
      selector: header,
      selectors: splitSelectors(header),
      body,
      start: headerStart,
      end: close,
    });
    headerFloor = close;
    cursor = close;
  }

  const conditionalRules = rules.filter((rule) =>
    atRules.some((atRule) => rule.start > atRule.start && rule.end <= atRule.end)
  );
  const baseRules = rules.filter((rule) => !conditionalRules.includes(rule));

  return { source, imports, atRules, rules, conditionalRules, baseRules };
};

const parsed = parseStylesheet(css);

const ruleDeclarations = (rule: StyleRule): Declaration[] =>
  rule.body
    .split(";")
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => {
      const colon = part.indexOf(":");
      if (colon === -1) return null;
      return {
        property: part.slice(0, colon).trim().toLowerCase(),
        value: part.slice(colon + 1).trim().toLowerCase(),
      };
    })
    .filter((declaration): declaration is Declaration => declaration !== null);

const hasDeclaration = (
  rule: StyleRule | undefined,
  property: string,
  value?: string
): boolean => {
  if (!rule) return false;
  return ruleDeclarations(rule).some(
    (declaration) =>
      declaration.property === property.toLowerCase() &&
      (value === undefined || declaration.value === value.toLowerCase())
  );
};

const declarationValue = (
  rule: StyleRule,
  property: string
): string | undefined =>
  ruleDeclarations(rule).find(
    (declaration) => declaration.property === property.toLowerCase()
  )?.value;

const rulesWithSelector = (
  rules: StyleRule[],
  selector: string
): StyleRule[] => rules.filter((rule) => rule.selectors.includes(selector));

const baseRulesWith = (selector: string): StyleRule[] =>
  rulesWithSelector(parsed.baseRules, selector);

const occurrences = (source: string, pattern: RegExp): number =>
  (source.match(pattern) ?? []).length;

const normalizeSelector = (selector: string): string =>
  selector.replace(/\s+/g, "");

const splitTopLevel = (value: string): string[] => {
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  for (const char of value) {
    if (char === "(") depth += 1;
    if (char === ")") depth -= 1;
    if (/\s/.test(char) && depth === 0) {
      if (current.length > 0) {
        parts.push(current);
        current = "";
      }
      continue;
    }
    current += char;
  }
  if (current.length > 0) parts.push(current);
  return parts;
};

const trackCount = (value: string): number => {
  const repeat = /^repeat\(\s*(\d+)\s*,/.exec(value);
  if (repeat) return Number(repeat[1]);
  return splitTopLevel(value).length;
};

const ZERO_VALUE = /^0(\.0+)?(px|%|em|rem)?$/;
const BANNED_DECLARATIONS = new Set([
  "display:none",
  "visibility:hidden",
  "visibility:collapse",
  "opacity:0",
  "width:0",
  "height:0",
  "max-width:0",
  "max-height:0",
  "font-size:0",
]);

const FORBIDDEN_ZERO_EXCEPTIONS: string[] = [];
const BASE_ZERO_EXCEPTIONS: string[] = [];

const collectZeroSizingViolations = (rules: StyleRule[]): string[] => {
  const violations: string[] = [];
  for (const rule of rules) {
    for (const declaration of ruleDeclarations(rule)) {
      const value = declaration.value.replace(/\s*!important\s*$/, "").trim();
      const key = `${declaration.property}:${
        ZERO_VALUE.test(value) ? "0" : value
      }`;
      if (BANNED_DECLARATIONS.has(key)) {
        violations.push(
          `${rule.selector} { ${declaration.property}: ${declaration.value} }`
        );
      }
    }
  }
  return violations;
};

const CONTAINER_CONDITION = "@container rm-body (min-width: 860px)";
const containerAtRule = parsed.atRules.find(
  (atRule) => atRule.name === "container"
);
const inContainer = parsed.rules.filter(
  (rule) =>
    containerAtRule !== undefined &&
    rule.start > containerAtRule.start &&
    rule.end <= containerAtRule.end
);
const outOfContainer = parsed.rules.filter(
  (rule) =>
    containerAtRule === undefined ||
    !(rule.start > containerAtRule.start && rule.end <= containerAtRule.end)
);

const METRIC_LEG_SELECTORS = [
  ".rm-group-main > span:not(.rm-group-identity):not(.rm-network-pill)",
  ".rm-process-header > span:not(:first-child)",
  ".rm-process-row > span:not(:first-child)",
];

const FOCUS_SELECTORS = [
  ".rm-action-btn",
  ".rm-filter-seg-btn",
  ".rm-filter-chip",
  ".rm-filter-clear",
  ".rm-group-main",
  ".rm-kill-btn",
  ".rm-titlebar-btn",
  ".rm-filter-pid-input",
  ".rm-filter-search-input",
  ".rm-filter-input-clear",
  ".rm-pid-chip",
  ".rm-sort-field",
  ".rm-sort-direction",
];

const NEW_SELECTORS = [
  ".rm-filter-pid",
  ".rm-filter-search",
  ".rm-filter-pid-input",
  ".rm-filter-search-input",
  ".rm-filter-input-clear",
  ".rm-filter-help",
  ".rm-filter-notice",
  ".rm-filter-notice.is-error",
  ".rm-pid-chip",
  ".rm-pid-chip.is-unmatched",
  ".rm-filter-actions",
  ".rm-sort",
  ".rm-sort-field",
  ".rm-sort-direction",
  ".rm-sort-direction:disabled",
  ".rm-group-identity-line",
  ".rm-group-identity-line .rm-group-name",
  ".rm-partial-pill",
  ".rm-group-row.is-expanded",
  ".rm-process-row.is-pid-match",
  ".rm-process-row.is-pid-match > span:nth-child(2)",
  ".rm-process-row > span:first-child.is-tree",
];

describe("resource-monitor.css contract", () => {
  it("26: sweeps both rule families for hiding and zero-sizing declarations", () => {
    expect(FORBIDDEN_ZERO_EXCEPTIONS).toEqual([]);
    expect(BASE_ZERO_EXCEPTIONS).toEqual([]);
    expect(parsed.imports).toHaveLength(1);

    const atRuleNames = parsed.atRules.map((atRule) => atRule.name);
    expect(atRuleNames.filter((name) => name === "media")).toHaveLength(1);
    expect(atRuleNames.filter((name) => name === "container")).toHaveLength(1);
    expect(
      atRuleNames.filter((name) => name !== "media" && name !== "container")
    ).toEqual([]);

    expect(collectZeroSizingViolations(parsed.baseRules)).toEqual(
      BASE_ZERO_EXCEPTIONS
    );
    expect(collectZeroSizingViolations(parsed.conditionalRules)).toEqual(
      FORBIDDEN_ZERO_EXCEPTIONS
    );

    expect(parsed.baseRules.length).toBeGreaterThanOrEqual(60);
    expect(parsed.baseRules.length + parsed.conditionalRules.length).toBe(
      parsed.rules.length
    );

    const automationRules = rulesWithSelector(
      parsed.rules,
      ".rm-automation-metric"
    );
    expect(automationRules).toHaveLength(1);
    expect(hasDeclaration(automationRules[0], "width", "1px")).toBe(true);
    expect(hasDeclaration(automationRules[0], "height", "1px")).toBe(true);
    expect(hasDeclaration(automationRules[0], "opacity", "0.01")).toBe(true);
  });

  it("27: pins the container query, its five rule groups and the source order", () => {
    const bodyRule = rulesWithSelector(parsed.rules, ".rm-body");
    expect(bodyRule).toHaveLength(1);
    expect(hasDeclaration(bodyRule[0], "container-type", "inline-size")).toBe(
      true
    );
    expect(hasDeclaration(bodyRule[0], "container-name", "rm-body")).toBe(true);

    expect(occurrences(parsed.source, /container-type\s*:/g)).toBe(1);
    expect(occurrences(parsed.source, /container-name\s*:/g)).toBe(1);
    expect(occurrences(parsed.source, /(^|[;{])\s*container\s*:/gm)).toBe(0);

    expect(occurrences(parsed.source, /@container\b/g)).toBe(1);
    expect(containerAtRule?.condition).toBe(CONTAINER_CONDITION);

    const groupMain = inContainer.find((rule) =>
      rule.selectors.includes(".rm-group-main")
    );
    expect(hasDeclaration(groupMain, "display", "grid")).toBe(true);
    const groupTemplate = declarationValue(
      groupMain as StyleRule,
      "grid-template-columns"
    );
    expect(trackCount(groupTemplate ?? "")).toBe(8);

    const processGrid = inContainer.find(
      (rule) =>
        rule.selectors.includes(".rm-process-header") &&
        hasDeclaration(rule, "display", "grid")
    );
    expect(processGrid).toBeDefined();
    const processTemplate = declarationValue(
      processGrid as StyleRule,
      "grid-template-columns"
    );
    expect(trackCount(processTemplate ?? "")).toBe(6);

    const metricLeg = inContainer.find(
      (rule) =>
        rule.selectors.includes(METRIC_LEG_SELECTORS[0]) &&
        hasDeclaration(rule, "min-width", "0")
    );
    expect(metricLeg).toBeDefined();

    const statusStrip = inContainer.find((rule) =>
      rule.selectors.includes(".rm-status-strip")
    );
    expect(
      hasDeclaration(statusStrip, "grid-template-columns", "repeat(5, minmax(110px, 1fr))")
    ).toBe(true);

    const header = inContainer.find((rule) =>
      rule.selectors.includes(".rm-header")
    );
    expect(hasDeclaration(header, "flex-direction", "row")).toBe(true);

    const blockSelectorGroups = [
      [".rm-group-main"],
      [".rm-process-header", ".rm-process-row"],
      METRIC_LEG_SELECTORS,
      [".rm-status-strip"],
      [".rm-header"],
    ];
    for (const selectors of blockSelectorGroups) {
      for (const rule of outOfContainer) {
        if (selectors.some((selector) => rule.selectors.includes(selector))) {
          expect(containerAtRule!.start).toBeGreaterThan(rule.end);
        }
      }
    }
  });

  it("27b: closes the block's negative set and its template selectors", () => {
    for (const rule of inContainer) {
      expect(rule.body).not.toContain("calc(100% - 58px)");
      expect(rule.body).not.toContain("nth-child");
    }
    const killButtonInBlock = inContainer.flatMap((rule) =>
      rule.selectors.includes(".rm-kill-btn") ? ruleDeclarations(rule) : []
    );
    expect(
      killButtonInBlock.filter((declaration) => declaration.property === "width")
    ).toEqual([]);
    expect(collectZeroSizingViolations(inContainer)).toEqual([]);

    const templateSelectors = new Set<string>();
    for (const rule of inContainer) {
      if (hasDeclaration(rule, "grid-template-columns")) {
        for (const selector of rule.selectors) templateSelectors.add(selector);
      }
    }
    expect([...templateSelectors].sort()).toEqual(
      [
        ".rm-group-main",
        ".rm-process-header",
        ".rm-process-row",
        ".rm-status-strip",
      ].sort()
    );
  });

  it("27c: pins the single max-content leg and the network pill case", () => {
    const maxContentRules = parsed.rules.filter((rule) =>
      hasDeclaration(rule, "min-width", "max-content")
    );
    expect(maxContentRules).toHaveLength(1);
    expect(
      maxContentRules[0].selectors.map(normalizeSelector).sort()
    ).toEqual(METRIC_LEG_SELECTORS.map(normalizeSelector).sort());

    const pillRules = baseRulesWith(".rm-network-pill");
    expect(pillRules.length).toBeGreaterThan(0);
    expect(
      pillRules.some(
        (rule) =>
          hasDeclaration(rule, "min-width", "0") &&
          hasDeclaration(rule, "text-overflow", "ellipsis")
      )
    ).toBe(true);
  });

  it("28: removes float and pins the group-row grid without a gap", () => {
    expect(occurrences(parsed.source, /\bfloat\s*:/g)).toBe(0);

    const groupRow = rulesWithSelector(parsed.rules, ".rm-group-row");
    expect(groupRow).toHaveLength(1);
    expect(hasDeclaration(groupRow[0], "display", "grid")).toBe(true);
    expect(
      hasDeclaration(groupRow[0], "grid-template-columns", "minmax(0, 1fr) 64px")
    ).toBe(true);
    for (const gap of ["gap", "row-gap", "column-gap", "grid-gap"]) {
      expect(hasDeclaration(groupRow[0], gap)).toBe(false);
    }
  });

  it("28b: keeps the safe wrap layout in the unconditional base", () => {
    for (const selector of [
      ".rm-group-main",
      ".rm-process-header",
      ".rm-process-row",
    ]) {
      const baseRules = baseRulesWith(selector);
      expect(
        baseRules.some(
          (rule) =>
            hasDeclaration(rule, "display", "flex") &&
            hasDeclaration(rule, "flex-wrap", "wrap")
        )
      ).toBe(true);
      expect(
        baseRules.flatMap((rule) => ruleDeclarations(rule)).filter(
          (declaration) => declaration.property === "grid-template-columns"
        )
      ).toEqual([]);
    }

    const groupMainBase = baseRulesWith(".rm-group-main");
    expect(
      groupMainBase.flatMap((rule) => ruleDeclarations(rule)).filter(
        (declaration) => declaration.property === "width"
      )
    ).toEqual([]);
    expect(
      groupMainBase.some((rule) =>
        hasDeclaration(rule, "gap", "var(--spacing-sm)")
      )
    ).toBe(true);

    const identityLeg = baseRulesWith(".rm-group-main > .rm-group-identity");
    expect(
      identityLeg.some(
        (rule) =>
          hasDeclaration(rule, "flex", "1 1 auto") &&
          hasDeclaration(rule, "min-width", "0")
      )
    ).toBe(true);

    const [metricLegRule] = parsed.rules.filter((rule) =>
      hasDeclaration(rule, "min-width", "max-content")
    );
    expect(hasDeclaration(metricLegRule, "flex", "0 0 auto")).toBe(true);

    const processListBase = baseRulesWith(".rm-process-list");
    expect(
      processListBase.some((rule) =>
        hasDeclaration(rule, "grid-column", "1 / -1")
      )
    ).toBe(true);
    expect(
      processListBase.flatMap((rule) => ruleDeclarations(rule)).filter(
        (declaration) => declaration.property === "clear"
      )
    ).toEqual([]);

    const statusStripBase = baseRulesWith(".rm-status-strip");
    const baseStripTemplate = statusStripBase
      .map((rule) => declarationValue(rule, "grid-template-columns"))
      .find((value) => value !== undefined);
    expect(splitTopLevel(baseStripTemplate ?? "")).toHaveLength(1);
    expect(trackCount(baseStripTemplate ?? "")).toBe(2);

    const headerBase = baseRulesWith(".rm-header");
    expect(
      headerBase.some((rule) =>
        hasDeclaration(rule, "flex-direction", "column")
      )
    ).toBe(true);
  });

  it("29: keeps the eight- and six-track templates only inside the block", () => {
    const templates = parsed.rules.flatMap((rule) =>
      ruleDeclarations(rule)
        .filter((declaration) => declaration.property === "grid-template-columns")
        .map((declaration) => ({ rule, value: declaration.value }))
    );
    const eightTracks = templates.filter(
      (template) => trackCount(template.value) === 8
    );
    const sixTracks = templates.filter(
      (template) => trackCount(template.value) === 6
    );
    expect(eightTracks).toHaveLength(1);
    expect(sixTracks).toHaveLength(1);
    expect(eightTracks[0].rule.start).toBeGreaterThan(containerAtRule!.start);
    expect(sixTracks[0].rule.start).toBeGreaterThan(containerAtRule!.start);
    expect(
      templates.filter(
        (template) =>
          template.rule.start < containerAtRule!.start &&
          [6, 8].includes(trackCount(template.value))
      )
    ).toEqual([]);
  });

  it("30: declares the thirteen focus rings and their two negative offsets", () => {
    const focusRule = parsed.rules.find((rule) =>
      rule.selectors.includes(".rm-action-btn:focus-visible")
    );
    expect(focusRule).toBeDefined();
    for (const selector of FOCUS_SELECTORS) {
      expect(focusRule!.selectors).toContain(`${selector}:focus-visible`);
    }
    expect(hasDeclaration(focusRule, "outline-offset", "2px")).toBe(true);

    const negative = parsed.rules.filter((rule) =>
      hasDeclaration(rule, "outline-offset", "-2px")
    );
    expect(negative).toHaveLength(1);
    expect(negative[0].selectors.slice().sort()).toEqual(
      [".rm-group-main:focus-visible", ".rm-kill-btn:focus-visible"].sort()
    );
  });

  it("31: keeps a reduced-motion block that cancels transitions", () => {
    const motion = parsed.atRules.find(
      (atRule) =>
        atRule.name === "media" &&
        atRule.condition.includes("prefers-reduced-motion")
    );
    expect(motion).toBeDefined();
    const motionRules = parsed.rules.filter(
      (rule) => rule.start > motion!.start && rule.end <= motion!.end
    );
    expect(
      motionRules.some((rule) => hasDeclaration(rule, "transition", "none"))
    ).toBe(true);
  });

  it("32: applies tabular numerals to tiles, group rows and process rows", () => {
    const numericRules = parsed.rules.filter((rule) =>
      hasDeclaration(rule, "font-variant-numeric", "tabular-nums")
    );
    for (const selector of [
      ".rm-status-tile",
      ".rm-group-main",
      ".rm-process-row",
    ]) {
      expect(
        numericRules.some((rule) => rule.selectors.includes(selector))
      ).toBe(true);
    }
  });

  it("33: keeps every new rule's colours on tokens", () => {
    for (const selector of NEW_SELECTORS) {
      const rules = rulesWithSelector(parsed.rules, selector);
      expect(rules.length, `no rule for ${selector}`).toBeGreaterThan(0);
      for (const rule of rules) {
        expect(rule.body).not.toMatch(/#|rgb\(|rgba\(|hsl\(/i);
      }
    }
  });
});
