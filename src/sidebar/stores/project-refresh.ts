export function normalizeProjectPathForCompare(path: string): string {
  let normalized = path.replace(/\\/g, "/").toLowerCase();
  if (normalized.startsWith("//?/unc/")) {
    normalized = `//${normalized.slice("//?/unc/".length)}`;
  } else if (/^\/\/\?\/[a-z]:\//.test(normalized)) {
    normalized = normalized.slice("//?/".length);
  }
  return normalized.replace(/\/+$/, "");
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
