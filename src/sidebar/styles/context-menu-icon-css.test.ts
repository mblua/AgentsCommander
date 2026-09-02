import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1708 - the context-menu icon colours are theme-following TOKENS, not the raw
// dark-theme literals the issue proposed. jsdom never applies these stylesheets,
// so the bytes on disk are the only place the rule can be pinned. #00d4ff and
// #eab308 are only the dark values of --sidebar-accent and --status-pending;
// hard-coded they render at contrast 1.49 and 1.61 on the light
// --sidebar-surface (#ebebf0), which is the failure this test exists to prevent.
//
// The third case asserts that each light-theme override EXISTS and DIFFERS from
// the :root value. It deliberately does NOT pin the bytes #0066cc and #ca8a04.
// The light amber is a known, disclosed contrast shortfall (#1708 acceptance
// criterion 6, 2.47 on #ebebf0), so retuning either light value is a legitimate
// follow-up; a byte pin would make that follow-up fail here with a message that
// says nothing about contrast. Presence is asserted separately from inequality
// because a DELETED override would otherwise satisfy the inequality vacuously,
// and a deleted override is the exact regression this case exists to catch.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const VARS = readFileSync(new URL("./variables.css", import.meta.url), "utf8");

function blockBody(source: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = source.match(new RegExp(`^${escaped} \\{([^}]*)\\}`, "m"));
  if (!match) throw new Error(`missing block: ${selector}`);
  return match[1];
}

function tokenValue(body: string, token: string): string | null {
  const match = body.match(new RegExp(`${token}:\\s*([^;]+);`));
  return match ? match[1].trim() : null;
}

describe("#1708 context-menu icon colours", () => {
  it("colours the detach icon with --sidebar-accent, never a hex literal", () => {
    const body = blockBody(CSS, ".session-context-detach-icon");
    expect(body).toContain("color: var(--sidebar-accent);");
    expect(body).not.toMatch(/#[0-9a-fA-F]{3,8}/);
  });

  it("colours the TASK-title pencil with --status-pending, never a hex literal", () => {
    const body = blockBody(CSS, ".session-context-task-icon");
    expect(body).toContain("color: var(--status-pending);");
    expect(body).not.toMatch(/#[0-9a-fA-F]{3,8}/);
  });

  it("keeps a light-theme override for both tokens, distinct from the dark value", () => {
    const dark = blockBody(VARS, ":root");
    const light = blockBody(VARS, "html.light-theme");
    // The verdict is built as a string per token so the assertion diff names the
    // token AND the reason. A DELETED override yields null and is reported as
    // "missing", so it cannot slip through by satisfying an inequality vacuously.
    const report = ["--sidebar-accent", "--status-pending"].map((token) => {
      const darkValue = tokenValue(dark, token);
      const lightValue = tokenValue(light, token);
      if (darkValue === null) return `${token}: missing from :root`;
      if (lightValue === null) return `${token}: missing from html.light-theme`;
      if (lightValue === darkValue) return `${token}: light value equals dark ${darkValue}`;
      return `${token}: ok`;
    });
    expect(report).toEqual(["--sidebar-accent: ok", "--status-pending: ok"]);
  });
});
