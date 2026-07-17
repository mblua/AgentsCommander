
export interface GithubRepoRef {
  owner: string;
  repo: string;
}

const GITHUB_HOSTS = new Set(["github.com", "www.github.com"]);
const NAME_RE = /^[A-Za-z0-9._-]+$/;
const SCP_LIKE_RE = /^(?:[^@/]+@)?([^:/]+):(.+)$/;
const URL_SCHEME_RE = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//;
const ALLOWED_SCHEMES = new Set(["http", "https", "ssh", "git"]);

function isSafeSegment(segment: string): boolean {
  return segment.length > 0 && segment !== "." && segment !== "..";
}

function stripDotGit(value: string): string {
  return value.replace(/\.git$/i, "");
}

function refFromHostAndPath(host: string, path: string): GithubRepoRef | null {
  if (!GITHUB_HOSTS.has(host.toLowerCase())) return null;
  const segments = path.split("/").filter((segment) => segment.length > 0);
  if (segments.length !== 2) return null;
  const owner = segments[0];
  const repo = stripDotGit(segments[1]);
  if (!isSafeSegment(owner) || !isSafeSegment(repo)) return null;
  if (!NAME_RE.test(owner) || !NAME_RE.test(repo)) return null;
  return { owner, repo };
}

export function parseGithubRemote(remote: string | null | undefined): GithubRepoRef | null {
  const trimmed = (remote ?? "").trim();
  if (!trimmed) return null;

  if (URL_SCHEME_RE.test(trimmed)) {
    let url: URL;
    try {
      url = new URL(trimmed);
    } catch {
      return null;
    }
    const scheme = url.protocol.replace(":", "").toLowerCase();
    if (!ALLOWED_SCHEMES.has(scheme)) return null;
    return refFromHostAndPath(url.hostname, url.pathname);
  }

  const scp = SCP_LIKE_RE.exec(trimmed);
  if (!scp) return null;
  return refFromHostAndPath(scp[1], scp[2]);
}

export function githubRepoUrl(ref: GithubRepoRef): string {
  return `https://github.com/${ref.owner}/${ref.repo}`;
}

export function githubBranchUrl(ref: GithubRepoRef, branch: string): string | null {
  const segments = branch.split("/").filter((segment) => segment.length > 0);
  if (segments.length === 0) return null;
  if (!segments.every(isSafeSegment)) return null;
  const encoded = segments.map(encodeURIComponent).join("/");
  return `${githubRepoUrl(ref)}/tree/${encoded}`;
}
