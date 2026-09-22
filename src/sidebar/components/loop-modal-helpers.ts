import type {
  AcLoopSummary,
  AcWorkgroup,
  BusyCoordinatorPolicy,
  LoopSessionStart,
  LoopUpdateInput,
} from "../../shared/types";

export interface LoopCoordinatorOption {
  workgroup: string;
  coordinatorName: string;
  label: string;
}

export function hasFiveCronFields(expr: string): boolean {
  return expr.trim().split(/\s+/).filter(Boolean).length === 5;
}

export function coordinatorOptionsFromWorkgroups(
  workgroups: AcWorkgroup[]
): LoopCoordinatorOption[] {
  return workgroups
    .map((wg) => {
      const coordinator = wg.agents.find((agent) => agent.isCoordinator);
      if (!coordinator) return null;
      return {
        workgroup: wg.name,
        coordinatorName: coordinator.name,
        label: `${wg.name} - ${coordinator.name}`,
      };
    })
    .filter((option): option is LoopCoordinatorOption => option !== null);
}

export function busyPolicyFromForceCheckbox(forceInject: boolean): BusyCoordinatorPolicy {
  return forceInject ? "forceInject" : "waitUntilIdle";
}

export function busyPolicyForEdit(
  initialPolicy: BusyCoordinatorPolicy,
  forceInject: boolean,
  forceCheckboxTouched: boolean
): BusyCoordinatorPolicy {
  if (initialPolicy === "skip" && !forceCheckboxTouched) return "skip";
  return busyPolicyFromForceCheckbox(forceInject);
}

export function sessionStartFromAccumulateCheckbox(accumulate: boolean): LoopSessionStart {
  return accumulate ? "accumulate" : "fresh";
}

export function accumulateCheckboxFromSessionStart(value: LoopSessionStart): boolean {
  return value === "accumulate";
}

export function formatLoopNextDue(nextDueAt: string | null | undefined): string {
  if (!nextDueAt) return "";
  const date = new Date(nextDueAt);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function normalizeLoopError(error: unknown, fallback: string): string {
  if (typeof error === "string") return error;
  if (error instanceof Error && error.message) return error.message;
  return fallback;
}

export interface LoopEditBaseline {
  summary: AcLoopSummary;
  promptBody: string;
}

export interface LoopEditValues {
  name: string;
  expr: string;
  workgroup: string;
  promptBody: string;
  busyCoordinator: AcLoopSummary["busyCoordinator"];
  sessionStart: AcLoopSummary["sessionStart"];
  enabled: boolean;
}

export function buildLoopUpdateInput(
  baseline: LoopEditBaseline,
  next: LoopEditValues
): LoopUpdateInput {
  const input: LoopUpdateInput = {};
  if (next.name !== baseline.summary.name) input.name = next.name;
  if (next.expr !== baseline.summary.expr) input.expr = next.expr;
  if (next.workgroup !== baseline.summary.workgroup) input.workgroup = next.workgroup;
  if (next.promptBody !== baseline.promptBody) input.promptBody = next.promptBody;
  if (next.busyCoordinator !== baseline.summary.busyCoordinator) {
    input.busyCoordinator = next.busyCoordinator;
  }
  if (next.sessionStart !== baseline.summary.sessionStart) input.sessionStart = next.sessionStart;
  if (next.enabled !== baseline.summary.enabled) input.enabled = next.enabled;
  return input;
}

export const SCHEDULE_RESET_KEYS = [
  "expr",
  "workgroup",
  "promptBody",
  "busyCoordinator",
  "sessionStart",
  "enabled",
] as const;

export function resetsLoopSchedule(input: LoopUpdateInput): boolean {
  return SCHEDULE_RESET_KEYS.some((key) => input[key] !== undefined);
}
