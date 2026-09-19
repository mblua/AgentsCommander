import { describe, expect, it } from "vitest";

// #2180 — the non-stop alarm must have exactly one importer.
//
// `app.emit` from the backend is a broadcast to every window, so a second module
// that imports the alarm would double the tone in a second webview while every
// unit test stayed green. This guard scans the sources for the alarm identifiers
// and pins the closed set of files allowed to reach them.
//
// A file is recorded when BOTH hold:
//   1. it references the sound module, in a static import/export ... from whose
//      specifier ends in `sound` after the extension is stripped, or in a
//      dynamic import("...") with such a specifier; and
//   2. its source text mentions at least one of the three alarm identifiers.
// The named-binding clause is deliberately not parsed: a clause parser is exact
// for `import { x } from "..."` and blind to `import * as sound from "..."` and
// to a dynamic import, both of which reach the same functions. Condition 2 is
// what keeps primeAudio, playTeamIdleBeep and setSoundsEnabled importers out.
//
// Declared limit: fully computed access (`sound[someVariable]`) is invisible to a
// text scan, and no static guard in this repo can see it. Accepted as residual risk.
const IMPORT_RE = /^\s*(?:import|export)\b[^;]*?\bfrom\s*["']([^"']+)["']/gm;
const DYNAMIC_IMPORT_RE = /\bimport\s*\(\s*["']([^"']+)["']/g;
const SELF = "sidebar/watchdog/no-alarm-import.test.ts";
const SOURCES = import.meta.glob<string>("../../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const ALARM_IDENTIFIERS = [
  "playNonStopAlarm",
  "stopNonStopAlarm",
  "stopAllNonStopAlarms",
];

const ALLOWED = [
  "shared/sound.test.ts",
  "sidebar/watchdog/non-stop-watchdog-client.test.ts",
  "sidebar/watchdog/non-stop-watchdog-client.ts",
];

function rel(globKey: string): string {
  const normalized = globKey.replace(/\\/g, "/");
  if (normalized.startsWith("../../")) return normalized.slice("../../".length);
  if (normalized.startsWith("../")) return `sidebar/${normalized.slice("../".length)}`;
  if (normalized.startsWith("./")) return `sidebar/watchdog/${normalized.slice("./".length)}`;
  return normalized;
}

function soundModule(specifier: string): boolean {
  const normalized = specifier.replace(/\\/g, "/").replace(/\.(ts|tsx|js|jsx|mjs)$/, "");
  return normalized.endsWith("sound");
}

function mentionsAlarm(source: string): boolean {
  return ALARM_IDENTIFIERS.some((identifier) => source.includes(identifier));
}

describe("non-stop alarm import boundary", () => {
  it("allows the alarm only in sound.ts, its test and the single listener", () => {
    const actual = new Set<string>();

    for (const [file, source] of Object.entries(SOURCES)) {
      const relativeFile = rel(file);
      if (relativeFile === SELF) continue;

      let referencesSound = false;
      for (const match of source.matchAll(IMPORT_RE)) {
        if (soundModule(match[1])) referencesSound = true;
      }
      for (const match of source.matchAll(DYNAMIC_IMPORT_RE)) {
        if (soundModule(match[1])) referencesSound = true;
      }
      if (!referencesSound) continue;

      if (mentionsAlarm(source)) actual.add(relativeFile);
    }

    expect(Array.from(actual).sort()).toEqual(ALLOWED.slice().sort());
  });
});
