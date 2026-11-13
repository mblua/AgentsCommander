// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  isTauri: false,
  handler: null as null | ((event: { preventDefault: () => void }) => void),
  register: vi.fn(),
  destroy: vi.fn(),
  off: vi.fn(),
}));
vi.mock("./platform", () => ({ get isTauri() { return state.isTauri; } }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: state.register, destroy: state.destroy }),
}));
import { WindowErrorBoundary } from "./window-error-boundary";

let cleanup: () => void;
let root: HTMLDivElement;
let broken: boolean;
async function flush() {
  for (let i = 0; i < 24; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}
function Child() {
  if (broken) throw new Error("boom");
  return <span>working</span>;
}
function mount(label = "watchers") {
  cleanup = render(() => <WindowErrorBoundary label={label}><Child /></WindowErrorBoundary>, root);
}
function click(text: string) {
  Array.from(root.querySelectorAll("button")).find((b) => b.textContent === text)!.click();
}
beforeEach(() => {
  root = document.createElement("div");
  document.body.append(root);
  broken = true;
  state.isTauri = false;
  state.handler = null;
  vi.clearAllMocks();
  vi.spyOn(console, "error").mockImplementation(() => {});
  state.register.mockImplementation(async (handler) => {
    state.handler = handler;
    return state.off;
  });
});
afterEach(() => {
  cleanup?.();
  root.remove();
  vi.restoreAllMocks();
});
it("renders normally without logging", () => {
  broken = false;
  mount();
  expect(root.textContent).toContain("working");
  expect(console.error).not.toHaveBeenCalled();
});
it("shows the error and logs strings exactly once", () => {
  mount();
  expect(root.textContent).toContain("boom");
  expect(console.error).toHaveBeenCalledExactlyOnceWith(
    "[window-error-boundary]", "watchers", expect.stringContaining("boom"),
  );
  expect(vi.mocked(console.error).mock.calls[0].every((arg) => typeof arg === "string")).toBe(true);
});
it("remounts the child on Try again", () => {
  mount();
  broken = false;
  click("Try again");
  expect(root.textContent).toBe("working");
});
it("confirms main close, cancels, exits and cleans up on recovery", async () => {
  state.isTauri = true;
  mount("main");
  await flush();
  expect(state.register).toHaveBeenCalledTimes(1);
  const event = { preventDefault: vi.fn() };
  state.handler!(event);
  expect(event.preventDefault).toHaveBeenCalledOnce();
  expect(root.textContent).toContain("Exit AgentsCommander and stop all sessions?");
  click("Cancel");
  expect(root.textContent).not.toContain("Exit AgentsCommander and stop all sessions?");
  expect(state.destroy).not.toHaveBeenCalled();
  state.handler!(event);
  click("Exit");
  expect(state.destroy).toHaveBeenCalledOnce();
  broken = false;
  click("Try again");
  expect(state.off).toHaveBeenCalledOnce();
});
it.each([["main", false], ["watchers", true]] as const)(
  "does not register for %s with isTauri=%s", async (label, isTauri) => {
    state.isTauri = isTauri;
    mount(label);
    expect(root.textContent).toContain("boom");
    await flush();
    expect(state.register).not.toHaveBeenCalled();
    expect(root.textContent).not.toContain("Exit AgentsCommander and stop all sessions?");
  },
);
it("unlistens when registration resolves after recovery", async () => {
  state.isTauri = true;
  let resolve!: (off: () => void) => void;
  state.register.mockReturnValue(new Promise<() => void>((done) => { resolve = done; }));
  mount("main");
  await flush();
  expect(state.register).toHaveBeenCalledOnce();
  broken = false;
  click("Try again");
  resolve(state.off);
  await flush();
  expect(state.off).toHaveBeenCalledOnce();
});
