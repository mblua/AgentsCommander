// Isolated build for the #4248 WebKitGTK renderer fixture. Test-only; never shipped.
// Usage: npx --no-install vite build --config scripts/ac4248-fixture.vite.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export default defineConfig({
  configFile: false,
  root: path.join(repoRoot, "tests", "fixtures", "ac4248-snapshot"),
  base: "./",
  publicDir: false,
  plugins: [],
  build: {
    outDir: path.join(repoRoot, "target", "ac4248-fixture"),
    emptyOutDir: true,
  },
});
