// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { PreflightFields } from "./PreflightFields";

// The Preflight command tooltip: same at both levels, in sync with the backend tokens.

const noop = () => {};

function commandTitle(level: "agent" | "profile"): string {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(() => (
    <PreflightFields
      level={level} command={undefined} timeoutText="" timeoutInvalid={false}
      onCommand={noop} onTimeoutText={noop} onTimeoutReset={noop} onTest={noop}
      testid={`pf-${level}`}
    />
  ), host);
  const input = host.querySelector(`[data-ac-testid="pf-${level}.command"]`);
  const title = input?.getAttribute("title") ?? "";
  dispose();
  host.remove();
  return title;
}

/** `AC_PLACEHOLDER_TOKENS` values from `src-tauri/src/config/placeholders.rs`. */
function backendTokens(): string[] {
  // Variable base: Vite rewrites the literal form under jsdom (SettingsModal.jev.test.tsx).
  const moduleUrl = import.meta.url;
  const source = readFileSync(
    new URL("../../../../src-tauri/src/config/placeholders.rs", moduleUrl), "utf8",
  );
  const body = /pub const AC_PLACEHOLDER_TOKENS: &\[&str\] = &\[([^\]]*)\];/.exec(source);
  if (!body) throw new Error("placeholders.rs: AC_PLACEHOLDER_TOKENS not found");
  const ids = body[1].match(/[A-Z_][A-Z0-9_]*/g) ?? [];
  return ids.map((id) => {
    const value = new RegExp(`pub const ${id}: &str =\\s*"([^"]*)";`).exec(source);
    if (!value) throw new Error(`placeholders.rs: ${id} not found`);
    return value[1];
  });
}

describe("PreflightFields command tooltip", () => {
  it("is present and equal at agent and profile level", () => {
    const agent = commandTitle("agent");
    expect(agent).not.toBe("");
    expect(commandTitle("profile")).toBe(agent);
  });

  it("lists exactly the backend AC_PLACEHOLDER_TOKENS", () => {
    const tokens = backendTokens();
    expect(tokens.length).toBeGreaterThan(0);
    // Fails closed: a backend token outside this shape is missing from the title set.
    const shown = [...new Set(commandTitle("agent").match(/%AC_[A-Z_]+%/g) ?? [])].sort();
    expect([...new Set(tokens)].sort()).toEqual(shown);
  });

  it("explains other markers and $VAR", () => {
    const title = commandTitle("agent");
    expect(title).toContain("blocks the launch");
    expect(title).toContain("$VAR is not expanded");
    expect(title).not.toContain("%USER_HOME%");
  });
});
