// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import type { AgentUpdatePrompt } from "../../shared/types";
import { agentUpdateStore } from "../agent-update";
import AgentUpdateOverlay from "./AgentUpdateOverlay";

const [, setAgentUpdateStore] = agentUpdateStore;

const PROMPT: AgentUpdatePrompt = { command: "claude", label: "Claude" };

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("AgentUpdateOverlay (#1327)", () => {
  let fake: FakeTransport;
  let restore: () => void;
  let dispose: (() => void) | null = null;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    setAgentUpdateStore({ inProgress: false, prompt: null });
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(() => <AgentUpdateOverlay />, root);
  });

  afterEach(() => {
    dispose?.();
    dispose = null;
    toastStore.clear();
    setAgentUpdateStore({ inProgress: false, prompt: null });
    document.body.replaceChildren();
    restore();
    vi.restoreAllMocks();
  });

  it("shows the splash while in progress and the prompt modal when a prompt is pending", () => {
    setAgentUpdateStore({ inProgress: true, prompt: null });
    const overlay = byTestId("agent-update.overlay");
    expect(overlay.textContent).toContain("Actualizando coding agents...");

    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });
    expect(overlay.textContent).toContain("coding agent Claude");
    expect(byTestId("agent-update.prompt.yes").textContent).toBe("Sí");
    expect(byTestId("agent-update.prompt.no").textContent).toBe("No");
  });

  it("Sí answer applied this boot closes the modal and runs nothing else", async () => {
    fake.resolve("agent_update_answer", true);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    byTestId("agent-update.prompt.yes").dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true })
    );
    await Promise.resolve();
    await Promise.resolve();

    expect(agentUpdateStore[0].prompt).toBeNull();
    expect(fake.lastCall("agent_update_answer")?.args).toEqual({
      command: "claude",
      enabled: true,
    });
    expect(toastStore.items).toHaveLength(0);
  });

  it("late Sí answer closes the modal AND toasts the pinned conditional info text", async () => {
    fake.resolve("agent_update_answer", false);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    byTestId("agent-update.prompt.yes").dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true })
    );
    await Promise.resolve();
    await Promise.resolve();

    expect(agentUpdateStore[0].prompt).toBeNull();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[0].message).toBe("Se actualizará en el próximo arranque.");
  });

  it("late No answer closes the modal AND toasts the conditional No text", async () => {
    fake.resolve("agent_update_answer", false);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    byTestId("agent-update.prompt.no").dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true })
    );
    await Promise.resolve();
    await Promise.resolve();

    expect(agentUpdateStore[0].prompt).toBeNull();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("No se volverá a preguntar.");
  });

  it("IPC failure keeps the modal open and toasts the error (retry)", async () => {
    fake.reject("agent_update_answer", "settings lock contention");
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    byTestId("agent-update.prompt.no").dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true })
    );
    await Promise.resolve();
    await Promise.resolve();

    expect(agentUpdateStore[0].prompt).toEqual(PROMPT); // still open
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
  });

  it("R2: while the answer IPC is in flight, Enter/Esc and a second click are ignored", async () => {
    const pending = deferred<boolean>();
    fake.onInvoke("agent_update_answer", () => pending.promise);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    byTestId("agent-update.prompt.yes").dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true })
    );
    // In flight: a second click, Enter, and Esc must NOT fire another answer.
    byTestId("agent-update.prompt.yes").dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true })
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await Promise.resolve();

    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
    expect(agentUpdateStore[0].prompt).toEqual(PROMPT); // still open while in flight

    pending.resolve(true);
    await Promise.resolve();
    await Promise.resolve();
    expect(agentUpdateStore[0].prompt).toBeNull();
    expect(fake.callsFor("agent_update_answer")).toHaveLength(1);
  });

  it("Enter and Esc answer No by default", async () => {
    fake.resolve("agent_update_answer", true);
    setAgentUpdateStore({ inProgress: true, prompt: PROMPT });

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();

    expect(fake.lastCall("agent_update_answer")?.args).toEqual({
      command: "claude",
      enabled: false,
    });
  });
});
