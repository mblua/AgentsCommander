// #4825 - every interactive control in the three sidebar menu files carries a
// non-empty data-ac-testid. Parser-backed (TSX), so comments and strings are
// never counted; the minimum scanned counts fail a wrong path or parse mode.
import { readFileSync } from "node:fs";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const CONTROL_TAGS = new Set(["input", "button", "textarea", "select"]);

const FILES: ReadonlyArray<{ path: string; minControls: number }> = [
  { path: "./context-menu/SessionRowMenu.tsx", minControls: 16 },
  { path: "./ProjectPanel.tsx", minControls: 59 },
  { path: "./TemporaryRoomsPanel.tsx", minControls: 5 },
];

function isEmptyValue(attr: ts.JsxAttribute): boolean {
  const init = attr.initializer;
  if (!init) return true;
  if (ts.isStringLiteral(init)) return init.text.trim() === "";
  if (!ts.isJsxExpression(init) || !init.expression) return true;
  const expr = init.expression;
  if (ts.isStringLiteral(expr) || ts.isNoSubstitutionTemplateLiteral(expr)) {
    return expr.text.trim() === "";
  }
  return false;
}

function scan(path: string): { controls: number; missing: string[] } {
  const name = path.split("/").pop() ?? path;
  const text = readFileSync(new URL(path, import.meta.url), "utf8");
  const source = ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let controls = 0;
  const missing: string[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
      const tag = node.tagName.getText(source);
      if (CONTROL_TAGS.has(tag)) {
        controls += 1;
        const attr = node.attributes.properties.find(
          (p): p is ts.JsxAttribute =>
            ts.isJsxAttribute(p) && p.name.getText(source) === "data-ac-testid",
        );
        if (!attr || isEmptyValue(attr)) {
          const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
          missing.push(`${name}:${line}`);
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return { controls, missing };
}

describe("#4825 sidebar menu controls carry data-ac-testid", () => {
  for (const file of FILES) {
    it(`${file.path}: every control has a non-empty data-ac-testid`, () => {
      const result = scan(file.path);
      expect(result.controls).toBeGreaterThanOrEqual(file.minControls);
      expect(result.missing).toEqual([]);
    });
  }
});
