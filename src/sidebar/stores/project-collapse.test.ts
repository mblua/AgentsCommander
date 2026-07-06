import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  PROJECT_PANEL_COLLAPSE_KEY_SEP,
  projectCollapseStore,
  projectPanelCollapseKey,
} from "./project-collapse";

const projectA = "C:\\ProjectA";
const projectB = "C:\\ProjectB";

describe("projectCollapseStore", () => {
  beforeEach(() => {
    projectCollapseStore.resetForTests();
  });

  afterEach(() => {
    projectCollapseStore.resetForTests();
  });

  it("defaults to expanded (false) for an unknown project path", () => {
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
  });

  it("setProjectCollapsed / isProjectCollapsed round-trip", () => {
    projectCollapseStore.setProjectCollapsed(projectA, true);
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(true);
    projectCollapseStore.setProjectCollapsed(projectA, false);
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
  });

  it("normalizes paths before keying (C:\\ProjectA == c:/projecta)", () => {
    projectCollapseStore.setProjectCollapsed("C:\\ProjectA", true);
    expect(projectCollapseStore.isProjectCollapsed("c:/projecta")).toBe(true);
    expect(projectCollapseStore.isProjectCollapsed("C:\\ProjectA\\")).toBe(true);
  });

  it("toggleProjectCollapsed flips state", () => {
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
    projectCollapseStore.toggleProjectCollapsed(projectA);
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(true);
    projectCollapseStore.toggleProjectCollapsed(projectA);
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
  });

  it("collapseAllProjectsExcept only touches keys already in the map (known-set semantics)", () => {
    // Seed A and B so the map has both keys.
    projectCollapseStore.setProjectCollapsed(projectA, false);
    projectCollapseStore.setProjectCollapsed(projectB, false);
    // Collapse every known project except A.
    projectCollapseStore.collapseAllProjectsExcept(projectA);
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
    expect(projectCollapseStore.isProjectCollapsed(projectB)).toBe(true);
  });

  it("collapseAllProjectsExcept is a no-op on a fresh (empty) map (grinch F2 rationale)", () => {
    // No project has been toggled yet -> map is {} -> nothing to collapse.
    projectCollapseStore.collapseAllProjectsExcept(projectA);
    expect(projectCollapseStore.isProjectCollapsed(projectB)).toBe(false);
  });

  it("collapseAllExceptKnown collapses every path in the list except owner (grinch F2 fix)", () => {
    // Fresh session: map is {}. The rail feeds the live project list.
    projectCollapseStore.collapseAllExceptKnown(projectA, [projectA, projectB]);
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
    expect(projectCollapseStore.isProjectCollapsed(projectB)).toBe(true);
  });

  it("collapseAllExceptKnown preserves owner's prior state and other known entries", () => {
    projectCollapseStore.setProjectCollapsed(projectA, true);
    projectCollapseStore.setProjectCollapsed(projectB, false);
    projectCollapseStore.collapseAllExceptKnown(projectA, [projectA, projectB]);
    // Owner is deliberately untouched by collapseAllExceptKnown; caller must
    // explicitly expand it.
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(true);
    expect(projectCollapseStore.isProjectCollapsed(projectB)).toBe(true);
  });

  it("requestProjectFocus / focusTarget / consumeProjectFocus: consume returns once then null", () => {
    expect(projectCollapseStore.focusTarget()).toBe(null);
    projectCollapseStore.requestProjectFocus(projectA);
    // Stored normalized.
    expect(projectCollapseStore.focusTarget()).toBe("c:/projecta");
    expect(projectCollapseStore.consumeProjectFocus()).toBe("c:/projecta");
    expect(projectCollapseStore.focusTarget()).toBe(null);
    // Second consume is null (already consumed).
    expect(projectCollapseStore.consumeProjectFocus()).toBe(null);
  });

  it("requestProjectFocus normalizes the path before storing", () => {
    projectCollapseStore.requestProjectFocus("C:\\ProjectA\\");
    expect(projectCollapseStore.focusTarget()).toBe("c:/projecta");
  });

  it("resetForTests clears collapse state and focus target", () => {
    projectCollapseStore.setProjectCollapsed(projectA, true);
    projectCollapseStore.requestProjectFocus(projectB);
    projectCollapseStore.resetForTests();
    expect(projectCollapseStore.isProjectCollapsed(projectA)).toBe(false);
    expect(projectCollapseStore.focusTarget()).toBe(null);
  });

  it("projectPanelCollapseKey joins normalized path + section + id with the separator", () => {
    expect(projectPanelCollapseKey("C:\\Project", "project")).toBe(
      `c:/project${PROJECT_PANEL_COLLAPSE_KEY_SEP}project${PROJECT_PANEL_COLLAPSE_KEY_SEP}`
    );
    expect(projectPanelCollapseKey("C:\\Project", "workgroup", "wg-1")).toBe(
      `c:/project${PROJECT_PANEL_COLLAPSE_KEY_SEP}workgroup${PROJECT_PANEL_COLLAPSE_KEY_SEP}wg-1`
    );
  });
});
