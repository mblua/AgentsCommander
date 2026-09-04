// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  bridge,
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { liveSelection, SESSION_A } from "../shared/testing/session-selection";

const projectPath = "C:\\Project";
const agentPath = `${projectPath}\\.ac\\_agent_General`;
const generalSession = session({
  id: SESSION_A,
  name: "General",
  workingDirectory: agentPath,
  status: "active",
});
const bridgePayload = bridge({
  sessionId: SESSION_A,
  botId: "bot-1",
  botLabel: "Ops Bot",
  color: "#4ade80",
  status: "active",
});

function setupMessagingTransport(fake: FakeTransport): void {
  fake.resolve(
    "get_settings",
    baseSettings({
      projectPaths: [projectPath],
      projectPath,
      telegramBots: [
        {
          id: "bot-1",
          label: "Ops Bot",
          token: "redacted",
          chatId: 123,
          color: "#4ade80",
        },
      ],
    })
  );
  fake.resolve("open_project", {
    path: projectPath,
    registered: true,
    created: false,
  });
  fake.resolve(
    "discover_project",
    discovery({
      agents: [
        {
          name: "General",
          path: agentPath,
          roleExists: true,
        },
      ],
      teams: [],
      workgroups: [],
    })
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", [generalSession]);
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("telegram_attach", bridgePayload);
  fake.resolve("telegram_detach", undefined);
}

describe("SidebarApp Telegram messaging workflow", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("invokes telegram_attach and updates from bridge attached event", async () => {
    const fake = new FakeTransport();
    setupMessagingTransport(fake);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("General");
        expect(rendered.root.querySelector(".session-item-telegram")).not.toBeNull();
      });

      const button = rendered.root.querySelector(".session-item-telegram")!;
      click(button);

      await waitFor(() =>
        expect(fake.lastCall("telegram_attach")?.args).toEqual({
          sessionId: SESSION_A,
          botId: "bot-1",
        })
      );

      fake.emitFromBackend("telegram_bridge_attached", bridgePayload);

      await waitFor(() => {
        const activeButton = rendered.root.querySelector(".session-item-telegram.active");
        expect(activeButton).not.toBeNull();
        expect(activeButton?.getAttribute("title")).toBe("Detach Telegram: Ops Bot");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("invokes telegram_detach from an active bridge and updates from detach event", async () => {
    const fake = new FakeTransport();
    setupMessagingTransport(fake);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector(".session-item-telegram")).not.toBeNull()
      );

      fake.emitFromBackend("telegram_bridge_attached", bridgePayload);
      await waitFor(() =>
        expect(rendered.root.querySelector(".session-item-telegram.active")).not.toBeNull()
      );

      click(rendered.root.querySelector(".session-item-telegram.active")!);

      await waitFor(() =>
        expect(fake.lastCall("telegram_detach")?.args).toEqual({
          sessionId: SESSION_A,
        })
      );

      fake.emitFromBackend("telegram_bridge_detached", { sessionId: SESSION_A });

      await waitFor(() => {
        expect(rendered.root.querySelector(".session-item-telegram.active")).toBeNull();
        expect(rendered.root.querySelector(".session-item-bridge-dot")).toBeNull();
      });
    } finally {
      rendered.cleanup();
    }
  });
});
