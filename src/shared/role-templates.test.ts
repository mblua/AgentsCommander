import { describe, it, expect } from "vitest";
import { filterRoleTemplates, slugifyTemplateName } from "./role-templates";
import type { RoleTemplateMeta } from "./types";

const meta = (over: Partial<RoleTemplateMeta>): RoleTemplateMeta => ({
  id: "agency:frontend-developer",
  source: "agency",
  name: "Frontend Developer",
  description: "Builds React/SolidJS UIs",
  category: "Engineering",
  color: null,
  emoji: null,
  hasSkills: false,
  ...over,
});

describe("filterRoleTemplates", () => {
  const templates: RoleTemplateMeta[] = [
    meta({ id: "agency:frontend-developer" }),
    meta({
      id: "agency:backend-engineer",
      name: "Backend Engineer",
      description: "Designs APIs and services",
      category: "Engineering",
    }),
    meta({
      id: "agency:paid-media-lead",
      name: "Paid Media Lead",
      description: "Runs ad campaigns",
      category: "Paid Media",
    }),
    meta({
      id: "local:my-helper",
      source: "local",
      name: "my-helper",
      description: "",
      category: "Local",
    }),
  ];

  it("empty_query_returns_all_templates", () => {
    expect(filterRoleTemplates(templates, "")).toHaveLength(templates.length);
  });

  it("whitespace_only_query_returns_all_templates", () => {
    expect(filterRoleTemplates(templates, "   ")).toHaveLength(templates.length);
  });

  it("query_matches_name_case_insensitively", () => {
    const r = filterRoleTemplates(templates, "FRONTEND");
    expect(r).toHaveLength(1);
    expect(r[0].id).toBe("agency:frontend-developer");
  });

  it("query_matches_description_case_insensitively", () => {
    const r = filterRoleTemplates(templates, "apis");
    expect(r).toHaveLength(1);
    expect(r[0].id).toBe("agency:backend-engineer");
  });

  it("query_matches_category_case_insensitively", () => {
    const r = filterRoleTemplates(templates, "paid media");
    expect(r).toHaveLength(1);
    expect(r[0].id).toBe("agency:paid-media-lead");
  });

  it("query_matches_source", () => {
    const r = filterRoleTemplates(templates, "local");
    expect(r.map((t) => t.id)).toContain("local:my-helper");
  });

  it("non_matching_query_returns_empty_array", () => {
    expect(filterRoleTemplates(templates, "nothing-here-zzz")).toEqual([]);
  });

  it("matches_substring_inside_name_via_name_only", () => {
    // "backend" only appears in backend-engineer's name, so this isolates the
    // name-substring path from the category/description matchers.
    const r = filterRoleTemplates(templates, "backend");
    expect(r.map((t) => t.id)).toEqual(["agency:backend-engineer"]);
  });
});

describe("slugifyTemplateName", () => {
  it("lowercases_input", () => {
    expect(slugifyTemplateName("Frontend")).toBe("frontend");
  });

  it("turns_spaces_into_hyphens", () => {
    expect(slugifyTemplateName("Frontend Developer")).toBe("frontend-developer");
  });

  it("turns_underscores_into_hyphens", () => {
    expect(slugifyTemplateName("frontend_developer")).toBe("frontend-developer");
  });

  it("collapses_runs_of_whitespace_and_underscores", () => {
    expect(slugifyTemplateName("Frontend   _  Developer")).toBe("frontend-developer");
  });

  it("drops_chars_outside_a_z_0_9_hyphen", () => {
    expect(slugifyTemplateName("Front!end @Dev?")).toBe("frontend-dev");
  });

  it("collapses_repeated_hyphens", () => {
    expect(slugifyTemplateName("front---end")).toBe("front-end");
  });

  it("trims_leading_and_trailing_hyphens", () => {
    expect(slugifyTemplateName("-frontend-")).toBe("frontend");
    expect(slugifyTemplateName("--front-end--")).toBe("front-end");
  });

  it("is_idempotent_on_an_already_slug_input", () => {
    expect(slugifyTemplateName("frontend-developer")).toBe("frontend-developer");
    expect(slugifyTemplateName(slugifyTemplateName("Frontend Developer"))).toBe(
      "frontend-developer",
    );
  });

  it("returns_empty_string_when_no_slugable_chars", () => {
    expect(slugifyTemplateName("!!!")).toBe("");
    expect(slugifyTemplateName("   ")).toBe("");
    expect(slugifyTemplateName("")).toBe("");
  });

  it("preserves_digits", () => {
    expect(slugifyTemplateName("Agent 007")).toBe("agent-007");
  });
});
