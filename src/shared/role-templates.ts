import type { RoleTemplateMeta } from "./types";

/** Case-insensitive substring filter over name + description + category + source. */
export function filterRoleTemplates(
  templates: RoleTemplateMeta[],
  query: string,
): RoleTemplateMeta[] {
  const q = query.trim().toLowerCase();
  if (!q) return templates;
  return templates.filter(
    (t) =>
      t.name.toLowerCase().includes(q) ||
      t.description.toLowerCase().includes(q) ||
      t.category.toLowerCase().includes(q) ||
      t.source.toLowerCase().includes(q),
  );
}

/**
 * Convert a template display name into a valid agent-folder name: lowercase,
 * whitespace/underscores → hyphens, drop chars outside [a-z0-9-], collapse
 * repeated hyphens, trim leading/trailing hyphens. Idempotent on a name that
 * is already a slug. Returns "" when the name has no slug-able characters —
 * the caller then leaves the Name field empty (same as the no-template path).
 *
 * The New Agent dialog's canCreate() rejects a Name containing a space, `/`,
 * or `\`. Template display names are free text, so a raw setName(t.name) can
 * prefill an invalid Name and silently disable Create. Always run through this.
 */
export function slugifyTemplateName(name: string): string {
  return name
    .toLowerCase()
    .replace(/[\s_]+/g, "-")
    .replace(/[^a-z0-9-]/g, "")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/**
 * Human-readable "SOURCE: …" label rendered in the picker's source badge.
 * Agency templates point at the upstream repo so users can audit them; local
 * templates state they come from the user's own templates folder.
 */
export function sourceLabel(template: Pick<RoleTemplateMeta, "source">): string {
  return template.source === "agency"
    ? "SOURCE: https://github.com/msitarzewski/agency-agents"
    : "SOURCE: LOCAL AGENT-TEMPLATES";
}
