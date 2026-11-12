import {
  chmod,
  copyFile,
  mkdtemp,
  mkdir,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SOURCE_NAMES = { win32: "agentscommander.exe", linux: "agentscommander" };
const DESTINATION_NAME = "agentscommander_testeable.exe";

const USAGE = `Usage: node scripts/copy-testable-binary.mjs [--self-test] [--help]

Copies the production Tauri binary to the testable binary name. The testable
binary is supported on Windows and Linux only; on Linux the historical
agentscommander_testeable.exe name holds a copy of the agentscommander ELF with
its permission bits. On any other platform this is a no-op that exits 0.

  --self-test  Run the in-memory self-test (8 cases; Linux-only cases skip on Windows).
  --help       Print this usage and exit 0.`;

export async function runCopyTestableBinary({ platform, releaseDir }) {
  const logs = [];
  const errors = [];

  if (platform !== "win32" && platform !== "linux") {
    logs.push(
      `Skipping testable binary copy on ${platform}: ` +
        "the testable binary supports Windows and Linux only.",
    );
    return { code: 0, logs, errors };
  }

  const source = path.join(releaseDir, SOURCE_NAMES[platform]);
  const destination = path.join(releaseDir, DESTINATION_NAME);
  let sourceStat;

  try {
    sourceStat = await stat(source);
    if (!sourceStat.isFile()) {
      errors.push(`Expected production binary to be a file: ${source}`);
      return { code: 1, logs, errors };
    }
  } catch (error) {
    errors.push(
      `Production binary is missing: ${source}`,
      `Run the production Tauri build before copying the testable binary.`
    );
    if (error?.message) {
      errors.push(error.message);
    }
    return { code: 1, logs, errors };
  }

  if (platform === "linux" && (sourceStat.mode & 0o111) === 0) {
    errors.push(`Production binary is not executable: ${source}`);
    return { code: 1, logs, errors };
  }

  try {
    await copyFile(source, destination);
    if (platform === "linux") {
      await chmod(destination, sourceStat.mode & 0o777);
    }
  } catch (error) {
    errors.push(error?.message ?? String(error));
    return { code: 1, logs, errors };
  }

  logs.push(
    `Copied production binary: ${source}`,
    `Created testable binary: ${destination}`
  );
  return { code: 0, logs, errors };
}

async function selfTest() {
  const failures = [];
  const skipped = [];
  const root = await mkdtemp(path.join(os.tmpdir(), "copy-testable-binary-"));
  const check = (name, condition, expected, actual) => {
    if (!condition) failures.push(`${name}: expected ${expected}, got ${actual}`);
  };
  const exists = (file) => stat(file).then(() => true, () => false);
  const hasSuccessLogs = (logs) =>
    logs.some((line) => line.startsWith("Copied production binary:")) &&
    logs.some((line) => line.startsWith("Created testable binary:"));

  try {
    const skipDir = path.join(root, "skip-darwin");
    await mkdir(skipDir);
    const skip = await runCopyTestableBinary({ platform: "darwin", releaseDir: skipDir });
    check("darwin skip exit code", skip.code === 0, "0", String(skip.code));
    check(
      "darwin skip log",
      skip.logs.some((line) => line.includes("Windows and Linux only")),
      "a log line mentioning Windows and Linux only",
      JSON.stringify(skip.logs),
    );
    const created = await exists(path.join(skipDir, DESTINATION_NAME));
    check("darwin skip creates no file", created === false, "no file", "a file");

    const okDir = path.join(root, "win32-ok");
    await mkdir(okDir);
    await writeFile(path.join(okDir, SOURCE_NAMES.win32), "fake-binary-bytes");
    const ok = await runCopyTestableBinary({ platform: "win32", releaseDir: okDir });
    check("win32 copy exit code", ok.code === 0, "0", String(ok.code));
    check(
      "win32 copy logs",
      hasSuccessLogs(ok.logs),
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

    for (const platform of ["win32", "linux"]) {
      const missingDir = path.join(root, `${platform}-missing`);
      await mkdir(missingDir);
      const missing = await runCopyTestableBinary({ platform, releaseDir: missingDir });
      check(`${platform} missing exit code`, missing.code === 1, "1", String(missing.code));
      check(
        `${platform} missing error`,
        missing.errors.some((line) => line.includes("Production binary is missing")),
        "an error containing 'Production binary is missing'",
        JSON.stringify(missing.errors),
      );

      const dirSourceDir = path.join(root, `${platform}-directory`);
      await mkdir(path.join(dirSourceDir, SOURCE_NAMES[platform]), { recursive: true });
      const dirSource = await runCopyTestableBinary({ platform, releaseDir: dirSourceDir });
      check(
        `${platform} directory exit code`,
        dirSource.code === 1,
        "1",
        String(dirSource.code),
      );
      check(
        `${platform} directory error`,
        dirSource.errors.some((line) =>
          line.includes("Expected production binary to be a file"),
        ),
        "an error containing 'Expected production binary to be a file'",
        JSON.stringify(dirSource.errors),
      );
    }

    if (process.platform === "win32") {
      skipped.push("linux success", "linux non-executable");
    } else {
      const linuxDir = path.join(root, "linux-ok");
      await mkdir(linuxDir);
      const linuxSource = path.join(linuxDir, SOURCE_NAMES.linux);
      const linuxDestination = path.join(linuxDir, DESTINATION_NAME);
      await writeFile(linuxSource, "fake-elf-bytes");
      await chmod(linuxSource, 0o750);
      const linux = await runCopyTestableBinary({ platform: "linux", releaseDir: linuxDir });
      check("linux success exit code", linux.code === 0, "0", String(linux.code));
      check(
        "linux success logs",
        hasSuccessLogs(linux.logs),
        "both success lines",
        JSON.stringify(linux.logs),
      );
      const linuxBytes = await readFile(linuxDestination, "utf8").catch(() => null);
      check(
        "linux success contents",
        linuxBytes === "fake-elf-bytes",
        "identical bytes",
        JSON.stringify(linuxBytes),
      );
      const linuxMode = await stat(linuxDestination).then(
        (info) => (info.mode & 0o777).toString(8),
        () => "no file",
      );
      check("linux success mode", linuxMode === "750", "750", linuxMode);

      const plainDir = path.join(root, "linux-non-executable");
      await mkdir(plainDir);
      const plainSource = path.join(plainDir, SOURCE_NAMES.linux);
      await writeFile(plainSource, "fake-elf-bytes");
      await chmod(plainSource, 0o644);
      const plain = await runCopyTestableBinary({ platform: "linux", releaseDir: plainDir });
      check("linux non-executable exit code", plain.code === 1, "1", String(plain.code));
      check(
        "linux non-executable error",
        plain.errors.some((line) => line.includes("Production binary is not executable")),
        "an error containing 'Production binary is not executable'",
        JSON.stringify(plain.errors),
      );
      check(
        "linux non-executable logs",
        !hasSuccessLogs(plain.logs),
        "no success lines",
        JSON.stringify(plain.logs),
      );
      const plainCreated = await exists(path.join(plainDir, DESTINATION_NAME));
      check(
        "linux non-executable creates no file",
        plainCreated === false,
        "no file",
        "a file",
      );
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }

  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`copy-testable-binary self-test failed: ${failure}`);
    }
    return 1;
  }
  for (const name of skipped) {
    console.log(`copy-testable-binary self-test SKIP: ${name} (Linux-only case on Windows)`);
  }
  if (skipped.length > 0) {
    console.log(
      `copy-testable-binary self-test passed (${8 - skipped.length} cases, ` +
        `${skipped.length} skipped)`,
    );
  } else {
    console.log("copy-testable-binary self-test passed (8 cases)");
  }
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
