import type {
  AgentConfig,
  CodingAgentEnv,
  CodingAgentProfilesConfig,
  ProfileAssignmentScope,
  ProfileCellConfig,
  Session,
} from "./types";

export const PROFILE_LETTERS = Array.from({ length: 26 }, (_, index) =>
  String.fromCharCode("A".charCodeAt(0) + index)
);

export interface ProfileResolutionPreview {
  requestedProfile: string;
  effectiveProfile: string;
  fallbackChain: string[];
  fallbackApplied: boolean;
}

export interface ArgvParseResult {
  argv: string[];
  error: string | null;
}

const EMPTY_CELL: ProfileCellConfig = {
  enabled: true,
  command: "",
  env: {},
  notes: "",
};

export const AC_REPLICA_ROOT_PLACEHOLDER = "%AC_REPLICA_ROOT%";
export const AC_WORKSPACE_ROOT_PLACEHOLDER = "%AC_WORKSPACE_ROOT%";
export const AC_MATRIX_ROOT_PLACEHOLDER = "%AC_MATRIX_ROOT%";
export const AC_PLACEHOLDERS = [
  AC_REPLICA_ROOT_PLACEHOLDER,
  AC_WORKSPACE_ROOT_PLACEHOLDER,
  AC_MATRIX_ROOT_PLACEHOLDER,
] as const;

export function isProfileLetter(value: string): boolean {
  return /^[A-Z]$/.test(value);
}

export function normalizeProfileLetter(value: string | null | undefined): string | null {
  if (!value) return null;
  const upper = value.trim().toUpperCase();
  return isProfileLetter(upper) ? upper : null;
}

export function sortedProfileLetters(profiles: CodingAgentProfilesConfig): string[] {
  const letters = new Set(Object.keys(profiles.profileSlots).filter(isProfileLetter));
  letters.add("A");
  return [...letters].sort();
}

export function resolveProfileLabel(
  profiles: CodingAgentProfilesConfig,
  agents: AgentConfig[],
  agentId: string | null | undefined,
  letter: string,
): string {
  const labels = profiles.profileLabelsByAgent;
  const own = agentId ? labels?.[agentId]?.[letter]?.trim() : "";
  const primigenioId = agents[0]?.id;
  const inherited = primigenioId ? labels?.[primigenioId]?.[letter]?.trim() : "";
  const legacy = profiles.profileSlots[letter]?.label?.trim() ?? "";
  return own || inherited || legacy || "";
}

export function profileDisplayLabel(
  profiles: CodingAgentProfilesConfig,
  agents: AgentConfig[],
  agentId: string | null | undefined,
  letter: string,
): string {
  const name = resolveProfileLabel(profiles, agents, agentId, letter);
  return name ? `${letter}-${name.toUpperCase()}` : letter;
}

export function nextAvailableProfileLetter(
  profiles: CodingAgentProfilesConfig,
): string | null {
  const used = new Set(Object.keys(profiles.profileSlots));
  return PROFILE_LETTERS.find((letter) => !used.has(letter)) ?? null;
}

function cellForLetter(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): ProfileCellConfig | null {
  const cell = profiles.profilesByAgent[agentId]?.[letter] ?? null;
  if (!cell || !cell.enabled) return null;
  return cell;
}

export function profileCellOrDefault(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): ProfileCellConfig {
  return profiles.profilesByAgent[agentId]?.[letter] ?? EMPTY_CELL;
}

export function profileCellCommandText(cell: ProfileCellConfig | null | undefined): string {
  return cell?.command ?? "";
}

export function composeEffectiveCommand(base: string, cell: string): string {
  const b = base.trim();
  const c = cell.trim();
  if (!b) return c;
  if (!c) return b;
  return `${b} ${c}`;
}

export function hasAcPlaceholder(value: string): boolean {
  return AC_PLACEHOLDERS.some((token) => value.includes(token));
}

export function deriveWorkspaceRoot(replicaPath: string | null | undefined): string | null {
  if (!replicaPath) return null;
  const parts = replicaPath.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  const idx = parts.lastIndexOf(".ac");
  if (idx < 0) return null;
  const sep = replicaPath.includes("\\") ? "\\" : "/";
  return parts.slice(0, idx + 1).join(sep);
}

export function deriveMatrixRoot(replicaPath: string | null | undefined): string | null {
  if (!replicaPath) return null;
  const workspace = deriveWorkspaceRoot(replicaPath);
  if (!workspace) return null;
  const parts = replicaPath.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  const leaf = parts[parts.length - 1] ?? "";
  const parent = parts[parts.length - 2] ?? "";
  if (!/^__agent_/.test(leaf) || !/^wg-/.test(parent)) return null;
  const name = leaf.replace(/^__agent_/, "");
  const sep = replicaPath.includes("\\") ? "\\" : "/";
  return `${workspace}${sep}_agent_${name}`;
}

export function expandAcPlaceholdersPreview(
  value: string,
  replicaRoot: string | null | undefined,
): string {
  if (!replicaRoot || !hasAcPlaceholder(value)) return value;
  let out = value.split(AC_REPLICA_ROOT_PLACEHOLDER).join(replicaRoot);
  const workspace = deriveWorkspaceRoot(replicaRoot);
  if (workspace) out = out.split(AC_WORKSPACE_ROOT_PLACEHOLDER).join(workspace);
  const matrix = deriveMatrixRoot(replicaRoot);
  if (matrix) out = out.split(AC_MATRIX_ROOT_PLACEHOLDER).join(matrix);
  return out;
}

export function resolveProfilePreview(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  requested: string | null | undefined,
): ProfileResolutionPreview {
  const requestedProfile = normalizeProfileLetter(requested) ?? "A";
  const fallbackChain: string[] = [];
  let effectiveProfile = "A";

  for (
    let code = requestedProfile.charCodeAt(0);
    code >= "A".charCodeAt(0);
    code -= 1
  ) {
    const letter = String.fromCharCode(code);
    fallbackChain.push(letter);
    if (letter === "A" || cellForLetter(profiles, agentId, letter)) {
      effectiveProfile = letter;
      break;
    }
  }

  return {
    requestedProfile,
    effectiveProfile,
    fallbackChain,
    fallbackApplied: requestedProfile !== effectiveProfile,
  };
}

export type ProfileBadgeKind = "match" | "configured" | "fallback" | "missing" | "invalid";

export function profileConfiguredElsewhere(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): boolean {
  return Object.entries(profiles.profilesByAgent).some(
    ([id, cells]) => id !== agentId && Boolean(cells[letter]?.enabled),
  );
}

export function profileBadgeKind(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): Exclude<ProfileBadgeKind, "invalid"> {
  const cell = profiles.profilesByAgent[agentId]?.[letter];
  const configuredHere = letter === "A" || Boolean(cell?.enabled);
  if (configuredHere) {
    return letter === "A" ? "match" : "configured";
  }
  if (profileConfiguredElsewhere(profiles, agentId, letter)) return "missing";
  return resolveProfilePreview(profiles, agentId, letter).fallbackApplied
    ? "fallback"
    : "missing";
}

export function parseArgvText(input: string): ArgvParseResult {
  const argv: string[] = [];
  let current = "";
  let quote: "'" | '"' | null = null;

  for (let index = 0; index < input.length;) {
    const char = input[index];

    if (quote && char === "\\") {
      let slashCount = 0;
      while (input[index] === "\\") {
        slashCount += 1;
        index += 1;
      }
      if (input[index] === quote) {
        current += "\\".repeat(Math.floor(slashCount / 2));
        if (slashCount % 2 === 1) {
          current += quote;
          index += 1;
        } else {
          quote = null;
          index += 1;
        }
      } else {
        current += "\\".repeat(slashCount);
      }
      continue;
    }

    if (quote) {
      if (char === quote) {
        quote = null;
      } else {
        current += char;
      }
      index += 1;
      continue;
    }

    if (char === "'" || char === '"') {
      quote = char;
      index += 1;
      continue;
    }

    if (/\s/.test(char)) {
      if (current.length > 0) {
        argv.push(current);
        current = "";
      }
      index += 1;
      continue;
    }

    current += char;
    index += 1;
  }

  if (quote) return { argv: [], error: `Unclosed ${quote} quote` };
  if (current.length > 0) argv.push(current);
  return { argv, error: null };
}

function quoteArgvToken(token: string): string {
  if (token.length === 0) return '""';
  if (!/[\s"']/.test(token)) return token;

  let quoted = "";
  let slashCount = 0;
  for (const char of token) {
    if (char === "\\") {
      slashCount += 1;
      continue;
    }
    if (char === '"') {
      quoted += "\\".repeat(slashCount * 2 + 1);
      quoted += char;
      slashCount = 0;
      continue;
    }
    quoted += "\\".repeat(slashCount);
    slashCount = 0;
    quoted += char;
  }
  quoted += "\\".repeat(slashCount * 2);
  return `"${quoted}"`;
}

export function stringifyArgv(argv: string[]): string {
  return argv.map(quoteArgvToken).join(" ");
}

export function envKeyCompare(key: string): string {
  return key.trim().toUpperCase();
}

export function validateEnvRows(rows: CodingAgentEnv[]): string | null {
  const seen = new Set<string>();
  for (const row of rows) {
    if (!row.key.trim()) return "Environment variable keys cannot be empty.";
    const normalized = envKeyCompare(row.key);
    if (seen.has(normalized)) {
      return `Duplicate environment variable key: ${row.key.trim()}`;
    }
    seen.add(normalized);
  }
  return null;
}

export function hasEnabledEnvKey(rows: CodingAgentEnv[], key: string): boolean {
  const target = envKeyCompare(key);
  return rows.some((row) => row.enabled && envKeyCompare(row.key) === target);
}

/**
 * #1052 - env value display rule. A row's value is masked only when its key
 * starts with PASSWORD, matched case-insensitively on the trimmed key. Reuses
 * envKeyCompare (trim + uppercase) so the rule stays consistent with the rest
 * of the env-key handling. Display-only; values are stored as plaintext.
 */
export function shouldMaskEnvValue(key: string): boolean {
  return envKeyCompare(key).startsWith("PASSWORD");
}

export type EnvOrigin = "system" | "profile" | "accepted";

const MANAGED_HOME_ENV_KEYS = new Set([
  "CODEX_HOME",
  "CLAUDE_CONFIG_DIR",
  "CLAUDE_HOME",
  "GEMINI_HOME",
]);

function isLiteralAbsolutePath(value: string): boolean {
  const v = value.trim();
  return /^[A-Za-z]:[\\/]/.test(v) || v.startsWith("/") || v.startsWith("\\\\");
}

export function profileEnvOrigin(key: string, value: string): EnvOrigin {
  if (MANAGED_HOME_ENV_KEYS.has(envKeyCompare(key))) {
    if (hasAcPlaceholder(value)) return "system";
    if (isLiteralAbsolutePath(value)) return "accepted";
  }
  return "profile";
}

export interface EffectiveEnvEntry {
  key: string;
  value: string;
  origin: EnvOrigin;
}

export function effectiveEnvProjection(
  agentEnvs: CodingAgentEnv[] | undefined,
  profileEnv: Record<string, string> | undefined,
  acRoot: string | null | undefined,
): EffectiveEnvEntry[] {
  const byKey = new Map<string, EffectiveEnvEntry>();
  for (const row of agentEnvs ?? []) {
    if (!row.enabled) continue;
    const key = row.key.trim();
    if (!key) continue;
    byKey.set(envKeyCompare(key), {
      key,
      value: expandAcPlaceholdersPreview(row.value, acRoot),
      origin: row.source === "system" ? "system" : "accepted",
    });
  }
  for (const [rawKey, rawValue] of Object.entries(profileEnv ?? {})) {
    const key = rawKey.trim();
    if (!key) continue;
    byKey.set(envKeyCompare(key), {
      key,
      value: expandAcPlaceholdersPreview(rawValue, acRoot),
      origin: profileEnvOrigin(key, rawValue),
    });
  }
  return [...byKey.values()].sort((a, b) => a.key.localeCompare(b.key));
}

export function executableTokenBasename(token: string): string {
  const normalized = token.replace(/\\/g, "/");
  const leaf = normalized.split("/").pop() || normalized;
  return leaf.replace(/\.[^.]+$/, "").toLowerCase();
}

export function executableBasename(command: string): string {
  const parsed = parseArgvText(command);
  const first = parsed.error ? command.trim().split(/\s+/)[0] ?? "" : parsed.argv[0] ?? "";
  return executableTokenBasename(first);
}

export function commandExecutableBasename(command: string): string {
  return executableBasename(command);
}

export function isCodexAgent(agent: AgentConfig): boolean {
  return agent.id.toLowerCase() === "codex" || executableBasename(agent.command) === "codex";
}

export function defaultInstructionsFilename(command: string): string {
  const stems = command.trim().split(/\s+/).filter(Boolean).map(executableTokenBasename);
  if (stems.some((s) => s.startsWith("claude"))) return "CLAUDE.md";
  if (stems.some((s) => s.startsWith("codex"))) return "AGENTS.md";
  if (stems.some((s) => s.startsWith("gemini"))) return "GEMINI.md";
  return "AGENTS.md";
}

export const CLAUDE_CONTEXT_REGEX = String.raw`^ {2}Context [░█]+ (\d{1,3})%`;
export const CODEX_CONTEXT_REGEX = String.raw`^ {2}.*· Context (\d{1,3})% used`;
export const PI_CONTEXT_REGEX = String.raw`^(?:.*? )?(\d{1,3})\.\d%/`;

export function suggestedContextRegex(command: string): string | null {
  const parsed = parseArgvText(command);
  const tokens = parsed.error
    ? command.trim().split(/\s+/).filter(Boolean)
    : parsed.argv;
  const directStem = executableTokenBasename(tokens[0] ?? "");
  const piExecutableStem =
    directStem === "cmd" && tokens[1]?.toLowerCase() === "/c"
      ? executableTokenBasename(tokens[2] ?? "")
      : directStem;
  if (piExecutableStem === "pi") return PI_CONTEXT_REGEX;

  const stems = command.trim().split(/\s+/).filter(Boolean).map(executableTokenBasename);
  if (stems.some((s) => s.startsWith("claude"))) return CLAUDE_CONTEXT_REGEX;
  if (stems.some((s) => s.startsWith("codex"))) return CODEX_CONTEXT_REGEX;
  return null;
}

export function agentNameFromPathOrSession(
  path: string | null | undefined,
  sessionName: string,
): string {
  if (path) {
    const leaf = path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").pop() ?? "";
    const agent = leaf.replace(/^__?agent_/, "");
    if (agent) return agent;
  }
  const last = sessionName.split("/").pop();
  return last || sessionName;
}

export function targetProfileFqn(
  path: string | null | undefined,
  sessionName: string,
): string {
  if (path) {
    const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
    const parts = normalized.split("/");
    const acIndex = parts.lastIndexOf(".ac");
    if (acIndex > 0) {
      const project = parts[acIndex - 1];
      const agent = agentNameFromPathOrSession(path, sessionName);
      return `${project}:${agent}`;
    }
  }
  return agentNameFromPathOrSession(path, sessionName);
}

export function isAcAgentPath(path: string | null | undefined): boolean {
  if (!path) return false;
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const parts = normalized.split("/");
  const leaf = parts[parts.length - 1] ?? "";
  return parts.includes(".ac") && /^__?agent_/.test(leaf);
}

export function isWgReplicaPath(path: string | null | undefined): boolean {
  if (!path) return false;
  const parts = path.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  const leaf = parts[parts.length - 1] ?? "";
  const parent = parts[parts.length - 2] ?? "";
  return parts.includes(".ac") && /^__agent_/.test(leaf) && /^wg-/.test(parent);
}

export function shouldOfferRestartAfterAssign(
  selection: { scope: ProfileAssignmentScope; restartSessions: boolean },
  session: Pick<Session, "status" | "workingDirectory"> | undefined,
): boolean {
  if (selection.scope !== "replica") return false;
  if (selection.restartSessions) return false;
  if (!session) return false;
  if (!isWgReplicaPath(session.workingDirectory)) return false;
  return typeof session.status === "string";
}

export function sessionProfileBadge(
  session: Pick<
    Session,
    | "requestedProfile"
    | "effectiveProfile"
    | "profileFallbackApplied"
  >
): string | null {
  if (!session.requestedProfile && !session.effectiveProfile) return null;
  if (
    session.profileFallbackApplied &&
    session.requestedProfile &&
    session.effectiveProfile
  ) {
    return `${session.requestedProfile}->${session.effectiveProfile}`;
  }
  return session.effectiveProfile ?? session.requestedProfile;
}
