import { createEffect, createRoot, createSignal, untrack } from "solid-js";
import { settingsStore } from "../../shared/stores/settings";
import type { AcWorkgroup } from "../../shared/types";
import { projectStore } from "./project";
import { normalizeProjectPathForCompare } from "./project-refresh";
import { roomIsRawActive } from "../components/room-session-source";

const ROOM_IDLE_CONFIRMATION_SECONDS_DEFAULT = 15;

export function roomActivityKey(projectPath: string, roomPath: string): string | null {
  if (!projectPath.trim() || !roomPath.trim()) return null;
  return JSON.stringify([
    normalizeProjectPathForCompare(projectPath),
    normalizeProjectPathForCompare(roomPath),
  ]);
}

type ActivityEntry = {
  lastRaw: boolean;
  deadline: number | null;
  timer: ReturnType<typeof setTimeout> | null;
  generation: number;
  confirmed: boolean;
};
const entries = new Map<string, ActivityEntry>();
const [revision, setRevision] = createSignal(0);
let lifecycleGeneration = 0;
let owners = 0;
let disposeObserver: (() => void) | undefined;

function publish(): void {
  untrack(() => setRevision((value) => value + 1));
}

function cancel(entry: ActivityEntry): void {
  if (entry.timer !== null) clearTimeout(entry.timer);
  entry.timer = null;
  entry.deadline = null;
  entry.generation++;
}

function currentRoom(key: string): AcWorkgroup | undefined {
  for (const project of projectStore.projects) {
    for (const room of project.workgroups) {
      if (roomActivityKey(project.path, room.path) === key) return room;
    }
  }
}

function schedule(key: string, entry: ActivityEntry): void {
  const realm = lifecycleGeneration;
  const generation = entry.generation;
  entry.timer = setTimeout(() => {
    if (realm !== lifecycleGeneration || entries.get(key) !== entry || generation !== entry.generation) return;
    const room = currentRoom(key);
    if (!room) {
      cancel(entry);
      entries.delete(key);
      publish();
      return;
    }
    entry.timer = null;
    if (roomIsRawActive(room)) {
      cancel(entry);
      entry.lastRaw = true;
      entry.confirmed = true;
      publish();
    } else if (entry.deadline !== null && performance.now() < entry.deadline) {
      schedule(key, entry);
    } else {
      entry.deadline = null;
      entry.confirmed = false;
      publish();
    }
  }, Math.max(0, (entry.deadline ?? performance.now()) - performance.now()));
}

function sample(): void {
  // Read every raw dependency under the effect, but never subscribe to our own
  // publication or mutable entries. Repeated idle facts cannot reset a deadline.
  const catalogue = new Map<string, boolean>();
  for (const project of projectStore.projects) {
    for (const room of project.workgroups) {
      const key = roomActivityKey(project.path, room.path);
      if (key !== null) catalogue.set(key, roomIsRawActive(room));
    }
  }
  untrack(() => {
    let changed = false;
    for (const [key, entry] of entries) {
      if (!catalogue.has(key)) {
        cancel(entry);
        entries.delete(key);
        changed = true;
      }
    }
    for (const [key, raw] of catalogue) {
      const entry = entries.get(key);
      if (!entry) {
        entries.set(key, { lastRaw: raw, deadline: null, timer: null, generation: 0, confirmed: raw });
        changed = true;
      } else if (raw) {
        cancel(entry);
        changed ||= !entry.confirmed;
        entry.confirmed = true;
        entry.lastRaw = true;
      } else if (entry.lastRaw) {
        entry.lastRaw = false;
        const seconds = settingsStore.current?.roomIdleConfirmationSeconds ?? ROOM_IDLE_CONFIRMATION_SECONDS_DEFAULT;
        entry.deadline = performance.now() + seconds * 1000;
        entry.generation++;
        schedule(key, entry);
      }
    }
    if (changed) publish();
  });
}

export function confirmedRoomIsActive(projectPath: string, room: AcWorkgroup): boolean {
  revision();
  if (roomIsRawActive(room)) return true;
  const key = roomActivityKey(projectPath, room.path);
  return key === null ? false : untrack(() => entries.get(key)?.confirmed ?? false);
}

export function acquireRoomActivityObserver(): () => void {
  if (owners++ === 0) {
    createRoot((dispose) => {
      disposeObserver = dispose;
      createEffect(sample);
    }, null);
  }
  let released = false;
  return () => {
    if (released) return;
    released = true;
    if (--owners !== 0) return;
    disposeObserver?.();
    disposeObserver = undefined;
    lifecycleGeneration++;
    for (const entry of entries.values()) cancel(entry);
    entries.clear();
    publish();
  };
}
