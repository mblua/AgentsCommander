import { entityDirNumber } from "../shared/entity-prefix";
import type { LoopEventPayload } from "../shared/types";
import { formatLoopNextDue } from "./components/loop-modal-helpers";

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
        message: data.message ?? `Loop "${name}" skipped because the orchestrator is busy`,
        className: "toast-error",
      };
    case "pending":
    case "pendingBusy":
      return {
        message: data.message ?? `Loop "${name}" is pending until the orchestrator is idle`,
        className: "toast-info",
      };
    case "delivered": {
      const room = data.summary ? entityDirNumber(data.summary.workgroup) : null;
      const next = formatLoopNextDue(data.summary?.nextDueAt);
      let message = `Loop "${name}" delivered`;
      if (room !== null) message += ` to room ${room}`;
      if (next) message += ` · next ${next}`;
      return { message, className: "toast-info" };
    }
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
