if [[ "$RUNNER_OS" == "Windows" ]]; then
  issue1850_python=python
else
  issue1850_python=python3
fi
"$issue1850_python" - <<'PY_PREFLIGHT'
import json
import os
import pathlib
import sys

if sys.version_info < (3, 11):
    raise RuntimeError("Python >=3.11 with tomllib required")
import tomllib


def inspect_configs(cwd, cargo_home, env):
    records = []
    seen = set()
    for base in [cwd, *cwd.parents]:
        config_dir = base / ".cargo"
        records.append(config_dir)
    records.append(cargo_home)
    found = []
    def reject_include(value, key, filename):
        if isinstance(value, dict):
            for name, item in value.items():
                full = key + "." + name if key else name
                if name == "include":
                    raise RuntimeError(f"Unsupported {full} in {filename}")
                reject_include(item, full, filename)
    for folder in records:
        legacy, modern = folder / "config", folder / "config.toml"
        filename = legacy if legacy.exists() else modern
        if not filename.exists():
            continue
        filename = filename.resolve(strict=True)
        if filename in seen:
            continue
        seen.add(filename)
        with filename.open("rb") as handle:
            config = tomllib.load(handle)
        reject_include(config, "", filename)
        if config.get("env"):
            raise RuntimeError(f"Unsupported env in {filename}")
        build = config.get("build", {})
        for name in ("target", "rustc"):
            if name in build:
                raise RuntimeError(f"Unsupported build.{name} in {filename}")
        for target, settings in config.get("target", {}).items():
            if isinstance(settings, dict) and "runner" in settings:
                raise RuntimeError(f"Unsupported target.{target}.runner in {filename}")
        relevant = {key: build[key] for key in ("jobs", "target-dir", "build-dir") if key in build}
        presence = {key: key in build for key in
                    ("rustc-wrapper", "rustc-workspace-wrapper", "rustflags", "rustdocflags")}
        target_presence = {target: {key: key in settings
                                    for key in ("rustflags", "rustdocflags", "linker")}
                           for target, settings in config.get("target", {}).items()
                           if isinstance(settings, dict)}
        found.append({"path": str(filename), "build": relevant, "buildPresence": presence,
                      "targetPresence": target_presence})
    for name, value in env.items():
        if value and (name in ("CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTC", "RUSTC")
                      or name.startswith("CARGO_TARGET_") and name.endswith("_RUNNER")):
            raise RuntimeError(f"Unsupported environment key {name}")
    controls = {name: env.get(name) for name in
                ("RUSTUP_TOOLCHAIN", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR",
                 "CARGO_BUILD_BUILD_DIR")}
    presence = {name: bool(value) for name, value in env.items()
                if name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTFLAGS",
                            "CARGO_ENCODED_RUSTFLAGS", "RUSTDOCFLAGS",
                            "CARGO_ENCODED_RUSTDOCFLAGS")
                or name.startswith("CARGO_")
                and name.endswith(("_WRAPPER", "_RUSTFLAGS", "_RUSTDOCFLAGS"))}
    return {"files": found, "controls": controls, "presence": presence}


def main():
    if os.environ.get("CI") != "true":
        raise RuntimeError("Native configuration preflight runs in CI only")
    cwd = pathlib.Path.cwd().resolve()
    cargo_home = pathlib.Path(
        os.environ.get("CARGO_HOME") or pathlib.Path.home() / ".cargo").resolve()
    result = inspect_configs(cwd, cargo_home, os.environ)
    context_file = pathlib.Path(os.environ["ISSUE1850_DIAGNOSTICS"]) / "context.json"
    context = json.loads(context_file.read_text(encoding="utf-8"))
    context.update({"config": result,
                    "python": {"executable": sys.executable, "version": sys.version}})
    context_file.write_text(json.dumps(context, indent=2), encoding="utf-8")
    print(json.dumps({"python": context["python"], "config": result}))


if __name__ == "__main__":
    main()
PY_PREFLIGHT
node <<'NODE_CONTEXT'
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const cp = require('node:child_process');
function requireValue(ok, message) { if (!ok) throw new Error(message); }
function cleanPath(value) {
  requireValue(typeof value === 'string' && value.length > 0 && !/[\r\n]/.test(value),
    'Invalid or newline path');
  requireValue(path.isAbsolute(value), 'Expected native absolute path: ' + value);
  return path.resolve(value);
}
function canonical(value) { return fs.realpathSync.native(cleanPath(value)); }
function key(value) { return process.platform === 'win32' ? value.toLowerCase() : value; }
function same(a, b) { return key(a) === key(b); }
function inside(root, value) {
  const relative = path.relative(key(root), key(value));
  return relative === '' || (!path.isAbsolute(relative) && relative !== '..' &&
    !relative.startsWith('..' + path.sep));
}
function directory(value) {
  const resolved = canonical(value);
  requireValue(fs.statSync(resolved).isDirectory(), 'Missing library directory: ' + value);
  return resolved;
}
function nativeHost(platform = process.platform, arch = process.arch) {
  const host = {
    'win32/x64': 'x86_64-pc-windows-msvc',
    'linux/x64': 'x86_64-unknown-linux-gnu',
    'darwin/arm64': 'aarch64-apple-darwin',
    'darwin/x64': 'x86_64-apple-darwin'
  }[platform + '/' + arch];
  requireValue(host, 'Unsupported native host: ' + platform + '/' + arch);
  return host;
}
function digest(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}
function checkout() {
  const result = cp.spawnSync('git', ['rev-parse', 'HEAD'], {encoding: 'utf8'});
  requireValue(!result.error && result.status === 0, 'git checkout identity failed');
  const sha = result.stdout.trim();
  requireValue(sha === process.env.GITHUB_SHA, 'Checkout differs from GITHUB_SHA');
  return sha;
}
function command(program, args) {
  const result = cp.spawnSync(program, args,
    {encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit']});
  requireValue(!result.error && result.status === 0,
    program + ' preflight failed: ' + (result.error || result.status));
  return result.stdout.trim();
}
function main() {
  const dir = canonical(process.env.ISSUE1850_DIAGNOSTICS);
  const contextFile = path.join(dir, 'context.json');
  const context = JSON.parse(fs.readFileSync(contextFile, 'utf8'));
  const cwd = canonical(process.cwd());
  const manifestPath = canonical(path.join(cwd, 'Cargo.toml'));
  const sourcePath = canonical(path.join(cwd, 'src', 'lib.rs'));
  requireValue(Number(process.versions.node.split('.')[0]) >= 22, 'Node >=22 required');
  const versions = {node: {executable: process.execPath, version: process.version},
    python: context.python, cargo: command('cargo', ['--version']),
    rustc: command('rustc', ['-vV'])};
  const host = versions.rustc.match(/^host: (.+)$/m)?.[1];
  requireValue(host === nativeHost(),
    'rustc host mismatch: ' + host + ' versus ' + process.platform + '/' + process.arch);
  const targetLibdir = directory(command('rustc', ['--print', 'target-libdir']));
  const metadataText = command('cargo',
    ['metadata', '--locked', '--no-deps', '--format-version=1']);
  fs.writeFileSync(path.join(dir, 'metadata.json'), metadataText, {flag: 'wx'});
  const metadata = JSON.parse(metadataText);
  const packages = metadata.packages.filter(
    pkg => same(canonical(pkg.manifest_path), manifestPath));
  requireValue(packages.length === 1, 'Expected exactly one manifest package');
  const pkg = packages[0];
  const targets = pkg.targets.filter(target => target.name === 'agentscommander_lib' &&
    target.kind.includes('lib') && same(canonical(target.src_path), sourcePath));
  requireValue(targets.length === 1, 'Expected exact library target');
  // Compilation may create this directory; canonicalize it after success.
  const targetDirectory = cleanPath(metadata.target_directory);
  const checkoutSha = checkout();
  requireValue(/^[a-f0-9]{40}$/i.test(process.env.ISSUE1850_PR_HEAD || ''), 'Missing PR head');
  Object.assign(context, {runId: process.env.GITHUB_RUN_ID,
    runAttempt: process.env.GITHUB_RUN_ATTEMPT,
    job: process.env.GITHUB_JOB, checkoutSha, prHeadSha: process.env.ISSUE1850_PR_HEAD, cwd,
    packageId: pkg.id, manifestPath, sourcePath, targetName: targets[0].name, host,
    targetDirectory, targetLibdir, versions, buildStartedAt: new Date().toISOString()});
  fs.writeFileSync(contextFile, JSON.stringify(context, null, 2));
  console.log(JSON.stringify({preflight: context}));
}
main();
NODE_CONTEXT
cargo test --locked --release --lib issue_1850 --no-run \
  --message-format=json-render-diagnostics | tee "$ISSUE1850_DIAGNOSTICS/cargo-messages.jsonl"
node <<'NODE_SELECT'
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const cp = require('node:child_process');
function requireValue(ok, message) { if (!ok) throw new Error(message); }
function cleanPath(value) {
  requireValue(typeof value === 'string' && value.length > 0 && !/[\r\n]/.test(value),
    'Invalid or newline path');
  requireValue(path.isAbsolute(value), 'Expected native absolute path: ' + value);
  return path.resolve(value);
}
function canonical(value) { return fs.realpathSync.native(cleanPath(value)); }
function key(value) { return process.platform === 'win32' ? value.toLowerCase() : value; }
function same(a, b) { return key(a) === key(b); }
function inside(root, value) {
  const relative = path.relative(key(root), key(value));
  return relative === '' || (!path.isAbsolute(relative) && relative !== '..' &&
    !relative.startsWith('..' + path.sep));
}
function directory(value) {
  const resolved = canonical(value);
  requireValue(fs.statSync(resolved).isDirectory(), 'Missing library directory: ' + value);
  return resolved;
}
function nativeHost(platform = process.platform, arch = process.arch) {
  const host = {
    'win32/x64': 'x86_64-pc-windows-msvc',
    'linux/x64': 'x86_64-unknown-linux-gnu',
    'darwin/arm64': 'aarch64-apple-darwin',
    'darwin/x64': 'x86_64-apple-darwin'
  }[platform + '/' + arch];
  requireValue(host, 'Unsupported native host: ' + platform + '/' + arch);
  return host;
}
function digest(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}
function checkout() {
  const result = cp.spawnSync('git', ['rev-parse', 'HEAD'], {encoding: 'utf8'});
  requireValue(!result.error && result.status === 0, 'git checkout identity failed');
  const sha = result.stdout.trim();
  requireValue(sha === process.env.GITHUB_SHA, 'Checkout differs from GITHUB_SHA');
  return sha;
}
function recordsFrom(text) {
  const records = [];
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try { records.push(JSON.parse(trimmed)); }
    catch (error) {
      if (/^[{[]/.test(trimmed)) throw new Error('Malformed Cargo JSON: ' + error.message);
    }
  }
  return records;
}
function selectArtifact(records, context) {
  const finished = records.filter(record => record?.reason === 'build-finished');
  requireValue(finished.length === 1 && finished[0].success === true,
    'Expected one successful build-finished');
  const matches = records.filter(record => record?.reason === 'compiler-artifact' &&
    record.package_id === context.packageId && record.target?.name === context.targetName &&
    Array.isArray(record.target.kind) && record.target.kind.includes('lib') &&
    record.profile?.test === true &&
    typeof record.executable === 'string' && record.executable.length > 0 &&
    same(canonical(record.manifest_path), context.manifestPath) &&
    same(canonical(record.target.src_path), context.sourcePath));
  requireValue(matches.length === 1,
    'Expected exactly one release harness record; got ' + matches.length);
  const targetDirectory = directory(context.targetDirectory);
  const deps = directory(path.join(targetDirectory, 'release', 'deps'));
  const executable = canonical(matches[0].executable);
  requireValue(fs.statSync(executable).isFile() && same(path.dirname(executable), deps) &&
    (process.platform !== 'win32' || path.extname(executable).toLowerCase() === '.exe'),
    'Invalid release executable location/type');
  const loaderDirs = [];
  function add(value) {
    const dir = directory(value);
    if (!loaderDirs.some(existing => same(existing, dir))) loaderDirs.push(dir);
  }
  for (const record of records) {
    if (record?.reason !== 'build-script-executed') continue;
    requireValue(Array.isArray(record.linked_paths), 'Missing linked_paths');
    for (let linked of record.linked_paths) {
      requireValue(typeof linked === 'string', 'Invalid linked path');
      const prefix = linked.match(/^([^=]+)=/);
      if (prefix) {
        requireValue(['native', 'dependency', 'crate', 'framework', 'all'].includes(prefix[1]),
          'Unknown linked KIND: ' + prefix[1]);
        linked = linked.slice(prefix[0].length);
      }
      linked = cleanPath(linked);
      // Resolve existing aliases before containment; absent outside paths are excluded.
      const resolved = fs.existsSync(linked) ? canonical(linked) : linked;
      if (inside(targetDirectory, resolved)) add(resolved);
    }
  }
  add(deps);
  add(path.join(targetDirectory, 'release'));
  add(context.targetLibdir);
  return {targetDirectory, executable, executableSha256: digest(executable),
    executableBytes: fs.statSync(executable).size, loaderDirs};
}
function main() {
  const dir = canonical(process.env.ISSUE1850_DIAGNOSTICS);
  const context = JSON.parse(fs.readFileSync(path.join(dir, 'context.json'), 'utf8'));
  requireValue(checkout() === context.checkoutSha, 'Checkout changed during build');
  const selected = selectArtifact(
    recordsFrom(fs.readFileSync(path.join(dir, 'cargo-messages.jsonl'), 'utf8')), context);
  const manifest = {schema: 1};
  for (const name of ['runId', 'runAttempt', 'job', 'checkoutSha', 'prHeadSha', 'cwd', 'packageId',
    'manifestPath', 'sourcePath', 'targetName', 'host', 'versions']) manifest[name] = context[name];
  Object.assign(manifest, selected, {createdAt: new Date().toISOString()});
  const filename = path.join(dir, 'manifest.json');
  fs.writeFileSync(filename, JSON.stringify(manifest, null, 2), {flag: 'wx'});
  fs.appendFileSync(process.env.GITHUB_OUTPUT, 'manifest=' + filename + '\n');
  console.log(JSON.stringify({selected: manifest, buildStartedAt: context.buildStartedAt,
    completedAt: new Date().toISOString()}));
}
main();
NODE_SELECT
