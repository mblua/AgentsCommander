const REPO_NAME_RE = /\/([^/]+?)(?:\.git)?$/;

/** Last path segment of a repo URL without a trailing `.git`; the URL itself when none. */
export function repoDisplayName(url: string): string {
  const match = REPO_NAME_RE.exec(url);
  return match?.[1] ?? url;
}
