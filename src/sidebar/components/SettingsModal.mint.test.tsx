// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import type { AcAgentReplica, AcWorkgroup } from "../../shared/types";

const PROJECT_PATH = "C:\\Users\\Maria\\Project";
const WORKGROUP_NAME = "wg-11-dev-v4-team";
const WORKGROUP_PATH = `${PROJECT_PATH}\\.ac\\${WORKGROUP_NAME}`;
const REPLICA_PATH = `${WORKGROUP_PATH}\\__agent_dev-webpage-ui`;

function replica(overrides: Partial<AcAgentReplica> = {}): AcAgentReplica {
  return {
    name: "dev-webpage-ui",
    path: REPLICA_PATH,
    repoPaths: [],
    isCoordinator: false,
    ...overrides,
  };
}

function workgroup(agents: AcAgentReplica[] = [replica()]): AcWorkgroup {
  return {
    name: WORKGROUP_NAME,
    path: WORKGROUP_PATH,
    task: null,
    agents,
  };
}

function prepareFake(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_web_server_status", false);
  fake.resolve("api_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  fake.resolve("new_project", { path: PROJECT_PATH, registered: true, created: true });
  fake.resolve("discover_project", discovery({ workgroups: [workgroup()] }));
}

async function loadReplicaRoot(): Promise<void> {
  await projectStore.createAndLoad(PROJECT_PATH);
}

function target<T extends Element>(root: ParentNode, testId: string): T {
  const element = root.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`missing ${testId}`);
  return element;
}

describe("SettingsModal API client mint UI (#853)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("sources selectable roots from discovered workgroup replicas and shows the one-time token", async () => {
    const fake = new FakeTransport();
    prepareFake(fake);
    fake.resolve("mint_api_client", {
      clientId: "client-123",
      token: "secret-once",
      boundFqn: `${WORKGROUP_NAME}/dev-webpage-ui`,
      boundRoot: REPLICA_PATH,
      scopes: ["send", "list-peers-lean"],
      expiresAt: "2026-07-08T21:00:00Z",
      note: "Store this token now.",
    });
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    const rendered = renderWithFakeTransport(() => <SettingsModal onClose={() => {}} />, fake);
    try {
      await waitFor(() => expect(target(rendered.root, "settings.apiClientMint.surface")).toBeTruthy());
      await loadReplicaRoot();

      await waitFor(() => {
        const rootSelect = target<HTMLSelectElement>(rendered.root, "settings.apiClientMint.root");
        expect(rootSelect.tagName).toBe("SELECT");
        expect(rootSelect.value).toBe(REPLICA_PATH);
        expect(rootSelect.options).toHaveLength(1);
        expect(rootSelect.options[0].textContent).toContain("dev-webpage-ui");
      });

      click(target(rendered.root, "settings.apiClientMint.submit"));

      await waitFor(() =>
        expect(target<HTMLInputElement>(rendered.root, "settings.apiClientMint.token").value).toBe(
          "secret-once",
        ),
      );
      expect(target(rendered.root, "settings.apiClientMint.clientId").textContent).toContain(
        "client-123",
      );
      expect(target(rendered.root, "settings.apiClientMint.boundFqn").textContent).toContain(
        `${WORKGROUP_NAME}/dev-webpage-ui`,
      );
      expect(target(rendered.root, "settings.apiClientMint.expiresAt").textContent).toContain(
        "2026-07-08T21:00:00Z",
      );

      expect(fake.callsFor("mint_api_client")[0]?.args).toEqual({
        root: REPLICA_PATH,
        scopes: ["send", "list-peers-lean"],
        label: null,
        expires: null,
      });

      click(target(rendered.root, "settings.apiClientMint.copy"));
      await waitFor(() => expect(writeText).toHaveBeenCalledWith("secret-once"));

      click(target(rendered.root, "settings.apiClientMint.clear"));
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="settings.apiClientMint.token"]')).toBeNull(),
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("surfaces backend mint errors verbatim", async () => {
    const fake = new FakeTransport();
    prepareFake(fake);
    fake.reject("mint_api_client", `unknown or invalid root: ${REPLICA_PATH}`);
    const rendered = renderWithFakeTransport(() => <SettingsModal onClose={() => {}} />, fake);
    try {
      await waitFor(() => expect(target(rendered.root, "settings.apiClientMint.surface")).toBeTruthy());
      await loadReplicaRoot();
      await waitFor(() =>
        expect(target<HTMLSelectElement>(rendered.root, "settings.apiClientMint.root").value).toBe(
          REPLICA_PATH,
        ),
      );

      click(target(rendered.root, "settings.apiClientMint.submit"));

      await waitFor(() =>
        expect(target(rendered.root, "settings.apiClientMint.error").textContent).toContain(
          `unknown or invalid root: ${REPLICA_PATH}`,
        ),
      );
      expect(rendered.root.querySelector('[data-ac-testid="settings.apiClientMint.token"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
