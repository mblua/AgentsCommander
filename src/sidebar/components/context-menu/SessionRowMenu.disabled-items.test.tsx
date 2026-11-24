// @vitest-environment jsdom
// The catalogue's Unleash item and the dimmed DisabledItemSpec forms of
// Add to Group and Edit TASK title. Existing hosts that pass none of them must
// render exactly as before.
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionRowMenu from "./SessionRowMenu";
import type { SessionRowMenuCaps } from "./session-row-menu-types";
import { installBrowserDomStubs } from "../../../shared/testing/ui-harness";

const PREFIX = "tempRoom.temp-room-tr01";
let disposers: (() => void)[] = [];
let cleanupDom: (() => void) | null = null;

function mount(caps: SessionRowMenuCaps, onDismiss = () => {}): HTMLElement {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(
    () => (
      <SessionRowMenu
        open={true}
        x={10}
        y={10}
        testIdPrefix={PREFIX}
        caps={caps}
        onDismiss={onDismiss}
      />
    ),
    root,
  );
  disposers.push(() => {
    dispose();
    root.remove();
  });
  const menu = document.querySelector<HTMLElement>(`[data-ac-testid="${PREFIX}.menu"]`);
  if (!menu) throw new Error("menu did not render");
  return menu;
}

const q = (id: string) =>
  document.querySelector<HTMLButtonElement>(`[data-ac-testid="${PREFIX}${id}"]`);
const rowIds = (menu: HTMLElement) =>
  Array.from(menu.querySelectorAll("[data-ac-testid]")).map((el) =>
    el.getAttribute("data-ac-testid"),
  );

describe("SessionRowMenu disabled items", () => {
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
  });
  afterEach(() => {
    for (const dispose of disposers) dispose();
    disposers = [];
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("Unleash sits right before Close Session, labelled, danger, dimmed when disabled", () => {
    const onSelect = vi.fn();
    const menu = mount({
      restart: { onSelect: vi.fn() },
      unleash: { disabled: true, title: "why not", onSelect },
      close: { onSelect: vi.fn() },
    });
    expect(rowIds(menu)).toEqual([
      `${PREFIX}.restart`,
      `${PREFIX}.menu.unleash`,
      `${PREFIX}.close`,
    ]);
    const unleash = q(".menu.unleash")!;
    expect(unleash.textContent).toContain("Unleash");
    expect(unleash.textContent).toContain("⛓");
    expect(unleash.className).toBe(
      "session-context-option context-option-danger context-option-disabled",
    );
    expect(unleash.disabled).toBe(true);
    expect(unleash.title).toBe("why not");
    unleash.click();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("Unleash alone keeps group 1, so a separator precedes the next group", () => {
    const menu = mount({
      unleash: { disabled: true, title: "t", onSelect: () => {} },
      detach: { on: false, onSelect: () => {} },
    });
    expect(menu.querySelectorAll(".context-separator")).toHaveLength(1);
  });

  it("disabled Add to Group and Edit TASK title: dimmed, titled, inert, no flyout/editor", () => {
    const onDismiss = vi.fn();
    const menu = mount(
      {
        restart: { onSelect: () => {} },
        addToGroup: { disabled: true, title: "Groups need a project room" },
        editTaskTitle: { disabled: true, title: "Temp rooms have no TASK.md" },
      },
      onDismiss,
    );
    for (const [id, label, title] of [
      [".menu.addToGroup", "Add to Group", "Groups need a project room"],
      [".menu.editTaskTitle", "Edit TASK title", "Temp rooms have no TASK.md"],
    ] as const) {
      const row = q(id)!;
      expect(row.className).toBe("session-context-option context-option-disabled");
      expect(row.disabled).toBe(true);
      expect(row.title).toBe(title);
      expect(row.textContent).toContain(label);
      row.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
      row.click();
    }
    expect(document.querySelector(".session-context-flyout")).toBeNull();
    expect(document.querySelector(".session-context-title-edit")).toBeNull();
    expect(onDismiss).not.toHaveBeenCalled();
    // Group 4 and group 5 still get their separators.
    expect(menu.querySelectorAll(".context-separator")).toHaveLength(2);
  });

  it("a host that omits unleash renders exactly as before", () => {
    const caps: SessionRowMenuCaps = {
      restart: { onSelect: () => {} },
      close: { onSelect: () => {} },
    };
    const menu = mount(caps);
    expect(rowIds(menu)).toEqual([`${PREFIX}.restart`, `${PREFIX}.close`]);
    expect(q(".menu.unleash")).toBeNull();
  });
});
