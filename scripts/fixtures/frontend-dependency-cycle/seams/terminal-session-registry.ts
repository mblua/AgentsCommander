import { fixtureView } from "./TerminalView";
import { fixtureSidebar } from "./sidebar";
import { fixtureIpc } from "./ipc";
import { invoke } from "@tauri-apps/api/core";
import { fixtureAdmission } from "./terminal-output-admission";
export const registry = [fixtureView, fixtureSidebar, fixtureIpc, invoke, fixtureAdmission];
