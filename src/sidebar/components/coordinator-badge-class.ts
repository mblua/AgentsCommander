import type { CoordinatorIdleLevel } from "../../shared/coordinator-badge";

/**
 * #882 Render boundary: coordinator idle-badge severity -> CSS class. The value
 * type is narrowed to the three literals so a transposed entry is a compile
 * error rather than a silently wrong color. Classes are defined in sidebar.css.
 * This is the only module outside sidebar.css allowed to name these colors.
 */
export const COORD_IDLE_CLASS: Record<CoordinatorIdleLevel, "green" | "yellow" | "red"> = {
  ok: "green",
  warn: "yellow",
  stale: "red",
};
