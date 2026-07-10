import { afterEach, describe, expect, it } from "vitest";
import { autoUnarchiveStore } from "../../sidebar/stores/auto-unarchive";
import { resetUiStoresForTests } from "./ui-harness";

describe("resetUiStoresForTests archive state (#881)", () => {
  afterEach(() => {
    resetUiStoresForTests();
  });

  it("clears pending auto-unarchive entries", () => {
    autoUnarchiveStore.push({
      path: "C:\\Project",
      folderName: "Project",
      archived: false,
      reason: "autoUnarchive",
      sessionName: "wg-1/dev",
    });

    expect(autoUnarchiveStore.open).toBe(true);

    resetUiStoresForTests();

    expect(autoUnarchiveStore.open).toBe(false);
    expect(autoUnarchiveStore.entries).toEqual([]);
  });
});
