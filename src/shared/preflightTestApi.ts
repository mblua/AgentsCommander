import type { Transport } from "./transport";
import type {
  PreflightTargetDescription,
  PreflightTestDraft,
  PreflightTestSnapshot,
  PreflightTestTarget,
} from "./preflightTypes";

/** Undefined omits the draft key so old callers keep their exact arguments. */
function withDraft(
  args: Record<string, unknown>,
  draft: PreflightTestDraft | null | undefined,
): Record<string, unknown> {
  return draft === undefined ? args : { ...args, draft };
}

/** The preflight chain test over an injected transport (Tauri or WS). */
export function createPreflightTestAPI(transport: Pick<Transport, "invoke">) {
  return {
    listTargets: (agentId: string, replicaPaths: string[]) =>
      transport.invoke<PreflightTestTarget[]>("list_preflight_test_targets",
        { agentId, replicaPaths }),
    describe: (agentId: string, profileLetter: string | null, replicaPath: string,
               draft?: PreflightTestDraft | null) =>
      transport.invoke<PreflightTargetDescription>("describe_preflight_target",
        withDraft({ agentId, profileLetter, replicaPath }, draft)),
    start: (agentId: string, profileLetter: string | null, replicaPath: string,
            settingsRevision: string, draft?: PreflightTestDraft | null) =>
      transport.invoke<string>("start_preflight_test",
        withDraft({ agentId, profileLetter, replicaPath, settingsRevision }, draft)),
    get: (runId: string) =>
      transport.invoke<PreflightTestSnapshot>("get_preflight_test", { runId }),
    cancel: (runId: string) =>
      transport.invoke<PreflightTestSnapshot>("cancel_preflight_test", { runId }),
  };
}
