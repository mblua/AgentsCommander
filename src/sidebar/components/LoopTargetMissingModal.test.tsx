// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { UnresolvedLoopTarget } from "../../shared/types";
import { click, waitFor } from "../../shared/testing/ui-harness";
import LoopTargetMissingModal from "./LoopTargetMissingModal";

const projectPath = "C:\\Project";

function alert(overrides: Partial<UnresolvedLoopTarget> = {}): UnresolvedLoopTarget {
  return {
    projectPath,
    loopId: "daily-release",
    loopName: "Daily release",
    workgroup: "room-29-git-npm-expert-team",
    error: "Room 'room-29-git-npm-expert-team' not found in project C:\\Project",
    ...overrides,
  };
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

function mount(props: {
  alerts: UnresolvedLoopTarget[];
  openErrors?: Record<string, string>;
  busyLoopId?: string | null;
  onOpenConfig?: (a: UnresolvedLoopTarget) => void;
  onDismiss?: () => void;
}): () => void {
  const root = document.createElement("div");
  document.body.appendChild(root);
  return render(
    () => (
      <LoopTargetMissingModal
        alerts={props.alerts}
        openErrors={props.openErrors ?? {}}
        busyLoopId={props.busyLoopId ?? null}
        onOpenConfig={props.onOpenConfig ?? (() => {})}
        onDismiss={props.onDismiss ?? (() => {})}
      />
    ),
    root,
  );
}

describe("LoopTargetMissingModal (#2171)", () => {
  let dispose: (() => void) | null = null;

  afterEach(() => {
    dispose?.();
    dispose = null;
    document.body.replaceChildren();
  });

  it("renders one row per alert with the Loop name, room and the verbatim error", async () => {
    const second = alert({
      loopId: "weekly-report",
      loopName: "Weekly report",
      workgroup: "room-3-missing",
      error: "Room 'room-3-missing' has no identity-verified orchestrator",
    });
    dispose = mount({ alerts: [alert(), second] });

    await waitFor(() => expect(byTestId("loopTargetMissing.modal")).toBeTruthy());
    const text = byTestId("loopTargetMissing.modal").textContent ?? "";
    expect(text).toContain("Daily release");
    expect(text).toContain("room-29-git-npm-expert-team");
    expect(text).toContain(projectPath);
    expect(text).toContain(alert().error);
    expect(text).toContain("Weekly report");
    expect(text).toContain(second.error);
    expect(byTestId("loopTargetMissing.open.daily-release")).toBeTruthy();
    expect(byTestId("loopTargetMissing.open.weekly-report")).toBeTruthy();
  });

  it("calls onOpenConfig with that row's alert", async () => {
    const onOpenConfig = vi.fn();
    dispose = mount({ alerts: [alert()], onOpenConfig });

    await waitFor(() => expect(byTestId("loopTargetMissing.open.daily-release")).toBeTruthy());
    click(byTestId("loopTargetMissing.open.daily-release"));

    expect(onOpenConfig).toHaveBeenCalledTimes(1);
    expect(onOpenConfig.mock.calls[0][0]).toEqual(alert());
  });

  it("renders a non-empty openErrors entry as an error line in that row", async () => {
    dispose = mount({
      alerts: [alert(), alert({ loopId: "other", loopName: "Other" })],
      openErrors: { "daily-release": "Open the project, then try again." },
    });

    await waitFor(() => expect(byTestId("loopTargetMissing.modal")).toBeTruthy());
    const errors = document.querySelectorAll(".new-agent-error");
    expect(errors).toHaveLength(1);
    expect(errors[0].textContent).toContain("Open the project, then try again.");
  });

  it("disables only the busy row's button", async () => {
    dispose = mount({
      alerts: [alert(), alert({ loopId: "other", loopName: "Other" })],
      busyLoopId: "daily-release",
    });

    await waitFor(() => expect(byTestId("loopTargetMissing.modal")).toBeTruthy());
    expect(byTestId<HTMLButtonElement>("loopTargetMissing.open.daily-release").disabled).toBe(true);
    expect(byTestId<HTMLButtonElement>("loopTargetMissing.open.other").disabled).toBe(false);
  });

  it("dismisses on the dismiss button, the overlay click and Escape", async () => {
    const onDismiss = vi.fn();
    dispose = mount({ alerts: [alert()], onDismiss });

    await waitFor(() => expect(byTestId("loopTargetMissing.dismiss")).toBeTruthy());
    click(byTestId("loopTargetMissing.dismiss"));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    click(byTestId("loopTargetMissing.overlay"));
    expect(onDismiss).toHaveBeenCalledTimes(2);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(onDismiss).toHaveBeenCalledTimes(3);
  });
});
