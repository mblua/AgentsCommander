export const MAIN_SIDEBAR_MIN_WIDTH = 400;
export const MAIN_SIDEBAR_MAX_WIDTH = 600;
export const MAIN_TERMINAL_MIN_WIDTH = 300;
export const DEFAULT_MAIN_SIDEBAR_WIDTH = 440;

export function clampMainSidebarWidth(raw: number, windowWidth: number): number {
  const upper = Math.min(
    MAIN_SIDEBAR_MAX_WIDTH,
    Math.max(MAIN_SIDEBAR_MIN_WIDTH, windowWidth - MAIN_TERMINAL_MIN_WIDTH),
  );
  return Math.max(MAIN_SIDEBAR_MIN_WIDTH, Math.min(upper, raw));
}
