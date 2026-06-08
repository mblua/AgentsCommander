import type { AcProjectRefreshRequestedPayload } from "../shared/types";
import { projectStore } from "./stores/project";

export function handleProjectRefreshRequested(
  data: AcProjectRefreshRequestedPayload,
  store: Pick<typeof projectStore, "loadProject" | "reloadProjectIfLoaded"> = projectStore
) {
  if (data.reason === "projectRegistered") {
    void store.loadProject(data.projectPath);
    return;
  }
  void store.reloadProjectIfLoaded(data.projectPath);
}
