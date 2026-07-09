import { describe, expect, it } from "vitest";

type PresentationModule = "session-status" | "replica-dot";

const IMPORT_RE = /^\s*import\b[^;]*?\bfrom\s*["']([^"']+)["']/gm;
const SELF = "sidebar/watchdog/no-presentation-import.test.ts";
const SOURCES = import.meta.glob<string>("../../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const ALLOWED: Record<PresentationModule, string[]> = {
  "session-status": [
    "shared/session-activity.test.ts",
    "sidebar/components/ProjectPanel.tsx",
    "sidebar/components/RootAgentBanner.tsx",
    "sidebar/components/SessionItem.tsx",
    "sidebar/components/replica-dot.ts",
    "sidebar/components/session-status.test.ts",
  ],
  "replica-dot": [
    "sidebar/components/ProjectPanel.tsx",
  ],
};

function rel(globKey: string): string {
  const normalized = globKey.replace(/\\/g, "/");
  if (normalized.startsWith("../../")) return normalized.slice("../../".length);
  if (normalized.startsWith("../")) return `sidebar/${normalized.slice("../".length)}`;
  if (normalized.startsWith("./")) return `sidebar/watchdog/${normalized.slice("./".length)}`;
  return normalized;
}

function presentationModule(specifier: string): PresentationModule | null {
  const normalized = specifier.replace(/\\/g, "/").replace(/\.(ts|tsx)$/, "");
  if (normalized.endsWith("session-status")) return "session-status";
  if (normalized.endsWith("replica-dot")) return "replica-dot";
  return null;
}

describe("presentation import boundary", () => {
  it("allows session-status and replica-dot only at render/test boundaries", () => {
    const actual: Record<PresentationModule, string[]> = {
      "session-status": [],
      "replica-dot": [],
    };

    for (const [file, source] of Object.entries(SOURCES)) {
      const relativeFile = rel(file);
      if (relativeFile === SELF) continue;
      for (const match of source.matchAll(IMPORT_RE)) {
        const module = presentationModule(match[1]);
        if (module) actual[module].push(relativeFile);
      }
    }

    expect({
      "session-status": actual["session-status"].sort(),
      "replica-dot": actual["replica-dot"].sort(),
    }).toEqual({
      "session-status": ALLOWED["session-status"].slice().sort(),
      "replica-dot": ALLOWED["replica-dot"].slice().sort(),
    });
  });
});
