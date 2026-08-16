import { fixtureView } from "./TerminalView";
import { fixtureSidebar } from "./sidebar";
import { fixtureIpc } from "./ipc";
import { invoke } from "@tauri-apps/api/core";
import { fixtureRegistry } from "./terminal-session-registry";
export const admission = [fixtureView, fixtureSidebar, fixtureIpc, invoke, fixtureRegistry];
