import type { AcAgentReplica } from "../../shared/types";
import { matchPathRoot, trimTrailingSeparators } from "../../shared/path-extractors";

function pathSeparatorFor(path: string): "\\" | "/" {
  return path.includes("\\") ? "\\" : "/";
}

function trimTrailingPathSeparators(path: string): string {
  return trimTrailingSeparators(path) || path;
}

function isAbsolutePath(path: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(path)
    || /^[\\/]{2}[^\\/]+[\\/][^\\/]+/.test(path)
    || /^[\\/]/.test(path);
}

function pushPathSegment(segments: string[], segment: string, hasRoot: boolean): void {
  if (!segment || segment === ".") return;
  if (segment === "..") {
    if (segments.length > 0 && segments[segments.length - 1] !== "..") {
      segments.pop();
    } else if (!hasRoot) {
      segments.push(segment);
    }
    return;
  }
  segments.push(segment);
}

function normalizePath(path: string, separator: "\\" | "/"): string {
  const trimmed = path.trim();
  const parsed = matchPathRoot(trimmed);
  let root = "";
  let rest = trimmed;

  if (parsed && "drive" in parsed) {
    root = `${parsed.drive}${separator}`;
    rest = parsed.rest;
  } else if (parsed) {
    root = `${separator}${separator}${parsed.server}${separator}${parsed.share}${separator}`;
    rest = parsed.rest;
  } else if (/^[\\/]/.test(trimmed)) {
    root = separator;
    rest = trimmed.replace(/^[\\/]+/, "");
  }

  const segments: string[] = [];
  for (const segment of rest.split(/[\\/]+/)) {
    pushPathSegment(segments, segment, root !== "");
  }

  const suffix = segments.join(separator);
  if (!root) return suffix || ".";
  return suffix ? `${root}${suffix}` : root;
}

function matrixFolderFromIdentityPath(
  replicaPath: string,
  identityPath: string | undefined
): string | null {
  const path = identityPath?.trim();
  if (!path) return null;

  const separator = pathSeparatorFor(replicaPath || path);
  const identityTarget = /(^|[\\/])identity\.json$/i.exec(path)
    ? path.replace(/[\\/][^\\/]+$/, "")
    : path;
  const absoluteTarget = isAbsolutePath(identityTarget)
    ? identityTarget
    : `${trimTrailingPathSeparators(replicaPath)}${separator}${identityTarget}`;

  return normalizePath(absoluteTarget, separator);
}

export function replicaMatrixFolder(replica: AcAgentReplica): string | null {
  return matrixFolderFromIdentityPath(replica.path, replica.identityPath);
}
