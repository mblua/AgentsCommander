// Generated from Rust by ts-rs (#4389). Do not edit by hand.

export type CiState = "unknown" | "idle" | "running";

export type StalenessState = "unknown" | "current" | "stale";

export type RemoteActivityUpdate = {
  repoPaths: Array<string>,
  ciStates: Array<CiState>,
  stalenessStates: Array<StalenessState>,
  behindBy: Array<number | null>,
};
