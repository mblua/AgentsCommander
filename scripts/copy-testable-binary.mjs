import { copyFile, mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SOURCE_NAME = "agentscommander.exe";
const DESTINATION_NAME = "agentscommander_testeable.exe";

const USAGE = `Usage: node scripts/copy-testable-binary.mjs [--self-test] [--help]

Copies the production Tauri binary to the testable binary name. The testable
binary is Windows-only by contract (src-tauri/src/testability/), so on any other
platform this is a no-op that exits 0.

  --self-test  Run the in-memory self-test (5 cases).
  --help       Print this usage and exit 0.`;

export async function runCopyTestableBinary({ platform, releaseDir }) {
  const logs = [];
  const errors = [];

  if (platform !== "win32") {
    logs.push(
      `Skipping testable binary copy on ${platform}: the testable binary is Windows-only.`,
    );
    return { code: 0, logs, errors };
  }

  const source = path.join(releaseDir, SOURCE_NAME);
  const destination = path.join(releaseDir, DESTINATION_NAME);

  try {
    const sourceStat = await stat(source);
    if (!sourceStat.isFile()) {
      errors.push(`Expected production binary to be a file: ${source}`);
      return { code: 1, logs, errors };
    }
  } catch (error) {
    errors.push(`Production binary is missing: ${source}`);
    errors.push(`Run the production Tauri build before copying the testable binary.`);
    if (error?.message) {
      errors.push(error.message);
    }
    return { code: 1, logs, errors };
  }

  try {
    await copyFile(source, destination);
  } catch (error) {
    errors.push(error?.message ?? String(error));
    return { code: 1, logs, errors };
  }

  logs.push(`Copied production binary: ${source}`);
  logs.push(`Created testable binary: ${destination}`);
  return { code: 0, logs, errors };
}

async function selfTest() {
  const failures = [];
  const root = await mkdtemp(path.join(os.tmpdir(), "copy-testable-binary-"));
  const check = (name, condition, expected, actual) => {
    if (!condition) failures.push(`${name}: expected ${expected}, got ${actual}`);
  };

  try {
    for (const platform of ["linux", "darwin"]) {
      const releaseDir = path.join(root, `skip-${platform}`);
      await mkdir(releaseDir);
      const result = await runCopyTestableBinary({ platform, releaseDir });
      check(`${platform} skip exit code`, result.code === 0, "0", String(result.code));
      check(
        `${platform} skip log`,
        result.logs.some((line) => line.includes("Windows-only")),
        "a log line mentioning Windows-only",
        JSON.stringify(result.logs),
      );
      const created = await stat(path.join(releaseDir, DESTINATION_NAME)).then(
        () => true,
        () => false,
      );
      check(`${platform} skip creates no file`, created === false, "no file", "a file");
    }

    const okDir = path.join(root, "win32-ok");
    await mkdir(okDir);
    await writeFile(path.join(okDir, SOURCE_NAME), "fake-binary-bytes");
    const ok = await runCopyTestableBinary({ platform: "win32", releaseDir: okDir });
    check("win32 copy exit code", ok.code === 0, "0", String(ok.code));
    check(
      "win32 copy logs",
      ok.logs.some((line) => line.startsWith("Copied production binary:")) &&
        ok.logs.some((line) => line.startsWith("Created testable binary:")),
      "both success lines",
      JSON.stringify(ok.logs),
    );
    const copied = await readFile(path.join(okDir, DESTINATION_NAME), "utf8").catch(
      () => null,
    );
    check(
      "win32 copy contents",
      copied === "fake-binary-bytes",
      "identical bytes",
      JSON.stringify(copied),
    );

    const missingDir = path.join(root, "win32-missing");
    await mkdir(missingDir);
    const missing = await runCopyTestableBinary({
      platform: "win32",
      releaseDir: missingDir,
    });
    check("win32 missing exit code", missing.code === 1, "1", String(missing.code));
    check(
      "win32 missing error",
      missing.errors.some((line) => line.includes("Production binary is missing")),
      "an error containing 'Production binary is missing'",
      JSON.stringify(missing.errors),
    );

    const dirSourceDir = path.join(root, "win32-directory");
    await mkdir(path.join(dirSourceDir, SOURCE_NAME), { recursive: true });
    const dirSource = await runCopyTestableBinary({
      platform: "win32",
      releaseDir: dirSourceDir,
    });
    check("win32 directory exit code", dirSource.code === 1, "1", String(dirSource.code));
    check(
      "win32 directory error",
      dirSource.errors.some((line) =>
        line.includes("Expected production binary to be a file"),
      ),
      "an error containing 'Expected production binary to be a file'",
      JSON.stringify(dirSource.errors),
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }

  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`copy-testable-binary self-test failed: ${failure}`);
    }
    return 1;
  }
  console.log("copy-testable-binary self-test passed (5 cases)");
  return 0;
}

async function main(argv) {
  if (argv.includes("--help")) {
    console.log(USAGE);
    return 0;
  }
  if (argv.includes("--self-test")) {
    return selfTest();
  }

  const scriptDir = path.dirname(fileURLToPath(import.meta.url));
  const repoRoot = path.resolve(scriptDir, "..");
  const releaseDir = path.join(repoRoot, "target", "release");
  const { code, logs, errors } = await runCopyTestableBinary({
    platform: process.platform,
    releaseDir,
  });
  for (const line of logs) console.log(line);
  for (const line of errors) console.error(line);
  return code;
}

process.exit(await main(process.argv.slice(2)));
