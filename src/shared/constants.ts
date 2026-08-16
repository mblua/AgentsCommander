export const NO_TEAM = "__no_team__";

export const CANONICAL_AC_ROOT_DIR = ".ac";

export const WINDOW_TYPE = (() => {
  const search = typeof window === "undefined" ? "" : window.location.search;
  const params = new URLSearchParams(search);
  return params.get("window") || "sidebar";
})();

export const IS_SIDEBAR = WINDOW_TYPE === "sidebar";
export const IS_TERMINAL = WINDOW_TYPE === "terminal";
