import { Component, Index, Show, createMemo, onCleanup, onMount } from "solid-js";
import type { UnlistenFn } from "../../shared/transport";
import { onAgentInstallStateChanged, onAgentUpdatesFinished } from "../../shared/ipc";
import { agentUpdateStore } from "../agent-update";
import { CONFIGURED_LABELS, deriveAutoUpdateRows } from "../agent-update-status";
import { agentUpdateOverviewStore } from "../stores/agent-update-overview";

const LISTENER_EVENTS = ["agent_install_state_changed", "agent_updates_finished"] as const;

/**
 * #1551 - read-only Auto-update table of the Coding Agents screen (plan 5.12): one row
 * per update-capable catalog entry (catalog order, duplicates kept), the configured
 * tri-state read from the modal DRAFT (so the table follows the dropdown before Save),
 * the detected install state, and this boot's live state. It writes nothing.
 *
 * Mount sequence: `open()` runs synchronously in the component body, before the rows
 * memo and the first render, so a remount never paints the previous mount's rows for a
 * frame; the two listeners settle (never `Promise.all`) BEFORE the first overview
 * invoke, so no event can precede its listener; a listener that did register is kept
 * and unlistened on disposal even when the other one rejected.
 */
const AgentAutoUpdateStatusList: Component<{
  autoUpdateByCommand: () => Record<string, boolean>;
  registeredCommands: () => string[];
}> = (props) => {
  agentUpdateOverviewStore.open(); // component body: state is { rows: null, loading: false, error: null } before the first render
  let disposed = false;
  const unlisteners: UnlistenFn[] = [];
  onCleanup(() => {
    disposed = true;
    for (const unlisten of unlisteners.splice(0)) unlisten();
    agentUpdateOverviewStore.close();
  });
  const rows = createMemo(() =>
    deriveAutoUpdateRows(agentUpdateOverviewStore.state.rows ?? [], {
      autoUpdateByCommand: props.autoUpdateByCommand(),
      registeredCommands: props.registeredCommands(),
      running: agentUpdateStore[0].running,
      results: agentUpdateStore[0].results,
    })
  );
  onMount(() => {
    void (async () => {
      // Total settlement (never Promise.all): a listener that DID register is kept and unlistened on
      // disposal even when the other one rejects; each rejection is logged with its event name; the
      // overview is invoked only after BOTH settled, so no event can precede its listener.
      const settled = await Promise.allSettled([
        onAgentInstallStateChanged((payload) =>
          agentUpdateOverviewStore.applyInstallState(payload.command, payload.install)
        ),
        onAgentUpdatesFinished(() => {
          void agentUpdateOverviewStore.refresh();
        }),
      ]);
      const registered: UnlistenFn[] = [];
      settled.forEach((outcome, index) => {
        if (outcome.status === "fulfilled") registered.push(outcome.value);
        else
          console.error(
            `[agent-update-overview] listener ${LISTENER_EVENTS[index]} unavailable (live updates degraded):`,
            outcome.reason
          );
      });
      if (disposed) {
        for (const unlisten of registered) unlisten();
        return;
      }
      unlisteners.push(...registered);
      await agentUpdateOverviewStore.refresh();
    })();
  });

  const state = agentUpdateOverviewStore.state;

  return (
    <div
      class="settings-agents-actions-block settings-auto-update-block"
      data-ac-testid="settings.autoUpdate.block"
      data-ac-role="region"
    >
      <div class="settings-agents-actions-title">Auto-update</div>
      <Show when={state.rows === null && state.error === null}>
        <div class="settings-empty-note" data-ac-testid="settings.autoUpdate.loading" data-ac-role="status">
          Loading auto-update status...
        </div>
      </Show>
      <Show when={state.rows !== null && state.rows.length === 0}>
        <div class="settings-empty-note" data-ac-testid="settings.autoUpdate.empty" data-ac-role="status">
          No coding agent in the catalog supports auto-update.
        </div>
      </Show>
      <Show when={state.rows !== null && state.rows.length > 0}>
        <table
          class="settings-auto-update-table"
          aria-label="Auto-update status"
          data-ac-testid="settings.autoUpdate.list"
          data-ac-role="table"
        >
          <thead>
            <tr>
              <th scope="col">Agent</th>
              <th scope="col">Command</th>
              <th scope="col">Auto-update</th>
              <th scope="col">Installed</th>
              <th scope="col">Status</th>
            </tr>
          </thead>
          {/* one polite region for the whole table; never per cell */}
          <tbody aria-live="polite" aria-relevant="text">
            <Index each={rows()}>
              {(row) => (
                <tr
                  data-ac-testid={`settings.autoUpdate.row.${row().key}`}
                  data-ac-role="row"
                  data-ac-command={row().command}
                >
                  <td
                    data-ac-testid={`settings.autoUpdate.row.${row().key}.agent`}
                    data-ac-state={row().registered ? "registered" : "unregistered"}
                  >
                    <span class="settings-color-dot" style={{ background: row().color }} />
                    {row().label}
                    <Show when={!row().registered}>
                      <span
                        class="settings-auto-update-note"
                        title="Only registered coding agents are updated at startup"
                      >
                        (not registered)
                      </span>
                    </Show>
                  </td>
                  <td>
                    <code>{row().command}</code>
                  </td>
                  <td
                    data-ac-testid={`settings.autoUpdate.row.${row().key}.configured`}
                    data-ac-role="status"
                    data-ac-state={row().configured}
                  >
                    {CONFIGURED_LABELS[row().configured]}
                  </td>
                  <td
                    data-ac-testid={`settings.autoUpdate.row.${row().key}.installed`}
                    data-ac-role="status"
                    data-ac-state={row().installed.state}
                    title={row().installed.title}
                  >
                    {row().installed.label}
                  </td>
                  <td
                    data-ac-testid={`settings.autoUpdate.row.${row().key}.live`}
                    data-ac-role="status"
                    data-ac-state={row().live.state}
                    title={row().live.title}
                  >
                    {row().live.label}
                  </td>
                </tr>
              )}
            </Index>
          </tbody>
        </table>
      </Show>
      <Show when={state.error !== null}>
        <div
          class="settings-empty-note"
          data-ac-testid="settings.autoUpdate.error"
          data-ac-role="status"
          data-ac-state="error"
        >
          Auto-update status unavailable: {state.error}
        </div>
      </Show>
      <div class="settings-label-hint" data-ac-testid="settings.autoUpdate.hint" data-ac-role="status">
        Only registered coding agents are updated at startup. Change a setting with the Auto-update
        dropdown of the corresponding agent above.
      </div>
    </div>
  );
};

export default AgentAutoUpdateStatusList;
