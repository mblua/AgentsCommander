/**
 * #1171 - a zero-padded 24-hour clock, independent of locale.
 *
 * The Resource Monitor's own formatter (`resource-monitor/App.tsx:62-71`) could not be
 * reused for two reasons: it is a module-local `const`, not exported, and it returns
 * `toLocaleTimeString(...)`, which is `02:31:05 PM` under en-US. The watcher table needs a
 * fixed-width column whose rows line up, so the format is pinned here instead of delegated
 * to the viewer's locale. The Resource Monitor is deliberately left on its own formatter.
 */
export function formatClockTime(value: string | null | undefined): string {
  if (!value) return "--:--:--";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return "--:--:--";
  const hours = String(parsed.getHours()).padStart(2, "0");
  const minutes = String(parsed.getMinutes()).padStart(2, "0");
  const seconds = String(parsed.getSeconds()).padStart(2, "0");
  return `${hours}:${minutes}:${seconds}`;
}

/**
 * #1682 - the terminal status strip's stamp: local `MM-DD HH:MM`, zero padded,
 * no seconds and no year. `new Date(value)` fixes the instant from the offset the
 * backend wrote; `getMonth`/`getDate`/`getHours`/`getMinutes` are the local-time
 * getters, so the host's zone and DST are applied by the platform.
 *
 * Returns "" for a missing or unparseable value: the strip renders nothing at
 * all rather than a placeholder.
 */
export function formatAgentMessageStamp(value: string | null | undefined): string {
  if (!value) return "";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return "";
  const month = String(parsed.getMonth() + 1).padStart(2, "0");
  const day = String(parsed.getDate()).padStart(2, "0");
  const hours = String(parsed.getHours()).padStart(2, "0");
  const minutes = String(parsed.getMinutes()).padStart(2, "0");
  return `${month}-${day} ${hours}:${minutes}`;
}
