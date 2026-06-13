import type { AcAgentReplica, Session } from "../../shared/types";

export interface RepoBadgeView {
  label: string;
  branch: string | null;
  sourcePath: string;
}

function labelFromPath(path: string): string {
  const dir = path.replace(/\\/g, "/").split("/").pop() ?? "";
  return dir.startsWith("repo-") ? dir.slice(5) : dir;
}

export function repoBadgesForReplica(
  replica: AcAgentReplica,
  session?: Session | null
): RepoBadgeView[] {
  if (session?.gitRepos?.length) {
    return session.gitRepos.map((repo) => ({
      label: repo.label,
      branch: repo.branch ?? null,
      sourcePath: repo.sourcePath,
    }));
  }

  const paths = replica.repoPaths ?? [];
  return paths.map((path) => ({
    label: labelFromPath(path),
    branch: paths.length === 1 ? replica.repoBranch ?? null : null,
    sourcePath: path,
  }));
}
