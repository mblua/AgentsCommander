// @vitest-environment jsdom
// #4825 - the TASK title editor input, Save and Cancel carry data-ac-testid
// under the menu prefix, and each id drives its own handler.
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionRowMenu from "./SessionRowMenu";
import type { EditTaskTitleSpec } from "./session-row-menu-types";
import { click, input, installBrowserDomStubs } from "../../../shared/testing/ui-harness";

const PREFIX = "tempRoom.r1";

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
});
