import type { LoopEventPayload } from "../shared/types";

export type LoopToast = {
  message: string;
  className: "toast-info" | "toast-error";
};

export function loopToastFromEvent(data: LoopEventPayload): LoopToast | null {
  const name = data.summary?.name ?? data.loopId;
  switch (data.kind) {
    case "missed":
    case "missedWhileClosed":
      return {
        message: data.message ?? `Loop "${name}" was missed while AgentsCommander was closed`,
        className: "toast-error",
      };
    case "failed":
    case "deliveryFailed":
      return {
        message: data.message ?? `Loop "${name}" failed`,
        className: "toast-error",
      };
    case "skipped":
    case "skippedBusy":
      return {
        message: data.message ?? `Loop "${name}" skipped because the coordinator is busy`,
        className: "toast-error",
      };
    case "pending":
    case "pendingBusy":
      return {
        message: data.message ?? `Loop "${name}" is pending until the coordinator is idle`,
        className: "toast-info",
      };
    case "delivered":
      return {
        message: data.message ?? `Loop "${name}" delivered`,
        className: "toast-info",
      };
    case "coalesced":
    case "coalescedPending":
      return {
        message: data.message ?? `Loop "${name}" coalesced into the pending delivery`,
        className: "toast-info",
      };
    default:
      return null;
  }
}
