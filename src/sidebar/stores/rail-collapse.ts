import { createSignal } from "solid-js";
import { normalizeProjectPathForCompare } from "./project-refresh";
import { SettingsAPI } from "../../shared/ipc";
import type { AppSettings } from "../../shared/types";

// #965 - collapse state for the WorkgroupGroupRail's category headers.
//
// DELIBERATELY SEPARATE from `project-collapse.ts` (the ProjectPanel's card
// collapse). The user's ruling: the rail folds ONLY on an explicit header click.
// The #810 auto-focus (`collapseAllExceptKnown`, fired on every rail group click)
// keeps driving the ProjectPanel exactly as before and must have ZERO effect here.
// That is why this is its own module with its own signals and only two mutators,
// both bound to a header click. Do not add a `collapseAllExcept*` here, and do not
// import this store from `project-collapse.ts`. (Plan RC-1 / RC-2.)
//
// Persisted (unlike the ProjectPanel store) via the `set_rail_collapse` narrow
// setter, written straight through on each toggle - no debounce, no dedupe: a
// header click is a human-rate event, and a trailing debounce would lose the last
// toggle if the user quits inside the window.

// Keyed by the normalized project path directly. No composite section key: this
// map holds exactly one notion. The persisted `railCollapsedProjects` is this key
// set verbatim, so restore is exact and idempotent.
const [collapsedProjects, setCollapsedProjects] = createSignal<Record<string, boolean>>({});
const [favoritesCollapsed, setFavoritesCollapsed] = createSignal(false);

function key(projectPath: string): string {
  return normalizeProjectPathForCompare(projectPath);
}

function snapshot(): { collapsedProjects: string[]; favoritesCollapsed: boolean } {
  const map = collapsedProjects();
  return {
    collapsedProjects: Object.keys(map)
      .filter((k) => map[k])
      .sort(),
    favoritesCollapsed: favoritesCollapsed(),
  };
}

function persist(): void {
  const next = snapshot();
  // Fire-and-forget: this is cosmetic UI state, and a settings-write failure must
  // never surface as a broken rail. But it is NOT silent: the backend save is
  // read+serialize+tmp+rename, so a transiently unreadable settings.json aborts it
  // (plan §0.5). Warn so that class is debuggable instead of invisible.
  void SettingsAPI.setRailCollapse(next.collapsedProjects, next.favoritesCollapsed).catch((err) => {
    console.warn("[rail-collapse] failed to persist rail collapse:", err);
  });
}

export const railCollapseStore = {
  isProjectCollapsed(projectPath: string): boolean {
    return collapsedProjects()[key(projectPath)] ?? false;
  },
  isFavoritesCollapsed(): boolean {
    return favoritesCollapsed();
  },

  // The ONLY two mutators. Both are an explicit header click. See RC-1 before
  // adding a third.
  toggleProjectCollapsed(projectPath: string): void {
    const k = key(projectPath);
    setCollapsedProjects((prev) => ({ ...prev, [k]: !(prev[k] ?? false) }));
    persist();
  },
  toggleFavoritesCollapsed(): void {
    setFavoritesCollapsed((prev) => !prev);
    persist();
  },

  // #965 - single hydration point (RC-3). Sets the signals directly, never through
  // the toggles, so it never writes back.
  //
  // DO NOT wrap this in a createEffect over `settingsStore.current`. That store
  // re-fetches on `coding_agent_settings_updated` (`shared/stores/settings.ts:28-32`),
  // and a reactive hydrate would clobber the user's in-session rail collapse with
  // the on-disk snapshot, mid-session. One-shot call only.
  hydrateFromSettings(settings: AppSettings | null): void {
    const next: Record<string, boolean> = {};
    for (const path of settings?.railCollapsedProjects ?? []) next[key(path)] = true;
    setCollapsedProjects(next);
    setFavoritesCollapsed(!!settings?.railFavoritesCollapsed);
  },

  resetForTests(): void {
    setCollapsedProjects({});
    setFavoritesCollapsed(false);
  },
};
