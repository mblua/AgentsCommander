import type { AcWorkgroup, BusyCoordinatorPolicy } from "../../shared/types";

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
