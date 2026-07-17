import { createEffect, onCleanup } from "solid-js";
import { NonStopAPI } from "../../shared/ipc";
import type { NonStopReport } from "../../shared/types";
import { workgroupGroupsStore, nonStopDisplayName, nonStopMatchesWorkgroup } from "../stores/workgroup-groups";
import { splitWorkgroupsByWorking } from "../components/workgroup-session";
import { projectStore } from "../stores/project";


const KEEPALIVE_MS = 10_000;

export function buildSnapshot(): NonStopReport[] {
  const reports: NonStopReport[] = [];
  for (const project of projectStore.projects) {
    void workgroupGroupsStore.ensureLoaded(project.path);
    const ns = workgroupGroupsStore.config(project.path).nonStop;
    if (!ns || !ns.show) continue;
    const measuresOn = ns.telegram.enabled || ns.sound.enabled;
    if (!measuresOn) continue; // nothing to fire, do not report (show-on-zero-measures is silent by design)
    const members = project.workgroups.filter((wg) => nonStopMatchesWorkgroup(ns, wg));
    const total = members.length;
    if (total === 0) continue; // empty group, no disparity possible
    const { working: workingWgs, notWorking } = splitWorkgroupsByWorking(members);
    const working = workingWgs.length;
    reports.push({
      projectPath: project.path,
      groupName: nonStopDisplayName(ns.name),
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
    void NonStopAPI.report(snapshot).catch(() => {});
  };
  createEffect(push);
  const timer = setInterval(() => {
    lastJson = ""; // force a resend even if unchanged
    push();
  }, KEEPALIVE_MS);
  onCleanup(() => clearInterval(timer));
}
