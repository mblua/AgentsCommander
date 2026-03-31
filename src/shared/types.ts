export interface Session {
  id: string;
  name: string;
  shell: string;
  shellArgs: string[];
  createdAt: string;
  workingDirectory: string;
  status: SessionStatus;
  waitingForInput: boolean;
  pendingReview: boolean;
  lastPrompt: string | null;
  gitBranch: string | null;
  token: string;
}

export type SessionStatus = "active" | "running" | "idle" | { exited: number };

export interface SessionGroup {
  id: string;
  name: string;
  color: string;
  collapsed: boolean;
  order: string[];
}

export interface ShellProfile {
  name: string;
  command: string;
  args: string[];
  icon: string;
  color: string;
  env: Record<string, string>;
  workingDirectory: string;
}

export interface AppConfig {
  general: GeneralConfig;
  sidebar: SidebarConfig;
  terminal: TerminalConfig;
  keybindings: Record<string, string>;
}

export interface GeneralConfig {
  defaultShell: string;
  defaultShellArgs: string[];
  theme: string;
  confirmOnClose: boolean;
}

export interface SidebarConfig {
  width: number;
  alwaysOnTop: boolean;
  opacity: number;
  showShellType: boolean;
  showStatusIcon: boolean;
}

export interface TerminalConfig {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  scrollback: number;
  cursorStyle: "block" | "underline" | "bar";
  cursorBlink: boolean;
  webglRenderer: boolean;
}

export interface PtyOutputEvent {
  sessionId: string;
  data: number[];
}

export interface AgentConfig {
  id: string;
  label: string;
  command: string;
  color: string;
  gitPullBefore: boolean;
  excludeGlobalClaudeMd: boolean;
}

export interface RepoMatch {
  name: string;
  path: string;
  agents: string[];
}

export interface TelegramBotConfig {
  id: string;
  label: string;
  token: string;
  chatId: number;
  color: string;
}

export interface BridgeInfo {
  botId: string;
  botLabel: string;
  sessionId: string;
  status: BridgeStatus;
  color: string;
}

export type BridgeStatus = "active" | { error: string } | "detaching";

export interface WindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface AppSettings {
  defaultShell: string;
  defaultShellArgs: string[];
  repoPaths: string[];
  agents: AgentConfig[];
  telegramBots: TelegramBotConfig[];
  sidebarAlwaysOnTop: boolean;
  raiseTerminalOnClick: boolean;
  voiceToTextEnabled: boolean;
  geminiApiKey: string;
  geminiModel: string;
  voiceAutoExecute: boolean;
  voiceAutoExecuteDelay: number;
  sidebarZoom: number;
  terminalZoom: number;
  guideZoom: number;
  darkfactoryZoom: number;
  sidebarGeometry: WindowGeometry | null;
  terminalGeometry: WindowGeometry | null;
}

// Team grouping for sidebar
export interface TeamSessionGroup {
  team: Team;
  coordinator: Session | null;
  members: Session[];
}

// Dark Factory types

export interface TeamMember {
  name: string;
  path: string;
}

export interface DarkFactoryLayer {
  id: string;
  name: string;
}

export interface CoordinatorLink {
  supervisorTeamId: string;
  subordinateTeamId: string;
}

export interface Team {
  id: string;
  name: string;
  members: TeamMember[];
  coordinatorName?: string;
  layerId?: string;
}

export interface DarkFactoryConfig {
  teams: Team[];
  layers: DarkFactoryLayer[];
  coordinatorLinks: CoordinatorLink[];
}

// Sidebar store state
export interface SessionsState {
  sessions: Session[];
  activeId: string | null;
  teams: Team[];
  teamFilter: string | null;
  showInactive: boolean;
  repos: RepoMatch[];
}

// Phone communication types

export interface PhoneMessage {
  id: string;
  from: string;
  to: string;
  team: string;
  content: string;
  timestamp: string;
  status: "pending" | "delivered" | "error";
}

export interface PhoneConversation {
  id: string;
  participants: string[];
  createdAt: string;
  messages: PhoneMessage[];
}

export interface AgentInfo {
  name: string;
  path: string;
  teams: string[];
  isCoordinatorOf: string[];
}

// Dark Factory component props

export interface OrgChartProps {
  config: DarkFactoryConfig;
}

export interface LayerColumnProps {
  layer: DarkFactoryLayer;
  teams: Team[];
  hoveredTeamId: string | null;
  onHoverTeam: (id: string | null) => void;
  onNodeRect: (teamId: string, rect: DOMRect) => void;
  wrapperRef: HTMLDivElement | undefined;
}

export interface TeamNodeProps {
  team: Team;
  highlighted: boolean;
  onHover: (hovering: boolean) => void;
  onNodeRect: (teamId: string, rect: DOMRect) => void;
  wrapperRef: HTMLDivElement | undefined;
}

export interface ConnectionLinesProps {
  links: CoordinatorLink[];
  teams: Team[];
  nodeRects: Map<string, DOMRect>;
  hoveredTeamId: string | null;
}
