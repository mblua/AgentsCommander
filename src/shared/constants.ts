export const NO_TEAM = "__no_team__";

export const AC_WORKSPACE_DIR = ".ac";
export const LEGACY_AC_WORKSPACE_DIR = ".ac-new";
export const AC_WORKSPACE_DIRS = [AC_WORKSPACE_DIR, LEGACY_AC_WORKSPACE_DIR] as const;

export const WINDOW_TYPE = (() => {
  const search = typeof window === "undefined" ? "" : window.location.search;
  const params = new URLSearchParams(search);
  return params.get("window") || "sidebar";
})();

export const IS_SIDEBAR = WINDOW_TYPE === "sidebar";
export const IS_TERMINAL = WINDOW_TYPE === "terminal";
