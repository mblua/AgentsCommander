import { describe, expect, it } from "vitest";
import { SESSION_WARNING_KINDS } from "./types";

const BACKEND_WARNING_KIND_SOURCES = import.meta.glob<string>(
  "../../src-tauri/src/{pty/container_paths,session/warnings}.rs",
  {
    query: "?raw",
    import: "default",
    eager: true,
  }
);

function backendWarningKinds(): string[] {
  const kinds = new Set<string>();
  const pattern = /pub const WARNING_KIND_[A-Z0-9_]+:\s*&str\s*=\s*"([^"]+)";/g;

  for (const source of Object.values(BACKEND_WARNING_KIND_SOURCES)) {
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(source)) !== null) {
      kinds.add(match[1]);
    }
  }

  return [...kinds].sort();
}

describe("SESSION_WARNING_KINDS", () => {
  it("matches the backend warning-kind constants", () => {
    expect([...SESSION_WARNING_KINDS].sort()).toEqual(backendWarningKinds());
  });
});
