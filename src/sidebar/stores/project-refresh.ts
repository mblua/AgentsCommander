export function normalizeProjectPathForCompare(path: string): string {
  return path.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
}

export function findLoadedProjectPathForRefresh(
  projects: readonly { readonly path: string }[],
  incomingPath: string
): string | null {
  const normalizedIncoming = normalizeProjectPathForCompare(incomingPath);
  const match = projects.find(
    (project) =>
      normalizeProjectPathForCompare(project.path) === normalizedIncoming
  );
  return match?.path ?? null;
}
