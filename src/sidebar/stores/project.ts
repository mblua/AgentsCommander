import { createSignal } from "solid-js";
import type { AcWorkgroup, AcAgentMatrix, AcTeam } from "../../shared/types";
import { ProjectAPI, SettingsAPI, AgentCreatorAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";

export interface ProjectState {
  path: string;
  folderName: string;
  workgroups: AcWorkgroup[];
  agents: AcAgentMatrix[];
  teams: AcTeam[];
}

const [project, setProject] = createSignal<ProjectState | null>(null);
const [loading, setLoading] = createSignal(false);

export const projectStore = {
  get current() {
    return project();
  },

  get isLoading() {
    return loading();
  },

  /** Load project from a path: discover and persist to settings */
  async loadProject(path: string) {
    setLoading(true);
    try {
      const result = await ProjectAPI.discover(path);
      const folderName = path.replace(/\\/g, "/").split("/").pop() ?? "unknown";
      setProject({
        path,
        folderName,
        workgroups: result.workgroups,
        agents: result.agents,
        teams: result.teams,
      });

      // Persist project path to settings using cached settingsStore value
      const cached = settingsStore.current;
      if (cached) {
        await SettingsAPI.update({ ...cached, projectPath: path });
      }
    } catch (e) {
      console.error("Failed to load project:", e);
      setProject(null);
      // Clear stale project path from settings
      const cached = settingsStore.current;
      if (cached && cached.projectPath) {
        await SettingsAPI.update({ ...cached, projectPath: null }).catch(() => {});
      }
    } finally {
      setLoading(false);
    }
  },

  /** Initialize from saved settings (call on mount) */
  async initFromSettings(projectPath: string | null) {
    if (projectPath) {
      await projectStore.loadProject(projectPath);
    }
  },

  /** Create .ac-new in path and load as project */
  async createAndLoad(path: string) {
    await ProjectAPI.createAcProject(path);
    await projectStore.loadProject(path);
  },

  /** Full open flow: pick folder, check .ac-new, auto-load if found */
  async pickAndCheck(): Promise<{ picked: string | null; hasAcNew: boolean }> {
    const picked = await AgentCreatorAPI.pickFolder();
    if (!picked) return { picked: null, hasAcNew: false };

    const hasAcNew = await ProjectAPI.checkPath(picked);
    if (hasAcNew) {
      await projectStore.loadProject(picked);
    }
    return { picked, hasAcNew };
  },

  clear() {
    setProject(null);
  },
};
