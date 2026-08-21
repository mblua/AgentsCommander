import type {
  PtyTerminalActiveBuffer,
  PtyTerminalAlternateEntryMode,
  PtyTerminalHistoryTruncationReason,
  PtyTerminalReplayStage,
  PtyTerminalSeedlessReason,
} from "../types";

export const TEST_TERMINAL_DOCUMENT_EPOCH = "1";

export interface TerminalSnapshotWireOptions {
  readonly replayData?: readonly number[];
  readonly rows?: number;
  readonly cols?: number;
  readonly sequence?: number;
  readonly activeBuffer?: PtyTerminalActiveBuffer;
  readonly alternateEntryMode?: PtyTerminalAlternateEntryMode | null;
  readonly replayStage?: PtyTerminalReplayStage;
  readonly historyIncluded?: boolean;
  readonly historyTruncated?: boolean;
  readonly historyTruncationReason?: PtyTerminalHistoryTruncationReason;
  readonly historyBoundaryHardened?: boolean;
  readonly normalScreenIncluded?: boolean;
  readonly retainedHistoryRows?: number;
  readonly includedHistoryRows?: number;
  readonly semanticHistoryBytes?: number;
  readonly pendingParserBytes?: number;
  readonly activeScreenHasText?: boolean;
  readonly activeBottomLineHasText?: boolean;
}

export function terminalSnapshotWire(
  options: TerminalSnapshotWireOptions = {},
): Record<string, unknown> {
  const replayData = [...(options.replayData ?? [])];
  const activeBuffer = options.activeBuffer ?? "normal";
  const alternateEntryMode =
    options.alternateEntryMode ??
    (activeBuffer === "alternate" ? "mode1049" : null);
  const historyIncluded = options.historyIncluded ?? false;
  const includedHistoryRows = options.includedHistoryRows ?? (historyIncluded ? 1 : 0);
  const semanticHistoryBytes =
    options.semanticHistoryBytes ?? (historyIncluded ? Math.min(replayData.length, 65_536) : 0);
  const historyTruncated = options.historyTruncated ?? false;
  const historyTruncationReason =
    options.historyTruncationReason ?? (historyTruncated ? "rowLimitReached" : "none");
  return {
    replayData,
    rows: options.rows ?? 24,
    cols: options.cols ?? 80,
    sequence: options.sequence ?? 0,
    activeBuffer,
    ...(alternateEntryMode === null ? {} : { alternateEntryMode }),
    replayStage: options.replayStage ?? "semanticHistory",
    historyIncluded,
    historyTruncated,
    historyTruncationReason,
    historyBoundaryHardened: options.historyBoundaryHardened ?? false,
    normalScreenIncluded: options.normalScreenIncluded ?? true,
    retainedHistoryRows: options.retainedHistoryRows ?? includedHistoryRows,
    includedHistoryRows,
    semanticHistoryBytes,
    replayBytes: replayData.length,
    pendingParserBytes: options.pendingParserBytes ?? 0,
    activeScreenHasText: options.activeScreenHasText ?? replayData.length > 0,
    activeBottomLineHasText: options.activeBottomLineHasText ?? false,
  };
}

export function terminalActivationWire(
  args: Record<string, unknown>,
  snapshotOptions: TerminalSnapshotWireOptions = {},
): Record<string, unknown> {
  return {
    snapshot: terminalSnapshotWire(snapshotOptions),
    attachGeneration: args.attachGeneration,
    documentEpoch: args.documentEpoch,
  };
}

export function terminalSeedlessActivationWire(
  args: Record<string, unknown>,
  reason: PtyTerminalSeedlessReason = "seedlessParserUnavailable",
): Record<string, unknown> {
  return {
    seedlessReason: reason,
    attachGeneration: args.attachGeneration,
    documentEpoch: args.documentEpoch,
  };
}
