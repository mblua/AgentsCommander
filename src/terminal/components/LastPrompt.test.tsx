// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import LastPrompt from "./LastPrompt";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { renderWithFakeTransport, session } from "../../shared/testing/ui-harness";
import type { Session } from "../../shared/types";
import type { UnlistenFn } from "../../shared/transport";
import { terminalStore } from "../stores/terminal";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
// Drain the finite transport/listener promise chain without timers or polling.
async function flush() { for (let i = 0; i < 20; i++) await Promise.resolve(); }
const row = (id: string, lastPrompt: string | null | undefined) => session({ id, lastPrompt });
const views: Array<ReturnType<typeof renderWithFakeTransport>> = [];
function mount(fake: FakeTransport, id: () => string | undefined = () => "x") {
  const view = renderWithFakeTransport(() => <LastPrompt sessionId={id()} />, fake);
  views.push(view);
  return { ...view, text: () => view.root.querySelector(".last-prompt-text")?.textContent };
}
const event = (fake: FakeTransport, text: string, sessionId = "x") => fake.emitFromBackend("last_prompt", { sessionId, text });
const reload = (fake: FakeTransport) => fake.emitFromBackend("session_created", session());
afterEach(() => {
  views.splice(0).reverse().forEach(view => view.cleanup());
  terminalStore.resetForTests();
  vi.restoreAllMocks();
});

describe("issue_3145 LastPrompt", () => {
  it("AC-3145-004 preserves N when pending list A resolves", async () => {
    const fake = new FakeTransport(); const a = deferred<Session[]>();
    fake.onInvoke("list_sessions", () => a.promise);
    const view = mount(fake); await flush();
    expect(fake.callsFor("list_sessions")).toHaveLength(1);
    event(fake, "N"); expect(view.text()).toBe("N");
    a.resolve([row("x", "A")]); await flush();
    expect(view.text()).toBe("N");
  });

  it("ignores older list A after newer list B including removed rows", async () => {
    const fake = new FakeTransport(); const a = deferred<Session[]>(); const b = deferred<Session[]>();
    fake.onInvoke("list_sessions", () => fake.callsFor("list_sessions").length === 1 ? a.promise : b.promise);
    const [id, setId] = createSignal("x"); const view = mount(fake, id); await flush();
    reload(fake); b.resolve([row("x", "B")]); await flush(); expect(view.text()).toBe("B");
    a.resolve([row("x", "A"), row("y", "resurrected")]); await flush(); expect(view.text()).toBe("B");
    setId("y"); expect(view.text()).toBe("...");
  });

  it("protects X per id while Y accepts the snapshot and selection stays isolated", async () => {
    const fake = new FakeTransport(); const a = deferred<Session[]>();
    fake.onInvoke("list_sessions", () => a.promise);
    const [id, setId] = createSignal<string | undefined>("x"); const view = mount(fake, id); await flush();
    event(fake, "N"); a.resolve([row("x", "A"), row("y", "Y")]); await flush();
    expect(view.text()).toBe("N"); setId("y"); expect(view.text()).toBe("Y");
    setId(undefined); terminalStore.setActiveSessionForTests("x"); expect(view.text()).toBe("N");
    terminalStore.setActiveSessionForTests("y"); expect(view.text()).toBe("Y");
  });

  it.each(["", null, undefined])("clears old text for snapshot value %s", async value => {
    const fake = new FakeTransport(); fake.resolve("list_sessions", [row("x", "old")]);
    const view = mount(fake); await flush(); expect(view.text()).toBe("old");
    fake.resolve("list_sessions", [row("x", value)]); reload(fake); await flush(); expect(view.text()).toBe("...");
  });

  it("stores an empty event and allows a later request to refresh that id", async () => {
    const fake = new FakeTransport(); const a = deferred<Session[]>(); fake.onInvoke("list_sessions", () => a.promise);
    const view = mount(fake); await flush(); event(fake, "old"); event(fake, "");
    a.resolve([row("x", "A")]); await flush(); expect(view.text()).toBe("...");
    fake.resolve("list_sessions", [row("x", "fresh")]); reload(fake); await flush(); expect(view.text()).toBe("fresh");
  });

  it("removes omitted cached ids unless an event arrived during the request", async () => {
    const fake = new FakeTransport(); fake.resolve("list_sessions", [row("x", "old"), row("y", "old Y")]);
    const [id, setId] = createSignal("x"); const view = mount(fake, id); await flush();
    const a = deferred<Session[]>(); fake.onInvoke("list_sessions", () => a.promise); reload(fake);
    event(fake, "N"); a.resolve([]); await flush(); expect(view.text()).toBe("N");
    setId("y"); expect(view.text()).toBe("...");
    fake.resolve("list_sessions", []); reload(fake); await flush(); setId("x"); expect(view.text()).toBe("...");
  });

  it("preserves cache on list rejection and can reload afterwards", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fake = new FakeTransport(); fake.resolve("list_sessions", [row("x", "kept")]);
    const view = mount(fake); await flush();
    fake.reject("list_sessions", "secret prompt"); reload(fake); await flush();
    expect(view.text()).toBe("kept"); expect(warn).toHaveBeenCalledWith("LastPrompt: could not load prompts");
    fake.resolve("list_sessions", [row("x", "next")]); reload(fake); await flush(); expect(view.text()).toBe("next");
  });

  it("releases both listeners once and ignores late list and callbacks after cleanup", async () => {
    const fake = new FakeTransport(); const a = deferred<Session[]>(); fake.onInvoke("list_sessions", () => a.promise);
    const releases: Array<ReturnType<typeof vi.fn>> = []; const callbacks = new Map<string, (value: unknown) => void>();
    vi.spyOn(fake, "listen").mockImplementation(async (name, cb) => {
      callbacks.set(name, cb); const release = vi.fn(); releases.push(release); return release;
    });
    const view = mount(fake); await flush(); const text = view.root.querySelector(".last-prompt-text")!;
    event(fake, "unused"); view.cleanup();
    callbacks.get("last_prompt")!({ sessionId: "x", text: "late" }); callbacks.get("session_created")!(session());
    a.resolve([row("x", "late list")]); await flush();
    expect(text.textContent).toBe("..."); expect(fake.callsFor("list_sessions")).toHaveLength(1);
    releases.forEach(release => expect(release).toHaveBeenCalledOnce());
  });

  it.each([1, 2])("cleans up while listener registration %s is pending", async pending => {
    const fake = new FakeTransport(); const registration = deferred<UnlistenFn>(); const release = vi.fn(); const first = vi.fn();
    const listen = vi.spyOn(fake, "listen").mockImplementation(() => listen.mock.calls.length === pending ? registration.promise : Promise.resolve(first));
    const view = mount(fake); await flush(); expect(listen).toHaveBeenCalledTimes(pending);
    expect(fake.callsFor("list_sessions")).toHaveLength(0); view.cleanup(); registration.resolve(release); await flush();
    expect(release).toHaveBeenCalledOnce(); expect(first).toHaveBeenCalledTimes(pending - 1);
    expect(listen).toHaveBeenCalledTimes(pending); expect(fake.callsFor("list_sessions")).toHaveLength(0);
  });

  it.each([1, 2])("handles listener registration %s rejection without leaked resources", async rejected => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fake = new FakeTransport(); const release = vi.fn();
    const listen = vi.spyOn(fake, "listen").mockImplementation(() => listen.mock.calls.length === rejected ? Promise.reject(new Error("private")) : Promise.resolve(release));
    mount(fake); await flush();
    expect(release).toHaveBeenCalledTimes(rejected - 1); expect(fake.callsFor("list_sessions")).toHaveLength(0);
    expect(warn).toHaveBeenCalledWith("LastPrompt: could not register listeners");
  });
});
