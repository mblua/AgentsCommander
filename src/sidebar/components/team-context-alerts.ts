export const CONTEXT_ALERT_PERCENTAGE_MIN = 1;
export const CONTEXT_ALERT_PERCENTAGE_MAX = 100;
export const MAX_CONTEXT_ALERT_THRESHOLDS = 3;

export interface ContextAlertThresholdDraft {
  id: number;
  raw: string;
}

export type ContextAlertThresholdErrorCode =
  | "blank"
  | "wholeNumber"
  | "range"
  | "duplicate"
  | "cardinality";

export interface ContextAlertThresholdRowValidation {
  draftId: number;
  errorCodes: ContextAlertThresholdErrorCode[];
  messages: string[];
}

export type ContextAlertThresholdValidation =
  | {
      valid: true;
      canonicalPercentages: number[];
      rows: ContextAlertThresholdRowValidation[];
      summaryMessages: string[];
    }
  | {
      valid: false;
      canonicalPercentages: null;
      rows: ContextAlertThresholdRowValidation[];
      summaryMessages: string[];
    };

const CARDINALITY_SUMMARY = "A team can have at most 3 context alert thresholds.";
const CARDINALITY_ROW_MESSAGE = "Remove a threshold so no more than 3 remain.";
const BLANK_MESSAGE = "Enter a threshold percentage.";
const WHOLE_NUMBER_MESSAGE = "Enter a whole-number percentage using digits only.";
const RANGE_MESSAGE = "Threshold must be between 1% and 100%.";
const DUPLICATE_MESSAGE =
  "Thresholds must be distinct; this percentage is already configured.";

interface ParsedDraft {
  value: number | null;
  lexicalCode: "blank" | "wholeNumber" | "range" | null;
  lexicalMessage: string | null;
}

function parseDraft(raw: string): ParsedDraft {
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    return { value: null, lexicalCode: "blank", lexicalMessage: BLANK_MESSAGE };
  }
  if (!/^[0-9]+$/.test(trimmed)) {
    return {
      value: null,
      lexicalCode: "wholeNumber",
      lexicalMessage: WHOLE_NUMBER_MESSAGE,
    };
  }

  const value = Number(trimmed);
  if (value < CONTEXT_ALERT_PERCENTAGE_MIN || value > CONTEXT_ALERT_PERCENTAGE_MAX) {
    return { value: null, lexicalCode: "range", lexicalMessage: RANGE_MESSAGE };
  }
  return { value, lexicalCode: null, lexicalMessage: null };
}

export function validateContextAlertThresholdDrafts(
  drafts: readonly ContextAlertThresholdDraft[],
): ContextAlertThresholdValidation {
  const parsedDrafts = drafts.map((draft) => parseDraft(draft.raw));
  const valueCounts = new Map<number, number>();
  for (const parsed of parsedDrafts) {
    if (parsed.value !== null) {
      valueCounts.set(parsed.value, (valueCounts.get(parsed.value) ?? 0) + 1);
    }
  }

  const rows = drafts.map<ContextAlertThresholdRowValidation>((draft, index) => {
    const errorCodes: ContextAlertThresholdErrorCode[] = [];
    const messages: string[] = [];
    const parsed = parsedDrafts[index];

    if (index >= MAX_CONTEXT_ALERT_THRESHOLDS) {
      errorCodes.push("cardinality");
      messages.push(CARDINALITY_ROW_MESSAGE);
    }
    if (parsed.lexicalCode !== null && parsed.lexicalMessage !== null) {
      errorCodes.push(parsed.lexicalCode);
      messages.push(parsed.lexicalMessage);
    }
    if (parsed.value !== null && (valueCounts.get(parsed.value) ?? 0) > 1) {
      errorCodes.push("duplicate");
      messages.push(DUPLICATE_MESSAGE);
    }

    return { draftId: draft.id, errorCodes, messages };
  });

  const summaryMessages: string[] = [];
  if (drafts.length > MAX_CONTEXT_ALERT_THRESHOLDS) {
    summaryMessages.push(CARDINALITY_SUMMARY);
  }
  for (const [index, row] of rows.entries()) {
    for (const message of row.messages) {
      summaryMessages.push(`Threshold ${index + 1}: ${message}`);
    }
  }

  if (rows.some((row) => row.errorCodes.length > 0)) {
    return {
      valid: false,
      canonicalPercentages: null,
      rows,
      summaryMessages,
    };
  }

  const canonicalPercentages = parsedDrafts
    .map((parsed) => parsed.value)
    .filter((value): value is number => value !== null)
    .sort((left, right) => left - right);

  return {
    valid: true,
    canonicalPercentages,
    rows,
    summaryMessages,
  };
}

export function hydrateContextAlertThresholdDrafts(
  values: readonly number[],
): ContextAlertThresholdDraft[] {
  const hydrated = values.map((value, index) => ({ id: index + 1, raw: String(value) }));
  const validation = validateContextAlertThresholdDrafts(hydrated);
  if (!validation.valid) return hydrated;

  return validation.canonicalPercentages.map((value, index) => ({
    id: index + 1,
    raw: String(value),
  }));
}
