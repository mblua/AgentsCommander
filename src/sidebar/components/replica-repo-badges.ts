import type {
  AcAgentReplica,
  AcWorkgroup,
  RepoBranchByPath,
  RepoDirtyByPath,
  SessionRepo,
} from "../../shared/types";
import type { RemoteActivityEntry } from "../stores/remote-activity";

export function stripRepoPrefix(name: string): string {
  return name.startsWith("repo-") ? name.slice(5) : name;
}

export function repoLabelFromPath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const dirName = normalized.split("/").pop() ?? "";
  return stripRepoPrefix(dirName);
}

export function formatReplicaRepoBadgeLabel(repo: Pick<SessionRepo, "label" | "branch">): string {
  return `${repo.label}${repo.branch ? `/${repo.branch}` : ""}`;
}

/** #2064 — the CI and staleness suffixes on the repo chip's tooltip. `unknown` CI is
 *  SILENT on purpose: it is the majority case (no `gh`, feature off, not yet swept)
 *  and has nothing to say, so the distinction between "no CI" and "we do not know"
 *  lives in the tooltip rather than in a third marker on the chip. */
function remoteActivityTitleSuffix(
  remoteActivity: Pick<RemoteActivityEntry, "ci" | "staleness" | "behindBy"> | null | undefined
): string {
  if (!remoteActivity) return "";
  let suffix = "";
  if (remoteActivity.ci === "running") suffix += " - CI running";
  else if (remoteActivity.ci === "idle") suffix += " - no CI activity for this commit";
  // A `stale` state with a null count is omitted rather than guessed: the bar still
  // renders, and a suffix with no number explains nothing.
  if (remoteActivity.staleness === "stale" && typeof remoteActivity.behindBy === "number") {
    suffix += ` - base is ${remoteActivity.behindBy} commits ahead`;
  }
  return suffix;
}

/** The optional second parameter keeps every existing call site and every existing
 *  test compiling unedited, and with it omitted the output is byte-identical to
 *  today's — which is what a user with no `gh` or with the feature off still sees. */
export function formatReplicaRepoBadgeTitle(
  repo: Pick<SessionRepo, "sourcePath" | "dirty">,
  remoteActivity?: Pick<RemoteActivityEntry, "ci" | "staleness" | "behindBy"> | null
): string {
  const suffix = remoteActivityTitleSuffix(remoteActivity);
  if (repo.dirty === true) return `${repo.sourcePath} (local work not confirmed by cached origin tracking)${suffix}`;
  if (repo.dirty === false) return `${repo.sourcePath}${suffix}`;
  return `${repo.sourcePath} (status unknown)${suffix}`;
}

export function configuredReplicaRepoBadges(
  replica: Pick<AcAgentReplica, "repoPaths" | "repoBranch"> & {
    repoBranchByPath?: RepoBranchByPath;
    repoDirtyByPath?: RepoDirtyByPath;
  },
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  const repoPaths = replica.repoPaths ?? [];
  const sourcePaths = repoPaths.length > 0
    ? repoPaths
    : workgroup.repoPath
      ? [workgroup.repoPath]
      : [];
  const singleRepoBranch = sourcePaths.length === 1 ? replica.repoBranch ?? null : null;
  const byPath = replica.repoBranchByPath;
  const dirtyByPath = replica.repoDirtyByPath;

  return sourcePaths
    .map((sourcePath) => {
      const live = byPath?.[sourcePath];
      return {
        label: repoLabelFromPath(sourcePath),
        sourcePath,
        branch: live === undefined ? singleRepoBranch : live,
        dirty: dirtyByPath?.[sourcePath] ?? null,
      };
    })
    .filter((repo) => repo.label.length > 0);
}

export function automationIdPart(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown";
}
