import type { ScopeFault } from "../../shared/types";

/** #2572 - one line per replica the removal count skipped: which one and why.
 *  The default names a code this build does not know, so no replica goes mute. */
export function scopeFaultLine(fault: ScopeFault): string {
  const name = fault.replicaName;
  switch (fault.code) {
    case "configUnreadable":
      return `${name} has no readable config.json. It is not counted.`;
    case "configNotJson":
      return `${name} has an invalid config.json (not valid JSON). It is not counted.`;
    case "configNotObject":
      return `${name} has a config.json that is not a JSON object. It is not counted.`;
    case "identityMissing":
      return `${name} has no identity in its config.json. It is not counted.`;
    case "identityMismatch":
      return `${name} points at another agent (identity mismatch). It is not counted.`;
    case "locationInvalid":
      return `${name} is not in a valid replica location. It is not counted.`;
    case "pathUnreadable":
      return `${name} could not be read from disk. It is not counted.`;
    case "folderUnreadable":
      return `The folder ${name} could not be read. Replicas inside it are not counted.`;
    default:
      return `${name} could not be checked. It is not counted.`;
  }
}

export function scopeFaultOverflowLine(hidden: number): string {
  return `+${hidden} more not counted.`;
}
