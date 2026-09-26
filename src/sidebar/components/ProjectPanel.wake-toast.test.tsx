// @vitest-environment jsdom
import { describe, expect } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { baseSettings, click, discovery, session, waitFor } from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { automationIdPart } from "./replica-repo-badges";
import { describeRestartToast } from "./restart-toast-harness";

const projectPath = "C:\\Project";
const workgroupName = "wg-1-dev-team";
const workgroupPath = `${projectPath}\\.ac\\${workgroupName}`;
const replicaName = "dev-webpage-ui";
const replicaPath = `${workgroupPath}\\__agent_${replicaName}`;
const rowSelector = `[data-ac-testid="replica.row.quick.${automationIdPart(workgroupName)}.${automationIdPart(replicaName)}"]`;

describe("ProjectPanel wake-a-stopped-replica toast (#2573)", () => {
  describeRestartToast({
    failName: "waking_a_stopped_replica_shows_the_plain_error_toast",
    okName: "a_successful_wake_shows_no_toast",
    setup: (fake) => {
      fake.resolve("new_project", { path: projectPath, registered: true, created: false });
      fake.resolve(
        "discover_project",
        discovery({
          teams: [{ name: "dev-team", agents: [replicaName], coordinator: replicaName }],
          workgroups: [
            {
              name: workgroupName,
              path: workgroupPath,
              task: null,
              taskTitle: "Wake toast",
              teamName: "dev-team",
              agents: [{ name: replicaName, path: replicaPath, repoPaths: [], isCoordinator: true }],
            },
          ],
        }),
      );
      fake.resolve("get_settings", baseSettings());
    },
    ui: () => <ProjectPanel />,
    trigger: async (root) => {
      // A STOPPED replica session: clicking its row wakes it via restart.
      sessionsStore.setSessions([
        session({
          id: "sess-1",
          name: `${workgroupName}/${replicaName}`,
          workingDirectory: replicaPath,
          status: { exited: 0 },
          isCoordinator: true,
        }),
      ]);
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(root.querySelector(rowSelector)).toBeTruthy());
      click(root.querySelector(rowSelector)!);
    },
  });
});
