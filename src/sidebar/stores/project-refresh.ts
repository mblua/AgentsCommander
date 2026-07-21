// #1077 §3.4/§5.2: casing is OS-shape aware. Windows-shaped input (drive-rooted,
// UNC, or verbatim/device `\\?\`/`\\.\` prefixed) is case-folded because its
// filesystem is case-insensitive and its aliases must collapse. Everything else
// is treated as POSIX: case- and backslash-preserving, so two projects that
// differ only by case (or a filename containing a literal `\`) stay distinct.
const WINDOWS_DRIVE_ROOTED = /^[a-zA-Z]:(?:[\\/]|$)/;
const WINDOWS_UNC_OR_DEVICE = /^[\\/][\\/]/;
// Only the ordinary `?` verbatim forms are stripped to a plain key; the `.`
// device namespace is preserved as an unsupported marker (`//./…`).
const WINDOWS_VERBATIM_UNC = /^[\\/][\\/]\?[\\/]unc[\\/]/i;
const WINDOWS_VERBATIM_DRIVE = /^[\\/][\\/]\?[\\/][a-zA-Z]:[\\/]/;

function isWindowsShaped(path: string): boolean {
  return WINDOWS_DRIVE_ROOTED.test(path) || WINDOWS_UNC_OR_DEVICE.test(path);
}

export function normalizeProjectPathForCompare(path: string): string {
  if (!isWindowsShaped(path)) {
    // POSIX-shaped: preserve case and literal backslashes; trim only redundant
    // trailing `/` while keeping the bare root `/`.
    const trimmed = path.replace(/\/+$/, "");
    if (trimmed === "" && path.startsWith("/")) return "/";
    return trimmed;
  }

  let normalized = path.replace(/\\/g, "/").toLowerCase();
  if (WINDOWS_VERBATIM_UNC.test(path)) {
    // \\?\UNC\server\share → //server/share
    normalized = `//${normalized.slice("//?/unc/".length)}`;
  } else if (WINDOWS_VERBATIM_DRIVE.test(path)) {
    // \\?\C:\… → C:\…
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
