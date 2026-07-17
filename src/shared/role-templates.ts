import type { RoleTemplateMeta } from "./types";

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

export function slugifyTemplateName(name: string): string {
  return name
    .toLowerCase()
    .replace(/[\s_]+/g, "-")
    .replace(/[^a-z0-9-]/g, "")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function sourceLabel(template: Pick<RoleTemplateMeta, "source">): string {
  return template.source === "agency"
    ? "SOURCE: https://github.com/msitarzewski/agency-agents"
    : "SOURCE: LOCAL AGENT-TEMPLATES";
}

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
