import { FakeTransport } from "../../shared/testing/fake-transport";
import { baseSettings, discovery } from "../../shared/testing/ui-harness";

// Shared AcDiscoveryPanel test fixture: one origin agent row and one replica row.
export const agentRowSel = '[data-ac-testid="acDiscovery.agent.proj-dev"]';
export const replicaRowSel = '[data-ac-testid="acDiscovery.replica.wg-1-team.dev"]';
export const filterSel = '[data-ac-testid="agentPicker.agentFilter"]';

export function acDiscoveryTransport(): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("discover_ac_agents", discovery({
    agents: [{ name: "proj/dev", path: "C:\\Project\\.ac\\_agent_dev", roleExists: true }],
    workgroups: [{
      name: "wg-1-team",
      path: "C:\\Project\\.ac\\wg-1-team",
      task: null,
      taskTitle: null,
      agents: [{ name: "dev", path: "C:\\Project\\.ac\\wg-1-team\\__agent_dev", repoPaths: [], isCoordinator: false }],
    }],
  }));
  // The picker filter renders only with at least one coding agent.
  fake.resolve("get_settings", baseSettings({
    agents: [{ id: "codex", label: "Codex", command: "codex", color: "#10b981", envs: [], isolatedHome: false }],
  }));
  return fake;
}
