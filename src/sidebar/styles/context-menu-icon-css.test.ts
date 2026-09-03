import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1708/#1731 - the context-menu icon colours are theme-following TOKENS, not
// the raw dark-theme literals the issues proposed. jsdom never applies these
// stylesheets, so the bytes on disk are the only place the rule can be pinned.
// #00d4ff, #eab308 and #22c55e are only the dark values of --sidebar-accent,
// --status-pending and --status-waiting; hard-coded they render at contrast
// 1.49, 1.61 and 1.92 on the light --sidebar-surface (#ebebf0), which is the
// failure this test exists to prevent.
//
// The third case asserts that each light-theme override EXISTS and DIFFERS from
// the :root value. It deliberately does NOT pin the bytes #0066cc, #ca8a04 and
// #16a34a. The light amber and the light green are known, disclosed contrast
// shortfalls (#1708 acceptance criterion 6, 2.47 on #ebebf0, and #1731, 2.77 on
// the same surface), so retuning any of these light values is a legitimate
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

describe("#1708/#1731 context-menu icon colours", () => {
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

  it("keeps a light-theme override for all three tokens, distinct from the dark value", () => {
    const dark = blockBody(VARS, ":root");
    const light = blockBody(VARS, "html.light-theme");
    // The verdict is built as a string per token so the assertion diff names the
    // token AND the reason. A DELETED override yields null and is reported as
    // "missing", so it cannot slip through by satisfying an inequality vacuously.
    const report = ["--sidebar-accent", "--status-pending", "--status-waiting"].map((token) => {
      const darkValue = tokenValue(dark, token);
      const lightValue = tokenValue(light, token);
      if (darkValue === null) return `${token}: missing from :root`;
      if (lightValue === null) return `${token}: missing from html.light-theme`;
      if (lightValue === darkValue) return `${token}: light value equals dark ${darkValue}`;
      return `${token}: ok`;
    });
    expect(report).toEqual(["--sidebar-accent: ok", "--status-pending: ok", "--status-waiting: ok"]);
  });

  // #1731 - the Add to Group user-plus SVG. #22c55e is only the dark value of
  // --status-waiting: hard-coded it renders at contrast 1.92 on the light
  // #ebebf0, worse than the light value #16a34a at 2.77, which is itself short
  // of the 3:1 non-text minimum. Hard-coding a darker green at the call site
  // would hide that instead of fixing the token, and is the exact failure this
  // case blocks.
  it("colours the Add to Group user-plus icon with --status-waiting, never a hex literal", () => {
    const body = blockBody(CSS, ".session-context-group-add-icon");
    expect(body).toContain("color: var(--status-waiting);");
    expect(body).not.toMatch(/#[0-9a-fA-F]{3,8}/);
  });
});
