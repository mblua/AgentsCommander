import { createSignal } from "solid-js";

const [activeSessionId, setActiveSessionId] = createSignal<string | null>(null);
const [activeSessionName, setActiveSessionName] = createSignal<string>("");
const [activeShell, setActiveShell] = createSignal<string>("");
const [activeShellArgs, setActiveShellArgs] = createSignal<string[] | null>(null);
const [activeWorkingDirectory, setActiveWorkingDirectory] = createSignal<string>('');

export const terminalStore = {
  get activeSessionId() {
    return activeSessionId();
  },
  get activeSessionName() {
    return activeSessionName();
  },
  get activeShell() {
    return activeShell();
  },
  get activeShellArgs() {
    return activeShellArgs();
  },
  get activeWorkingDirectory() {
    return activeWorkingDirectory();
  },

  /**
   * Partial-update contract: `id` always applied; any of `name` / `shell` /
   * `shellArgs` / `workingDirectory` omitted or passed as `undefined` leaves
   * the current value untouched. Rename events rely on this — they pass only
   * `(id, name)` so shell/args/cwd are preserved. Do NOT change the
   * undefined-skip semantics without auditing every caller.
   */
  setActiveSession(
    id: string | null,
    name?: string,
    shell?: string,
    shellArgs?: string[] | null,
    workingDirectory?: string
  ) {
    setActiveSessionId(id);
    if (name !== undefined) setActiveSessionName(name);
    if (shell !== undefined) setActiveShell(shell);
    if (shellArgs !== undefined) setActiveShellArgs(shellArgs);
    if (workingDirectory !== undefined) setActiveWorkingDirectory(workingDirectory);
  },
};
