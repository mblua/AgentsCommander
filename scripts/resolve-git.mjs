import fs from 'node:fs';
import path from 'node:path';

const GIT_CANDIDATES = Object.freeze(
  process.platform === 'win32'
    ? [
        String.raw`C:\Program Files\Git\cmd\git.exe`,
        String.raw`C:\Program Files (x86)\Git\cmd\git.exe`,
      ]
    : ['/usr/bin/git', '/usr/local/bin/git', '/opt/homebrew/bin/git'],
);

let cachedGit = null;

/**
 * Absolute path to a git executable; never a `PATH` lookup.
 *
 * The override is read on every call, before any cache lookup, and is never
 * memoized: an earlier unoverridden call must not defeat a later
 * `GATE_GIT_BIN`. Only the fixed-location scan is cached.
 */
export function resolveGit() {
  const override = (process.env.GATE_GIT_BIN ?? '').trim();
  if (override) {
    if (!path.isAbsolute(override)) {
      throw new Error(
        `GATE_GIT_BIN must be an absolute path to a git executable; got '${override}'.`,
      );
    }
    try {
      fs.accessSync(override, fs.constants.X_OK);
      return override;
    } catch {
      throw new Error(
        `no executable git found at any of: ${override}. ` +
          `Set GATE_GIT_BIN to an absolute path to git.`,
      );
    }
  }
  if (cachedGit) return cachedGit;
  for (const candidate of GIT_CANDIDATES) {
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      cachedGit = candidate;
      return cachedGit;
    } catch {
      // try the next fixed location
    }
  }
  throw new Error(
    `no executable git found at any of: ${GIT_CANDIDATES.join(', ')}. ` +
      `Set GATE_GIT_BIN to an absolute path to git.`,
  );
}
