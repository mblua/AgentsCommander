/// <reference types="vite/client" />

declare module "*.png" {
  const src: string;
  export default src;
}

/* #1167 - node:fs for static source guards ONLY (today: the ones in
   src/sidebar/styles/agent-badge-css.test.ts and
   src/sidebar/styles/coord-quick-access-css.test.ts, which have to read real stylesheet
   bytes because Vitest replaces every CSS module with `export default ""` unless
   test.css is enabled). @types/node is deliberately not a dependency of this
   frontend, so the single function that guard needs is declared here, next to the
   other ambient module declarations. It is narrowed to a URL argument on purpose.
   Do NOT import node:fs from application code: there is no fs in the Tauri
   webview, and this declaration is not permission to pretend otherwise. */
declare module "node:fs" {
  export function readFileSync(path: URL, encoding: "utf8"): string;
}
