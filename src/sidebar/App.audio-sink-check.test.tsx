// @vitest-environment jsdom
// The sidebar start asks the sound sink status exactly once.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { checkAudioSink } from "./audio-sink-warning";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../shared/testing/ui-harness";

vi.mock("./audio-sink-warning", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./audio-sink-warning")>();
  return { ...actual, checkAudioSink: vi.fn(async () => {}) };
});
vi.mock("./components/TemporaryRoomsPanel", () => ({ default: () => null }));

describe("SidebarApp sound sink check", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    vi.mocked(checkAudioSink).mockClear();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("calls checkAudioSink once per sidebar start", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("drain_session_warnings", []);
    fake.resolve("get_update_status", null);
    fake.resolve("list_sessions", []);
    fake.resolve("telegram_list_bridges", []);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(checkAudioSink).toHaveBeenCalled());
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(checkAudioSink).toHaveBeenCalledTimes(1);
      expect(vi.mocked(checkAudioSink).mock.calls[0]).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });
});
