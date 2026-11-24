// Settings > Coding Agents: the Export and Import buttons (desktop only).
// Export writes the draft agents and profiles; import hands new agents to the modal.
import { Component, Show, createSignal, onCleanup } from "solid-js";
import { isTauri } from "../../../shared/platform";
import { CodingAgentsTransferAPI } from "../../../shared/ipc";
import { newAgentId } from "../../../shared/agent-presets";
import { toastStore } from "../../../shared/stores/toasts";
import type { AgentConfig, AppSettings, CodingAgentProfilesConfig } from "../../../shared/types";
import {
  buildTransferFile,
  exportFileName,
  mergeTransferFile,
  parseTransferFile,
  transferHasEnvValues,
} from "../../../shared/coding-agents-transfer";

export type TransferImportHandler = (
  added: AgentConfig[],
  profiles: CodingAgentProfilesConfig,
) => void;

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

export const CodingAgentsTransfer: Component<{
  data: AppSettings | null;
  onImport: TransferImportHandler;
}> = (props) => {
  const [busy, setBusy] = createSignal(false);
  let disposed = false;
  onCleanup(() => (disposed = true));

  const exportAgents = async () => {
    const data = props.data;
    if (!data || busy()) return;
    setBusy(true);
    try {
      const now = new Date();
      const file = buildTransferFile(data.agents, data.codingAgentProfiles, now);
      const content = JSON.stringify(file, null, 2);
      const path = await CodingAgentsTransferAPI.pickSaveExport(content, exportFileName(now));
      if (path === null || disposed) return;
      const done = `Exported ${file.agents.length} coding agents to ${path}.`;
      if (!transferHasEnvValues(file)) toastStore.success(done);
      else {
        const warn = " The file contains environment variable values; share it carefully.";
        toastStore.info(done + warn, { durationMs: null });
      }
    } catch (e) {
      if (!disposed) toastStore.error(`Export failed: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const importAgents = async () => {
    if (!props.data || busy()) return;
    setBusy(true);
    try {
      const text = await CodingAgentsTransferAPI.pickOpenImport();
      const data = props.data;
      if (text === null || disposed || !data) return;
      const file = parseTransferFile(text);
      const merged = mergeTransferFile(data.agents, data.codingAgentProfiles, file, newAgentId);
      props.onImport(merged.added, merged.profiles);
      const n = merged.added.length;
      toastStore.success(`Imported ${n} coding agents. Press Save to keep them.`);
    } catch (e) {
      if (!disposed) toastStore.error(`Import failed: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Show when={isTauri}>
      <button
        type="button"
        class="settings-row-btn"
        disabled={busy()}
        onClick={() => void exportAgents()}
        title="Export all coding agents and profiles to a JSON file"
        data-ac-testid="settings.agents.export"
        data-ac-role="button"
      >
        Export
      </button>
      <button
        type="button"
        class="settings-row-btn"
        disabled={busy()}
        onClick={() => void importAgents()}
        title="Add coding agents and profiles from a JSON file"
        data-ac-testid="settings.agents.import"
        data-ac-role="button"
      >
        Import
      </button>
    </Show>
  );
};
