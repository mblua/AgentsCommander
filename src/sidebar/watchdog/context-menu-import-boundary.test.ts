// #1871 section 10.4 - the catalogue imports no store, no ipc, no presentation
// module and no other component: two EXACT ALLOWLISTS over raw source, on the
// model of no-presentation-import.test.ts. Raw source is what makes this immune
// to import elision; `toEqual` against a written-out literal is what makes an
// unexpected import a failure rather than a silent pass. The negative-rule
// form ("no file matches these forbidden specifiers") is deliberately not used
// anywhere in this file: a violations list that comes out empty passes for any
// specifier nobody thought to forbid.
import { describe, expect, it } from "vitest";

const IMPORT_RE = /^\s*(?:import|export)\b[^;]*?\bfrom\s*["']([^"']+)["']/gm;
const DYNAMIC_IMPORT_RE = /\bimport\s*\(\s*["']([^"']+)["']/g;
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

function specifiersOf(source: string): string[] {
  const found = new Set<string>();
  for (const match of source.matchAll(IMPORT_RE)) found.add(match[1]);
  for (const match of source.matchAll(DYNAMIC_IMPORT_RE)) found.add(match[1]);
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
      actual[relativeFile] = specifiersOf(source);
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
      for (const specifier of specifiersOf(source)) {
        if (endsWithContextMenuSurface(specifier)) importers.add(relativeFile);
      }
    }
    expect(Array.from(importers).sort()).toEqual(ALLOWED_SURFACE_IMPORTERS.slice().sort());
  });
});
