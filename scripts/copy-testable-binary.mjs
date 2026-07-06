import { copyFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const releaseDir = path.join(repoRoot, "target", "release");
const source = path.join(releaseDir, "agentscommander.exe");
const destination = path.join(releaseDir, "agentscommander_testeable.exe");

try {
  const sourceStat = await stat(source);
  if (!sourceStat.isFile()) {
    console.error(`Expected production binary to be a file: ${source}`);
    process.exit(1);
  }
} catch (error) {
  console.error(`Production binary is missing: ${source}`);
  console.error(`Run the production Tauri build before copying the testable binary.`);
  if (error?.message) {
    console.error(error.message);
  }
  process.exit(1);
}

await copyFile(source, destination);
console.log(`Copied production binary: ${source}`);
console.log(`Created testable binary: ${destination}`);
