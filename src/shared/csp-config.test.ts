import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const readConfig = (name: string) =>
  JSON.parse(readFileSync(new URL(`../../src-tauri/${name}`, import.meta.url), "utf8"));
const security = readConfig("tauri.conf.json").app.security;
const expectedPolicy = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: http: https:",
  "font-src 'self' data:",
  "connect-src 'self' ipc: http://ipc.localhost",
  "object-src 'none'",
  "base-uri 'self'",
  "frame-src 'none'",
  "form-action 'none'",
].join("; ");
const directives = new Map<string, string[]>(
  security.csp.split(";").map((part: string) => {
    const [name, ...tokens] = part.trim().split(/\s+/);
    return [name, tokens];
  }),
);

describe("packaged CSP configuration", () => {
  it("pins the complete policy", () => {
    expect(security.csp).toBe(expectedPolicy);
  });
  it("allows inline styles while keeping scripts restricted", () => {
    expect(directives.get("script-src")).toEqual(["'self'"]);
    for (const [name, tokens] of directives) {
      expect(tokens).not.toContain("'unsafe-eval'");
      if (name !== "style-src") expect(tokens).not.toContain("'unsafe-inline'");
    }
    expect(directives.get("style-src")).toContain("'unsafe-inline'");
  });
  it("preserves IPC and image sources while blocking objects", () => {
    expect(directives.get("connect-src")).toEqual(
      expect.arrayContaining(["ipc:", "http://ipc.localhost"]),
    );
    expect(directives.get("img-src")).toEqual(
      expect.arrayContaining(["data:", "http:", "https:"]),
    );
    expect(directives.get("img-src")).not.toContain("blob:");
    expect(directives.get("object-src")).toEqual(["'none'"]);
  });
  it("keeps Tauri from adding nonces to the inline style directive", () => {
    expect(security.dangerousDisableAssetCspModification).toEqual(["style-src"]);
  });
  it("leaves desktop development without a dev CSP", () => {
    expect(security).not.toHaveProperty("devCsp");
  });
  it("inherits base app security in both packaged profiles", () => {
    for (const name of ["tauri.prod.conf.json", "tauri.stage.conf.json"]) {
      expect(readConfig(name)).not.toHaveProperty("app");
    }
  });
});
