import { describe, it, expect } from "vitest";
import {
  applyTemplatePrefill,
  filterRoleTemplates,
  slugifyTemplateName,
  sourceLabel,
} from "./role-templates";
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

describe("applyTemplatePrefill", () => {
  const geographer = {
    name: "Geographer",
    description: "Expert in physical and human geography.",
  };
  const historian = {
    name: "Historian",
    description: "Expert in historical analysis.",
  };

  it("populates_both_fields_from_a_blank_form", () => {
    const next = applyTemplatePrefill(geographer, {
      name: "",
      description: "",
      nameDirty: false,
      descriptionDirty: false,
    });
    expect(next).toEqual({
      name: "geographer",
      description: "Expert in physical and human geography.",
    });
  });

  it("replaces_previous_template_values_on_second_selection_278", () => {
    // Regression for #278: after picking Geographer, picking Historian must
    // overwrite both fields because nothing in between marked them dirty.
    const afterFirst = applyTemplatePrefill(geographer, {
      name: "",
      description: "",
      nameDirty: false,
      descriptionDirty: false,
    });
    const afterSecond = applyTemplatePrefill(historian, {
      name: afterFirst.name,
      description: afterFirst.description,
      nameDirty: false,
      descriptionDirty: false,
    });
    expect(afterSecond).toEqual({
      name: "historian",
      description: "Expert in historical analysis.",
    });
  });

  it("preserves_user_edited_name_but_populates_description", () => {
    const next = applyTemplatePrefill(geographer, {
      name: "my-custom-name",
      description: "",
      nameDirty: true,
      descriptionDirty: false,
    });
    expect(next.name).toBe("my-custom-name");
    expect(next.description).toBe("Expert in physical and human geography.");
  });

  it("preserves_user_edited_description_but_populates_name", () => {
    const next = applyTemplatePrefill(geographer, {
      name: "",
      description: "user wrote this",
      nameDirty: false,
      descriptionDirty: true,
    });
    expect(next.name).toBe("geographer");
    expect(next.description).toBe("user wrote this");
  });

  it("preserves_both_when_both_were_user_edited", () => {
    const next = applyTemplatePrefill(geographer, {
      name: "my-name",
      description: "my description",
      nameDirty: true,
      descriptionDirty: true,
    });
    expect(next).toEqual({ name: "my-name", description: "my description" });
  });

  it("clamps_template_description_to_250_chars", () => {
    const long = { name: "Long", description: "x".repeat(400) };
    const next = applyTemplatePrefill(long, {
      name: "",
      description: "",
      nameDirty: false,
      descriptionDirty: false,
    });
    expect(next.description).toHaveLength(250);
    expect(next.name).toBe("long");
  });

  it("preserves_user_blank_field_marked_dirty", () => {
    // If the user cleared a template-populated field on purpose, picking
    // another template must not refill it — nameDirty is the user's signal of
    // intent, regardless of whether the current value is empty.
    const next = applyTemplatePrefill(historian, {
      name: "",
      description: "",
      nameDirty: true,
      descriptionDirty: true,
    });
    expect(next).toEqual({ name: "", description: "" });
  });
});

describe("sourceLabel", () => {
  it("returns_agency_upstream_url_for_agency_templates", () => {
    expect(sourceLabel({ source: "agency" })).toBe(
      "SOURCE: https://github.com/msitarzewski/agency-agents",
    );
  });

  it("returns_local_templates_label_for_local_templates", () => {
    expect(sourceLabel({ source: "local" })).toBe("SOURCE: LOCAL AGENT-TEMPLATES");
  });

  it("accepts_full_template_meta", () => {
    expect(sourceLabel(meta({ source: "local", id: "local:foo" }))).toBe(
      "SOURCE: LOCAL AGENT-TEMPLATES",
    );
    expect(sourceLabel(meta({ source: "agency" }))).toBe(
      "SOURCE: https://github.com/msitarzewski/agency-agents",
    );
  });
});
