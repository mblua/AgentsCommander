/// #943 - GitHub remote parsing for the coordinator repo Browse submenu.
/// Only github.com is supported. Any other host (self-hosted GHE, GitLab,
/// Bitbucket, a local path remote) yields null, which the caller renders as
/// "no Browse items".

export interface GithubRepoRef {
  owner: string;
  repo: string;
}

const GITHUB_HOSTS = new Set(["github.com", "www.github.com"]);
/// GitHub's owner/repo charset. NOTE: `.` is in this class, so it does NOT
/// reject `.` or `..` on its own; `isSafeSegment` does that. (Rev 1 claimed the
/// regex was an injection guard. It was not: `git@github.com:../evil` parsed to
/// {owner: "..", repo: "evil"}.)
const NAME_RE = /^[A-Za-z0-9._-]+$/;
/// scp-like remote: [user@]host:owner/repo(.git)
const SCP_LIKE_RE = /^(?:[^@/]+@)?([^:/]+):(.+)$/;
const URL_SCHEME_RE = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//;
const ALLOWED_SCHEMES = new Set(["http", "https", "ssh", "git"]);

/// Only the URL branch gets WHATWG path normalization; the scp-like branch does
/// not, so `.`/`..` must be rejected explicitly on both.
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
    // URL.hostname already drops userinfo and port (and Rust already stripped
    // userinfo before this ever crossed IPC; this is the second layer).
    return refFromHostAndPath(url.hostname, url.pathname);
  }

  const scp = SCP_LIKE_RE.exec(trimmed);
  if (!scp) return null;
  return refFromHostAndPath(scp[1], scp[2]);
}

export function githubRepoUrl(ref: GithubRepoRef): string {
  return `https://github.com/${ref.owner}/${ref.repo}`;
}

/// Branch names contain `/` (`fix/909-agency-agents-roles-yaml-skip`), so the
/// separators stay literal and each segment is percent-encoded (`#`, `?`, spaces
/// and newlines all encode; no second URL can be smuggled in).
///
/// Returns null for an unusable ref. `.`/`..` segments are rejected rather than
/// dropped: git already forbids `..` in a ref name, so rejecting is lossless for
/// real branches, and `repo.branch` is NOT always git-sourced (it also arrives
/// from config.json `repoBranch` via configuredReplicaRepoBadges and from the
/// volatile branch-event layer, neither of which validates). Without this,
/// `../../../../evil/repo` produced a URL the browser normalized into a
/// different repository.
export function githubBranchUrl(ref: GithubRepoRef, branch: string): string | null {
  const segments = branch.split("/").filter((segment) => segment.length > 0);
  if (segments.length === 0) return null;
  if (!segments.every(isSafeSegment)) return null;
  const encoded = segments.map(encodeURIComponent).join("/");
  return `${githubRepoUrl(ref)}/tree/${encoded}`;
}
