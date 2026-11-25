// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewTeamModal from "./NewTeamModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, discovery, input, installBrowserDomStubs, renderWithFakeTransport,
  resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";


const destination = "C:/Destination";
const local = { name: "Copied role", path: `${destination}/.ac/_agent_local`,
  projectName: "Destination", projectPath: destination, isLocal: true };

describe("NewTeamModal tutorial markers", () => {
  let rendered: ReturnType<typeof renderWithFakeTransport> | undefined;
  let restoreDom: () => void;
  let fake: FakeTransport;
  beforeEach(() => {
    restoreDom = installBrowserDomStubs();
    resetUiStoresForTests();
    fake = new FakeTransport();
    fake.resolve("create_team", undefined);
    fake.resolve("discover_project", discovery());
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });
  afterEach(() => {
    rendered?.cleanup();
    restoreDom();
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });
  function button(text: string): HTMLButtonElement {
    const found = Array.from(document.querySelectorAll("button"))
      .find(b => b.textContent?.trim() === text);
    if (!found) throw new Error(`Missing button ${text}`);
    return found;
  }
  function field(selector: string): HTMLInputElement {
    const found = document.querySelector<HTMLInputElement>(selector);
    if (!found) throw new Error(`Missing input ${selector}`);
    return found;
  }
  const marker = (id: string) => document.querySelector(`[data-ac-testid="${id}"]`);
  const page = () => marker("newTeam.modal")?.getAttribute("data-ac-state");

  it("marks the page in newTeam.modal", async () => {
    fake.resolve("list_all_agents", [local]);
    rendered = renderWithFakeTransport(
      () => <NewTeamModal projectPath={destination} onClose={vi.fn()} />, fake);
    expect(page()).toBe("step-1");
    expect(marker("newTeam.name")).not.toBeNull();
    input(field('[placeholder="dream-team"]'), "team");
    click(button("Next"));
    await waitFor(() => expect(document.querySelector(".wizard-loading")).toBeNull());
    expect(page()).toBe("step-2");
    expect(marker("newTeam.agents")).not.toBeNull();
    click(field('.wizard-agent-row input[type="checkbox"]'));
    click(field('input[type="radio"]'));
    click(button("Next"));
    expect(page()).toBe("step-3");
    expect(marker("newTeam.create")).not.toBeNull();
    click(button("Back"));
    expect(page()).toBe("step-2");
  });
});
