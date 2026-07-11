import { createSignal } from "solid-js";
import type { ProjectArchiveChanged } from "../../shared/types";
import { normalizeProjectPathForCompare } from "./project-refresh";

const [entries, setEntries] = createSignal<ProjectArchiveChanged[]>([]);

function samePath(left: string, right: string): boolean {
  return normalizeProjectPathForCompare(left) === normalizeProjectPathForCompare(right);
}

export const autoUnarchiveStore = {
  get entries() {
    return entries();
  },

  get open() {
    return entries().length > 0;
  },

  push(event: ProjectArchiveChanged) {
    if (event.reason !== "autoUnarchive") return;
    setEntries((prev) =>
      prev.some((entry) => samePath(entry.path, event.path)) ? prev : [...prev, event]
    );
  },

  acknowledge() {
    setEntries([]);
  },
};
