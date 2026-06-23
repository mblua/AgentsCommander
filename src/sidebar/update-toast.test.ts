import { describe, it, expect, beforeEach } from "vitest";
import { createUpdateToaster } from "./update-toast";
import { toastStore } from "../shared/stores/toasts";
import type { UpdateInfo } from "../shared/types";

function info(latestVersion: string): UpdateInfo {
  return {
    currentVersion: "0.9.16",
    latestVersion,
    upgradeCommand: "npm i -g @mblua/agentscommander",
  };
}

describe("createUpdateToaster (#609)", () => {
  beforeEach(() => toastStore.clear());

  it("shows one sticky info toast carrying the version + upgrade command", () => {
    const show = createUpdateToaster();
    show(info("0.9.17"));

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[0].message).toContain("0.9.17");
    expect(toastStore.items[0].message).toContain(
      "npm i -g @mblua/agentscommander",
    );
  });

  it("dedups the same latestVersion (event + snapshot race) to one toast", () => {
    const show = createUpdateToaster();
    show(info("0.9.17"));
    show(info("0.9.17"));

    expect(toastStore.items).toHaveLength(1);
  });

  it("shows a second toast when latestVersion changes", () => {
    const show = createUpdateToaster();
    show(info("0.9.17"));
    show(info("0.9.18"));

    expect(toastStore.items).toHaveLength(2);
  });

  it("gives each toaster instance independent dedup state", () => {
    const a = createUpdateToaster();
    const b = createUpdateToaster();
    a(info("0.9.17"));
    b(info("0.9.17"));

    expect(toastStore.items).toHaveLength(2);
  });
});
