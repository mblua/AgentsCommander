/** On-disk prefix of a Room directory (#1614). */
export const ROOM_DIR_PREFIX = "room-";
/** On-disk prefix of a legacy Workgroup directory. Never produced again; still supported. */
export const LEGACY_WORKGROUP_DIR_PREFIX = "wg-";

/** Case-sensitive: does this directory name carry a Room or legacy Workgroup prefix? */
export function isEntityDirName(name: string): boolean {
  return /^(?:room|wg)-/.test(name);
}

/** Case-sensitive: prefix followed by at least one digit. */
export function isNumberedEntityDirName(name: string): boolean {
  return /^(?:room|wg)-\d+/.test(name);
}

/** Case-insensitive slot number, or null. */
export function entityDirNumber(name: string): number | null {
  const m = name.match(/^(?:room|wg)-(\d+)/i);
  return m ? Number.parseInt(m[1], 10) : null;
}

/** Case-insensitive short label: "ROOM1" for a Room, "WG1" for a legacy Workgroup. */
export function entityShortLabel(name: string): string | null {
  const m = name.match(/^(room|wg)-(\d+)/i);
  return m ? `${m[1].toUpperCase()}${m[2]}` : null;
}

/** Case-sensitive: does this path contain a Room or legacy Workgroup directory segment? */
export function pathHasEntityDirSegment(path: string): boolean {
  return /[\/\\](?:room|wg)-/.test(path);
}
