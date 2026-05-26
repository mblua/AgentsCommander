const FRONTMATTER_RE = /^---[ \t]*\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/;
const BOM = 0xfeff;

export function stripFrontmatter(text: string): string {
  if (!text) return text;
  const stripped = text.charCodeAt(0) === BOM ? text.slice(1) : text;
  return stripped.replace(FRONTMATTER_RE, '').replace(/^[ \t]*\r?\n+/, '');
}

/**
 * Mirror of `extract_task_first_line` in `src-tauri/src/commands/ac_discovery.rs`.
 * Returns the first non-empty content line (frontmatter stripped, leading "# "
 * prefixes greedily removed), or null when the input has none. Keeping this in
 * lockstep with the Rust function avoids the sidebar showing a different value
 * after a watcher-emit vs. after a fresh `discover_project` call.
 */
export function taskFirstLine(content: string | null | undefined): string | null {
  if (!content) return null;
  const stripped = stripFrontmatter(content);
  for (const raw of stripped.split(/\r?\n/)) {
    const line = raw.trim();
    if (line.length === 0) continue;
    return line.replace(/^(?:# )+/, "");
  }
  return null;
}
