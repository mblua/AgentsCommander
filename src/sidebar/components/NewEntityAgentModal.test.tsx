// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import NewEntityAgentModal from "./NewEntityAgentModal";
import type { RoleTemplateMeta } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, input, renderWithFakeTransport, waitFor } from "../../shared/testing/ui-harness";

const templates: RoleTemplateMeta[] = ["Alpha", "Beta"].map((name) => ({
  id: name.toLowerCase(),
  name,
  description: name + " role",
  source: "local",
  category: "Fixture",
  hasSkills: false,
}));

const optionNames = (options: HTMLButtonElement[]) =>
  options.map((option) => option.querySelector(".tpl-picker-item-name")?.textContent);

const selected = (options: HTMLButtonElement[]) =>
  options.map((option) => option.getAttribute("aria-selected"));

function keyDown(target: EventTarget, key: string) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  const dispatched = target.dispatchEvent(event);
  return { event, dispatched };
}

describe("NewEntityAgentModal template picker", () => {
  let dispose: (() => void) | undefined;

  afterEach(() => {
    dispose?.();
    dispose = undefined;
  });

  async function mount() {
    const fake = new FakeTransport();
    const onClose = vi.fn();
    fake.resolve("list_role_templates", templates);
    fake.resolve("get_agency_templates_status", { available: true, path: "/fixture" });
    fake.reject("create_agent_matrix", "Unexpected create");
    const { cleanup, root } = renderWithFakeTransport(
      () => <NewEntityAgentModal projectPath="/fixture" onClose={onClose} />,
      fake,
    );
    dispose = cleanup;
    const list = root.querySelector<HTMLDivElement>(".tpl-picker-list")!;
    const options = () => Array.from(list.querySelectorAll<HTMLButtonElement>('[role="option"]'));
    await waitFor(() => expect(optionNames(options())).toEqual(["No template", "Alpha", "Beta"]));
    return {
      fake,
      onClose,
      root,
      list,
      options,
      search: root.querySelector<HTMLInputElement>("#new-agent-template-search")!,
      name: root.querySelector<HTMLInputElement>('input[placeholder="my-agent"]')!,
      description: root.querySelector<HTMLTextAreaElement>("textarea")!,
      create: root.querySelector<HTMLButtonElement>(".new-agent-create-btn")!,
    };
  }

  it("keeps native option stops in order and filters them without losing No template", async () => {
    const { list, options, search, name, create } = await mount();
    expect(list.getAttribute("role")).toBe("listbox");
    expect(list.getAttribute("aria-label")).toBe("Role templates");
    expect(selected(options())).toEqual(["true", "false", "false"]);
    for (const option of options()) {
      expect(option).toBeInstanceOf(HTMLButtonElement);
      expect(option.type).toBe("button");
      expect(option.tabIndex).toBe(0);
      expect(option.disabled).toBe(false);
    }
    expect(name.hasAttribute("autofocus")).toBe(true);
    expect(create.disabled).toBe(true);
    input(search, "Beta");
    expect(optionNames(options())).toEqual(["No template", "Beta"]);
    expect(selected(options())).toEqual(["true", "false"]);
  });

  it("makes the listbox programmatically focusable without adding a Tab stop", async () => {
    const { list, options } = await mount();
    const [none, alpha, beta] = options();
    expect(list.getAttribute("tabindex")).toBe("-1");

    list.focus();
    expect(document.activeElement).toBe(list);
    keyDown(list, "ArrowDown");
    expect(document.activeElement).toBe(none);

    list.focus();
    expect(document.activeElement).toBe(list);
    keyDown(list, "ArrowUp");
    expect(document.activeElement).toBe(beta);

    expect(options()).toEqual([none, alpha, beta]);
    expect(options().map((option) => option.tabIndex)).toEqual([0, 0, 0]);
    expect(selected(options())).toEqual(["true", "false", "false"]);
  });

  it("moves focus from Search and wraps through options without selecting", async () => {
    const { options, search, name, description } = await mount();
    const [none, alpha, beta] = options();
    search.focus();
    expect(document.activeElement).toBe(search);
    keyDown(search, "ArrowDown");
    expect(document.activeElement).toBe(none);
    keyDown(none, "ArrowUp");
    expect(document.activeElement).toBe(beta);
    keyDown(beta, "ArrowDown");
    expect(document.activeElement).toBe(none);
    keyDown(none, "ArrowDown");
    expect(document.activeElement).toBe(alpha);
    expect(selected(options())).toEqual(["true", "false", "false"]);
    expect(name.value).toBe("");
    expect(description.value).toBe("");
  });

  it("prefills pristine fields on option click and clears selection with No template", async () => {
    const { options, name, description, create, fake } = await mount();
    const [none, alpha, beta] = options();
    click(alpha);
    expect(selected(options())).toEqual(["false", "true", "false"]);
    expect(name.value).toBe("alpha");
    expect(description.value).toBe("Alpha role");
    expect(create.disabled).toBe(false);
    click(beta);
    expect(selected(options())).toEqual(["false", "false", "true"]);
    expect(name.value).toBe("beta");
    expect(description.value).toBe("Beta role");
    click(none);
    expect(selected(options())).toEqual(["true", "false", "false"]);
    expect(name.value).toBe("beta");
    expect(description.value).toBe("Beta role");
    expect(fake.callsFor("create_agent_matrix")).toHaveLength(0);
  });

  it("cancels Search Enter with a valid prefilled name without creating", async () => {
    const { options, search, name, create, fake, onClose, root } = await mount();
    click(options()[1]);
    expect(name.value).toBe("alpha");
    expect(create.disabled).toBe(false);
    search.focus();
    expect(document.activeElement).toBe(search);
    const { event, dispatched } = keyDown(search, "Enter");
    expect(event.defaultPrevented).toBe(true);
    expect(dispatched).toBe(false);
    await Promise.resolve();
    expect(fake.callsFor("create_agent_matrix")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
    expect(root.querySelector(".agent-modal")).not.toBeNull();
  });

  it("contains option Enter with a valid prefilled name and leaves native activation", async () => {
    const { options, name, create, fake } = await mount();
    const [, alpha, beta] = options();
    click(alpha);
    expect(name.value).toBe("alpha");
    expect(create.disabled).toBe(false);
    beta.focus();
    expect(document.activeElement).toBe(beta);
    const { event, dispatched } = keyDown(beta, "Enter");
    await Promise.resolve();
    expect(fake.callsFor("create_agent_matrix")).toHaveLength(0);
    expect(event.defaultPrevented).toBe(false);
    expect(dispatched).toBe(true);
  });

  it("closes on Escape and removes the document listener on cleanup", async () => {
    const { options, root, onClose } = await mount();
    options()[0].focus();
    keyDown(options()[0], "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
    dispose?.();
    dispose = undefined;
    expect(root.isConnected).toBe(false);
    keyDown(document, "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("exposes data-ac-testid hooks that track selection, filtering and cancel", async () => {
    const { root, onClose } = await mount();
    const byId = (id: string) => root.querySelectorAll(`[data-ac-testid="${id}"]`);
    const one = (id: string) => {
      expect(byId(id)).toHaveLength(1);
      return byId(id)[0] as HTMLElement;
    };
    for (const id of ["modal", "name", "templateSearch", "template.list", "template.none"]) {
      one(`newAgent.${id}`);
    }
    one("newAgent.cancel");
    one("newAgent.create");
    const optionRows = () =>
      Array.from(root.querySelectorAll('[data-ac-testid^="newAgent.template.option."]'));
    expect(optionRows().map((row) => row.getAttribute("data-ac-detail"))).toEqual([
      "Alpha",
      "Beta",
    ]);
    expect(optionRows().map((row) => row.getAttribute("data-ac-testid"))).toEqual([
      "newAgent.template.option.0",
      "newAgent.template.option.1",
    ]);
    expect(one("newAgent.template.none").getAttribute("data-ac-state")).toBe("selected");
    click(one("newAgent.template.option.1"));
    expect(one("newAgent.template.option.1").getAttribute("data-ac-state")).toBe("selected");
    expect(one("newAgent.template.none").getAttribute("data-ac-state")).toBe("unselected");

    const before = optionRows().length;
    input(one("newAgent.templateSearch") as HTMLInputElement, "Alpha");
    expect(optionRows().length).toBeLessThan(before);
    expect(optionRows()).toHaveLength(1);
    expect(optionRows()[0].getAttribute("data-ac-detail")).toBe("Alpha");

    click(one("newAgent.cancel"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows the install templates hook as idle when Agency templates are missing", async () => {
    const fake = new FakeTransport();
    fake.resolve("list_role_templates", templates);
    fake.resolve("get_agency_templates_status", { available: false, path: "/fixture" });
    const { cleanup, root } = renderWithFakeTransport(
      () => <NewEntityAgentModal projectPath="/fixture" onClose={() => {}} />,
      fake,
    );
    dispose = cleanup;
    await waitFor(() =>
      expect(root.querySelectorAll('[data-ac-testid="newAgent.installTemplates"]')).toHaveLength(1),
    );
    const install = root.querySelector('[data-ac-testid="newAgent.installTemplates"]')!;
    expect(install.getAttribute("data-ac-state")).toBe("idle");
  });
});
