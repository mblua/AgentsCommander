#!/usr/bin/env node
/**
 * #1283 - pinned resolved-TypeScript-dependency gate (plan Section 14.6).
 *
 * Proves the pinned dependency-cruiser@18.0.0 resolver and its negative
 * fixture matrix are live, then requires the complete `src` root to pass.
 *
 * - The tool is resolved from this repository's `node_modules` only; no global
 *   binary is accepted.
 * - Structured JSON output is parsed; verdicts are read from `summary.error`
 *   and the violation records, never from the process exit code (the JSON
 *   reporter always exits 0) and never from an unvalidated resolver error.
 * - No extra targets, filters, ignored-known-cycle options, warning
 *   severities, or resolver fallbacks are accepted.
 * - The CommonJS fixture pair is required only when the effective
 *   `tsconfig.json` supports `import = require`; applicability is printed.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const CRUISE_BIN = path.join(
  REPO_ROOT,
  "node_modules",
  "dependency-cruiser",
  "bin",
  "dependency-cruise.mjs",
);
const CONFIG = path.join(REPO_ROOT, "dependency-cruiser.config.mjs");
const FIXTURE_ROOT = "scripts/fixtures/frontend-dependency-cycle";
const PINNED_VERSION = "18.0.0";

const CYCLE_PAIRS = [
  "direct-value",
  "type-only",
  "re-export",
  "path-alias",
  "dynamic-import",
  "commonjs",
];
const SEAM_HELPERS = ["terminal-session-registry", "terminal-output-admission"];
const SUPPORTED_IMPORT_EQUALS_MODULES = new Set([
  "commonjs",
  "amd",
  "umd",
  "system",
  "node16",
  "nodenext",
  "node18",
  "node20",
]);

function fail(message) {
  process.stderr.write(`check:frontend-dependencies FAILED\n${message}\n`);
  process.exitCode = 1;
}

function normalize(relativePath) {
  return relativePath.replace(/\\/g, "/");
}

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function runCruise(target) {
  const result = spawnSync(
    process.execPath,
    [CRUISE_BIN, "--config", CONFIG, "--output-type", "json", target],
    { cwd: REPO_ROOT, encoding: "utf8" },
  );
  if (result.error) {
    throw new Error(`cannot spawn pinned dependency-cruiser: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `pinned dependency-cruiser exited ${result.status}: ${(result.stderr ?? "").slice(0, 4000)}`,
    );
  }
  let parsed;
  try {
    parsed = JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`unvalidated resolver output for '${target}': ${error.message}`);
  }
  if (!parsed || !Array.isArray(parsed.modules) || !parsed.summary) {
    throw new Error(`unvalidated resolver output for '${target}': missing modules/summary`);
  }
  return parsed;
}

function fixturePath(pair, file) {
  return `${FIXTURE_ROOT}/${pair}/${file}`;
}

function violationsFor(result, fromPath) {
  return result.summary.violations.filter(
    (violation) => normalize(violation.from) === normalize(fromPath),
  );
}

function classifySeamTarget(to) {
  const normalized = normalize(to);
  if (/TerminalView\.tsx$/.test(normalized)) return "view";
  if (/\/sidebar\.ts$/.test(normalized)) return "sidebar";
  if (/\/ipc\.ts$/.test(normalized)) return "ipc";
  if (normalized.includes("@tauri-apps/api")) return "tauri";
  if (/\/terminal-(session-registry|output-admission)\.ts$/.test(normalized)) {
    return "opposite-helper";
  }
  return null;
}

function verifyFixtureMatrix(result, commonjsApplicable) {
  const failures = [];

  // Every fixture violation must carry only the two declared rule IDs.
  const unexpectedRules = new Set(
    result.summary.violations
      .filter((v) => !["no-circular", "no-terminal-helper-back-edge"].includes(v.rule.name))
      .map((v) => v.rule.name),
  );
  if (unexpectedRules.size > 0) {
    failures.push(
      `fixture result contains unexpected rule IDs: ${[...unexpectedRules].join(", ")}`,
    );
  }

  // Acyclic control: zero errors.
  const controlViolations = violationsFor(result, `${FIXTURE_ROOT}/acyclic-control.ts`);
  if (controlViolations.length > 0) {
    failures.push(
      `acyclic-control must produce zero errors; got ${controlViolations.length}: ` +
        controlViolations.map((v) => `${v.rule.name} -> ${v.to}`).join("; "),
    );
  }

  // Paired cycle fixtures: every violation on the pair is no-circular, and the
  // pair is the named cycle.
  for (const pair of CYCLE_PAIRS) {
    const applicable = pair !== "commonjs" || commonjsApplicable;
    if (!applicable) {
      console.log(
        `  commonjs pair: NOT APPLICABLE (tsconfig module does not support import = require) - not required`,
      );
      continue;
    }
    const a = fixturePath(pair, "a.ts");
    const b = fixturePath(pair, "b.ts");
    const pairViolations = result.summary.violations.filter(
      (v) =>
        normalize(v.from) === a ||
        normalize(v.from) === b ||
        normalize(v.to) === a ||
        normalize(v.to) === b,
    );
    const foreignRules = pairViolations.filter((v) => v.rule.name !== "no-circular");
    if (foreignRules.length > 0) {
      failures.push(
        `${pair} must fail exclusively with no-circular; got ` +
          foreignRules.map((v) => `${v.rule.name} (${v.from} -> ${v.to})`).join("; "),
      );
      continue;
    }
    const namedCycle = pairViolations.some((v) => {
      const from = normalize(v.from);
      const to = normalize(v.to);
      return (
        v.rule.name === "no-circular" &&
        ((from === a && to === b) || (from === b && to === a))
      );
    });
    if (!namedCycle) {
      failures.push(`${pair} pair cycle not named with both a.ts and b.ts paths`);
    }
  }

  // Seam fixtures: no-terminal-helper-back-edge must name every forbidden
  // target; no-unresolved or any other rule ID is a failure. The mutual
  // opposite-helper imports mandated by Section 14.6.1 necessarily make the
  // two seam helpers a two-module cycle, so no-circular is an expected,
  // tolerated violation there (the seam verdict is the back-edge naming).
  for (const helper of SEAM_HELPERS) {
    const seamPath = `${FIXTURE_ROOT}/seams/${helper}.ts`;
    const violations = violationsFor(result, seamPath);
    const backEdge = violations.filter(
      (v) => v.rule.name === "no-terminal-helper-back-edge",
    );
    const foreign = violations.filter(
      (v) => !["no-terminal-helper-back-edge", "no-circular"].includes(v.rule.name),
    );
    if (foreign.length > 0) {
      failures.push(
        `${helper} seam fixture produced non-back-edge violations: ` +
          foreign.map((v) => `${v.rule.name} (-> ${v.to})`).join("; "),
      );
    }
    const seenTargets = new Set(backEdge.map((v) => classifySeamTarget(v.to)));
    if (seenTargets.has(null)) {
      const unknown = backEdge
        .filter((v) => classifySeamTarget(v.to) === null)
        .map((v) => v.to);
      failures.push(`${helper} seam back-edge names unexpected targets: ${unknown.join(", ")}`);
    }
    const expected = ["view", "sidebar", "ipc", "tauri", "opposite-helper"];
    const missing = expected.filter((target) => !seenTargets.has(target));
    if (missing.length > 0) {
      failures.push(
        `${helper} seam fixture must name every forbidden target under ` +
          `no-terminal-helper-back-edge; missing: ${missing.join(", ")} ` +
          `(named: ${[...seenTargets].join(", ")})`,
      );
    }
  }

  if (result.summary.error <= 0) {
    failures.push("fixture suite produced no errors (resolver is not live)");
  }

  return failures;
}

function verifyFullRoot(result) {
  const failures = [];
  if (result.summary.error !== 0) {
    const details = result.summary.violations
      .map((v) => `${v.rule.name}: ${v.from} -> ${v.to}`)
      .join("\n    ");
    failures.push(`complete src root has ${result.summary.error} error(s):\n    ${details}`);
    return failures;
  }

  const git = spawnSync("git", ["ls-files"], { cwd: REPO_ROOT, encoding: "utf8" });
  if (git.error || git.status !== 0) {
    failures.push(`cannot read tracked inventory: ${git.error?.message ?? git.stderr}`);
    return failures;
  }
  const tracked = new Set(
    git.stdout
      .split("\n")
      .map(normalize)
      .filter((file) => /^src\/.*\.(ts|tsx)$/.test(file)),
  );
  const moduleSources = new Set(
    result.modules.filter((m) => /^src\//.test(normalize(m.source))).map((m) => normalize(m.source)),
  );
  const missing = [...tracked].filter((file) => !moduleSources.has(file));
  if (missing.length > 0) {
    failures.push(
      `tracked src sources skipped/excluded/unresolved by the resolver (${missing.length}):\n    ` +
        missing.join("\n    "),
    );
  }
  return failures;
}

function main() {
  if (process.argv.length > 2) {
    fail("no arguments accepted; run exactly: npm run check:frontend-dependencies");
    return;
  }

  const versionPath = path.join(REPO_ROOT, "node_modules", "dependency-cruiser", "package.json");
  if (!existsSync(versionPath)) {
    fail(`dependency-cruiser is not installed (expected exact ${PINNED_VERSION})`);
    return;
  }
  const installed = readJson(versionPath).version;
  if (installed !== PINNED_VERSION) {
    fail(
      `pinned tool version assertion failed: installed dependency-cruiser ${installed}, ` +
        `expected exactly ${PINNED_VERSION}`,
    );
    return;
  }
  if (!existsSync(CRUISE_BIN)) {
    fail(`pinned dependency-cruiser binary not found at ${CRUISE_BIN}`);
    return;
  }

  const tsconfig = readJson(path.join(REPO_ROOT, "tsconfig.json"));
  const moduleKind = tsconfig.compilerOptions?.module ?? "esnext";
  const commonjsApplicable = SUPPORTED_IMPORT_EQUALS_MODULES.has(moduleKind);
  console.log(`dependency-cruiser version: ${installed} (pinned ${PINNED_VERSION})`);
  console.log(`tsconfig module: ${moduleKind}`);
  console.log(`commonjs (import = require) fixture applicable: ${commonjsApplicable}`);

  console.log(`\nFixture matrix: ${FIXTURE_ROOT}`);
  let fixtureResult;
  try {
    fixtureResult = runCruise(FIXTURE_ROOT);
  } catch (error) {
    fail(`fixture run failed: ${error.message}`);
    return;
  }
  const fixtureFailures = verifyFixtureMatrix(fixtureResult, commonjsApplicable);
  console.log(
    `  errors: ${fixtureResult.summary.error}, ` +
      `violations: ${fixtureResult.summary.violations.length}`,
  );
  for (const violation of fixtureResult.summary.violations) {
    console.log(`  ${violation.rule.name}: ${violation.from} -> ${violation.to}`);
  }
  if (fixtureFailures.length > 0) {
    fail(fixtureFailures.join("\n"));
    return;
  }
  console.log("  fixture matrix verdicts: PASS");

  console.log("\nComplete src root gate:");
  let srcResult;
  try {
    srcResult = runCruise("src");
  } catch (error) {
    fail(`full-root run failed: ${error.message}`);
    return;
  }
  const rootFailures = verifyFullRoot(srcResult);
  console.log(
    `  modules: ${srcResult.modules.length}, errors: ${srcResult.summary.error}, ` +
      `dependencies: ${srcResult.summary.totalDependenciesCruised}`,
  );
  if (rootFailures.length > 0) {
    fail(rootFailures.join("\n"));
    return;
  }
  console.log("  complete-root gate: PASS");
  console.log("\ncheck:frontend-dependencies OK");
}

main();
