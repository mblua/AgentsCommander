/**
 * #1283 resolved TypeScript graph gate (Section 10.2 / 14.6).
 *
 * Runs against the complete `src` root and against the negative fixture suite
 * in `scripts/fixtures/frontend-dependency-cycle` with the SAME rules and
 * resolver settings. The one-way rule `no-terminal-helper-back-edge` covers
 * both the two production helper paths and the two identically named seam
 * fixtures, and forbids either helper from reaching TerminalView.tsx, any
 * sidebar/UI sibling, `src/shared/ipc.ts`, `@tauri-apps/api`, or the other
 * helper.
 *
 * @type {import("dependency-cruiser").IConfiguration}
 */
export default {
  forbidden: [
    {
      name: "no-unresolved",
      severity: "error",
      from: {},
      to: { couldNotResolve: true },
    },
    {
      name: "no-circular",
      severity: "error",
      from: {},
      to: { circular: true },
    },
    {
      name: "no-terminal-helper-back-edge",
      severity: "error",
      from: {
        path: "^(src/terminal/components|scripts/fixtures/frontend-dependency-cycle/seams)/(terminal-session-registry|terminal-output-admission)\\.ts$",
      },
      to: {
        path: "([/\\\\]TerminalView\\.tsx$" +
          "|^src/(sidebar|browser|guide|main|resource-monitor|screenshot-overlay|spec-board|watchers)/" +
          "|^src/terminal/(components|stores)/" +
          "|^src/shared/ipc\\.ts$" +
          "|^scripts/fixtures/frontend-dependency-cycle/seams/(terminal-session-registry|terminal-output-admission|sidebar|ipc)\\.ts$" +
          "|^node_modules/@tauri-apps/api/)",
      },
    },
  ],
  options: {
    tsConfig: { fileName: "tsconfig.json" },
    tsPreCompilationDeps: true,
    // doNotFollow keeps resolved external edges (e.g. @tauri-apps/api) in the
    // graph so the one-way helper rule can forbid them; `exclude` would drop
    // the edge before any rule sees it.
    doNotFollow: { path: "node_modules" },
    enhancedResolveOptions: {
      exportsFields: ["exports"],
      conditionNames: ["import", "require", "node", "default", "types"],
      mainFields: ["module", "main", "types", "typings"],
      extensions: [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json"],
    },
  },
};
