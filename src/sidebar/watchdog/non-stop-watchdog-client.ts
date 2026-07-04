import { createEffect, onCleanup } from "solid-js";
import { NonStopAPI } from "../../shared/ipc";
import type { NonStopReport } from "../../shared/types";
import { workgroupGroupsStore, nonStopMatchesWorkgroup } from "../stores/workgroup-groups";
import { workgroupIsWorking } from "../components/workgroup-session";
import { projectStore } from "../stores/project";

// #777 Non-stop watchdog client (hybrid design, D1). The frontend owns ONLY the
// disparity detection, computed with the exact same code the rail counter uses
// (workgroupIsWorking + projectStore.projects), so parity is by construction and
// cannot drift. The backend owns the tolerance timer and actuation. Detection is
// reactive (a Solid effect that re-runs on any session/config/project change),
// which is immune to Chromium's hidden-window timer throttling; the keepalive
// setInterval is only a non-critical backstop.

const KEEPALIVE_MS = 10_000;

/** Compute the current watchdog snapshot from the live stores. Exported for unit
 *  testing the disparity/parity detection directly (the createEffect + keepalive
 *  wiring in startNonStopWatchdogClient is audited by review, mirroring the
 *  team-idle-watcher convention). */
export function buildSnapshot(): NonStopReport[] {
  const reports: NonStopReport[] = [];
  for (const project of projectStore.projects) {
    // (dev-webpage-ui #3) Load the groups config ourselves; do not depend on the
    // rail's mount order. Idempotent + dedupes in-flight (workgroup-groups.ts).
    void workgroupGroupsStore.ensureLoaded(project.path);
    const ns = workgroupGroupsStore.config(project.path).nonStop;
    if (!ns || !ns.show) continue;
    const measuresOn = ns.telegram.enabled || ns.sound.enabled;
    if (!measuresOn) continue; // nothing to fire, do not report (show-on-zero-measures is silent by design)
    const members = project.workgroups.filter((wg) => nonStopMatchesWorkgroup(ns, wg));
    const total = members.length;
    if (total === 0) continue; // empty group, no disparity possible
    const notWorking = members.filter((wg) => !workgroupIsWorking(wg));
    const working = total - notWorking.length;
    reports.push({
      projectPath: project.path,
      groupName: ns.name || "Non-stop",
      disparity: working < total,
      working,
      total,
      notWorkingWorkgroups: notWorking.map((wg) => wg.name),
      toleranceSeconds: ns.toleranceSeconds,
      telegramEnabled: ns.telegram.enabled,
      telegramBotId: ns.telegram.botId ?? null,
      soundEnabled: ns.sound.enabled,
      soundSeconds: ns.sound.seconds,
    });
  }
  return reports;
}

export function startNonStopWatchdogClient(): void {
  let lastJson = "";
  const push = () => {
    const snapshot = buildSnapshot();
    const json = JSON.stringify(snapshot);
    if (json === lastJson) return; // dedupe unchanged snapshots
    lastJson = json;
    // Fire-and-forget: a failed report must never throw (the keepalive retries,
    // and the backend times from its own clock once armed).
    void NonStopAPI.report(snapshot).catch(() => {});
  };
  // Reactive: recomputes whenever session state, groups config, or the project
  // list changes. This is the throttle-immune onset/clear detector (event-driven,
  // not a polled timer) and fires the first report on mount.
  createEffect(push);
  // Light keepalive backstop: refreshes config + confirms liveness. Non-critical
  // for timing (the backend times from its own clock once armed).
  const timer = setInterval(() => {
    lastJson = ""; // force a resend even if unchanged
    push();
  }, KEEPALIVE_MS);
  onCleanup(() => clearInterval(timer));
}
