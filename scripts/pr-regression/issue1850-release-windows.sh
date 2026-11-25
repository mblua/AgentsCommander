set -euo pipefail
node <<'NODE_CONSUMER' 2>&1 | tee test-1850-release.log
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const cp = require('node:child_process');
function requireValue(ok, message) { if (!ok) throw new Error(message); }
function cleanPath(value) {
  requireValue(typeof value === 'string' && value.length > 0 && !/[\r\n]/.test(value), 'Invalid or newline path');
  requireValue(path.isAbsolute(value), 'Expected native absolute path: ' + value);
  return path.resolve(value);
}
function canonical(value) { return fs.realpathSync.native(cleanPath(value)); }
function key(value) { return process.platform === 'win32' ? value.toLowerCase() : value; }
function same(a, b) { return key(a) === key(b); }
function inside(root, value) {
  const relative = path.relative(key(root), key(value));
  return relative === '' || (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith('..' + path.sep));
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
function digest(file) { return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'); }
function checkout() {
  const result = cp.spawnSync('git', ['rev-parse', 'HEAD'], {encoding: 'utf8'});
  requireValue(!result.error && result.status === 0, 'git checkout identity failed');
  const sha = result.stdout.trim();
  requireValue(sha === process.env.GITHUB_SHA, 'Checkout differs from GITHUB_SHA');
  return sha;
}
function validateManifest(manifest) {
  requireValue(manifest?.schema === 1, 'Invalid manifest schema');
  for (const [name, envName] of [['runId', 'GITHUB_RUN_ID'], ['runAttempt', 'GITHUB_RUN_ATTEMPT'],
    ['job', 'GITHUB_JOB'], ['prHeadSha', 'ISSUE1850_PR_HEAD']]) {
    requireValue(typeof manifest[name] === 'string' && manifest[name].length > 0 && manifest[name] === process.env[envName], 'Manifest identity mismatch: ' + name);
  }
  requireValue(manifest.checkoutSha === checkout(), 'Stale manifest checkout');
  requireValue(manifest.host === nativeHost(), 'Manifest native host mismatch');
  const cwd = canonical(process.cwd());
  requireValue(same(manifest.cwd, cwd) && same(canonical(manifest.cwd), cwd), 'Manifest cwd mismatch');
  for (const [name, expected] of [['manifestPath', path.join(cwd, 'Cargo.toml')], ['sourcePath', path.join(cwd, 'src', 'lib.rs')]]) {
    requireValue(same(manifest[name], canonical(expected)) && same(canonical(manifest[name]), canonical(expected)), 'Manifest path mismatch: ' + name);
  }
  requireValue(manifest.targetName === 'agentscommander_lib' && typeof manifest.packageId === 'string' && manifest.packageId.length > 0, 'Invalid package/target');
  const targetDirectory = directory(manifest.targetDirectory);
  requireValue(same(targetDirectory, manifest.targetDirectory), 'Noncanonical target directory');
  const executable = canonical(manifest.executable);
  requireValue(same(executable, manifest.executable) && same(path.dirname(executable), directory(path.join(targetDirectory, 'release', 'deps'))) &&
    fs.statSync(executable).isFile() && (process.platform !== 'win32' || path.extname(executable).toLowerCase() === '.exe'), 'Invalid manifest executable');
  requireValue(fs.statSync(executable).size === manifest.executableBytes && digest(executable) === manifest.executableSha256, 'Executable size/hash mismatch');
  requireValue(Array.isArray(manifest.loaderDirs) && manifest.loaderDirs.length > 0, 'Missing loader directories');
  const seen = new Set();
  for (const value of manifest.loaderDirs) {
    requireValue(same(directory(value), value) && !seen.has(key(value)), 'Invalid/duplicate loader directory');
    seen.add(key(value));
  }
  requireValue(typeof manifest.createdAt === 'string' && Number.isFinite(Date.parse(manifest.createdAt)) && manifest.versions && typeof manifest.versions === 'object', 'Invalid manifest context');
  return manifest;
}
function loaderEnvironment(manifest, inherited = process.env, platform = process.platform, paths = path) {
  const env = {...inherited};
  const variable = {win32: 'PATH', linux: 'LD_LIBRARY_PATH', darwin: 'DYLD_FALLBACK_LIBRARY_PATH'}[platform];
  requireValue(variable, 'Unsupported loader platform: ' + platform);
  let tail;
  if (platform === 'win32') {
    const keys = Object.keys(env).filter(name => name.toUpperCase() === 'PATH').sort();
    tail = keys.length ? env[keys[0]] : undefined;
    for (const name of keys) delete env[name];
  } else {
    tail = env[variable];
    if (platform === 'darwin' && !Object.prototype.hasOwnProperty.call(env, variable)) {
      requireValue(typeof env.HOME === 'string' && env.HOME.length > 0, 'HOME required for macOS loader fallback');
      tail = [paths.join(env.HOME, 'lib'), '/usr/local/lib', '/usr/lib'].join(paths.delimiter);
    }
  }
  env[variable] = manifest.loaderDirs.join(paths.delimiter) + (tail === undefined ? '' : paths.delimiter + tail);
  return {env, variable};
}
function launch(manifest, env, variable, spawn = cp.spawn, args = ['issue_1850', '--test-threads=1', '--nocapture']) {
  console.log(JSON.stringify({launchAt: new Date().toISOString(), executable: manifest.executable, argv: args,
    checkoutSha: manifest.checkoutSha, prHeadSha: manifest.prHeadSha, runId: manifest.runId,
    runAttempt: manifest.runAttempt, job: manifest.job, executableSha256: manifest.executableSha256, loaderVariable: variable}));
  const child = spawn(manifest.executable, args, {shell: false, cwd: manifest.cwd, env, stdio: ['ignore', 'inherit', 'inherit']});
  let failed = false;
  child.on('error', error => {
    failed = true;
    console.error('Harness spawn error:', error);
    process.exitCode = 1;
  });
  child.on('close', (code, signal) => {
    console.log(JSON.stringify({completedAt: new Date().toISOString(), code, signal, spawnError: failed}));
    if (failed || signal || !Number.isInteger(code)) {
      console.error('Harness failed: error=' + failed + ' signal=' + signal + ' status=' + code);
      process.exitCode = 1;
    } else {
      process.exitCode = code;
    }
  });
}
function main() {
  const filename = canonical(process.env.ISSUE1850_MANIFEST);
  const manifest = validateManifest(JSON.parse(fs.readFileSync(filename, 'utf8')));
  const {env, variable} = loaderEnvironment(manifest);
  launch(manifest, env, variable);
}
main();
NODE_CONSUMER

for name in \
  'config::profile::tests::issue_1850_config_dir_name_table_is_profile_independent' \
  'config::tests::issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome' \
  'config::tests::issue_1850_overrides_keep_precedence_and_identity_over_canonical_home' \
  'config::tests::issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes' \
  'config::tests::issue_1850_lazy_helper_probes_suffixed_routes_write_once'; do
  grep -qF "test ${name} ..." test-1850-release.log || {
    echo "::error::${name} never ran in the focused #1850 release run; it was renamed, moved, or gated out."
    exit 1
  }
done
grep -qE '^test result: ok\. [1-9][0-9]* passed; 0 failed' test-1850-release.log || {
  echo "::error::the focused #1850 release run did not report a positive passing count with zero failures."
  exit 1
}
if grep -qF 'ISSUE1850_WINDOWS_PROFILE_PROOF_OK' test-1850-release.log; then
  echo "::error::the ordinary Windows run must not claim the real-profile proof."
  exit 1
fi
