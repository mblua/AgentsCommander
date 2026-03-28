import { SessionAPI } from "./ipc";
import { voiceRecorder } from "./voice-recorder";

type ShortcutHandler = (e: KeyboardEvent) => void;

const shortcuts: Array<{
  ctrl: boolean;
  shift: boolean;
  key: string;
  handler: () => void;
}> = [
  {
    ctrl: true,
    shift: true,
    key: "n",
    handler: () => SessionAPI.create(),
  },
  {
    ctrl: true,
    shift: true,
    key: "w",
    handler: async () => {
      const activeId = await SessionAPI.getActive();
      if (activeId) SessionAPI.destroy(activeId);
    },
  },
  {
    ctrl: true,
    shift: true,
    key: "r",
    handler: async () => {
      const activeId = await SessionAPI.getActive();
      if (activeId) voiceRecorder.toggle(activeId);
    },
  },
];

// Prevent duplicate registration when SidebarApp + TerminalApp coexist in BrowserApp
let activeHandler: ShortcutHandler | null = null;

export function registerShortcuts(): ShortcutHandler {
  // If already registered (BrowserApp mounts both apps), return no-op
  if (activeHandler) {
    return activeHandler;
  }

  const handler = (e: KeyboardEvent) => {
    for (const shortcut of shortcuts) {
      if (
        e.ctrlKey === shortcut.ctrl &&
        e.shiftKey === shortcut.shift &&
        e.key.toLowerCase() === shortcut.key
      ) {
        e.preventDefault();
        shortcut.handler();
        return;
      }
    }
  };

  document.addEventListener("keydown", handler);
  activeHandler = handler;
  return handler;
}

export function unregisterShortcuts(handler: ShortcutHandler): void {
  document.removeEventListener("keydown", handler);
  if (activeHandler === handler) {
    activeHandler = null;
  }
}
