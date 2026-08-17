import { createSignal } from "solid-js";
import { normalizeProjectPathForCompare } from "./project-refresh";

export const PROJECT_PANEL_COLLAPSE_KEY_SEP = "\u0000";

export type ProjectPanelCollapseSection =
  | "project"
  | "coordinators"
  | "selected-workgroup"
  | "workgroups"
  | "workgroup"
  | "loops"
  | "agents"
  | "teams"
  | "team";

export function projectPanelCollapseKey(
  projectPath: string,
  section: ProjectPanelCollapseSection,
  id = ""
): string {
  return [
    normalizeProjectPathForCompare(projectPath),
    section,
    id,
  ].join(PROJECT_PANEL_COLLAPSE_KEY_SEP);
}

const [collapsedProjects, setCollapsedProjects] = createSignal<Record<string, boolean>>({});

function projectKey(projectPath: string): string {
  return projectPanelCollapseKey(projectPath, "project");
}

export const projectCollapseStore = {
  isProjectCollapsed(projectPath: string): boolean {
    return collapsedProjects()[projectKey(projectPath)] ?? false;
  },
  setProjectCollapsed(projectPath: string, collapsed: boolean): void {
    const key = projectKey(projectPath);
    setCollapsedProjects((prev) => ({ ...prev, [key]: collapsed }));
  },
  toggleProjectCollapsed(projectPath: string): void {
    const key = projectKey(projectPath);
    setCollapsedProjects((prev) => ({ ...prev, [key]: !(prev[key] ?? false) }));
  },
  collapseAllProjectsExcept(projectPath: string): void {
    const ownerKey = projectKey(projectPath);
    setCollapsedProjects((prev) => {
      const next: Record<string, boolean> = {};
      for (const k of Object.keys(prev)) {
        next[k] = k === ownerKey ? prev[k] : true;
      }
      return next;
    });
  },
  collapseAllExceptKnown(ownerPath: string, allPaths: string[]): void {
    const ownerKey = projectKey(ownerPath);
    setCollapsedProjects((prev) => {
      const next: Record<string, boolean> = { ...prev };
      for (const p of allPaths) {
        const k = projectKey(p);
        if (k !== ownerKey) next[k] = true;
      }
      return next;
    });
  },
  resetForTests(): void {
    setCollapsedProjects({});
  },
};
