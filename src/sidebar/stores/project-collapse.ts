import { createSignal } from "solid-js";
import { normalizeProjectPathForCompare } from "./project-refresh";

// #810/#941 - Project-level (NOT sub-section) collapse state is hoisted into a
// shared store so WorkgroupGroupRail can collapse others and expand the owner.
// Sub-section collapse (workgroups/loops/agents/teams/workgroup/team) STAYS in
// ProjectPanel's local collapsedByKey signal. This store only owns the "project"
// section key. Session-only, no persistence
// (consistent with Role.md: no localStorage for UI state).
export const PROJECT_PANEL_COLLAPSE_KEY_SEP = "\u0000";

export type ProjectPanelCollapseSection =
  | "project"
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

// Only the "project" section lives here. Keyed by the SAME composite string
// ProjectPanel used locally (projectPath-normalized + "project" + "").
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
  // #810 - one-shot: collapse every KNOWN project except the owner. "Known"
  // means any project path that has an entry in the map (i.e. has been
  // toggled or auto-focused before). The rail does NOT use this overload -
  // it uses collapseAllExceptKnown with the live projectStore.projects list,
  // because on a fresh session the map is {} and this method would no-op
  // (grinch F2). This method is kept for the unit test to assert the
  // known-set semantics and as a future-proof API.
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
  // Overload for collapsing an explicit list of project paths. Used by the
  // rail onClick (grinch F2): feeds the live projectStore.projects so
  // "collapse others" works on a fresh session where the map is empty. The
  // owner is deliberately left untouched so the caller composes the two
  // intents (this + setProjectCollapsed(owner, false)).
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
