// @vitest-environment jsdom
// The TASK title editor input, Save and Cancel carry data-ac-testid
// under the menu prefix, and each id drives its own handler.
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionRowMenu from "./SessionRowMenu";
import type { EditTaskTitleSpec } from "./session-row-menu-types";
import { click, input, installBrowserDomStubs } from "../../../shared/testing/ui-harness";

const PREFIX = "tempRoom.r1";

// Counts entries into the outer ContextMenuSurface click shield: Solid
// delegates onClick through the element's $$click slot, so wrapping it
// observes exactly the clicks whose bubbling reached the outer handler.
function armOuterShield(): { entries: () => number; stop: () => void } {
  type Delegated = HTMLElement & { $$click?: (this: HTMLElement, e: Event) => unknown };
  const menu = document.querySelector<Delegated>(".session-context-menu");
  const original = menu?.$$click;
  if (!menu || !original) throw new Error("outer shield not registered");
  let entries = 0;
  menu.$$click = function (this: HTMLElement, e: Event) {
    entries += 1;
    return original.call(this, e);
  };
  return { entries: () => entries, stop: () => (menu.$$click = original) };
}

function byId<T extends HTMLElement = HTMLElement>(id: string): T {
  const all = document.querySelectorAll<T>(`[data-ac-testid="${id}"]`);
  expect(all).toHaveLength(1);
  return all[0];
}

describe("#4825 SessionRowMenu TASK title editor test ids", () => {
  let cleanupDom: (() => void) | null = null;
  let dispose: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
  });
  afterEach(() => {
    dispose?.();
    dispose = null;
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
  });

  it("opens the editor; input, Save and Cancel act through their ids", () => {
    const [editing, setEditing] = createSignal(false);
    const [draft, setDraft] = createSignal("Old");
    const onSave = vi.fn();
    const onCancel = vi.fn();
    const spec = (): EditTaskTitleSpec => ({
      editing: editing(),
      draft: draft(),
      busy: false,
      error: null,
      onDraft: setDraft,
      onStart: () => setEditing(true),
      onSave,
      onCancel,
    });
    const root = document.createElement("div");
    document.body.appendChild(root);
    const unmount = render(
      () => (
        <SessionRowMenu
          open={true}
          x={10}
          y={10}
          testIdPrefix={PREFIX}
          caps={{ editTaskTitle: spec() }}
          onDismiss={() => {}}
        />
      ),
      root,
    );
    dispose = () => {
      unmount();
      root.remove();
    };

    expect(document.querySelector(`[data-ac-testid="${PREFIX}.menu.editTaskTitle.input"]`))
      .toBeNull();
    click(byId(`${PREFIX}.menu.editTaskTitle`));

    const field = byId<HTMLInputElement>(`${PREFIX}.menu.editTaskTitle.input`);
    input(field, "I4825 title");
    expect(draft()).toBe("I4825 title");

    click(byId(`${PREFIX}.menu.editTaskTitle.save`));
    expect(onSave).toHaveBeenCalledTimes(1);
    click(byId(`${PREFIX}.menu.editTaskTitle.cancel`));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  const mountEditor = (opts: { draft?: string; busy?: boolean } = {}) => {
    const [editing, setEditing] = createSignal(false);
    const [draft, setDraft] = createSignal(opts.draft ?? "Old title");
    const onSave = vi.fn();
    const onCancel = vi.fn(() => setEditing(false));
    const onDismiss = vi.fn();
    const spec = (): EditTaskTitleSpec => ({
      editing: editing(),
      draft: draft(),
      busy: opts.busy ?? false,
      error: null,
      onDraft: setDraft,
      onStart: () => setEditing(true),
      onSave,
      onCancel,
    });
    const root = document.createElement("div");
    document.body.appendChild(root);
    const unmount = render(
      () => (
        <SessionRowMenu
          open={true}
          x={10}
          y={10}
          testIdPrefix={PREFIX}
          caps={{ editTaskTitle: spec() }}
          onDismiss={onDismiss}
        />
      ),
      root,
    );
    dispose = () => {
      unmount();
      root.remove();
    };
    return { onSave, onCancel, onDismiss, editing };
  };
  const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 20));
  const field = () => byId<HTMLInputElement>(`${PREFIX}.menu.editTaskTitle.input`);
  const openEditor = async () => {
    await tick();
    click(byId(`${PREFIX}.menu.editTaskTitle`));
    await tick();
  };
  const key = (el: Element, k: string) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  const windowCount = (type: string) => {
    const seen: Event[] = [];
    const fn = (e: Event) => seen.push(e);
    window.addEventListener(type, fn);
    return { seen, stop: () => window.removeEventListener(type, fn) };
  };

  it("#4945 focuses and selects the input on open", async () => {
    mountEditor();
    await openEditor();
    expect(document.activeElement).toBe(field());
    expect(field().selectionStart).toBe(0);
    expect(field().selectionEnd).toBe("Old title".length);
  });

  it("#4945 editor clicks reach the window zero times and keep the menu open", async () => {
    const { onDismiss } = mountEditor();
    await openEditor();
    const win = windowCount("click");
    click(field());
    click(field().parentElement!);
    win.stop();
    expect(win.seen).toHaveLength(0);
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("#4945 the local editor shield stops clicks before the outer menu shield", async () => {
    const { onDismiss } = mountEditor();
    await openEditor();
    const outer = armOuterShield();
    click(field());
    click(field().parentElement!);
    expect(outer.entries()).toBe(0);
    click(document.querySelector(".session-context-menu")!);
    expect(outer.entries()).toBe(1);
    outer.stop();
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("#4945 Save button saves", async () => {
    const { onSave } = mountEditor();
    await openEditor();
    click(byId(`${PREFIX}.menu.editTaskTitle.save`));
    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it("#4945 Cancel button cancels", async () => {
    const { onCancel } = mountEditor();
    await openEditor();
    click(byId(`${PREFIX}.menu.editTaskTitle.cancel`));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("#4945 Enter saves when enabled", async () => {
    const { onSave } = mountEditor();
    await openEditor();
    expect(key(field(), "Enter")).toBe(false);
    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it("#4945 Enter does not save while busy", async () => {
    const { onSave } = mountEditor({ busy: true });
    await openEditor();
    key(field(), "Enter");
    expect(onSave).not.toHaveBeenCalled();
  });

  it("#4945 Escape cancels the edit and the outer menu stays open", async () => {
    const { onCancel, onDismiss, editing } = mountEditor();
    await openEditor();
    const win = windowCount("keydown");
    key(field(), "Escape");
    win.stop();
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(editing()).toBe(false);
    expect(win.seen).toHaveLength(0);
    expect(onDismiss).not.toHaveBeenCalled();
    expect(document.querySelector(".session-context-menu")).not.toBeNull();
  });
});
