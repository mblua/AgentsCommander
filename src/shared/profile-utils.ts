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
  command: "",
  env: {},
  notes: "",
};

/** Display-only `%AC_ROOT%` placeholder. The backend is authoritative for the
 *  real expansion + validation at launch (#384 F17). */
export const AC_ROOT_PLACEHOLDER = "%AC_ROOT%";

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

/**
 * #548: the resolved display NAME for (agent, letter). Single source of truth for
 * label resolution, shared by the SettingsModal rails, the AgentPickerModal cards,
 * and the coordinator quick-access / session-row tooltips. Chain (resolve-on-read):
 *   1. the agent's own label,
 *   2. else the primigenio (agents[0]) agent's own label,
 *   3. else the legacy shared slot label (back-compat for pre-#548 configs),
 *   4. else "" (caller shows the bare letter).
 * `agents` MUST be in persisted order (agents[0] = primigenio); pass the raw
 * settings.agents array, never a sorted copy.
 */
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

/** Presentation form for the picker cards and the tooltips:
 *  `${letter}-${NAME}` when a name resolves, else the bare letter. */
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

/** v2 (#384): the full invocation string stored on a profile cell. */
export function profileCellCommandText(cell: ProfileCellConfig | null | undefined): string {
  return cell?.command ?? "";
}

/** True when a value still contains the `%AC_ROOT%` placeholder (display check). */
export function hasAcRootPlaceholder(value: string): boolean {
  return value.includes(AC_ROOT_PLACEHOLDER);
}

/**
 * Display-only preview of `%AC_ROOT%` expansion against a known replica root.
 * Returns the value unchanged when no root is available — the backend performs
 * the authoritative expansion and absolute-path validation at launch.
 */
export function expandAcRootPreview(
  value: string,
  acRoot: string | null | undefined,
): string {
  if (!acRoot || !hasAcRootPlaceholder(value)) return value;
  return value.split(AC_ROOT_PLACEHOLDER).join(acRoot);
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

/** The data-driven profile-cell badge states. `invalid` (a live command parse
 *  error) is UI-local and layered on top by the caller; the rest are resolved
 *  from the persisted profile data. (#526/#527) */
export type ProfileBadgeKind = "match" | "configured" | "fallback" | "missing" | "invalid";

/** True when `letter` has an enabled cell on some coding agent OTHER than
 *  `agentId`. Distinguishes a MISSING slot (configured elsewhere, absent here)
 *  from a plain positional fallback. Excludes the agent itself. (#526/#527) */
export function profileConfiguredElsewhere(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): boolean {
  return Object.entries(profiles.profilesByAgent).some(
    ([id, cells]) => id !== agentId && Boolean(cells[letter]?.enabled),
  );
}

/**
 * Profile-cell badge taxonomy shared 1:1 by the Config rails (SettingsModal) and
 * the Selection profile cards (AgentPickerModal), so both screens read the same
 * state for a given (agent, letter). Mirrors the B2C1a prototype:
 *   - A / baseline               → "match"      (always configured)
 *   - non-A with its own cell    → "configured" (direct match on a non-baseline slot)
 *   - non-A, absent, but the slot is configured on another agent → "missing"
 *   - non-A, absent, resolves through a lower letter             → "fallback"
 * The `invalid` state is not produced here — a live parse error is decided by the
 * caller and takes precedence.
 */
export function profileBadgeKind(
  profiles: CodingAgentProfilesConfig,
  agentId: string,
  letter: string,
): Exclude<ProfileBadgeKind, "invalid"> {
  const cell = profiles.profilesByAgent[agentId]?.[letter];
  const configuredHere = letter === "A" || Boolean(cell?.enabled);
  if (configuredHere) {
    // A is the baseline (MATCH); a non-A slot with its own enabled cell is a
    // direct match on a non-baseline slot (CONFIGURED).
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

/** Origin badge for an env value shown in the Config/Selection projections (#526/#527).
 *  - `system`   — AgentsCommander-managed home path (%AC_ROOT% placeholder on a known home key)
 *  - `accepted` — a user literal absolute path kept as-is (nothing to expand)
 *  - `profile`  — a value defined by the profile cell (the common case) */
export type EnvOrigin = "system" | "profile" | "accepted";

/** Env keys AgentsCommander manages per replica (isolated home directories). */
const MANAGED_HOME_ENV_KEYS = new Set([
  "CODEX_HOME",
  "CLAUDE_CONFIG_DIR",
  "CLAUDE_HOME",
  "GEMINI_HOME",
]);

function isLiteralAbsolutePath(value: string): boolean {
  const v = value.trim();
  // Windows drive (C:\ or C:/), POSIX root, or UNC share.
  return /^[A-Za-z]:[\\/]/.test(v) || v.startsWith("/") || v.startsWith("\\\\");
}

/** Classify a profile-cell env value's origin for its badge. A managed home key
 *  using %AC_ROOT% is AC-managed (`system`); a managed home key with a literal
 *  absolute path is a user-accepted override (`accepted`); everything else is a
 *  plain profile-defined value (`profile`). */
export function profileEnvOrigin(key: string, value: string): EnvOrigin {
  if (MANAGED_HOME_ENV_KEYS.has(envKeyCompare(key))) {
    if (hasAcRootPlaceholder(value)) return "system";
    if (isLiteralAbsolutePath(value)) return "accepted";
  }
  return "profile";
}

export interface EffectiveEnvEntry {
  key: string;
  value: string;
  origin: EnvOrigin;
}

/** Merge a coding agent's enabled env rows with the selected profile cell's env
 *  into the effective env that is actually set at launch (#527 Env / EFFECTIVE).
 *  Agent rows form the base (source `system` → system badge, user → accepted);
 *  profile env overrides on key collision and carries the `profile` origin.
 *  `%AC_ROOT%` is expanded for display when a replica root is known. */
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
      value: expandAcRootPreview(row.value, acRoot),
      origin: row.source === "system" ? "system" : "accepted",
    });
  }
  for (const [rawKey, rawValue] of Object.entries(profileEnv ?? {})) {
    const key = rawKey.trim();
    if (!key) continue;
    byKey.set(envKeyCompare(key), {
      key,
      value: expandAcRootPreview(rawValue, acRoot),
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

/**
 * v2 (#384): basename of the executable token of a command string. Used as the
 * coding-agent subtitle in the Config Screen (binary name, not model/sandbox).
 * Identical resolution to {@link executableBasename}; named for the command-string model.
 */
export function commandExecutableBasename(command: string): string {
  return executableBasename(command);
}

export function isCodexAgent(agent: AgentConfig): boolean {
  return agent.id.toLowerCase() === "codex" || executableBasename(agent.command) === "codex";
}

/**
 * #529 — display-only default instructions filename derived from a coding
 * agent's launch command. Used solely as the Config Screen placeholder so a
 * blank field shows the default the backend will write at launch.
 *
 * Best-effort parity (G2) with the Rust resolver
 * (`default_instructions_filename_for_command` → `CodingAgentKind::detect`):
 * reduce EVERY whitespace token to its lowercased file-stem basename and match
 * by prefix in precedence **claude > codex > gemini**, scanning all tokens —
 * exactly what the Rust detector does. Scanning every token (not just the
 * first/last) is what gives parity on the common shapes including trailing
 * flags: `claude`, `claude --model sonnet`, `cmd.exe /c claude`, `claude-mb`,
 * an absolute-path `claude.exe`, `codex --yolo`, `opencode`, custom. The codex
 * branch returns `AGENTS.md` (same as the default) but is kept explicit so the
 * precedence is faithful — e.g. `codex ... gemini` resolves to `AGENTS.md`, not
 * `GEMINI.md`, matching Rust.
 *
 * This deliberately does NOT reproduce the full Rust shell-quote tokenizer, so
 * an exotic quoted path containing spaces may still diverge — acceptable for a
 * cosmetic placeholder, since the backend resolves the value authoritatively at
 * launch.
 */
export function defaultInstructionsFilename(command: string): string {
  const stems = command.trim().split(/\s+/).filter(Boolean).map(executableTokenBasename);
  if (stems.some((s) => s.startsWith("claude"))) return "CLAUDE.md";
  if (stems.some((s) => s.startsWith("codex"))) return "AGENTS.md";
  if (stems.some((s) => s.startsWith("gemini"))) return "GEMINI.md";
  return "AGENTS.md";
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

/**
 * True only for a workgroup replica directory, i.e. a `__agent_<name>` leaf whose
 * parent is a `wg-<name>` directory under an `.ac` root. Broad-scope assignment
 * (kind/workgroup) and backend preview/apply require a real WG replica anchor;
 * origin agents (single-underscore `_agent_<name>`) and normal repos do not
 * qualify. (#384 §7)
 */
export function isWgReplicaPath(path: string | null | undefined): boolean {
  if (!path) return false;
  const parts = path.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  const leaf = parts[parts.length - 1] ?? "";
  const parent = parts[parts.length - 2] ?? "";
  return parts.includes(".ac") && /^__agent_/.test(leaf) && /^wg-/.test(parent);
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
