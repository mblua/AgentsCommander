// #4300 - every raw-HTML sink in production code must be on ALLOWED,
// scanning src/**/*.ts and src/**/*.tsx, except test files. Comments are scanned too.
// Text scans miss computed properties, computed spread keys, and aliases such as d.write(x).
// javascript: URL sinks are out of scope. Substring evidence needs human review on every edit.
// The node:fs augmentation is program-wide, narrowed to URL for static source guards only;
// like src/vite-env.d.ts, it must never be used by application code.
import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

declare module "node:fs" {
  export function readdirSync(path: URL, o: { recursive: true; encoding: "utf8" }): string[];
}

const SINKS = [
  "innerHTML", "outerHTML", "insertAdjacentHTML", "createContextualFragment",
  "setHTMLUnsafe", "parseHTMLUnsafe", "parseFromString", "dangerouslySetInnerHTML",
];
const WRITE = String.raw`\bdocument\s*\.\s*write(?:ln)?\s*\(`;
const SINK_RE = new RegExp(String.raw`\b(?:${SINKS.join("|")})\b|${WRITE}`, "g");
// srcdoc (setAttribute lower-cases names) and execCommand("insertHTML") ignore case.
const SINK_RE_I = /\b(?:srcdoc|inserthtml)\b/gi;

const ALLOWED: Record<string, { count: number; evidence: string[] }> = {
  "main/components/HomeView.tsx": {
    count: 1,
    evidence: [
      'import DOMPurify from "dompurify";', "DOMPurify.sanitize(", "innerHTML={html()}",
    ],
  },
  "spec-board/components/MermaidPreview.tsx": {
    count: 1,
    evidence: ['securityLevel: "strict"', "containerRef.innerHTML = svg;"],
  },
};
const SRC = new URL("../", import.meta.url);
const SOURCES: Record<string, string> = Object.fromEntries(
  Object.keys(import.meta.glob("../**/*.{ts,tsx}"))
    .map((key) => [
      new URL(key, import.meta.url).href.slice(SRC.href.length),
      readFileSync(new URL(key, import.meta.url), "utf8"),
    ])
    .filter(([path]) => !/\.test\.tsx?$/.test(path)),
);
const WALKED: string[] = readdirSync(SRC, { recursive: true, encoding: "utf8" })
  .map((p) => p.replaceAll("\\", "/"))
  .filter((p) => /\.tsx?$/.test(p))
  .filter((p) => !/\.test\.tsx?$/.test(p))
  .sort();

function countSinks(text: string): number {
  return [...text.matchAll(SINK_RE)].length + [...text.matchAll(SINK_RE_I)].length;
}

function violations(sources: Record<string, string>): string[] {
  const found: string[] = [];
  for (const path of Object.keys(sources).sort()) {
    const text = sources[path];
    const n = countSinks(text);
    const allowed = ALLOWED[path];
    if (!allowed) {
      if (n > 0) found.push(`${path}: ${n} raw-HTML sink(s) not allowed`);
      continue;
    }
    if (n !== allowed.count) {
      found.push(`${path}: expected ${allowed.count} sink(s), found ${n}`);
    }
    for (const evidence of allowed.evidence) {
      if (!text.includes(evidence)) found.push(`${path}: missing "${evidence}"`);
    }
  }
  for (const path of Object.keys(ALLOWED).sort()) {
    if (!(path in sources)) found.push(`${path}: allowed file not found`);
  }
  return found;
}

describe("#4300 raw-HTML sinks", () => {
  it("S1 scan sanity", () => {
    expect(Object.keys(SOURCES).sort()).toEqual(WALKED);
    expect(Object.keys(SOURCES).length).toBeGreaterThan(100);
    for (const path of ["main.tsx", "shared/sound.ts", ...Object.keys(ALLOWED)]) {
      expect(SOURCES).toHaveProperty(path);
    }
    expect(Object.keys(SOURCES).some((path) => /\.test\.tsx?$/.test(path))).toBe(false);
  });

  it("S2 production code", () => {
    expect(violations(SOURCES)).toEqual([]);
  });

  it("S3 detector positive/negative controls", () => {
    for (const text of [
      "el.innerHTML = x", "<div innerHTML={y} />", "el.outerHTML = x",
      'el.insertAdjacentHTML("beforeend", x)', "f.srcdoc = x",
      "range.createContextualFragment(x)", "el.setHTMLUnsafe(x)",
      "Document.parseHTMLUnsafe(x)", "dangerouslySetInnerHTML", "document.write(x)",
      "document.writeln(x)", "document . write(x)", 'el.setAttribute("srcDoc", x)',
      "<iframe SRCDOC={x} />", 'document.execCommand("insertHTML", false, x)',
      'execCommand("InsertHtml", false, x)', 'p.parseFromString(x, "text/html")',
    ]) expect(countSinks(text), text).toBe(1);
    for (const text of [
      "el.textContent = x", "innerHtml", "myinnerHTMLx", "document.title",
      "document.writer(x)", "srcdocs", "parsefromstring",
    ]) expect(countSinks(text), text).toBe(0);
  });

  it("S4 checker mutation controls", () => {
    const home = "main/components/HomeView.tsx";
    const mermaid = "spec-board/components/MermaidPreview.tsx";
    expect(violations({
      ...SOURCES, "shared/__probe.ts": "document.body.innerHTML = location.hash;",
    })).toEqual(["shared/__probe.ts: 1 raw-HTML sink(s) not allowed"]);
    expect(violations({
      ...SOURCES, [home]: SOURCES[home].replace("DOMPurify.sanitize(", "String("),
    })).toEqual([`${home}: missing "DOMPurify.sanitize("`]);
    expect(violations({
      ...SOURCES, [mermaid]: SOURCES[mermaid].replace('"strict"', '"loose"'),
    })).toEqual([`${mermaid}: missing "securityLevel: "strict""`]);
    expect(violations({
      ...SOURCES, [mermaid]: SOURCES[mermaid] + "\nel.outerHTML = s;",
    })).toEqual([`${mermaid}: expected 1 sink(s), found 2`]);
    const withoutHome = { ...SOURCES };
    delete withoutHome[home];
    expect(violations(withoutHome)).toEqual([`${home}: allowed file not found`]);
  });
});
