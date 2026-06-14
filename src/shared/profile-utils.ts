import type {
  AgentConfig,
  CodingAgentEnv,
  CodingAgentProfilesConfig,
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
  argv: [],
  env: {},
  notes: "",
};

export function isProfileLetter(value: string): boolean {
  return /^[A-Z]$/.test(value);
}

export function normalizeProfileLetter(value: string | null | undefined): string | null {
  if (!value) return null;
  const upper = value.trim().toUpperCase();
  return isProfileLetter(upper) ? upper : null;
}

export function sortedProfileLetters(profiles: CodingAgentProfilesConfig): string[] {
  const letters = new Set(Object.keys(profiles.letters).filter(isProfileLetter));
  letters.add("A");
  return [...letters].sort();
}

export function profileDisplayLabel(
  profiles: CodingAgentProfilesConfig,
  letter: string,
): string {
  const name = profiles.letters[letter]?.name.trim();
  return name ? `${letter}-${name.toUpperCase()}` : letter;
}

export function nextAvailableProfileLetter(
  profiles: CodingAgentProfilesConfig,
): string | null {
  const used = new Set(Object.keys(profiles.letters));
  return PROFILE_LETTERS.find((letter) => !used.has(letter)) ?? null;
}

function cellForLetter(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): ProfileCellConfig | null {
  const cell = profiles.matrix[agentId]?.[letter] ?? null;
  if (!cell || !cell.enabled) return null;
  return cell;
}

export function profileCellOrDefault(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): ProfileCellConfig {
  return profiles.matrix[agentId]?.[letter] ?? EMPTY_CELL;
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

export function isCodexAgent(agent: AgentConfig): boolean {
  return agent.id.toLowerCase() === "codex" || executableBasename(agent.command) === "codex";
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
