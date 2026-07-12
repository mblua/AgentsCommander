import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AppSettings } from "../../shared/types";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { railCollapseStore } from "./rail-collapse";

const projectA = "C:\\Foo\\Bar";
const projectB = "C:\\Other\\Project";

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return overrides as AppSettings;
}

function railCollapseCalls(fake: FakeTransport) {
  return fake.callsFor("set_rail_collapse").map((call) => call.args);
}

describe("railCollapseStore (#965)", () => {
  let fake: FakeTransport;
  let restoreTransport: (() => void) | undefined;

  beforeEach(() => {
    fake = new FakeTransport();
    fake.resolve("set_rail_collapse", null);
    restoreTransport = __setTransportForTests(fake);
    railCollapseStore.resetForTests();
  });

  afterEach(() => {
    railCollapseStore.resetForTests();
    restoreTransport?.();
    restoreTransport = undefined;
  });

  describe("hydration (RC-3)", () => {
    it("restores collapsed project paths and the Favorites bit without writing back", () => {
      railCollapseStore.hydrateFromSettings(
        settings({ railCollapsedProjects: ["c:/foo/bar"], railFavoritesCollapsed: true })
      );

      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(true);
      expect(railCollapseStore.isFavoritesCollapsed()).toBe(true);
      // Hydration sets the signals directly, never through the toggles. A write-back
      // here would persist the snapshot we just read, and (worse) invite someone to
      // make hydration reactive.
      expect(railCollapseCalls(fake)).toEqual([]);
    });

    it("treats null settings and missing fields as nothing-collapsed", () => {
      railCollapseStore.hydrateFromSettings(null);
      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(false);
      expect(railCollapseStore.isFavoritesCollapsed()).toBe(false);

      railCollapseStore.hydrateFromSettings(settings());
      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(false);
      expect(railCollapseStore.isFavoritesCollapsed()).toBe(false);
      expect(railCollapseCalls(fake)).toEqual([]);
    });

    it("normalizes paths on both hydrate and query", () => {
      // Persisted entries are frontend-normalized, but a caller passes the raw
      // ProjectState.path. Both ends must agree or restore silently no-ops.
      railCollapseStore.hydrateFromSettings(settings({ railCollapsedProjects: ["C:\\Foo\\Bar"] }));

      expect(railCollapseStore.isProjectCollapsed("c:/foo/bar/")).toBe(true);
      expect(railCollapseStore.isProjectCollapsed("C:\\Foo\\Bar")).toBe(true);
    });

    it("replaces prior state rather than merging into it", () => {
      railCollapseStore.hydrateFromSettings(settings({ railCollapsedProjects: [projectA] }));
      railCollapseStore.hydrateFromSettings(settings({ railCollapsedProjects: [projectB] }));

      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(false);
      expect(railCollapseStore.isProjectCollapsed(projectB)).toBe(true);
    });
  });

  describe("toggles persist the full snapshot", () => {
    it("toggleProjectCollapsed flips the bit and issues exactly one write", () => {
      railCollapseStore.toggleProjectCollapsed(projectA);

      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(true);
      expect(railCollapseCalls(fake)).toEqual([
        { collapsedProjects: ["c:/foo/bar"], favoritesCollapsed: false },
      ]);
    });

    it("toggling back drops the path from the persisted snapshot", () => {
      railCollapseStore.toggleProjectCollapsed(projectA);
      railCollapseStore.toggleProjectCollapsed(projectA);

      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(false);
      expect(railCollapseCalls(fake)).toEqual([
        { collapsedProjects: ["c:/foo/bar"], favoritesCollapsed: false },
        { collapsedProjects: [], favoritesCollapsed: false },
      ]);
    });

    it("toggleFavoritesCollapsed flips the bit and carries the collapsed paths along", () => {
      railCollapseStore.toggleProjectCollapsed(projectA);
      fake.clearCalls();

      railCollapseStore.toggleFavoritesCollapsed();

      expect(railCollapseStore.isFavoritesCollapsed()).toBe(true);
      // Every payload is a FULL snapshot, not a delta. That is what makes an
      // out-of-order write self-healing rather than corrupting (plan §0.5).
      expect(railCollapseCalls(fake)).toEqual([
        { collapsedProjects: ["c:/foo/bar"], favoritesCollapsed: true },
      ]);
    });

    it("persists collapsed paths sorted and normalized", () => {
      railCollapseStore.toggleProjectCollapsed(projectB);
      railCollapseStore.toggleProjectCollapsed(projectA);

      expect(fake.lastCall("set_rail_collapse")?.args).toEqual({
        collapsedProjects: ["c:/foo/bar", "c:/other/project"],
        favoritesCollapsed: false,
      });
    });

    it("does not throw when the settings write fails", async () => {
      // A header click CAN genuinely fail: the backend save is read+serialize+tmp+
      // rename, so a transiently unreadable settings.json aborts it. Fire-and-forget
      // is the right call for cosmetic UI state, but it must not break the rail.
      fake.reject("set_rail_collapse", "settings.json is locked");

      expect(() => railCollapseStore.toggleProjectCollapsed(projectA)).not.toThrow();
      // The in-memory signal is the source of truth and stays correct regardless.
      expect(railCollapseStore.isProjectCollapsed(projectA)).toBe(true);
      await Promise.resolve();
    });
  });
});
