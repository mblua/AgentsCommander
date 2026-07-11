import { createSignal } from "solid-js";
import { normalizeProjectPathForCompare } from "./project-refresh";

// #810/#941 - Project-level (NOT sub-section) collapse state is hoisted into a
// shared store so WorkgroupGroupRail can collapse others and expand the owner.
// Its one-shot focus target coordinates the rail with App.tsx, which owns and
// positions the shared scroll container. Sub-section collapse (workgroups/loops/agents/
// teams/workgroup/team) STAYS in ProjectPanel's local collapsedByKey signal;
// this store only owns the "project" section key. Session-only, no persistence
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

// #810/#941 - one-shot focus target. Stored NORMALIZED (via
// normalizeProjectPathForCompare) so the rail can pass the raw props.project.path
// and SidebarApp can match it to the active semantic selection. SidebarApp
// consumes it after positioning that project's header at the top of the shared
// scrollport. It stays null when no focus is requested.
const [focusTarget, setFocusTarget] = createSignal<string | null>(null);

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
  // #810/#941 - focus target is stored NORMALIZED. The rail passes the raw
  // props.project.path; SidebarApp matches rendered header paths using the
  // same normalized form.
  focusTarget(): string | null {
    return focusTarget();
  },
  requestProjectFocus(projectPath: string): void {
    setFocusTarget(normalizeProjectPathForCompare(projectPath));
  },
  // #810/#941 - one-shot consume: returns the current target and clears it.
  // SidebarApp checks its captured semantic key after deferring, so a stale
  // microtask from click A cannot consume click B's pending target.
  consumeProjectFocus(): string | null {
    const current = focusTarget();
    if (current !== null) setFocusTarget(null);
    return current;
  },
  resetForTests(): void {
    setCollapsedProjects({});
    setFocusTarget(null);
  },
};
