import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../../shared/ipc", () => ({
  HomeAPI: { fetchMarkdown: vi.fn() },
}));

import { homeStore, __resetHomeStoreForTests } from "./home";
import { HomeAPI } from "../../shared/ipc";

describe("homeStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetHomeStoreForTests();
  });

  it("setInitialVisibility(false) -> visible=true (no active session)", () => {
    homeStore.setInitialVisibility(false);
    expect(homeStore.visible).toBe(true);
  });

  it("setInitialVisibility(true) -> visible=false (active session at boot)", () => {
    homeStore.setInitialVisibility(true);
    expect(homeStore.visible).toBe(false);
  });

  it("toggle flips visibility", () => {
    homeStore.hide();
    homeStore.toggle();
    expect(homeStore.visible).toBe(true);
    homeStore.toggle();
    expect(homeStore.visible).toBe(false);
  });

  it("fetch sets content on success", async () => {
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockResolvedValue("# Hello\n");
    await homeStore.fetch();
    expect(homeStore.content).toBe("# Hello\n");
    expect(homeStore.error).toBeNull();
    expect(homeStore.loading).toBe(false);
  });

  it("fetch records error on failure and content stays null when no prior content", async () => {
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockRejectedValue(new Error("Network error: down"));
    await homeStore.fetch();
    expect(homeStore.content).toBeNull();
    expect(homeStore.error).toContain("Network error");
    expect(homeStore.loading).toBe(false);
  });

  it("refresh on success replaces existing content", async () => {
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockResolvedValueOnce("# v1");
    await homeStore.fetch();
    expect(homeStore.content).toBe("# v1");
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockResolvedValueOnce("# v2");
    await homeStore.refresh();
    expect(homeStore.content).toBe("# v2");
  });

  it("refresh failure preserves prior content (does NOT wipe to null)", async () => {
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockResolvedValueOnce("# v1");
    await homeStore.fetch();
    expect(homeStore.content).toBe("# v1");
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("offline"));
    await homeStore.refresh();
    expect(homeStore.content).toBe("# v1");
    expect(homeStore.error).toContain("offline");
  });

  it("concurrent fetch is idempotent", async () => {
    let resolveFn: (v: string) => void = () => {};
    (HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mockReturnValue(
      new Promise<string>((r) => { resolveFn = r; })
    );
    const p1 = homeStore.fetch();
    const p2 = homeStore.fetch();
    resolveFn("ok");
    await Promise.all([p1, p2]);
    expect((HomeAPI.fetchMarkdown as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
  });
});
