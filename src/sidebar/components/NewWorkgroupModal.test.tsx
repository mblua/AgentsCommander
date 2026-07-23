// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewWorkgroupModal from "./NewWorkgroupModal";
import { toastStore } from "../../shared/stores/toasts";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-1-dev-team`;
const teams = [
  {
    name: "dev-team",
    agents: ["_agent_dev"],
    coordinator: "_agent_dev",
  },
];

function button(label: string): HTMLButtonElement {
  const found = Array.from(document.body.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(found instanceof HTMLButtonElement)) throw new Error(`Button not found: ${label}`);
  return found;
}

function taskInput(): HTMLInputElement {
  const found = document.body.querySelector('input[placeholder="Task title (required)"]');
  if (!(found instanceof HTMLInputElement)) throw new Error("Task input not found");
  return found;
}

describe("NewWorkgroupModal clone results", () => {
  let restoreDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    restoreDom = installBrowserDomStubs();
    resetUiStoresForTests();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    restoreDom?.();
    restoreDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("reloads and closes without a toast when every repository cloned", async () => {
    const fake = new FakeTransport();
    fake.resolve("create_workgroup", {
      path: workgroupPath,
      cloneErrors: [],
    });
    fake.resolve("discover_project", discovery());
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => (
        <NewWorkgroupModal projectPath={projectPath} teams={teams} onClose={onClose} />
      ),
      fake,
    );

    input(taskInput(), "Implement the feature");
    click(button("Create"));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("create_workgroup")).toEqual([
      {
        cmd: "create_workgroup",
        args: {
          projectPath,
          teamName: "dev-team",
          taskTitle: "Implement the feature",
        },
      },
    ]);
    expect(fake.callsFor("discover_project")).toHaveLength(1);
    expect(toastStore.items).toHaveLength(0);
  });

  it("surfaces partial clone failures in a sticky credential-safe toast", async () => {
    const fake = new FakeTransport();
    fake.resolve("create_workgroup", {
      path: workgroupPath,
      cloneErrors: [
        {
          url: "https://secret-token@github.com/acme/private.git",
          error: "git clone failed: terminal prompts disabled",
        },
      ],
    });
    fake.resolve("discover_project", discovery());
    const onClose = vi.fn();
    rendered = renderWithFakeTransport(
      () => (
        <NewWorkgroupModal projectPath={projectPath} teams={teams} onClose={onClose} />
      ),
      fake,
    );

    input(taskInput(), "Implement the feature");
    click(button("Create"));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("discover_project")).toHaveLength(1);
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0]).toMatchObject({ kind: "error", exiting: false });
    expect(toastStore.items[0].message).toContain("Workgroup created");
    expect(toastStore.items[0].message).toContain("1 repository failed to clone");
    expect(toastStore.items[0].message).toContain(
      "https://<credentials>@github.com/acme/private.git",
    );
    expect(toastStore.items[0].message).toContain("use an SSH URL");
    expect(toastStore.items[0].message).not.toContain("secret-token");
  });
});
