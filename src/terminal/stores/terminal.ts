import { createSignal } from "solid-js";

const [activeSessionId, setActiveSessionId] = createSignal<string | null>(null);
const [activeSessionName, setActiveSessionName] = createSignal<string>("");
const [activeShell, setActiveShell] = createSignal<string>("");
const [activeShellArgs, setActiveShellArgs] = createSignal<string[]>([]);
const [activeWorkingDirectory, setActiveWorkingDirectory] = createSignal<string>('');
const [termSize, setTermSize] = createSignal<{ cols: number; rows: number }>({
  cols: 0,
  rows: 0,
});

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
  get termSize() {
    return termSize();
  },

  setActiveSession(
    id: string | null,
    name?: string,
    shell?: string,
    shellArgs?: string[],
    workingDirectory?: string
  ) {
    setActiveSessionId(id);
    if (name !== undefined) setActiveSessionName(name);
    if (shell !== undefined) setActiveShell(shell);
    if (shellArgs !== undefined) setActiveShellArgs(shellArgs);
    if (workingDirectory !== undefined) setActiveWorkingDirectory(workingDirectory);
  },

  setTermSize(cols: number, rows: number) {
    setTermSize({ cols, rows });
  },
};
