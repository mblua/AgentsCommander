import { createSignal } from "solid-js";
import { projectStore } from "../../sidebar/stores/project";
import { advance, startCtx, type Probe, type RunCtx } from "../tutorials/create-first-team";
import { tutorialProgressStore } from "./tutorial-progress";

// #5228 - the running tutorial. Not saved: closing the app ends the run.
const [run, setRun] = createSignal<RunCtx | null>(null);

export const tutorialStore = {
  get active(): boolean { return run() !== null; },
  get ctx(): RunCtx | null { return run(); },
  /** Only the Start button calls this. A pick made before Start never counts. */
  start(): void {
    if (run() !== null) return;
    setRun(startCtx(projectStore.newProjectOutcome.seq));
  },
  exit(): void { setRun(null); },
  /** Bubble button "Skip download": templates are not needed for the rest of this run. */
  skipTemplates(): void {
    const ctx = run();
    if (ctx !== null) setRun({ ...ctx, skip: true });
  },
  /** Apply the rules until nothing changes, at most 5 times. */
  sync(probe: Probe): void {
    let ctx = run();
    if (ctx === null) return;
    for (let i = 0; i < 5; i++) {
      const n = advance(ctx, probe);
      if (n === ctx) break;
      ctx = n;
    }
    if (ctx !== run()) setRun(ctx);
  },
  /** Marks completed before clearing the run, so the hint never flashes. */
  async finish(): Promise<void> {
    const saved = tutorialProgressStore.markCompleted();
    setRun(null);
    await saved;
  },
};

// Test-only reset (gated to Vite MODE === "test", which vitest sets).
export function __resetTutorialStoreForTests() {
  if (import.meta.env.MODE !== "test") return;
  setRun(null);
}
