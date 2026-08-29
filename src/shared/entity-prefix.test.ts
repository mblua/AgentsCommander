import { describe, expect, it } from "vitest";
import {
  LEGACY_WORKGROUP_DIR_PREFIX,
  ROOM_DIR_PREFIX,
  entityDirNumber,
  entityShortLabel,
  isEntityDirName,
  isNumberedEntityDirName,
  pathHasEntityDirSegment,
} from "./entity-prefix";

// #1614 D2/D4: five functions rather than one, because each preserves its call
// site's exact case sensitivity. WorkgroupTask.tsx documents that its gate must
// stay case-sensitive or the TASK buttons enable for clicks that always fail,
// while the rail's two predicates are display-only and have always been
// case-insensitive. Collapsing them would be a silent behaviour change, so the
// split is asserted here rather than left to the call sites.
describe("entity-prefix", () => {
  it("exposes both on-disk prefixes", () => {
    expect(ROOM_DIR_PREFIX).toBe("room-");
    expect(LEGACY_WORKGROUP_DIR_PREFIX).toBe("wg-");
  });

  describe("isEntityDirName is case-SENSITIVE", () => {
    it.each(["room-1-t", "wg-1-t", "room-", "wg-"])("accepts %s", (n) => {
      expect(isEntityDirName(n)).toBe(true);
    });
    it.each(["ROOM-1-t", "WG-1-t", "Room-1-t", "roomx", "wgx", "", "_team_t", "__agent_x"])(
      "rejects %s",
      (n) => {
        expect(isEntityDirName(n)).toBe(false);
      },
    );
  });

  describe("isNumberedEntityDirName is case-SENSITIVE and needs a digit", () => {
    it.each(["room-1-t", "wg-1-t", "room-12"])("accepts %s", (n) => {
      expect(isNumberedEntityDirName(n)).toBe(true);
    });
    it.each(["ROOM-1-t", "WG-1-t", "room-t", "wg-t", "roomx", ""])("rejects %s", (n) => {
      expect(isNumberedEntityDirName(n)).toBe(false);
    });
  });

  describe("entityDirNumber is case-INSENSITIVE", () => {
    it.each([
      ["room-1-t", 1],
      ["wg-1-t", 1],
      ["ROOM-1-t", 1],
      ["WG-1-t", 1],
      ["room-12-t", 12],
    ])("%s -> %i", (n, want) => {
      expect(entityDirNumber(n as string)).toBe(want);
    });
    it.each(["roomx", "wgx", "room-t", ""])("%s -> null", (n) => {
      expect(entityDirNumber(n)).toBeNull();
    });
  });

  describe("entityShortLabel is case-INSENSITIVE and follows the real prefix", () => {
    it.each([
      ["room-1-t", "ROOM1"],
      ["wg-1-t", "WG1"],
      ["ROOM-1-t", "ROOM1"],
      ["WG-1-t", "WG1"],
      ["room-12-team", "ROOM12"],
    ])("%s -> %s", (n, want) => {
      expect(entityShortLabel(n as string)).toBe(want);
    });
    it.each(["roomx", "wgx", ""])("%s -> null", (n) => {
      expect(entityShortLabel(n)).toBeNull();
    });
  });

  describe("pathHasEntityDirSegment is case-SENSITIVE", () => {
    it.each([
      "C:\\P\\.ac\\room-1-t\\__agent_x",
      "C:\\P\\.ac\\wg-1-t\\__agent_x",
      "/p/.ac/room-1-t/__agent_x",
      "/p/.ac/wg-1-t/__agent_x",
    ])("accepts %s", (p) => {
      expect(pathHasEntityDirSegment(p)).toBe(true);
    });
    it.each([
      "C:\\P\\.ac\\ROOM-1-t\\__agent_x",
      "C:\\P\\.ac\\WG-1-t\\__agent_x",
      "C:\\P\\.ac\\_agent_x",
      "room-1-t",
      "",
    ])("rejects %s", (p) => {
      expect(pathHasEntityDirSegment(p)).toBe(false);
    });
  });
});
