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
 * is already a slug. Returns "" when the name has no slug-able characters -
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

/**
 * Decide the next {name, description} when a template is selected. A template's
 * defaults overwrite the current values UNLESS the user has manually edited the
 * field (the caller flips `nameDirty`/`descriptionDirty` in its onInput
 * handlers). Values that the previous template selection wrote stay
 * non-dirty, so a second template click freely replaces them - fixing #278
 * where the highlighted row moved but the form fields stuck on the first pick.
 *
 * The Name is slugified (display names are free text that the modal's
 * canCreate() would reject for spaces or punctuation); the Description is
 * clamped to 250 because the textarea's maxLength only limits typing, not
 * programmatic writes.
 */
export function applyTemplatePrefill(
  template: Pick<RoleTemplateMeta, "name" | "description">,
  current: {
    name: string;
    description: string;
    nameDirty: boolean;
    descriptionDirty: boolean;
  },
): { name: string; description: string } {
  return {
    name: current.nameDirty ? current.name : slugifyTemplateName(template.name),
    description: current.descriptionDirty
      ? current.description
      : template.description.slice(0, 250),
  };
}
