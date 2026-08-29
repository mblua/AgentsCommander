import { CANONICAL_AC_ROOT_DIR } from './constants';
import { isNumberedEntityDirName } from './entity-prefix';

function pathParts(workDir: string): string[] {
  return workDir.replace(/\\/g, '/').split('/').filter(s => s.length > 0);
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
  return idx > 0 ? parts[idx - 1] : null;
}

export function extractWorkgroupName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = lastAcRootIndex(parts);
  if (idx < 0 || idx + 1 >= parts.length) return null;
  const wg = parts[idx + 1];
  return isNumberedEntityDirName(wg) ? wg.toUpperCase() : null;
}

export function extractAgentName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = lastAcRootIndex(parts);
  if (idx < 0 || idx + 2 >= parts.length) return null;
  const seg = parts[idx + 2];
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
