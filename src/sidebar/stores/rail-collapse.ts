import { createSignal } from "solid-js";
import { normalizeProjectPathForCompare } from "./project-refresh";
import { SettingsAPI } from "../../shared/ipc";
import type { AppSettings } from "../../shared/types";


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

  toggleProjectCollapsed(projectPath: string): void {
    const k = key(projectPath);
    setCollapsedProjects((prev) => ({ ...prev, [k]: !(prev[k] ?? false) }));
    persist();
  },
  toggleFavoritesCollapsed(): void {
    setFavoritesCollapsed((prev) => !prev);
    persist();
  },

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
