import type {
  AcAgentReplica,
  AcWorkgroup,
  RepoBranchByPath,
  RepoDirtyByPath,
  SessionRepo,
} from "../../shared/types";

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

/**
 * #1028 - the badge tooltip, and the ONLY surface where all three dirty states are
 * distinguishable. Colour is a binary alarm at 8px (red = uncommitted work, violet =
 * everything else), so `false` ("detected clean") and `null` ("no answer yet") are
 * byte-identical on screen; this is what tells them apart, and what makes a
 * `?? null` -> `|| null` slip in the store observable at all.
 *
 * "(status unknown)" is the NORMAL startup state, not an error: a cold row shows it
 * until the first Gate A tick (<=15s) and a live row until the first GitWatcher tick
 * (<=5s). It is the first thing a user sees on every launch, so the wording says "not
 * yet known" rather than implying a fault.
 *
 * A third badge COLOUR was rejected: this codebase renders unknown as absence, never
 * as its own colour, and a third colour would advertise a transient internal failure
 * the user can do nothing about.
 */
export function formatReplicaRepoBadgeTitle(repo: Pick<SessionRepo, "sourcePath" | "dirty">): string {
  if (repo.dirty === true) return `${repo.sourcePath} (uncommitted changes)`;
  if (repo.dirty === false) return repo.sourcePath;
  return `${repo.sourcePath} (status unknown)`;
}

export function configuredReplicaRepoBadges(
  replica: Pick<AcAgentReplica, "repoPaths" | "repoBranch"> & {
    /** #943 B2 - live per-repo branches. Optional: absent before the first branch
     *  event, and absent for callers that do not read the volatile layer. */
    repoBranchByPath?: RepoBranchByPath;
    /** #1028 - live per-repo worktree-dirty. Optional for the same two reasons: the
     *  callers that only read `label`/`branch` (menu entries, search text) pass
     *  nothing and every repo resolves to `dirty: null`, which they ignore. */
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
  // Pre-B2 shorthand: discovery detects a branch only for a SINGLE-repo replica
  // (ac_discovery.rs `repo_paths.len() == 1`), so a multi-repo replica had none.
  // It stays as the fallback for any repo the per-repo layer has not covered: the
  // window between project load and the first watcher tick (<=15s), the legacy
  // workgroup.repoPath source above, or a repo added to config.json since the last
  // tick. NOT after a reload: `clearForPaths` deliberately preserves the per-repo
  // map, because discovery has no counterpart to re-supply it (H1).
  const singleRepoBranch = sourcePaths.length === 1 ? replica.repoBranch ?? null : null;
  const byPath = replica.repoBranchByPath;
  const dirtyByPath = replica.repoDirtyByPath;

  return sourcePaths
    .map((sourcePath) => {
      // #943 B2 - keyed BY PATH, never by position. The branch payload is stored
      // and re-read against a LATER discovery snapshot, so if config.json `repos`
      // is reordered or an entry removed, a positional merge would paint repo A's
      // branch onto repo B and Browse Branch would open a branch that does not
      // belong to that repo. A path miss yields no branch instead.
      // `undefined` = unknown path (fall back); `null` = known, no branch (mask).
      const live = byPath?.[sourcePath];
      return {
        label: repoLabelFromPath(sourcePath),
        sourcePath,
        branch: live === undefined ? singleRepoBranch : live,
        // #1028 - there is NO single-repo shorthand for dirty (discovery never had a
        // scalar dirty the way it had `repoBranch`), so a path miss resolves to
        // `null` = "never detected" = violet, never to a fallback. `?? null` maps
        // only the miss; a detected `false` passes through as `false`, which the
        // badge title reports differently from `null`.
        dirty: dirtyByPath?.[sourcePath] ?? null,
      };
    })
    .filter((repo) => repo.label.length > 0);
}

export function automationIdPart(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown";
}
