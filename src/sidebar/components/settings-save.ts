import type { AppSettings } from "../../shared/types";

export function mergeSettingsForSavePreservingProjects(
  draft: AppSettings,
  fresh: AppSettings
): AppSettings {
  return {
    ...draft,
    projectPaths: fresh.projectPaths ?? [],
    projectPath: fresh.projectPath ?? null,
  };
}
