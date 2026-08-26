// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import ProfileOutdatedBadge from "./ProfileOutdatedBadge";

const TEST_ID = "session.s1.profileOutdated";

function renderBadge(testIdPrivate?: boolean, onParentClick?: () => void) {
  const root = document.createElement("div");
  document.body.append(root);
  const onReload = vi.fn();
  const dispose = render(
    () => (
      <div onClick={onParentClick}>
        <ProfileOutdatedBadge
          onReload={onReload}
          testId={TEST_ID}
          testIdPrivate={testIdPrivate}
        />
      </div>
    ),
    root,
  );
  const button = root.querySelector<HTMLButtonElement>(`[data-ac-testid="${TEST_ID}"]`);
  if (!button) throw new Error("ProfileOutdatedBadge did not render its automation target");
  return { button, dispose, onReload };
}

describe("ProfileOutdatedBadge automation privacy", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it.each([undefined, false, true])(
    "emits the private marker only for the true per-instance opt-in (%s)",
    (testIdPrivate) => {
      const { button, dispose } = renderBadge(testIdPrivate);
      expect(button.getAttribute("data-ac-testid-private")).toBe(
        testIdPrivate ? "true" : null,
      );
      dispose();
    },
  );

  it("keeps the existing click behavior and stops row propagation", () => {
    const parentClick = vi.fn();
    const { button, dispose, onReload } = renderBadge(true, parentClick);
    button.click();
    expect(onReload).toHaveBeenCalledTimes(1);
    expect(parentClick).not.toHaveBeenCalled();
    dispose();
  });
});
