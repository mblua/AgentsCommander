import { Show } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcAgentReplica, Session } from "../../shared/types";
import { toastStore } from "../../shared/stores/toasts";
import { unleashedReplicasStore } from "../stores/unleashed-replicas";
import UnleashConfirmModal from "./UnleashConfirmModal";

type Restart = (sessionId: string) => Promise<unknown>;

// #4752 - store the flag, then fresh-restart so the new context applies. A failed
// restart keeps the flag (user decision Q2); the restart function toasts it.
async function applyUnleash(
  sessionId: string,
  replicaPath: string,
  unleashed: boolean,
  restart: Restart
) {
  unleashedReplicasStore.setBusy(true);
  try {
    await unleashedReplicasStore.set(replicaPath, unleashed);
  } catch (e) {
    toastStore.error(`Could not ${unleashed ? "unleash" : "leash"} the agent: ${String(e)}`);
    return;
  } finally {
    unleashedReplicasStore.setBusy(false);
  }
  unleashedReplicasStore.closePrompt();
  await restart(sessionId);
}

/**
 * #4752 - the session actions at the end of a live replica's menu: the
 * Unleash/Leash item (never for the Root Agent) and Close Session.
 */
export default function ReplicaSessionActions(props: {
  session: Session;
  replica: AcAgentReplica;
  close: () => void;
  closeSession: (event: MouseEvent, sessionId: string) => void;
  restart: Restart;
}) {
  // An unleashed container session can still be Leashed, so the halo can always go.
  const unleashDisabled = () =>
    props.session.backendKind === "containerTransport" &&
    !unleashedReplicasStore.isUnleashed(props.replica);
  return (
    <>
      <Show when={!props.session.isRootAgent}>
        <button
          class="session-context-option context-option-danger"
          classList={{ "context-option-disabled": unleashDisabled() }}
          data-ac-testid={`replica.${props.session.id}.menu.unleash`}
          data-ac-role="menuitem"
          data-ac-state={unleashedReplicasStore.isUnleashed(props.replica) ? "unleashed" : "leashed"}
          disabled={unleashDisabled()}
          title={
            unleashDisabled()
              ? "Not available for container sessions: the container mounts still limit what the agent can see."
              : undefined
          }
          onClick={() => {
            props.close();
            if (unleashedReplicasStore.isUnleashed(props.replica)) {
              void applyUnleash(props.session.id, props.replica.path, false, props.restart);
            } else {
              unleashedReplicasStore.openPrompt({
                sessionId: props.session.id,
                replicaPath: props.replica.path,
                replicaName: props.replica.name,
              });
            }
          }}
        >
          <span class="session-context-option-icon" aria-hidden="true">&#x26D3;</span>{" "}
          {unleashedReplicasStore.isUnleashed(props.replica) ? "Leash" : "Unleash"}
        </button>
      </Show>
      <button
        class="session-context-option context-option-danger"
        onClick={(event) => props.closeSession(event, props.session.id)}
      >
        <span class="session-context-option-icon" aria-hidden="true">&#x2715;</span> Close Session (Ctrl+Shift+W)
      </button>
    </>
  );
}

/** #4752 - the Unleash confirmation, mounted once at the stable ProjectPanel root. */
export function UnleashPromptHost(props: { restart: Restart }) {
  return (
    <Show when={unleashedReplicasStore.prompt()}>
      {(p) => (
        <Portal>
          <UnleashConfirmModal
            replicaName={p().replicaName}
            busy={unleashedReplicasStore.busy()}
            onCancel={() => unleashedReplicasStore.closePrompt()}
            onConfirm={() =>
              void applyUnleash(p().sessionId, p().replicaPath, true, props.restart)
            }
          />
        </Portal>
      )}
    </Show>
  );
}
