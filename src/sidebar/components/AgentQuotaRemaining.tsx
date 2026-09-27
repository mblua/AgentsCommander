import { Show, type Component } from "solid-js";
import { sessionsStore } from "../stores/sessions";
import { agentQuotaTitle, quotaRemainingLabel } from "./agent-quota";

/** #2681 - weekly quota remaining next to an agent name. Renders nothing
 *  without a valid reading. Reads the store in JSX, so it updates live. */
export const QuotaRemaining: Component<{ agentId: string }> = (props) => {
  const used = () => sessionsStore.weeklyQuotaUsedByAgentId[props.agentId];
  return (
    <Show when={quotaRemainingLabel(used())}>
      {(text) => (
        <span class="agent-quota-remaining" title={agentQuotaTitle(used() as number)}>{text()}</span>
      )}
    </Show>
  );
};
