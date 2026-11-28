import { Show } from "solid-js";
import type { AgentConfig } from "../../../shared/types";
import { sharedIdentityPeers } from "./codingAgentIdentityDuplicates";

export function SharedIdentityWarning(props: { agents: readonly AgentConfig[]; index: number }) {
  const peers = () => sharedIdentityPeers(props.agents, props.index);
  return (
    <Show when={peers().length > 0}>
      <div
        class="settings-hint settings-hint-warning"
        role="status"
        data-ac-testid={`settings.agentRow.${props.index}.sharedIdentity.warning`}
      >
        Same command and envs as {peers().join(", ")}: these Coding Agents share the same
        profiles. Change the command or envs of one of them.
      </div>
    </Show>
  );
}
