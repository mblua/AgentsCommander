// #4945 opt-in fixture variants for the ActionBar and OpenAgentModal accessibility suites.
// Only the exact importer/specifier pairs below resolve to a synthetic module. Every other
// import falls through, so ordinary mounts keep the real component.
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Plugin } from "vite";

const COMPONENTS_DIR = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../components",
);

export const ISSUE_4945_CONFIRM_PATH = "C:/issue-4945/fixture-project";
export const ISSUE_4945_SELECTED_REPO_EVENT = "issue-4945:selected-repo";

const CONFIRM_DECLARATION =
  "const [confirmPath, setConfirmPath] = createSignal<string | null>(null);";
const SELECTED_REPO_DECLARATION =
  "const [selectedRepo, setSelectedRepo] = " +
  "createSignal<RepoMatch | null>(props.initialRepo ?? null);";

interface Variant {
  specifier: string;
  suite: string;
  source: string;
  transform: (code: string) => string;
}

function replaceExactlyOnce(code: string, anchor: string, replacement: string): string {
  const first = code.indexOf(anchor);
  if (first < 0 || code.indexOf(anchor, first + anchor.length) >= 0) {
    throw new Error(`issue-4945 fixture: anchor must occur exactly once: ${anchor}`);
  }
  return code.slice(0, first) + replacement + code.slice(first + anchor.length);
}

const VARIANTS: Variant[] = [
  {
    specifier: "@sidebar/components/ActionBar",
    suite: "ActionBar.accessibility.test.tsx",
    source: "ActionBar.tsx",
    transform: (code) =>
      replaceExactlyOnce(
        code,
        CONFIRM_DECLARATION,
        CONFIRM_DECLARATION.replace(
          "(null);",
          `(${JSON.stringify(ISSUE_4945_CONFIRM_PATH)});`,
        ),
      ),
  },
  {
    specifier: "@sidebar/components/OpenAgentModal",
    suite: "OpenAgentModal.accessibility.test.tsx",
    source: "OpenAgentModal.tsx",
    transform: (code) =>
      'import { createEffect as issue4945Effect } from "solid-js";\n' +
      replaceExactlyOnce(
        code,
        SELECTED_REPO_DECLARATION,
        `${SELECTED_REPO_DECLARATION}\n  issue4945Effect(() => {\n` +
          "    document.dispatchEvent(\n" +
          `      new CustomEvent(${JSON.stringify(ISSUE_4945_SELECTED_REPO_EVENT)}, {\n` +
          "        detail: selectedRepo()?.path ?? null,\n" +
          "      }),\n" +
          "    );\n" +
          "  });",
      ),
  },
];

function normalizeId(id: string): string {
  const bare = id.split("?")[0]!.split("#")[0]!;
  return path.resolve(bare).split(path.sep).join("/");
}

function syntheticId(variant: Variant): string {
  const base = variant.source.replace(/\.tsx$/, "");
  return normalizeId(path.join(COMPONENTS_DIR, `${base}.issue-4945-fixture.tsx`));
}

export interface Issue4945FixtureOptions {
  // Extra importers allowed to load the variants (the ignored CDP browser entry module).
  browserImporters?: string[];
}

export function issue4945FixtureVariants(options: Issue4945FixtureOptions = {}): Plugin {
  const extra = new Set((options.browserImporters ?? []).map(normalizeId));
  const bySynthetic = new Map(VARIANTS.map((variant) => [syntheticId(variant), variant]));

  return {
    name: "issue-4945-fixture-variants",
    enforce: "pre",
    resolveId(source, importer) {
      if (!importer) return null;
      const normalizedImporter = normalizeId(importer);
      for (const variant of VARIANTS) {
        if (source !== variant.specifier) continue;
        const suite = normalizeId(path.join(COMPONENTS_DIR, variant.suite));
        if (normalizedImporter === suite || extra.has(normalizedImporter)) {
          return syntheticId(variant);
        }
      }
      return null;
    },
    load(id) {
      const variant = bySynthetic.get(normalizeId(id));
      if (!variant || normalizeId(id) !== id) return null;
      const original = readFileSync(path.join(COMPONENTS_DIR, variant.source), "utf8");
      return variant.transform(original);
    },
  };
}
