import { CANONICAL_AC_ROOT_DIR } from './constants';
import { isNumberedEntityDirName } from './entity-prefix';

/** Drops the trailing run of `/`. */
export function trimTrailingSlashes(path: string): string {
  let end = path.length;
  while (end > 0 && path[end - 1] === '/') end--;
  return path.slice(0, end);
}

/** Drops the trailing run of `/` and `\`. */
export function trimTrailingSeparators(path: string): string {
  let end = path.length;
  while (end > 0 && (path[end - 1] === '/' || path[end - 1] === '\\')) end--;
  return path.slice(0, end);
}

function pathParts(workDir: string): string[] {
  return workDir.replaceAll('\\', '/').split('/').filter(s => s.length > 0);
}

function lastAcRootIndex(parts: string[]): number {
  for (let i = parts.length - 1; i >= 0; i--) {
    if (parts[i] === CANONICAL_AC_ROOT_DIR) return i;
  }
  return -1;
}

export function extractProjectName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = lastAcRootIndex(parts);
  return idx > 0 ? parts[idx - 1] ?? null : null;
}

export function extractWorkgroupName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = lastAcRootIndex(parts);
  if (idx < 0 || idx + 1 >= parts.length) return null;
  const wg = parts[idx + 1];
  if (wg === undefined) return null;
  return isNumberedEntityDirName(wg) ? wg.toUpperCase() : null;
}

export function extractAgentName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = lastAcRootIndex(parts);
  if (idx < 0 || idx + 2 >= parts.length) return null;
  const seg = parts[idx + 2];
  if (seg === undefined) return null;
  if (!seg.startsWith('__agent_')) return null;
  const name = seg.slice('__agent_'.length);
  return name.length > 0 ? name : null;
}

/** Trailing label shown in titlebars: "agent@project" when both are derivable
 *  from the path; falls back through agent-only, sessionName@project, sessionName, null. */
export function computeTrailingText(
  workDir: string,
  sessionName: string | null | undefined,
): string | null {
  const proj = extractProjectName(workDir);
  const ag = extractAgentName(workDir);
  if (proj && ag) return `${ag}@${proj}`;
  if (ag) return ag;
  if (proj && sessionName) return `${sessionName}@${proj}`;
  return sessionName || null;
}

export type PathRoot =
  | { drive: string; rest: string }
  | { server: string; share: string; rest: string };

const LINE_TERMINATOR = /[\n\r\u2028\u2029]/;

function isSeparator(ch: string | undefined): boolean {
  return ch === '/' || ch === '\\';
}

function skipNonSeparators(path: string, start: number): number {
  let i = start;
  while (i < path.length && !isSeparator(path[i])) i++;
  return i;
}

/** `C:` followed by one or more separators; `rest` is what follows them. */
function matchDriveRoot(path: string): PathRoot | null {
  if (path.length < 3 || path[1] !== ':' || !isSeparator(path[2])) return null;
  if (!/^[A-Za-z]$/.test(path.charAt(0))) return null;
  let i = 3;
  while (isSeparator(path[i])) i++;
  const rest = path.slice(i);
  return LINE_TERMINATOR.test(rest) ? null : { drive: path.slice(0, 2), rest };
}

/** Two separators, `server`, a separator, `share`, an optional separator, `rest`. */
function matchUncRoot(path: string): PathRoot | null {
  if (!isSeparator(path[0]) || !isSeparator(path[1])) return null;
  const serverEnd = skipNonSeparators(path, 2);
  if (serverEnd === 2 || serverEnd >= path.length) return null;
  const shareStart = serverEnd + 1;
  const shareEnd = skipNonSeparators(path, shareStart);
  if (shareEnd === shareStart) return null;
  const rest = path.slice(shareEnd < path.length ? shareEnd + 1 : shareEnd);
  if (LINE_TERMINATOR.test(rest)) return null;
  return {
    server: path.slice(2, serverEnd),
    share: path.slice(shareStart, shareEnd),
    rest,
  };
}

/** Recognizes a drive root (`C:\`) first, then a UNC root (`\server\share`). */
export function matchPathRoot(path: string): PathRoot | null {
  return matchDriveRoot(path) ?? matchUncRoot(path);
}
