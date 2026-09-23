// #2410 - generic guard: every class token used in markup must have a CSS rule.
// Origin defect: #2409 (a literal class in new markup with no stylesheet rule).
//
// Used classes are PARSER-BACKED (TypeScript compiler API, same approach as
// sidebar/watchdog/context-menu-import-boundary.test.ts). Sites collected:
// JSX `class="..."`, `class={expr}`, `classList={{...}}` keys,
// `x.className = expr`, and string-literal arguments of
// `x.classList.add/remove/replace` plus the first argument of `toggle`.
// Documented limits: classes returned by helpers in other modules, values
// arriving through props/variables, and tokens touching a `${}` interpolation
// are not checked.
//
// Styled classes: any `.name` in a rule selector prelude of `src/**/*.css` or
// of a package stylesheet imported with a bare side-effect import.
// Hook-only classes need an allowlist entry with a reason; the allowlist is
// two-way (an entry that is now styled or no longer used fails as stale).
//
// File listing uses import.meta.glob keys (never loaded) and reads bytes with
// the narrowed node:fs shim in src/vite-env.d.ts; @types/node is not a dependency.
import { readFileSync } from "node:fs";
import ts from "typescript";
import { describe, expect, it } from "vitest";

type SourceFile = { path: string; text: string };
type Allowlist = { version: number; entries: { class: string; reason: string }[] };

const ROOT = new URL("../../../", import.meta.url);
const ALLOWLIST_PATH = "src/shared/styles/class-coverage.allowlist.json";
const TOKEN = /^-?[_a-zA-Z][\w-]*$/;
const XTERM_CSS = "@xterm/xterm/css/xterm.css";


// Keys are relative to this file; the loaders are never called.
const SRC_PATHS = Object.keys(import.meta.glob(["../../**/*.{ts,tsx,css}"]))
  .map((key) => new URL(key, import.meta.url).href.slice(ROOT.href.length))
  .sort();

function readRoot(path: string): string | undefined {
  try {
    return readFileSync(new URL(path, ROOT), "utf8");
  } catch {
    return undefined;
  }
}

function isSourcePath(path: string): boolean {
  return (
    /\.tsx?$/.test(path) && !/\.test\.tsx?$/.test(path) && !path.endsWith(".d.ts")
  );
}

function splitTokens(text: string): string[] {
  return text.split(/\s+/).filter((t) => TOKEN.test(t));
}

/** Class tokens from a class-valued expression (see file header). */
function expressionTokens(node: ts.Node, out: { token: string; node: ts.Node }[]): void {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
    for (const token of splitTokens(node.text)) out.push({ token, node });
    return;
  }
  if (ts.isTemplateExpression(node)) {
    const pieces = [node.head.text, ...node.templateSpans.map((s) => s.literal.text)];
    pieces.forEach((piece, i) => {
      const parts = piece.split(/\s+/);
      if (i > 0 && !/^\s/.test(piece)) parts.shift();
      if (i < pieces.length - 1 && !/\s$/.test(piece)) parts.pop();
      for (const token of parts) if (TOKEN.test(token)) out.push({ token, node });
    });
    for (const span of node.templateSpans) expressionTokens(span.expression, out);
    return;
  }
  if (ts.isCallExpression(node)) {
    for (const arg of node.arguments) expressionTokens(arg, out);
    return;
  }
  if (ts.isBinaryExpression(node)) {
    const op = node.operatorToken.kind;
    if (
      op === ts.SyntaxKind.EqualsEqualsEqualsToken ||
      op === ts.SyntaxKind.ExclamationEqualsEqualsToken ||
      op === ts.SyntaxKind.EqualsEqualsToken ||
      op === ts.SyntaxKind.ExclamationEqualsToken
    ) {
      return;
    }
  }
  ts.forEachChild(node, (child) => expressionTokens(child, out));
}

function isClassListCall(node: ts.CallExpression): string | undefined {
  const callee = node.expression;
  if (!ts.isPropertyAccessExpression(callee)) return undefined;
  const target = callee.expression;
  if (!ts.isPropertyAccessExpression(target) || target.name.text !== "classList") return undefined;
  const method = callee.name.text;
  return ["add", "remove", "replace", "toggle"].includes(method) ? method : undefined;
}

function parse(file: SourceFile): ts.SourceFile {
  const kind = file.path.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  return ts.createSourceFile(file.path, file.text, ts.ScriptTarget.Latest, true, kind);
}

function extractUsedClasses(files: SourceFile[]): Map<string, string[]> {
  const used = new Map<string, string[]>();
  for (const file of files) {
    const sf = parse(file);
    const found: { token: string; node: ts.Node }[] = [];
    const visit = (node: ts.Node): void => {
      if (ts.isJsxAttribute(node) && ts.isIdentifier(node.name) && node.initializer) {
        const name = node.name.text;
        const init = node.initializer;
        if (name === "class") {
          if (ts.isStringLiteral(init)) expressionTokens(init, found);
          else if (ts.isJsxExpression(init) && init.expression) expressionTokens(init.expression, found);
        } else if (
          name === "classList" &&
          ts.isJsxExpression(init) &&
          init.expression &&
          ts.isObjectLiteralExpression(init.expression)
        ) {
          for (const prop of init.expression.properties) {
            const key = prop.name;
            if (key && (ts.isIdentifier(key) || ts.isStringLiteral(key))) {
              for (const token of splitTokens(key.text)) found.push({ token, node: key });
            }
          }
        }
      } else if (
        ts.isBinaryExpression(node) &&
        node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
        ts.isPropertyAccessExpression(node.left) &&
        node.left.name.text === "className"
      ) {
        expressionTokens(node.right, found);
      } else if (ts.isCallExpression(node)) {
        const method = isClassListCall(node);
        const args = method === "toggle" ? node.arguments.slice(0, 1) : method ? node.arguments : [];
        for (const arg of args) {
          if (ts.isStringLiteral(arg) || ts.isNoSubstitutionTemplateLiteral(arg)) {
            for (const token of splitTokens(arg.text)) found.push({ token, node: arg });
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
    for (const { token, node } of found) {
      const line = sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
      const sites = used.get(token) ?? [];
      sites.push(`${file.path}:${line}`);
      used.set(token, sites);
    }
  }
  return used;
}

function extractCssClasses(sheets: string[]): Set<string> {
  const styled = new Set<string>();
  for (const sheet of sheets) {
    const text = sheet
      .replace(/\/\*[\s\S]*?\*\//g, " ")
      .replace(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/g, '""');
    let start = 0;
    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      if (ch === "{") {
        const prelude = text.slice(start, i).trim();
        if (!prelude.startsWith("@")) {
          for (const m of prelude.matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)) styled.add(m[1]);
        }
      }
      if (ch === "{" || ch === "}" || ch === ";") start = i + 1;
    }
  }
  return styled;
}

function findBareCssImports(files: SourceFile[]): string[] {
  const specs = new Set<string>();
  for (const file of files) {
    for (const stmt of parse(file).statements) {
      if (
        ts.isImportDeclaration(stmt) &&
        !stmt.importClause &&
        ts.isStringLiteral(stmt.moduleSpecifier)
      ) {
        const spec = stmt.moduleSpecifier.text;
        if (spec.endsWith(".css") && !spec.startsWith(".")) specs.add(spec);
      }
    }
  }
  return [...specs].sort();
}

function evaluate(
  used: Map<string, string[]>,
  styled: Set<string>,
  allow: Set<string>,
): { unstyled: string[]; stale: string[] } {
  const unstyled = [...used.keys()].filter((c) => !styled.has(c) && !allow.has(c)).sort();
  const stale = [...allow].filter((c) => styled.has(c) || !used.has(c)).sort();
  return { unstyled, stale };
}

function loadRealTree() {
  const sources = SRC_PATHS.filter(isSourcePath).map((path) => ({
    path,
    text: readRoot(path) ?? "",
  }));
  const bareImports = findBareCssImports(sources);
  const sheetPaths = [
    ...SRC_PATHS.filter((p) => p.endsWith(".css")),
    ...bareImports.map((spec) => `node_modules/${spec}`),
  ];
  const read = sheetPaths.map((p) => ({ p, text: readRoot(p) }));
  const missing = read.filter((r) => r.text === undefined).map((r) => r.p);
  const sheets = read.flatMap((r) => (r.text === undefined ? [] : [r.text]));
  const allowlist = JSON.parse(readRoot(ALLOWLIST_PATH) ?? "null") as Allowlist;
  return { sources, bareImports, missing, sheets, allowlist };
}

const fx = (text: string, path = "fixture.tsx"): SourceFile[] => [{ path, text }];
const usedIn = (text: string, path?: string) => [...extractUsedClasses(fx(text, path)).keys()].sort();

describe("class coverage guard (#2410)", () => {
  it("self-test: positive control flags unstyled fixture classes", () => {
    const used = extractUsedClasses(
      fx('const A = () => <div class="zz-probe other" classList={{ "zz-list": x }} />;'),
    );
    const none = new Set<string>();
    expect(evaluate(used, extractCssClasses([".other{}"]), none).unstyled).toEqual([
      "zz-list",
      "zz-probe",
    ]);
    expect(
      evaluate(used, extractCssClasses([".other{} .zz-probe{} .zz-list{}"]), none).unstyled,
    ).toEqual([]);
  });

  it("self-test: each extraction site", () => {
    expect(usedIn('const A = () => <Comp class="a  b" />;')).toEqual(["a", "b"]);
    expect(usedIn('const A = () => <div class={cond ? "p" : "q"} />;')).toEqual(["p", "q"]);
    expect(usedIn('const A = () => <div classList={{ k: x, "m n": y }} />;')).toEqual(["k", "m", "n"]);
    expect(usedIn('el.className = "cn " + (on ? "cn-on" : "");', "f.ts")).toEqual(["cn", "cn-on"]);
    expect(
      usedIn('el.classList.add("ad", v); el.classList.remove("rm"); el.classList.replace("r1", "r2");', "f.ts"),
    ).toEqual(["ad", "r1", "r2", "rm"]);
    expect(usedIn('el.classList.toggle("t", on);', "f.ts")).toEqual(["t"]);
    expect(usedIn("const A = () => <div class={`a ${b}-x c`} />;")).toEqual(["a", "c"]);
    expect(usedIn('const A = () => <div class={x === "no" ? "yes" : ""} />;')).toEqual(["yes"]);
    expect(usedIn('const A = () => <div class={cx("fn-arg")} />;')).toEqual(["fn-arg"]);

    const styled = extractCssClasses([
      ".a:not(.b) .c { color: red; }\n@media (x) { .m {} }\n/* .commented {} */",
    ]);
    expect([...styled].sort()).toEqual(["a", "b", "c", "m"]);
  });

  it("self-test: allowlist is two-way", () => {
    const used = new Map([["hook", ["f:1"]], ["styled-hook", ["f:2"]]]);
    const styled = new Set(["styled-hook"]);
    const allow = new Set(["hook", "styled-hook", "gone"]);
    expect(evaluate(used, styled, allow)).toEqual({
      unstyled: [],
      stale: ["gone", "styled-hook"],
    });
  });

  const tree = loadRealTree();
  const used = extractUsedClasses(tree.sources);
  const styled = extractCssClasses(tree.sheets);
  const allow = new Set(tree.allowlist.entries.map((e) => e.class));

  it("vacuity floors on the real tree", () => {
    expect(tree.missing).toEqual([]);
    expect(tree.sources.length).toBeGreaterThan(50);
    expect(used.size).toBeGreaterThan(500);
    expect(styled.size).toBeGreaterThan(500);
    expect(tree.bareImports).toContain(XTERM_CSS);
  });

  it("allowlist shape", () => {
    expect(tree.allowlist.version).toBe(1);
    expect(Array.isArray(tree.allowlist.entries)).toBe(true);
    for (const entry of tree.allowlist.entries) {
      expect(typeof entry.class).toBe("string");
      expect(typeof entry.reason).toBe("string");
      expect(entry.reason.trim()).not.toBe("");
    }
    const names = tree.allowlist.entries.map((e) => e.class);
    expect(new Set(names).size).toBe(names.length);
    expect(names).toEqual([...names].sort());
  });

  it("real tree: no unstyled class", () => {
    const { unstyled } = evaluate(used, styled, allow);
    const details = unstyled.map((c) => `  ${c} (${used.get(c)?.[0]})`).join("\n");
    expect(
      unstyled,
      `Unstyled classes:\n${details}\nadd a CSS rule, or an allowlist entry with a reason in ${ALLOWLIST_PATH} (use data-ac-testid for test anchors)`,
    ).toEqual([]);
  });

  it("real tree: no stale allowlist entry", () => {
    expect(evaluate(used, styled, allow).stale).toEqual([]);
  });
});
