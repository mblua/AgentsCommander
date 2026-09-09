// #1871 section 10.4 - the catalogue imports no store, no ipc, no presentation
// module and no other component: two EXACT ALLOWLISTS over raw source, on the
// model of no-presentation-import.test.ts. Raw source is what makes this immune
// to import elision; `toEqual` against a written-out literal is what makes an
// unexpected import a failure rather than a silent pass. The negative-rule
// form ("no file matches these forbidden specifiers") is deliberately not used
// anywhere in this file: a violations list that comes out empty passes for any
// specifier nobody thought to forbid.
//
// Extraction is PARSER-BACKED, not a regex over text. Two regex generations
// were defeated by legal syntax (an import after `do {} while(false)`, comment
// trivia between `import` and its specifier) and by a false boundary (a `;`
// inside a string literal). A pattern over raw text cannot tell a declaration
// from a comment or a string in general; the TypeScript parser can, and it is
// already a devDependency (`npm run typecheck`). Collected, per file: import
// declarations (`import x from`, `import type`, bare `import "x"`),
// re-exports with a specifier (`export ... from`), `import x = require("x")`,
// `typeof import("x")` type queries, and dynamic `import("x")` calls with a
// literal argument. Comments and string contents are never specifiers.
import ts from "typescript";
import { describe, expect, it } from "vitest";

const SELF = "sidebar/watchdog/context-menu-import-boundary.test.ts";
const SOURCES = import.meta.glob<string>("../../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const CONTEXT_MENU_DIR = "sidebar/components/context-menu/";

/** Assertion 1: exactly these four product files, exactly these specifiers. */
const ALLOWED_IMPORTS: Record<string, string[]> = {
  "sidebar/components/context-menu/ContextMenuSurface.tsx": ["solid-js", "solid-js/web"],
  "sidebar/components/context-menu/SessionRowMenu.tsx": [
    "../DetachIcon",
    "../ReattachIcon",
    "../TelegramIcon",
    "../TrashIcon",
    "../UserPlusIcon",
    "./ContextMenuSurface",
    "./session-row-menu-types",
    "solid-js",
    "solid-js/web",
  ],
  "sidebar/components/context-menu/session-row-menu-specs.ts": [
    "../../../shared/types",
    "./session-row-menu-types",
  ],
  "sidebar/components/context-menu/session-row-menu-types.ts": ["../../../shared/types"],
};

/** Assertion 2: ContextMenuSurface is private to the folder. One element, not
 *  two: the surface test is black-box through SessionRowMenu and imports
 *  nothing named ContextMenuSurface. */
const ALLOWED_SURFACE_IMPORTERS = ["sidebar/components/context-menu/SessionRowMenu.tsx"];

function rel(globKey: string): string {
  const normalized = globKey.replace(/\\/g, "/");
  if (normalized.startsWith("../../")) return normalized.slice("../../".length);
  if (normalized.startsWith("../")) return `sidebar/${normalized.slice("../".length)}`;
  if (normalized.startsWith("./")) return `sidebar/watchdog/${normalized.slice("./".length)}`;
  return normalized;
}

/** Strip the trivia wrappers an expression can carry without changing what it
 *  names: `(x)`, `x as T`, `<T>x`, `x satisfies T`, `x!`. Loops so a wrapper
 *  around a wrapper is stripped too. */
function unwrapExpression(node: ts.Expression): ts.Expression {
  let current = node;
  for (;;) {
    if (
      ts.isParenthesizedExpression(current) ||
      ts.isAsExpression(current) ||
      ts.isTypeAssertionExpression(current) ||
      ts.isSatisfiesExpression(current) ||
      ts.isNonNullExpression(current)
    ) {
      current = current.expression;
      continue;
    }
    return current;
  }
}

/** Every module specifier the file names, from the syntax tree. Throws, so the
 *  assertion fails loudly instead of the allowlist passing on a partial tree,
 *  when the source has a parse error or names a dynamic import whose argument
 *  is not a string literal after unwrapping (an identifier, a template with
 *  substitutions, or a wrapper kind nobody has listed yet). */
export function specifiersOf(source: string, fileName = "probe.tsx"): string[] {
  const kind = fileName.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, false, kind);
  const diagnostics =
    (file as unknown as { parseDiagnostics?: ts.DiagnosticWithLocation[] }).parseDiagnostics ?? [];
  if (diagnostics.length > 0) {
    const first = diagnostics[0];
    throw new Error(
      `${fileName}: parse error at offset ${first.start}: ` +
        ts.flattenDiagnosticMessageText(first.messageText, "\n"),
    );
  }
  const found = new Set<string>();
  const visit = (node: ts.Node): void => {
    if (ts.isImportDeclaration(node) && ts.isStringLiteral(node.moduleSpecifier)) {
      found.add(node.moduleSpecifier.text);
    } else if (
      ts.isExportDeclaration(node) &&
      node.moduleSpecifier !== undefined &&
      ts.isStringLiteral(node.moduleSpecifier)
    ) {
      found.add(node.moduleSpecifier.text);
    } else if (
      ts.isImportEqualsDeclaration(node) &&
      ts.isExternalModuleReference(node.moduleReference) &&
      ts.isStringLiteral(node.moduleReference.expression)
    ) {
      found.add(node.moduleReference.expression.text);
    } else if (
      ts.isImportTypeNode(node) &&
      ts.isLiteralTypeNode(node.argument) &&
      ts.isStringLiteral(node.argument.literal)
    ) {
      found.add(node.argument.literal.text);
    } else if (
      ts.isCallExpression(node) &&
      node.expression.kind === ts.SyntaxKind.ImportKeyword
    ) {
      const argument = node.arguments.length > 0 ? unwrapExpression(node.arguments[0]) : undefined;
      if (argument !== undefined && ts.isStringLiteralLike(argument)) {
        found.add(argument.text);
      } else {
        // Unclassified dependency: never drop it silently.
        throw new Error(
          `${fileName}: dynamic import at offset ${node.getStart(file)} has no string-literal ` +
            `specifier (${argument === undefined ? "no argument" : ts.SyntaxKind[argument.kind]})`,
        );
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return Array.from(found).sort();
}

function isTestFile(relativeFile: string): boolean {
  return /\.test\.tsx?$/.test(relativeFile);
}

function endsWithContextMenuSurface(specifier: string): boolean {
  const normalized = specifier.replace(/\\/g, "/").replace(/\.(ts|tsx|js|jsx|mjs)$/, "");
  return normalized.endsWith("ContextMenuSurface");
}

describe("#1871 context-menu import boundary", () => {
  it("context-menu/ product files import exactly the allowed specifiers", () => {
    const actual: Record<string, string[]> = {};
    for (const [file, source] of Object.entries(SOURCES)) {
      const relativeFile = rel(file);
      if (relativeFile === SELF) continue;
      if (!relativeFile.startsWith(CONTEXT_MENU_DIR)) continue;
      if (isTestFile(relativeFile)) continue;
      actual[relativeFile] = specifiersOf(source, relativeFile);
    }

    const sortedActual = Object.fromEntries(
      Object.keys(actual)
        .sort()
        .map((key) => [key, actual[key]]),
    );
    const sortedExpected = Object.fromEntries(
      Object.keys(ALLOWED_IMPORTS)
        .sort()
        .map((key) => [key, ALLOWED_IMPORTS[key].slice().sort()]),
    );
    expect(sortedActual).toEqual(sortedExpected);
  });

  it("ContextMenuSurface is imported by SessionRowMenu.tsx and nothing else", () => {
    const importers = new Set<string>();
    for (const [file, source] of Object.entries(SOURCES)) {
      const relativeFile = rel(file);
      if (relativeFile === SELF) continue;
      for (const specifier of specifiersOf(source, relativeFile)) {
        if (endsWithContextMenuSurface(specifier)) importers.add(relativeFile);
      }
    }
    expect(Array.from(importers).sort()).toEqual(ALLOWED_SURFACE_IMPORTERS.slice().sort());
  });

  // The extractor's own contract, as fixtures. These are cases, not controls:
  // the false-positive rows prove that a `;` or an `import` inside a string or
  // a comment does NOT change the collected set, which is what keeps correct
  // code green; the declaration rows prove every legal placement is collected.
  describe("extraction distinguishes declarations from comments and strings", () => {
    const BASE = 'import { a } from "./a";\n';
    const STORE = "../../stores/sessions";

    it.each([
      ["a string literal containing `; import \"...\";`", `const example = '; import "${STORE}";';`],
      ["a line comment", `// import "${STORE}";`],
      ["a block comment", `/* import "${STORE}"; */`],
      ["a template literal", "const t = `}\nimport \"" + STORE + "\";`;"],
    ])("does not collect from %s", (_name, line) => {
      expect(specifiersOf(BASE + line + "\n")).toEqual(specifiersOf(BASE));
      expect(specifiersOf(BASE + line + "\n")).toEqual(["./a"]);
    });

    it.each([
      ["a bare side-effect import at line start", `import "${STORE}";`],
      ["a side-effect import after a statement on the same line", `const x = 1; import "${STORE}";`],
      ["a side-effect import after a block comment", `/* review */ import "${STORE}";`],
      ["an import after do-while ASI", `do {} while(false) import "${STORE}";`],
      ["comment trivia between import and its specifier", `import /* comment */ "${STORE}";`],
      ["a type-only import after a statement", `const x = 1; import type { s } from "${STORE}";`],
      ["a re-export", `export { s } from "${STORE}";`],
      ["a star re-export", `export * from "${STORE}";`],
      ["a dynamic import", `const p = () => import("${STORE}");`],
      ["a dynamic import with a parenthesised argument", `const p = () => import(("${STORE}"));`],
      ["a dynamic import with an `as` cast", `const p = () => import("${STORE}" as string);`],
      ["a dynamic import with a `satisfies` wrapper", `const p = () => import("${STORE}" satisfies string);`],
      ["a dynamic import with a non-null wrapper", `const p = () => import("${STORE}"!);`],
      ["a dynamic import with nested wrappers", `const p = () => import((("${STORE}" as string)!));`],
      ["a typeof import type query", `type T = typeof import("${STORE}");`],
      ["an import-equals of a require", `import s = require("${STORE}");`],
    ])("collects %s", (_name, line) => {
      expect(specifiersOf(BASE + line + "\n")).toEqual(["../../stores/sessions", "./a"]);
    });

    // Loud failure, never a silent drop: a partial tree or an unclassified
    // dynamic import throws, so assertion 1 cannot pass on it.
    it("throws on a source with a parse error instead of walking the recovery tree", () => {
      expect(() => specifiersOf("const x = ;\n")).toThrow(/parse error/);
    });

    it.each([
      ["an identifier argument", `const p = (m: string) => import(m);`],
      ["a template with substitutions", "const p = (m: string) => import(`./${m}`);"],
      ["no argument", `const p = () => import();`],
    ])("throws on a dynamic import with %s", (_name, line) => {
      expect(() => specifiersOf(BASE + line + "\n")).toThrow(/no string-literal specifier/);
    });
  });
});
