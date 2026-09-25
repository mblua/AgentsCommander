// @vitest-environment jsdom
// AcDiscoveryPanel has no importer in src/ today (#2573 plan F-A), so it is
// rendered directly: no App route reaches this menu.
import { describe, expect } from "vitest";
import AcDiscoveryPanel from "./AcDiscoveryPanel";
import { click, contextMenu, discovery, session, waitFor } from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { describeRestartToast } from "./restart-toast-harness";

const wgPath = "C:\\Project\\.ac\\wg-1-dev-team";
const replicaPath = `${wgPath}\\__agent_dev`;

function restartButton(): HTMLButtonElement | undefined {
  return Array.from(document.body.querySelectorAll("button")).find(
    (b) => (b.textContent ?? "").trim() === "Restart Session",
  );
}

describe("AcDiscoveryPanel Restart Session toast (#2573)", () => {
  describeRestartToast({
    failName: "restart_session_from_the_discovery_menu_shows_the_plain_error_toast",
    okName: "a_successful_discovery_restart_shows_no_toast",
    setup: (fake) => {
      fake.resolve(
        "discover_ac_agents",
        discovery({
          workgroups: [
            {
              name: "wg-1-dev-team",
              path: wgPath,
              task: null,
              teamName: "dev-team",
              agents: [{ name: "dev", path: replicaPath, repoPaths: [], isCoordinator: false }],
            },
          ],
        }),
      );
    },
    ui: () => <AcDiscoveryPanel />,
    trigger: async (root) => {
      sessionsStore.setSessions([session({ id: "sess-1", workingDirectory: replicaPath })]);
      const rowSel = ".ac-wg-group .replica-item";
      await waitFor(() => expect(root.querySelector(rowSel)).toBeTruthy());
      contextMenu(root.querySelector(rowSel)!);
      await waitFor(() => expect(restartButton()).toBeTruthy());
      click(restartButton()!);
    },
  });
});
