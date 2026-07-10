import { Component, createSignal, createEffect, createMemo, For, Index, Show, onMount, onCleanup } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { isTauri } from "../../shared/platform";
import type {
  AppSettings,
  AgentConfig,
  CodingAgentEnv,
  CodingAgentDefinition,
  LogLevel,
  TelegramBotConfig,
  ProfileCellConfig,
  ApiClientMintResponse,
  ApiClientMintScope,
  SessionBackendKind,
} from "../../shared/types";
import { SettingsAPI, TelegramAPI, ReposAPI, CodingAgentsAPI } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import { validateScreenshotHotkeySyntax } from "../../shared/screenshot-hotkey";
import { settingsStore } from "../../shared/stores/settings";
import { setSoundsEnabled } from "../../shared/sound";
import { sessionsStore } from "../stores/sessions";
import { projectStore } from "../stores/project";
import { newAgentId, definitionToSeed } from "../../shared/agent-presets";
import { codingAgentsStore } from "../stores/coding-agents";
import TrashIcon from "./TrashIcon";
import XMarkIcon from "./XMarkIcon";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";
import {
  AC_MATRIX_ROOT_PLACEHOLDER,
  AC_REPLICA_ROOT_PLACEHOLDER,
  AC_WORKSPACE_ROOT_PLACEHOLDER,
  commandExecutableBasename,
  defaultInstructionsFilename,
  executableTokenBasename,
  hasAcPlaceholder,
  hasEnabledEnvKey,
  isCodexAgent,
  nextAvailableProfileLetter,
  parseArgvText,
  profileBadgeKind,
  profileEnvOrigin,
  resolveProfileLabel,
  resolveProfilePreview,
  sortedProfileLetters,
  validateEnvRows,
} from "../../shared/profile-utils";

type ProfileCellEnvRow = { key: string; value: string };

// #576: always-visible reference for the AC path placeholders usable in a
// profile's command and env values. The token strings are taken from the shared
// profile-utils constants so they match the backend (placeholders.rs) byte for
// byte; the backend stays the authority for real expansion + validation at launch.
const AC_PLACEHOLDER_HELP = [
  "AC path placeholders (expand at launch):",
  `${AC_REPLICA_ROOT_PLACEHOLDER} — this replica's working dir`,
  `${AC_WORKSPACE_ROOT_PLACEHOLDER} — the .ac workspace root`,
  `${AC_MATRIX_ROOT_PLACEHOLDER} — the canonical _agent_<name> dir`,
  "Full details: docs/agent-matrix-conventions.md §5",
].join("\n");

const ENV_ORIGIN_LABEL: Record<string, string> = {
  system: "SYSTEM",
  profile: "PROFILE",
  accepted: "ACCEPTED",
};

const GEMINI_MODELS = [
  { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash (recommended)" },
  { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
  { id: "gemini-2.0-flash", label: "Gemini 2.0 Flash" },
  { id: "gemini-1.5-flash", label: "Gemini 1.5 Flash" },
  { id: "gemini-1.5-pro", label: "Gemini 1.5 Pro" },
];

const API_CLIENT_SCOPE_OPTIONS: { value: ApiClientMintScope; label: string }[] = [
  { value: "send", label: "send" },
  { value: "list-peers-lean", label: "list-peers-lean" },
  { value: "session-transport", label: "session-transport" },
];

const API_CLIENT_EXPIRY_OPTIONS = [
  { value: "default", label: "24 hours (default)" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
] as const;

type ApiClientExpiryOption = (typeof API_CLIENT_EXPIRY_OPTIONS)[number]["value"];

const API_CLIENT_EXPIRY_MS: Record<Exclude<ApiClientExpiryOption, "default">, number> = {
  "7d": 7 * 24 * 60 * 60 * 1000,
  "30d": 30 * 24 * 60 * 60 * 1000,
};

const errorMessage = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

// #526: the former "agents" + "profiles" tabs are merged into one unified
// "Coding Agents" screen (agent list + dual comparison rails). The "profiles"
// section name is kept as an alias so existing callers still land on the
// unified screen. #516 adds a separate "resources" tab.
type SettingsTab = "general" | "agents" | "resources" | "integrations";

const TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "General" },
  { key: "agents", label: "Coding Agents" },
  { key: "resources", label: "Resources" },
  { key: "integrations", label: "Integrations" },
];

const resolveSettingsSection = (s: string | undefined): SettingsTab =>
  s === "agents" || s === "profiles"
    ? "agents"
    : s === "integrations"
      ? "integrations"
      : "general";

const BYTES_PER_GIB = 1024 ** 3;
const CONTAINER_IMAGE_EXAMPLE = "agentscommander/ac-claude:latest";

const bytesToGiBInput = (bytes: number): string => {
  const value = bytes / BYTES_PER_GIB;
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
};

const giBInputToBytes = (value: string): number =>
  Math.round(Number(value) * BYTES_PER_GIB);

const isContainerLoopbackBind = (bind: string | undefined): boolean => {
  const normalized = (bind ?? "").trim().toLowerCase();
  return (
    normalized === "" ||
    normalized === "127.0.0.1" ||
    normalized === "localhost" ||
    normalized === "::1"
  );
};

const apiServerEndpointChanged = (
  next: Pick<AppSettings, "apiServerBind" | "apiServerPort">,
  seed: Pick<AppSettings, "apiServerBind" | "apiServerPort"> | null,
): boolean =>
  !!seed &&
  (next.apiServerBind !== seed.apiServerBind ||
    next.apiServerPort !== seed.apiServerPort);

const cloneSettings = (value: AppSettings | null): AppSettings | null => {
  if (!value) return null;
  if (typeof structuredClone === "function") return structuredClone(value);
  return JSON.parse(JSON.stringify(value)) as AppSettings;
};

const SettingsModal: Component<{ onClose: () => void; section?: string }> = (props) => {
  const seededSettings = cloneSettings(settingsStore.current);
  const [settings, setSettings] = createStore<{ data: AppSettings | null }>({
    data: seededSettings,
  });
  const [modalSeed, setModalSeed] = createSignal<AppSettings | null>(
    cloneSettings(seededSettings),
  );
  const [draftDirty, setDraftDirty] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [testingBot, setTestingBot] = createSignal<string | null>(null);
  const [testResult, setTestResult] = createSignal<{
    id: string;
    ok: boolean;
    msg?: string;
  } | null>(null);
  // `props.section` lets callers (e.g. disabled mic click) open on a specific
  // tab. Invalid or absent → fall back to "general" default. The effect below
  // also snaps to the requested section when props.section changes while the
  // modal is already mounted (double-click on disabled mic re-targets).
  const initialTab: SettingsTab = resolveSettingsSection(props.section);
  const [activeTab, setActiveTab] = createSignal<SettingsTab>(initialTab);
  createEffect(() => {
    const s = props.section;
    if (s) setActiveTab(resolveSettingsSection(s));
  });

  const [webServerRunning, setWebServerRunning] = createSignal(false);
  const [apiServerRunning, setApiServerRunning] = createSignal(
    seededSettings?.apiServerEnabled ?? false,
  );
  const [apiServerBusy, setApiServerBusy] = createSignal(false);
  const [apiClientMintRoot, setApiClientMintRoot] = createSignal("");
  const [apiClientMintScopes, setApiClientMintScopes] = createSignal<ApiClientMintScope[]>([
    "send",
    "list-peers-lean",
  ]);
  const [apiClientMintLabel, setApiClientMintLabel] = createSignal("");
  const [apiClientMintExpiry, setApiClientMintExpiry] =
    createSignal<ApiClientExpiryOption>("default");
  const [apiClientMinting, setApiClientMinting] = createSignal(false);
  const [apiClientMintError, setApiClientMintError] = createSignal("");
  const [apiClientMintResult, setApiClientMintResult] =
    createSignal<ApiClientMintResponse | null>(null);
  const [apiClientSecretCopied, setApiClientSecretCopied] = createSignal(false);
  const [saveError, setSaveError] = createSignal("");
  const [profileCellText, setProfileCellText] = createStore<Record<string, string>>({});
  const [profileCellErrors, setProfileCellErrors] = createStore<Record<string, string>>({});
  // Per-cell env editing draft (ordered rows). Mirrors profileCellText: keeps
  // in-progress key/value edits stable while the underlying Record<string,string>
  // is rebuilt on every keystroke. Keyed by `${agentId}:${letter}`.
  const [profileCellEnvRows, setProfileCellEnvRows] = createStore<Record<string, ProfileCellEnvRow[]>>({});

  const s = () => settings.data;

  const apiServerChecked = () =>
    apiServerBusy() ? apiServerRunning() : settings.data?.apiServerEnabled ?? false;
  const apiServerBindWarning = () => {
    const bind = settings.data?.apiServerBind?.trim();
    return !!bind && !isContainerLoopbackBind(bind);
  };
  const apiServerStatusAddress = () => {
    const source = apiServerRunning() ? (modalSeed() ?? settings.data) : settings.data;
    return source ? `${source.apiServerBind}:${source.apiServerPort}` : "";
  };

  const saveCurrentSettingsDraft = async (): Promise<AppSettings> => {
    if (!settings.data) throw new Error("Settings are not loaded.");
    const nextSettings = mergeSettingsForSavePreservingProjects(
      settings.data,
      await SettingsAPI.get(),
      modalSeed(),
    );
    await SettingsAPI.saveDraft(nextSettings);
    setSettings("data", nextSettings);
    setModalSeed(cloneSettings(nextSettings));
    setDraftDirty(false);
    return nextSettings;
  };

  const syncApiServerRunning = (running: boolean, markDirty: boolean) => {
    setApiServerRunning(running);
    if (!settings.data) return;
    if (markDirty) setDraftDirty(true);
    setSettings("data", produce((draft) => {
      if (draft) draft.apiServerEnabled = running;
    }));
  };

  const refreshApiServerRunning = async (markDirty: boolean): Promise<boolean> => {
    const running = await SettingsAPI.apiServerStatus();
    syncApiServerRunning(running, markDirty);
    return running;
  };

  const handleApiServerToggle = async (enabled: boolean) => {
    if (apiServerBusy()) return;
    setSaveError("");
    setApiServerBusy(true);
    try {
      if (enabled && settings.data && apiServerEndpointChanged(settings.data, modalSeed())) {
        const validationError = currentValidationError();
        if (validationError) {
          setSaveError(validationError);
          return;
        }
        await saveCurrentSettingsDraft();
      }
      if (enabled) {
        await SettingsAPI.startApiServer();
      } else {
        await SettingsAPI.stopApiServer();
      }
      const running = await refreshApiServerRunning(true);
      if (enabled && !running) {
        setSaveError("API server did not report running after start.");
      } else if (!enabled && running) {
        setSaveError("API server is still running after stop.");
      }
    } catch (err: unknown) {
      try {
        await refreshApiServerRunning(false);
      } catch {}
      const action = enabled ? "start" : "stop";
      setSaveError(
        `API server ${action} failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    } finally {
      setApiServerBusy(false);
    }
  };

  const restartApiServerAfterEndpointSave = async (nextSettings: AppSettings) => {
    setApiServerBusy(true);
    try {
      const stopped = await SettingsAPI.stopApiServer();
      if (!stopped) {
        throw new Error("API server did not stop for bind/port restart.");
      }
      setApiServerRunning(false);

      if (nextSettings.apiServerEnabled) {
        const started = await SettingsAPI.startApiServer();
        if (!started) {
          throw new Error("API server did not start after bind/port change.");
        }
        const running = await SettingsAPI.apiServerStatus();
        syncApiServerRunning(running, false);
        if (!running) {
          throw new Error("API server did not report running after bind/port restart.");
        }
      }
    } finally {
      setApiServerBusy(false);
    }
  };

  const apiClientRootOptions = createMemo(() => {
    const seen = new Set<string>();
    const options = [];
    for (const project of projectStore.projects) {
      for (const workgroup of project.workgroups) {
        for (const replica of workgroup.agents) {
          const normalized = replica.path.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
          if (seen.has(normalized)) continue;
          seen.add(normalized);
          options.push({
            path: replica.path,
            label: `${project.folderName} / ${workgroup.name} / ${replica.name}${
              replica.isCoordinator ? " (coordinator)" : ""
            }`,
          });
        }
      }
    }
    return options;
  });

  createEffect(() => {
    const options = apiClientRootOptions();
    const current = apiClientMintRoot();
    if (options.length === 0) {
      if (current !== "") setApiClientMintRoot("");
      return;
    }
    if (!options.some((option) => option.path === current)) {
      setApiClientMintRoot(options[0].path);
    }
  });

  const clearApiClientSecret = () => {
    setApiClientMintResult(null);
    setApiClientSecretCopied(false);
  };

  const closeSettings = () => {
    clearApiClientSecret();
    props.onClose();
  };

  const updateApiClientScope = (scope: ApiClientMintScope, checked: boolean) => {
    setApiClientMintError("");
    setApiClientMintScopes((prev) => {
      const raw = checked ? [...prev, scope] : prev.filter((s) => s !== scope);
      return API_CLIENT_SCOPE_OPTIONS
        .map((option) => option.value)
        .filter((value) => raw.includes(value));
    });
  };

  const selectedApiClientScopesValid = () => apiClientMintScopes().length > 0;

  const selectedApiClientExpiry = (): string | null => {
    const option = apiClientMintExpiry();
    if (option === "default") return null;
    return new Date(Date.now() + API_CLIENT_EXPIRY_MS[option]).toISOString();
  };

  const handleMintApiClient = async () => {
    if (apiClientMinting()) return;
    const root = apiClientMintRoot();
    const scopes = apiClientMintScopes();
    if (!root) {
      setApiClientMintError("Load a project with at least one workgroup replica before minting.");
      return;
    }
    if (scopes.length === 0) {
      setApiClientMintError("Select at least one API client scope.");
      return;
    }

    setApiClientMinting(true);
    setApiClientMintError("");
    clearApiClientSecret();
    try {
      const label = apiClientMintLabel().trim();
      const result = await SettingsAPI.mintApiClient({
        root,
        scopes,
        label: label.length > 0 ? label : null,
        expires: selectedApiClientExpiry(),
      });
      setApiClientMintResult(result);
    } catch (err: unknown) {
      setApiClientMintError(errorMessage(err));
    } finally {
      setApiClientMinting(false);
    }
  };

  const copyApiClientToken = async () => {
    const token = apiClientMintResult()?.token;
    if (!token) return;
    try {
      await navigator.clipboard.writeText(token);
      setApiClientSecretCopied(true);
    } catch (err: unknown) {
      setApiClientMintError(`Copy failed: ${errorMessage(err)}`);
    }
  };

  // ── #526 Config Screen: comparison pair (two rails side by side) + the agent
  // whose inline config editor is expanded. The LEFT rail is the primary and is
  // always filled (positional fallback to the first agent). The RIGHT rail is an
  // explicit comparison slot: it shows exactly the agent in `rightRailId`, and is
  // empty when that is null — so "Remove" actually clears it. Seeded once on load
  // (onMount) rather than via an effect, so clearing it does not get re-filled.
  // The editor is collapsed by default — the resting screen reads as the
  // prototype (compact agent list + the two comparison rails as the protagonist).
  const [leftRailId, setLeftRailId] = createSignal<string | null>(null);
  const [rightRailId, setRightRailId] = createSignal<string | null>(null);
  const [activeAgentId, setActiveAgentId] = createSignal<string | null>(null);

  // #769 Phase 2 — "Re-seed default configuration" confirm flow. `reseedTarget`
  // holds the catalog def being re-seeded (null = modal closed).
  const [reseedTarget, setReseedTarget] = createSignal<CodingAgentDefinition | null>(null);
  const [reseeding, setReseeding] = createSignal(false);

  const agentList = () => settings.data?.agents ?? [];
  const agentById = (id: string | null) =>
    id ? agentList().find((a) => a.id === id) ?? null : null;
  const leftRailAgent = () => agentById(leftRailId()) ?? agentList()[0] ?? null;
  const rightRailAgent = () => {
    const explicit = agentById(rightRailId());
    return explicit && explicit.id !== leftRailAgent()?.id ? explicit : null;
  };

  const swapRails = () => {
    const left = leftRailAgent()?.id ?? null;
    const right = rightRailAgent()?.id ?? null;
    // #527: no-op when the right rail is empty. Swapping a null right into the
    // left would drop the left's explicit pin to the positional fallback
    // (agents[0]) — a surprising jump to a different agent. Keep the left pinned.
    if (right === null) return;
    setLeftRailId(right);
    setRightRailId(left);
  };

  type RailPill = "left" | "right" | "available";
  const railPillFor = (agentId: string): RailPill => {
    if (leftRailAgent()?.id === agentId) return "left";
    if (rightRailAgent()?.id === agentId) return "right";
    return "available";
  };

  // #895 — the agent list doubles as the rail picker: clicking the first row
  // targets the left/primary rail, clicking any later row targets the
  // right/comparison rail.
  const railSideForIndex = (index: number): "left" | "right" =>
    index === 0 ? "left" : "right";

  // What a click on that row would actually do. The head's title and
  // `aria-disabled` are driven off this, so the row never advertises an action
  // it will not perform.
  type RailAction = "assign" | "swap" | "none";
  const railActionFor = (agentId: string, index: number): RailAction => {
    const pill = railPillFor(agentId);
    if (pill === railSideForIndex(index)) return "none";
    return pill === "available" ? "assign" : "swap";
  };

  // The two rails must never point at the same agent (`rightRailAgent()`
  // collapses to null on a collision). So when the clicked agent already holds
  // the *other* rail, the two exchange places instead of one stomping the other.
  const selectAgentRail = (agentId: string, index: number) => {
    const left = leftRailAgent()?.id ?? null;
    const right = rightRailAgent()?.id ?? null;
    if (railSideForIndex(index) === "left") {
      if (left === agentId) return;
      // Row 0 already holds the comparison rail: hand the right rail the agent
      // being displaced. A bare setLeftRailId here would leave `rightRailId`
      // pointing at the new left agent — emptying the rail and wedging
      // "Swap Rails", which early-returns on a null derived right.
      if (right === agentId) setRightRailId(left);
      setLeftRailId(agentId);
      return;
    }
    if (right === agentId) return;
    // The clicked row holds the left rail. Demote it to the right and promote
    // whatever was on the right. When the right rail is empty, `setLeftRailId(null)`
    // hands the left back to the positional fallback (agents[0]) — never this
    // agent, which sits at index > 0.
    if (left === agentId) setLeftRailId(right);
    setRightRailId(agentId);
  };

  const toggleAgentEditor = (agentId: string) =>
    setActiveAgentId((prev) => (prev === agentId ? null : agentId));

  // Per profile-cell expand state on the Config rails. Cells can collapse to a
  // summary row (letter + label + status badge), but start expanded by default.
  // Keyed by `${agentId}:${letter}`.
  const [expandedCells, setExpandedCells] = createStore<Record<string, boolean | undefined>>({});
  const isCellExpanded = (agentId: string, letter: string): boolean => {
    // Read the key directly so the store tracks it even while still unset — a
    // hasOwnProperty short-circuit would skip the subscription and the card
    // would never react to a later toggle. `undefined` → default expanded.
    const value = expandedCells[`${agentId}:${letter}`];
    return value === undefined ? true : value;
  };
  const toggleCell = (agentId: string, letter: string) =>
    setExpandedCells(`${agentId}:${letter}`, !isCellExpanded(agentId, letter));

  onMount(async () => {
    // #769 — populate the backend-owned coding-agent catalog for the quick-add
    // row. Pre-seeded with the fallback, so the buttons render immediately.
    void codingAgentsStore.ensureLoaded();
    const [loaded, wsRunning, apiRunning] = await Promise.all([
      SettingsAPI.get(),
      SettingsAPI.getWebServerStatus().catch(() => false),
      SettingsAPI.apiServerStatus().catch(() => seededSettings?.apiServerEnabled ?? false),
    ]);
    setWebServerRunning(wsRunning);
    setApiServerRunning(apiRunning);
    if (!draftDirty()) {
      const nextSettings = cloneSettings(loaded);
      if (nextSettings) nextSettings.apiServerEnabled = apiRunning;
      setSettings("data", nextSettings);
      const loadedSeed = cloneSettings(loaded);
      setModalSeed(loadedSeed);
      // Seed the comparison pair from the loaded agents (left primary + right
      // comparison slot). Only when still unset, so a user's pick isn't clobbered.
      if (leftRailId() === null && loaded.agents[0]) setLeftRailId(loaded.agents[0].id);
      if (rightRailId() === null && loaded.agents[1]) setRightRailId(loaded.agents[1].id);
    }
  });

  // ── Generic field updater ──
  const updateField = <K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K]
  ) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", key as any, value as any);
  };

  // ── Agents ──
  const updateAgent = (
    index: number,
    field: keyof AgentConfig,
    value: string | boolean | string[] | CodingAgentEnv[]
  ) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "agents", index, field as any, value as any);
  };

  // #598 — config seed is a nested object on AgentConfig, so it cannot flow
  // through updateAgent (scalar fields only). These setters merge the nested
  // shape while editing; the disabled/empty case is normalized to omitted at
  // the save choke point (settings-save.ts::normalizeAgentConfigSeed).
  const setAgentConfigSeedEnabled = (index: number, enabled: boolean) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const current = settings.data.agents[index]?.configSeed;
    setSettings("data", "agents", index, "configSeed", {
      enabled,
      dest: current?.dest ?? "",
    });
  };

  const setAgentConfigSeedDest = (index: number, dest: string) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const current = settings.data.agents[index]?.configSeed;
    setSettings("data", "agents", index, "configSeed", {
      enabled: current?.enabled ?? true,
      dest,
    });
  };

  const setAgentBackendKind = (index: number, kind: SessionBackendKind) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const current = settings.data.agents[index]?.backend;
    setSettings("data", "agents", index, "backend", {
      kind,
      image: current?.image,
    });
  };

  const setAgentBackendImage = (index: number, image: string) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const current = settings.data.agents[index]?.backend;
    setSettings("data", "agents", index, "backend", {
      kind: current?.kind ?? "containerTransport",
      image,
    });
  };

  const updateAgentEnv = (
    agentIndex: number,
    rowIndex: number,
    field: keyof CodingAgentEnv,
    value: string | boolean
  ) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "agents", agentIndex, "envs", rowIndex, field as any, value as any);
  };

  const addAgentEnv = (agentIndex: number) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const row: CodingAgentEnv = {
      key: "",
      value: "",
      source: "user",
      enabled: true,
    };
    setSettings("data", "agents", agentIndex, "envs", (prev) => [...(prev ?? []), row]);
  };

  const removeAgentEnv = (agentIndex: number, rowIndex: number) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "agents", agentIndex, "envs", (prev) =>
      (prev ?? []).filter((_, i) => i !== rowIndex)
    );
  };

  const emptyProfileCell = (): ProfileCellConfig => ({
    enabled: true,
    command: "",
    env: {},
    notes: "",
  });

  const profileLetters = () =>
    settings.data ? sortedProfileLetters(settings.data.codingAgentProfiles) : ["A"];

  const profileCellKey = (agentId: string, letter: string) => `${agentId}:${letter}`;

  const profileCell = (agentId: string, letter: string): ProfileCellConfig | null =>
    settings.data?.codingAgentProfiles.profilesByAgent[agentId]?.[letter] ?? null;

  type ProfileBadge = "match" | "configured" | "fallback" | "missing" | "invalid";

  // #526: the data-driven taxonomy lives in profileBadgeKind() (shared 1:1 with
  // the Selection screen so both read the same state). A live command parse error
  // is UI-local and takes precedence here as the red "invalid" badge.
  const profileCellBadge = (agentId: string, letter: string): ProfileBadge => {
    if (profileCellErrors[profileCellKey(agentId, letter)]) return "invalid";
    return profileBadgeKind(settings.data!.codingAgentProfiles, agentId, letter);
  };

  const setProfileCell = (
    agentId: string,
    letter: string,
    cell: ProfileCellConfig
  ) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "codingAgentProfiles", "profilesByAgent", (byAgent) => ({
      ...byAgent,
      [agentId]: {
        ...(byAgent[agentId] ?? {}),
        [letter]: cell,
      },
    }));
  };

  // #548: per-agent label override. Keep the WHOLE-MAP functional replacement —
  // it creates the missing [agentId] node on an agent's first edit AND fires the
  // top-level property signal that drives the cross-rail inherited-placeholder
  // recompute. Do NOT "optimize" to a deep-path set (see plan §9 / §13.2).
  const updateProfileLabel = (agentId: string, letter: string, label: string) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "codingAgentProfiles", "profileLabelsByAgent", (byAgent) => ({
      ...byAgent,
      [agentId]: {
        ...(byAgent[agentId] ?? {}),
        [letter]: label,
      },
    }));
  };

  const addProfileLetter = () => {
    if (!settings.data) return;
    const letter = nextAvailableProfileLetter(settings.data.codingAgentProfiles);
    if (!letter) return;
    setDraftDirty(true);
    setSettings("data", "codingAgentProfiles", "profileSlots", (slots) => ({
      ...slots,
      [letter]: { label: "" },
    }));
  };

  // #526: slot-level delete — drop the letter from profileSlots and from every
  // coding agent's cells. produce() so the keys are actually removed; assigning a
  // fresh object to a store path merges (keeps absent keys), which would leave the
  // slot behind and the card would re-render as MISSING instead of vanishing.
  const removeProfileLetter = (letter: string) => {
    if (!settings.data || letter === "A") return;
    setDraftDirty(true);
    setSettings(
      "data",
      "codingAgentProfiles",
      produce((profiles) => {
        delete profiles.profileSlots[letter];
        for (const cells of Object.values(profiles.profilesByAgent)) {
          delete cells[letter];
        }
        for (const labels of Object.values(profiles.profileLabelsByAgent)) {
          delete labels[letter];
        }
      }),
    );
    // Drop any in-progress per-cell drafts for this letter so a stale parse error
    // can't block Save and stale env rows don't resurface.
    const suffix = `:${letter}`;
    setProfileCellText(produce((draft) => {
      for (const key of Object.keys(draft)) if (key.endsWith(suffix)) delete draft[key];
    }));
    setProfileCellErrors(produce((draft) => {
      for (const key of Object.keys(draft)) if (key.endsWith(suffix)) delete draft[key];
    }));
    setProfileCellEnvRows(produce((draft) => {
      for (const key of Object.keys(draft)) if (key.endsWith(suffix)) delete draft[key];
    }));
  };

  // ── Profile cell command string (params appended to the agent base command, #597) ──
  const updateProfileCellCommand = (agentId: string, letter: string, text: string) => {
    const key = profileCellKey(agentId, letter);
    setDraftDirty(true);
    setProfileCellText(key, text);
    const parsed = parseArgvText(text);
    setProfileCellErrors(key, parsed.error ?? "");
    const existing = profileCell(agentId, letter) ?? emptyProfileCell();
    setProfileCell(agentId, letter, { ...existing, command: text });
  };

  const displayedProfileCellCommand = (agentId: string, letter: string): string => {
    const key = profileCellKey(agentId, letter);
    if (Object.prototype.hasOwnProperty.call(profileCellText, key)) {
      return profileCellText[key] ?? "";
    }
    return profileCell(agentId, letter)?.command ?? "";
  };

  // ── Profile cell env rows ──
  const cellEnvRows = (agentId: string, letter: string): ProfileCellEnvRow[] => {
    const key = profileCellKey(agentId, letter);
    if (Object.prototype.hasOwnProperty.call(profileCellEnvRows, key)) {
      return profileCellEnvRows[key] ?? [];
    }
    const env = profileCell(agentId, letter)?.env ?? {};
    return Object.entries(env).map(([k, v]) => ({ key: k, value: v }));
  };

  const syncCellEnv = (agentId: string, letter: string, rows: ProfileCellEnvRow[]) => {
    const env: Record<string, string> = {};
    for (const row of rows) {
      const k = row.key.trim();
      if (k) env[k] = row.value;
    }
    const existing = profileCell(agentId, letter) ?? emptyProfileCell();
    setProfileCell(agentId, letter, { ...existing, env });
  };

  const setCellEnvRows = (agentId: string, letter: string, rows: ProfileCellEnvRow[]) => {
    const key = profileCellKey(agentId, letter);
    setDraftDirty(true);
    setProfileCellEnvRows(key, rows);
    syncCellEnv(agentId, letter, rows);
  };

  const addCellEnvRow = (agentId: string, letter: string) =>
    setCellEnvRows(agentId, letter, [...cellEnvRows(agentId, letter), { key: "", value: "" }]);

  const updateCellEnvRow = (
    agentId: string,
    letter: string,
    rowIndex: number,
    field: keyof ProfileCellEnvRow,
    value: string,
  ) => {
    const rows = cellEnvRows(agentId, letter).map((row, i) =>
      i === rowIndex ? { ...row, [field]: value } : row,
    );
    setCellEnvRows(agentId, letter, rows);
  };

  const removeCellEnvRow = (agentId: string, letter: string, rowIndex: number) =>
    setCellEnvRows(agentId, letter, cellEnvRows(agentId, letter).filter((_, i) => i !== rowIndex));

  // Display-only env-row validation reusing the agent-env validator.
  const cellEnvError = (agentId: string, letter: string): string | null =>
    validateEnvRows(
      cellEnvRows(agentId, letter).map((row) => ({
        key: row.key,
        value: row.value,
        source: "user" as const,
        enabled: true,
      })),
    );

  const addAgent = (preset?: Omit<AgentConfig, "id">) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const agent: AgentConfig = preset
      ? { id: newAgentId(), ...preset }
      : {
          id: newAgentId(),
          label: "",
          command: "",
          color: "#6366f1",
          envs: [],
          isolatedHome: false,
          instructionsFilename: "AGENTS.md",
        };
    setSettings("data", "agents", (prev) => [...prev, agent]);
    // A freshly added agent expands its inline editor so it is immediately
    // editable (the resting list stays collapsed for existing agents).
    setActiveAgentId(agent.id);
  };

  const removeAgent = (index: number) => {
    if (!settings.data) return;
    setDraftDirty(true);
    const removed = settings.data.agents[index];
    setSettings("data", "agents", (prev) => prev.filter((_, i) => i !== index));
    if (removed) {
      if (activeAgentId() === removed.id) setActiveAgentId(null);
      if (leftRailId() === removed.id) setLeftRailId(null);
      if (rightRailId() === removed.id) setRightRailId(null);
    }
  };

  // ── Telegram Bots ──
  const updateBot = (
    index: number,
    field: keyof TelegramBotConfig,
    value: string | number
  ) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "telegramBots", index, field as any, value as any);
  };

  const addBot = () => {
    if (!settings.data) return;
    setDraftDirty(true);
    const bot: TelegramBotConfig = {
      id: newAgentId(),
      label: "",
      token: "",
      chatId: 0,
      color: "#0088cc",
    };
    setSettings("data", "telegramBots", (prev) => [...(prev || []), bot]);
  };

  const removeBot = (index: number) => {
    if (!settings.data) return;
    setDraftDirty(true);
    setSettings("data", "telegramBots", (prev) => (prev || []).filter((_, i) => i !== index));
  };

  const handleTestBot = async (bot: TelegramBotConfig, index: number) => {
    setTestingBot(bot.id);
    setTestResult(null);
    try {
      const chatId = await TelegramAPI.sendTest(bot.token);
      updateBot(index, "chatId", chatId);
      setTestResult({ id: bot.id, ok: true });
    } catch (e: any) {
      setTestResult({ id: bot.id, ok: false, msg: e?.toString() });
    }
    setTestingBot(null);
  };

  const hasAgentByCommand = (command: string): boolean => {
    if (!settings.data) return false;
    return settings.data.agents.some((a) => a.command.startsWith(command));
  };

  // #769 Phase 2 — the catalog def a configured agent's command maps to IFF that
  // command ships a re-seedable default config folder. Gating is EXACT
  // executable-basename equality against the backend-derived reseedable set —
  // deliberately NOT `hasAgentByCommand`'s `.startsWith` (so `pi` matches only
  // `pi` not `pip`, and Cursor CLI `agent`/Hermes/Pi/custom get no button). The
  // set is empty until loaded and stays empty on fetch failure, so a failure is
  // fail-safe (no destructive button we cannot validate). Returns null → no button.
  const reseedableDefForCommand = (command: string): CodingAgentDefinition | null => {
    const basename = commandExecutableBasename(command).toLowerCase();
    if (!basename) return null;
    const inReseedableSet = codingAgentsStore
      .reseedableCommands()
      .some((c) => c.toLowerCase() === basename);
    if (!inReseedableSet) return null;
    const def = codingAgentsStore
      .catalog()
      .find((d) => commandExecutableBasename(d.command).toLowerCase() === basename);
    // A dest is required to name the affected folder in the confirm copy.
    return def && def.configSeed?.dest ? def : null;
  };

  const confirmReseed = async () => {
    const def = reseedTarget();
    if (!def) return;
    setReseeding(true);
    try {
      const result = await CodingAgentsAPI.reseedDefault(def.command);
      toastStore.success(
        result.backupPath
          ? `Default configuration restored (backup at ${result.backupPath})`
          : "Default configuration restored",
      );
      setReseedTarget(null);
    } catch (e) {
      // Non-destructive: the server-side re-check refused or the swap failed; the
      // prior master is intact.
      toastStore.error(`Could not re-seed ${def.label} default configuration: ${String(e)}`);
    } finally {
      setReseeding(false);
    }
  };

  const tokenHasUnclosedQuote = (token: string, quote: string): boolean =>
    (token.split(quote).length - 1) % 2 === 1;

  const advancePastConfigValue = (tokens: string[], start: number): number => {
    if (start >= tokens.length) return start;
    let index = start;
    let inSingle = false;
    let inDouble = false;
    while (index < tokens.length) {
      const token = tokens[index];
      if (tokenHasUnclosedQuote(token, "'")) inSingle = !inSingle;
      if (tokenHasUnclosedQuote(token, '"')) inDouble = !inDouble;
      index += 1;
      if (!inSingle && !inDouble) break;
    }
    return index;
  };

  const codexHasManualResume = (tokens: string[], codexIndex: number): boolean => {
    let index = codexIndex + 1;
    while (index < tokens.length) {
      const token = tokens[index].toLowerCase();
      if (token === "-c" || token === "--config") {
        index = advancePastConfigValue(tokens, index + 1);
        continue;
      }
      if (token === "resume" || token === "--last") return true;
      index += 1;
    }
    return false;
  };

  // ── Validation ──
  // Provider resume-flag rules, shared by agent commands and profile-cell
  // commands (#384 §2: enabled cell commands reject the same manual resume flags).
  const commandFlagError = (label: string, tokens: string[]): string | null => {
    const claudeIndex = tokens.findIndex((token) => executableTokenBasename(token) === "claude");
    if (
      claudeIndex >= 0 &&
      tokens
        .slice(claudeIndex + 1)
        .some((token) => token === "--continue" || token === "-c")
    ) {
      return `${label}: Claude commands must not include --continue or -c`;
    }
    const codexIndex = tokens.findIndex((token) => executableTokenBasename(token) === "codex");
    if (codexIndex >= 0 && codexHasManualResume(tokens, codexIndex)) {
      return `${label}: Codex commands must not include resume or --last; AgentsCommander injects codex resume --last automatically`;
    }
    return null;
  };

  const validateAgents = (): string | null => {
    if (!settings.data) return null;
    for (const agent of settings.data.agents) {
      const envError = validateEnvRows(agent.envs ?? []);
      if (envError) {
        return `Agent "${agent.label || "Unnamed"}": ${envError}`;
      }
      const parsedCommand = parseArgvText(agent.command);
      if (parsedCommand.error) {
        return `Agent "${agent.label || "Unnamed"}": ${parsedCommand.error}`;
      }
      const flagError = commandFlagError(`Agent "${agent.label || "Unnamed"}"`, parsedCommand.argv);
      if (flagError) return flagError;
    }
    // Live profile-cell command parse errors (red "invalid" badge) block save.
    for (const [key, error] of Object.entries(profileCellErrors)) {
      if (error) return `Profile cell ${key}: ${error}`;
    }
    // Enabled profile-cell command + env validation (#384 §2/§3).
    const byAgent = settings.data.codingAgentProfiles.profilesByAgent;
    for (const [agentId, cells] of Object.entries(byAgent)) {
      for (const [letter, cell] of Object.entries(cells)) {
        if (!cell.enabled) continue;
        const cellLabel = `Profile ${agentId}:${letter}`;
        const command = displayedProfileCellCommand(agentId, letter);
        if (command.trim()) {
          const parsed = parseArgvText(command);
          if (parsed.error) return `${cellLabel}: ${parsed.error}`;
          const flagError = commandFlagError(cellLabel, parsed.argv);
          if (flagError) return flagError;
        }
        const envError = cellEnvError(agentId, letter);
        if (envError) return `${cellLabel}: ${envError}`;
      }
    }
    return null;
  };

  const validateResources = (): string | null => {
    const data = settings.data;
    if (!data) return null;

    // #565: mirror the backend (validate_resource_settings) — floor at 1, NO
    // upper ceiling. The user sets this deliberately; the hard cap was replaced
    // by an adjacent latency warning (see below), not a maximum.
    if (
      !Number.isInteger(data.maxConcurrentAgentProcesses) ||
      data.maxConcurrentAgentProcesses < 1
    ) {
      return "Resources: max concurrent agent processes must be at least 1";
    }

    const warn = data.agentGroupWarnPrivateBytes;
    const groupKill = data.agentGroupKillPrivateBytes;
    const processKill = data.agentProcessKillPrivateBytes;
    if (warn <= 0 || groupKill <= 0 || processKill <= 0) {
      return "Resources: memory thresholds must be greater than 0 GiB";
    }
    if (warn > groupKill) {
      return "Resources: group warning threshold must be at or below group kill threshold";
    }

    return null;
  };

  // #552 Coordinator idle badge + auto-close thresholds. Enforced at Save (NOT
  // inline per keystroke — the numeric inputs use the non-clamping guard so they
  // don't fight the user mid-edit). yellow < red is required because the badge
  // color helper tests red first, so an inverted pair makes the yellow band
  // unreachable.
  const validateCoordinatorIdle = (): string | null => {
    const data = settings.data;
    if (!data) return null;

    const yellow = data.coordinatorIdleBadgeYellowMinutes;
    const red = data.coordinatorIdleBadgeRedMinutes;
    if (!Number.isInteger(yellow) || yellow < 1) {
      return "Coordinator idle: badge yellow threshold must be a whole number of at least 1 minute";
    }
    if (!Number.isInteger(red) || red < 1) {
      return "Coordinator idle: badge red threshold must be a whole number of at least 1 minute";
    }
    if (yellow >= red) {
      return "Coordinator idle: yellow threshold must be below the red threshold";
    }
    if (
      !Number.isInteger(data.coordinatorAutoCloseMinutes) ||
      data.coordinatorAutoCloseMinutes < 1
    ) {
      return "Coordinator idle: auto-close minutes must be a whole number of at least 1";
    }

    return null;
  };

  const validateApiServerSettings = (): string | null => {
    const data = settings.data;
    if (!data) return null;

    if (!data.apiServerBind.trim()) {
      return "API server: IP/address must not be empty";
    }
    if (
      !Number.isInteger(data.apiServerPort) ||
      data.apiServerPort < 1 ||
      data.apiServerPort > 65535
    ) {
      return "API server: port must be between 1 and 65535";
    }

    return null;
  };

  // #714 Lightweight pre-check mirroring the backend MVP parser (one Ctrl/Control
  // + one alphanumeric key). Backend stays the authority for OS registration.
  const validateScreenshotHotkey = (): string | null =>
    validateScreenshotHotkeySyntax(
      settings.data?.screenshotCaptureHotkey ?? "Ctrl+Q"
    );

  const currentValidationError = (): string | null =>
    validateAgents() ??
    validateResources() ??
    validateCoordinatorIdle() ??
    validateApiServerSettings() ??
    validateScreenshotHotkey();

  // ── Save ──
  const handleSave = async () => {
    if (!settings.data) return;
    const validationError = currentValidationError();
    if (validationError) {
      setSaveError(validationError);
      return;
    }
    setSaveError("");
    setSaving(true);
    try {
      const seedBeforeSave = modalSeed();
      const wasApiServerRunning = apiServerRunning();
      const nextSettings = await saveCurrentSettingsDraft();
      if (wasApiServerRunning && apiServerEndpointChanged(nextSettings, seedBeforeSave)) {
        await restartApiServerAfterEndpointSave(nextSettings);
      }
      // #158 — push soundsEnabled into sound.ts synchronously so the gate
      // updates before the settingsStore.refresh() roundtrip below resolves.
      // Without this, a beep emitted between this point and the next load()
      // would see the stale gate value.
      setSoundsEnabled(nextSettings.soundsEnabled ?? true);
      if (isTauri) {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().setAlwaysOnTop(nextSettings.sidebarAlwaysOnTop);
      }
      // Refresh settings store so mic button visibility updates
      settingsStore.refresh();
      // Refresh repos (project_paths may have changed)
      try {
        const allRepos = await ReposAPI.search("");
        sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
      } catch {}
      setSaving(false);
      closeSettings();
    } catch (err: unknown) {
      setSaveError(err instanceof Error ? err.message : String(err));
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") closeSettings();
  };

  document.addEventListener("keydown", handleKeyDown);
  onCleanup(() => {
    clearApiClientSecret();
    document.removeEventListener("keydown", handleKeyDown);
  });

  // ── Tab renderers ──

  const renderGeneralTab = () => (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Shell</div>
        <label class="settings-field">
          <span class="settings-label">Default Shell</span>
          <input
            class="settings-input"
            value={settings.data!.defaultShell}
            onInput={(e) => updateField("defaultShell", e.currentTarget.value)}
          />
        </label>
        <label class="settings-field">
          <span class="settings-label">Shell Arguments</span>
          <input
            class="settings-input"
            value={settings.data!.defaultShellArgs.join(" ")}
            onInput={(e) =>
              updateField(
                "defaultShellArgs",
                e.currentTarget.value.split(" ").filter(Boolean)
              )
            }
          />
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Window</div>
        <label class="settings-field">
          <span class="settings-label">App Theme</span>
          <select
            class="settings-input"
            value={settings.data!.sidebarStyle ?? "noir-minimal"}
            onChange={(e) => {
              updateField("sidebarStyle", e.currentTarget.value);
              document.documentElement.dataset.sidebarStyle = e.currentTarget.value;
            }}
          >
            <option value="noir-minimal">Noir Minimal</option>
            <option value="card-sections">Card Sections</option>
            <option value="command-center">Command Center</option>
            <option value="deep-space">Deep Space</option>
            <option value="arctic-ops">Arctic Ops</option>
            <option value="obsidian-mesh">Obsidian Mesh</option>
            <option value="neon-circuit">Neon Circuit</option>
          </select>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.restoreCoordinatorWakeState}
            onChange={(e) =>
              updateField("restoreCoordinatorWakeState", e.currentTarget.checked)
            }
          />
          <span>On start, wake coordinators that were awake when the app closed</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.sidebarAlwaysOnTop}
            onChange={(e) =>
              updateField("sidebarAlwaysOnTop", e.currentTarget.checked)
            }
          />
          <span>Sidebar always on top</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.raiseTerminalOnClick}
            onChange={(e) =>
              updateField("raiseTerminalOnClick", e.currentTarget.checked)
            }
          />
          <span>Raise terminal when clicking sidebar</span>
        </label>
        <label class="settings-field">
          <span class="settings-label">Screenshot hotkey</span>
          <input
            class="settings-input settings-input-sm"
            value={settings.data!.screenshotCaptureHotkey ?? "Ctrl+Q"}
            onInput={(e) =>
              updateField("screenshotCaptureHotkey", e.currentTarget.value)
            }
            data-ac-testid="settings.general.screenshotCaptureHotkey"
          />
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Coordinator idle</div>
        <label class="settings-field">
          <span class="settings-label">Badge turns yellow after (minutes)</span>
          <input
            class="settings-input settings-input-sm"
            type="number"
            min="1"
            step="1"
            value={settings.data!.coordinatorIdleBadgeYellowMinutes}
            onInput={(e) => {
              const value = parseInt(e.currentTarget.value, 10);
              if (!Number.isNaN(value)) {
                updateField("coordinatorIdleBadgeYellowMinutes", value);
              }
            }}
            data-ac-testid="settings.general.coordinatorIdleBadgeYellowMinutes"
            data-ac-role="spinbutton"
          />
        </label>
        <label class="settings-field">
          <span class="settings-label">Badge turns red after (minutes)</span>
          <input
            class="settings-input settings-input-sm"
            type="number"
            min="1"
            step="1"
            value={settings.data!.coordinatorIdleBadgeRedMinutes}
            onInput={(e) => {
              const value = parseInt(e.currentTarget.value, 10);
              if (!Number.isNaN(value)) {
                updateField("coordinatorIdleBadgeRedMinutes", value);
              }
            }}
            data-ac-testid="settings.general.coordinatorIdleBadgeRedMinutes"
            data-ac-role="spinbutton"
          />
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.coordinatorAutoCloseEnabled}
            onChange={(e) =>
              updateField("coordinatorAutoCloseEnabled", e.currentTarget.checked)
            }
            data-ac-testid="settings.general.coordinatorAutoCloseEnabled"
          />
          <span>Auto-close idle teams (terminate sessions to free resources)</span>
        </label>
        <label class="settings-field">
          <span class="settings-label">Auto-close after (minutes of total silence)</span>
          <input
            class="settings-input settings-input-sm"
            type="number"
            min="1"
            step="1"
            disabled={!settings.data!.coordinatorAutoCloseEnabled}
            value={settings.data!.coordinatorAutoCloseMinutes}
            onInput={(e) => {
              const value = parseInt(e.currentTarget.value, 10);
              if (!Number.isNaN(value)) {
                updateField("coordinatorAutoCloseMinutes", value);
              }
            }}
            data-ac-testid="settings.general.coordinatorAutoCloseMinutes"
            data-ac-role="spinbutton"
          />
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.coordinatorAutoCloseSkipTelegramAssigned}
            disabled={!settings.data!.coordinatorAutoCloseEnabled}
            onChange={(e) =>
              updateField(
                "coordinatorAutoCloseSkipTelegramAssigned",
                e.currentTarget.checked,
              )
            }
            data-ac-testid="settings.general.coordinatorAutoCloseSkipTelegramAssigned"
          />
          <span>Skip Telegram-assigned sessions during auto-close</span>
        </label>
        <div class="settings-hint">
          The idle badge shows minutes since your last message to a coordinator
          (green below yellow, yellow up to red, red beyond). Auto-close
          terminates a team's sessions after the whole team is silent for the
          configured minutes; the coordinator stays as a dormant row you can
          reopen by messaging it.
        </div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.coordinatorCascadeCloseEnabled}
            onChange={(e) =>
              updateField("coordinatorCascadeCloseEnabled", e.currentTarget.checked)
            }
            data-ac-testid="settings.general.coordinatorCascadeCloseEnabled"
          />
          <span>Always close team members when manually closing Coordinator</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.npmUpdateNotificationsEnabled}
            onChange={(e) =>
              updateField("npmUpdateNotificationsEnabled", e.currentTarget.checked)
            }
            data-ac-testid="settings.general.npmUpdateNotificationsEnabled"
          />
          <span>Notify me when a new version is available</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.autoSelfClearEnabled}
            onChange={(e) =>
              updateField("autoSelfClearEnabled", e.currentTarget.checked)
            }
            data-ac-testid="settings.general.autoSelfClearEnabled"
          />
          <span>
            Auto-clear and hand off context after 3 closed topics (on for
            coordinators and Root; other agents opt in per agent)
          </span>
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Web Remote Access</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.webServerEnabled}
            onChange={(e) =>
              updateField("webServerEnabled", e.currentTarget.checked)
            }
          />
          <span>Enable web server</span>
        </label>
        <Show when={settings.data!.webServerEnabled}>
          <div style="display: flex; gap: 6px; margin-top: 6px; align-items: center;">
            <button
              class="settings-add-btn"
              onClick={async () => {
                try {
                  const running = await SettingsAPI.getWebServerStatus();
                  if (running) {
                    await SettingsAPI.stopWebServer();
                    setWebServerRunning(false);
                  } else {
                    await SettingsAPI.startWebServer();
                    setWebServerRunning(true);
                  }
                } catch (err) {
                  console.error("Web server toggle failed:", err);
                }
              }}
            >
              {webServerRunning() ? "Stop Server" : "Start Server"}
            </button>
            <button
              class="settings-add-btn"
              disabled={!webServerRunning()}
              style={!webServerRunning() ? "opacity: 0.4; cursor: default;" : ""}
              onClick={() => {
                SettingsAPI.openWebRemote().catch((err) =>
                  console.error("Failed to open web remote:", err)
                );
              }}
            >
              Open in Browser
            </button>
            <span style={`font-size: 11px; opacity: 0.6;`}>
              {webServerRunning() ? "● Running" : "○ Stopped"}
            </span>
          </div>
        </Show>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Control Plane API</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={apiServerChecked()}
            disabled={apiServerBusy()}
            onChange={(e) => {
              void handleApiServerToggle(e.currentTarget.checked);
            }}
            data-ac-testid="settings.general.apiServerEnabled"
            data-ac-role="checkbox"
            data-ac-state={
              apiServerBusy()
                ? "updating"
                : apiServerChecked()
                  ? "checked"
                  : "unchecked"
            }
          />
          <span>Enable API server</span>
        </label>
        <label class="settings-field">
          <span class="settings-label">IP/address</span>
          <input
            class="settings-input"
            value={settings.data!.apiServerBind}
            onInput={(e) => updateField("apiServerBind", e.currentTarget.value)}
            placeholder="127.0.0.1"
            data-ac-testid="settings.general.apiServerBind"
            data-ac-role="textbox"
          />
        </label>
        <label class="settings-field">
          <span class="settings-label">Port</span>
          <input
            class="settings-input settings-input-sm"
            type="number"
            min="1"
            max="65535"
            step="1"
            value={settings.data!.apiServerPort}
            onInput={(e) => {
              const raw = e.currentTarget.value.trim();
              const value = raw === "" ? 0 : Number(e.currentTarget.value);
              updateField(
                "apiServerPort",
                Number.isFinite(value) ? Math.trunc(value) : 0,
              );
            }}
            data-ac-testid="settings.general.apiServerPort"
            data-ac-role="spinbutton"
          />
        </label>
        <Show when={apiServerBindWarning()}>
          <div
            class="settings-hint settings-hint-warning"
            data-ac-testid="settings.general.apiServerBind.warning"
            data-ac-role="status"
          >
            Non-loopback binds can expose the API beyond this machine. Use firewall
            rules; API client tokens are the trust boundary.
          </div>
        </Show>
        <div
          class="settings-hint"
          data-ac-testid="settings.general.apiServerStatus"
          data-ac-role="status"
          data-ac-state={apiServerBusy() ? "updating" : apiServerRunning() ? "running" : "stopped"}
        >
          {apiServerBusy()
            ? "Updating..."
            : apiServerRunning()
              ? `Running on ${apiServerStatusAddress()}`
              : "Stopped"}
        </div>
        <div
          class="settings-api-client-mint"
          data-ac-testid="settings.apiClientMint.surface"
          data-ac-role="region"
        >
          <div class="settings-subsection-title">API client credential</div>
          <label class="settings-field">
            <span class="settings-label">Replica root</span>
            <select
              class="settings-input"
              value={apiClientMintRoot()}
              disabled={apiClientRootOptions().length === 0 || apiClientMinting()}
              onChange={(e) => {
                setApiClientMintError("");
                setApiClientMintRoot(e.currentTarget.value);
              }}
              data-ac-testid="settings.apiClientMint.root"
              data-ac-role="combobox"
              data-ac-state={apiClientRootOptions().length === 0 ? "empty" : "ready"}
            >
              <For each={apiClientRootOptions()}>
                {(option) => (
                  <option value={option.path}>
                    {option.label}
                  </option>
                )}
              </For>
            </select>
          </label>
          <Show when={apiClientRootOptions().length === 0}>
            <div
              class="settings-hint"
              data-ac-testid="settings.apiClientMint.noRoots"
              data-ac-role="status"
            >
              Load a project with workgroup replicas before minting an API client.
            </div>
          </Show>
          <div class="settings-field">
            <span class="settings-label">Scopes</span>
            <div class="settings-api-client-scopes">
              <For each={API_CLIENT_SCOPE_OPTIONS}>
                {(scope) => (
                  <label class="settings-checkbox-field settings-api-client-scope">
                    <input
                      type="checkbox"
                      class="settings-checkbox"
                      checked={apiClientMintScopes().includes(scope.value)}
                      disabled={apiClientMinting()}
                      onChange={(e) => updateApiClientScope(scope.value, e.currentTarget.checked)}
                      data-ac-testid={`settings.apiClientMint.scope.${scope.value}`}
                      data-ac-role="checkbox"
                      data-ac-state={
                        apiClientMintScopes().includes(scope.value) ? "checked" : "unchecked"
                      }
                    />
                    <span>{scope.label}</span>
                  </label>
                )}
              </For>
            </div>
          </div>
          <label class="settings-field">
            <span class="settings-label">Label</span>
            <input
              class="settings-input"
              value={apiClientMintLabel()}
              disabled={apiClientMinting()}
              onInput={(e) => {
                setApiClientMintError("");
                setApiClientMintLabel(e.currentTarget.value);
              }}
              placeholder="optional audit label"
              data-ac-testid="settings.apiClientMint.label"
              data-ac-role="textbox"
            />
          </label>
          <label class="settings-field">
            <span class="settings-label">Expiry</span>
            <select
              class="settings-input"
              value={apiClientMintExpiry()}
              disabled={apiClientMinting()}
              onChange={(e) => {
                setApiClientMintError("");
                setApiClientMintExpiry(e.currentTarget.value as ApiClientExpiryOption);
              }}
              data-ac-testid="settings.apiClientMint.expiry"
              data-ac-role="combobox"
            >
              <For each={API_CLIENT_EXPIRY_OPTIONS}>
                {(option) => <option value={option.value}>{option.label}</option>}
              </For>
            </select>
          </label>
          <button
            class="settings-add-btn settings-api-client-mint-btn"
            onClick={() => void handleMintApiClient()}
            disabled={
              apiClientMinting() ||
              apiClientRootOptions().length === 0 ||
              !selectedApiClientScopesValid()
            }
            data-ac-testid="settings.apiClientMint.submit"
            data-ac-role="button"
            data-ac-state={apiClientMinting() ? "minting" : "ready"}
          >
            {apiClientMinting() ? "Minting..." : "Mint credential"}
          </button>
          <Show when={apiClientMintError()}>
            <div
              class="settings-api-client-error"
              role="alert"
              data-ac-testid="settings.apiClientMint.error"
              data-ac-role="alert"
            >
              {apiClientMintError()}
            </div>
          </Show>
          <Show when={apiClientMintResult()}>
            {(result) => (
              <div
                class="settings-api-client-secret"
                data-ac-testid="settings.apiClientMint.result"
                data-ac-role="surface"
              >
                <div
                  class="settings-api-client-warning"
                  data-ac-testid="settings.apiClientMint.warning"
                  data-ac-role="status"
                >
                  Shown once. Store this token now; it cannot be recovered later.
                </div>
                <label class="settings-field">
                  <span class="settings-label">Token</span>
                  <div class="settings-api-client-token-row">
                    <input
                      class="settings-input settings-api-client-token"
                      readOnly
                      value={result().token}
                      data-ac-testid="settings.apiClientMint.token"
                      data-ac-role="textbox"
                    />
                    <button
                      class="settings-add-btn"
                      onClick={() => void copyApiClientToken()}
                      data-ac-testid="settings.apiClientMint.copy"
                      data-ac-role="button"
                      data-ac-state={apiClientSecretCopied() ? "copied" : "ready"}
                    >
                      {apiClientSecretCopied() ? "Copied" : "Copy"}
                    </button>
                  </div>
                </label>
                <dl class="settings-api-client-result-grid">
                  <div>
                    <dt>Client ID</dt>
                    <dd data-ac-testid="settings.apiClientMint.clientId">{result().clientId}</dd>
                  </div>
                  <div>
                    <dt>Bound FQN</dt>
                    <dd data-ac-testid="settings.apiClientMint.boundFqn">{result().boundFqn}</dd>
                  </div>
                  <div>
                    <dt>Expires</dt>
                    <dd data-ac-testid="settings.apiClientMint.expiresAt">
                      {result().expiresAt ?? "No expiry"}
                    </dd>
                  </div>
                </dl>
                <button
                  class="settings-add-btn"
                  onClick={clearApiClientSecret}
                  data-ac-testid="settings.apiClientMint.clear"
                  data-ac-role="button"
                >
                  Clear secret
                </button>
              </div>
            )}
          </Show>
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Notifications</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.soundsEnabled}
            onChange={(e) =>
              updateField("soundsEnabled", e.currentTarget.checked)
            }
          />
          <span>Enable app sounds (master switch)</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.teamIdleBeepEnabled}
            disabled={!settings.data!.soundsEnabled}
            onChange={(e) =>
              updateField("teamIdleBeepEnabled", e.currentTarget.checked)
            }
          />
          <span>Beep when a team finishes working (all agents idle)</span>
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Logging</div>
        <label class="settings-field">
          <span class="settings-label">Log level</span>
          <select
            class="settings-input"
            value={settings.data!.logLevel ?? "info"}
            onChange={(e) =>
              updateField("logLevel", e.currentTarget.value as LogLevel)
            }
            data-ac-testid="settings.general.logLevel"
            data-ac-role="combobox"
          >
            <option value="error">Error</option>
            <option value="warn">Warn</option>
            <option value="info">Info</option>
            <option value="debug">Debug</option>
            <option value="trace">Trace</option>
          </select>
        </label>
        <div class="settings-hint">
          Controls AgentsCommander log verbosity (applies immediately, no
          restart). Higher levels are noisier. The RUST_LOG env var, if set,
          overrides this until restart.
        </div>
      </div>

    </>
  );

  const renderAgentEnvEditor = (agent: AgentConfig, agentIndex: number) => {
    const codex = () => isCodexAgent(agent);
    const hasUserCodexHome = () => hasEnabledEnvKey(agent.envs ?? [], "CODEX_HOME");
    return (
      <div
        class="settings-env-editor"
        data-ac-testid={`settings.agentRow.${agentIndex}.env`}
        data-ac-role="surface"
      >
        <div class="settings-subsection-title">Environment</div>
        <Show when={(agent.envs ?? []).length > 0} fallback={
          <div
            class="settings-empty-note"
            data-ac-testid={`settings.agentRow.${agentIndex}.env.empty`}
            data-ac-role="status"
          >
            No environment rows configured.
          </div>
        }>
          <For each={agent.envs ?? []}>
            {(row, rowIndex) => {
              const readOnly = () => row.source === "system";
              return (
                <div
                  class="settings-env-row"
                  data-ac-testid={`settings.agentRow.${agentIndex}.envRow.${rowIndex()}`}
                  data-ac-role="row"
                  data-ac-state={readOnly() ? "managed" : row.enabled ? "enabled" : "disabled"}
                  data-ac-env-source={row.source}
                >
                  <input
                    class="settings-input settings-env-key"
                    value={row.key}
                    disabled={readOnly()}
                    onInput={(e) => updateAgentEnv(agentIndex, rowIndex(), "key", e.currentTarget.value)}
                    placeholder="KEY"
                    data-ac-testid={`settings.agentRow.${agentIndex}.envRow.${rowIndex()}.key`}
                    data-ac-role="textbox"
                    data-ac-state={readOnly() ? "disabled" : "editable"}
                  />
                  <input
                    class="settings-input settings-env-value"
                    type="password"
                    value={row.value}
                    disabled={readOnly()}
                    onInput={(e) => updateAgentEnv(agentIndex, rowIndex(), "value", e.currentTarget.value)}
                    placeholder="value"
                    data-ac-testid={`settings.agentRow.${agentIndex}.envRow.${rowIndex()}.value`}
                    data-ac-role="textbox"
                    data-ac-state={readOnly() ? "disabled" : "editable"}
                  />
                  <label class="settings-env-toggle" title="Enable row">
                    <input
                      type="checkbox"
                      class="settings-checkbox"
                      checked={row.enabled}
                      disabled={readOnly()}
                      onChange={(e) => updateAgentEnv(agentIndex, rowIndex(), "enabled", e.currentTarget.checked)}
                      data-ac-testid={`settings.agentRow.${agentIndex}.envRow.${rowIndex()}.enabled`}
                      data-ac-role="checkbox"
                      data-ac-state={readOnly() ? "disabled" : row.enabled ? "checked" : "unchecked"}
                    />
                  </label>
                  <span
                    class={`settings-env-source ${row.source === "system" ? "generated" : ""}`}
                    data-ac-testid={`settings.agentRow.${agentIndex}.envRow.${rowIndex()}.source`}
                    data-ac-role="status"
                    data-ac-env-source={row.source}
                  >
                    {row.source}
                  </span>
                  <button
                    class="settings-env-delete"
                    disabled={readOnly()}
                    onClick={() => removeAgentEnv(agentIndex, rowIndex())}
                    title={readOnly() ? "Managed by AgentsCommander" : "Delete environment row"}
                    data-ac-testid={`settings.agentRow.${agentIndex}.envRow.${rowIndex()}.delete`}
                    data-ac-role="button"
                    data-ac-state={readOnly() ? "disabled" : "enabled"}
                  >
                    &#x2715;
                  </button>
                </div>
              );
            }}
          </For>
        </Show>
        <button
          class="settings-add-btn settings-env-add"
          onClick={() => addAgentEnv(agentIndex)}
          data-ac-testid={`settings.agentRow.${agentIndex}.env.add`}
          data-ac-role="button"
        >
          + Environment Row
        </button>
        <Show when={codex()}>
          <label class="settings-checkbox-field">
            <input
              type="checkbox"
              class="settings-checkbox"
              checked={agent.isolatedHome}
              onChange={(e) =>
                updateAgent(agentIndex, "isolatedHome", e.currentTarget.checked)
              }
              data-ac-testid={`settings.agentRow.${agentIndex}.codexHomeIsolation`}
              data-ac-role="checkbox"
              data-ac-state={agent.isolatedHome ? "checked" : "unchecked"}
            />
            <span>Isolate CODEX_HOME for this Codex agent</span>
          </label>
          <Show when={agent.isolatedHome}>
            <div
              class="settings-codex-home-preview warning"
              data-ac-testid={`settings.agentRow.${agentIndex}.codexHomeIsolation.preview`}
              data-ac-role="status"
              data-ac-state="isolated"
            >
              <span class="settings-env-source generated">system</span>
              Generated CODEX_HOME will be used for launches from this agent.
              <Show when={hasUserCodexHome()}>
                <span> Saved CODEX_HOME rows are kept but overridden while isolation is on.</span>
              </Show>
            </div>
          </Show>
          <Show when={!agent.isolatedHome && hasUserCodexHome()}>
            <div
              class="settings-codex-home-preview"
              data-ac-testid={`settings.agentRow.${agentIndex}.codexHomeIsolation.preview`}
              data-ac-role="status"
              data-ac-state="user"
            >
              User CODEX_HOME is active for this Codex agent. Values remain masked in Settings.
            </div>
          </Show>
        </Show>
        <div class="settings-hint">
          Env values are stored as plaintext local settings and are masked here by default.
        </div>
      </div>
    );
  };

  // Left-panel agent row: compact prototype-style header (dot, name, command
  // basename, color swatch+hex, rail pill, delete) with the full agent config
  // editor (#384: label/command/color/flags/env/CODEX_HOME isolation) collapsed
  // behind the chevron. Collapsed by default — the resting screen reads as the
  // prototype; the editor is secondary, behind the expand. #895: the head itself
  // is the rail picker, so the editor expand lives on the chevron button.
  const renderAgentRow = (agent: AgentConfig, index: () => number) => {
    const i = () => index();
    const expanded = () => activeAgentId() === agent.id;
    const pill = () => railPillFor(agent.id);
    const railAction = () => railActionFor(agent.id, i());
    const railSide = () => railSideForIndex(i());
    const railTitle = () => {
      switch (railAction()) {
        case "none":
          return `Already on the ${railSide()} comparison rail`;
        case "swap":
          return `Swap this agent onto the ${railSide()} comparison rail`;
        default:
          return `Show this agent in the ${railSide()} comparison rail`;
      }
    };
    const agentBackendKind = () => agent.backend?.kind ?? "localProcess";
    const containerImageMissing = () => !agent.backend?.image?.trim();
    const containerBindWarning = () => isContainerLoopbackBind(settings.data?.apiServerBind);
    return (
      <div
        class="settings-agent-row"
        classList={{ expanded: expanded() }}
        style={{ "--agent-color": agent.color }}
        data-ac-testid={`settings.agentRow.${i()}`}
        data-ac-role="group"
        data-ac-agent-id={agent.id}
        data-ac-rail={pill()}
      >
        <div
          class="settings-agent-row-head"
          role="button"
          tabindex={0}
          title={railTitle()}
          aria-disabled={railAction() === "none"}
          onClick={() => selectAgentRail(agent.id, i())}
          onKeyDown={(e) => {
            // The delete and chevron buttons live inside this head, and their
            // keydown bubbles here. Only act on keys aimed at the head itself,
            // or Enter on the trash icon would also reassign a rail.
            if (e.target !== e.currentTarget) return;
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              selectAgentRail(agent.id, i());
            }
          }}
          data-ac-testid={`settings.agentRow.${i()}.select`}
          data-ac-role="button"
          data-ac-state={pill()}
        >
          <span class="settings-agent-dot" style={{ background: agent.color }} />
          <div class="settings-agent-row-meta">
            <div class="settings-agent-row-name">{agent.label || "New Agent"}</div>
            <div class="settings-agent-row-cmd">
              {commandExecutableBasename(agent.command) || agent.command || "—"}
            </div>
            <div class="settings-agent-row-colorline">
              <span class="settings-agent-swatch" style={{ background: agent.color }} />
              <span class="settings-agent-color-hex">{agent.color}</span>
              <span class={`settings-rail-pill ${pill()}`} data-ac-rail={pill()}>
                {pill()}
              </span>
            </div>
          </div>
          <div class="settings-agent-row-actions">
            {/* #526: the rail indicator is shown once, on the color line. #895:
                the head click assigns the rail, so no "Use" button lives here —
                and no "Remove" either. A per-row Remove widened this column on
                exactly one row, squeezing the meta column until the rail pill
                wrapped to a second line: that row rendered 15px taller and the
                tall row moved on every click. Clearing the comparison now lives
                in the right rail's own header, next to the thing it clears. */}
            <button
              class="settings-agent-row-delete"
              onClick={(e) => {
                // Without this the head's click handler would also fire and
                // assign a rail on the way out.
                e.stopPropagation();
                removeAgent(i());
              }}
              title="Delete agent"
              aria-label={`Delete ${agent.label || "agent"}`}
              data-ac-testid={`settings.agentRow.${i()}.remove`}
              data-ac-role="button"
            >
              <TrashIcon class="settings-agent-row-delete-icon" />
            </button>
            <button
              class="settings-agent-chevron"
              onClick={(e) => {
                e.stopPropagation();
                toggleAgentEditor(agent.id);
              }}
              aria-expanded={expanded()}
              aria-label={expanded() ? "Collapse agent settings" : "Expand agent settings"}
              title={expanded() ? "Collapse agent settings" : "Expand agent settings"}
              data-ac-testid={`settings.agentRow.${i()}.toggle`}
              data-ac-role="button"
              data-ac-state={expanded() ? "expanded" : "collapsed"}
            >
              {expanded() ? "▾" : "▸"}
            </button>
          </div>
        </div>

        <Show when={expanded()}>
          <div class="settings-agent-editor" data-ac-testid={`settings.agentRow.${i()}.editor`}>
            <label class="settings-field">
              <span class="settings-label">Label</span>
              <input
                class="settings-input"
                value={agent.label}
                onInput={(e) => updateAgent(i(), "label", e.currentTarget.value)}
                placeholder="My Agent"
                data-ac-testid={`settings.agentRow.${i()}.label`}
                data-ac-role="textbox"
              />
            </label>
            <label class="settings-field">
              <span class="settings-label">Command</span>
              <input
                class="settings-input"
                value={agent.command}
                onInput={(e) => updateAgent(i(), "command", e.currentTarget.value)}
                placeholder="agent-cli"
                data-ac-testid={`settings.agentRow.${i()}.command`}
                data-ac-role="textbox"
              />
            </label>
            <label class="settings-field">
              <span class="settings-label">Runtime</span>
              <select
                class="settings-input"
                value={agentBackendKind()}
                onChange={(e) =>
                  setAgentBackendKind(i(), e.currentTarget.value as SessionBackendKind)
                }
                data-ac-testid={`settings.agentRow.${i()}.runtimeKind`}
                data-ac-role="combobox"
                data-ac-state={agentBackendKind()}
              >
                <option value="localProcess">Local (default)</option>
                <option value="containerTransport">Container (Docker)</option>
              </select>
            </label>
            <Show when={agentBackendKind() === "containerTransport"}>
              <label class="settings-field">
                <span class="settings-label">Docker image</span>
                <input
                  class="settings-input"
                  value={agent.backend?.image ?? ""}
                  onInput={(e) => setAgentBackendImage(i(), e.currentTarget.value)}
                  placeholder="Required unless AGENTSCOMMANDER_CONTAINER_IMAGE is set"
                  data-ac-testid={`settings.agentRow.${i()}.containerImage`}
                  data-ac-role="textbox"
                />
              </label>
              <div
                class="settings-hint"
                data-ac-testid={`settings.agentRow.${i()}.containerHint.image`}
              >
                Set a Docker image here, for example {CONTAINER_IMAGE_EXAMPLE}. Leave
                blank only if the AgentsCommander process has
                AGENTSCOMMANDER_CONTAINER_IMAGE set; there is no built-in image fallback.
              </div>
              <div
                class="settings-hint settings-hint-warning"
                data-ac-testid={`settings.agentRow.${i()}.containerHint.env`}
                data-ac-role="status"
              >
                Container sessions read CLAUDE_CONFIG_DIR from inside the container.
                Set it to %AC_REPLICA_ROOT%\.claude, or conversation state will not
                persist and auto-resume is skipped.
              </div>
              <Show when={containerImageMissing()}>
                <div
                  class="settings-hint settings-hint-warning"
                  data-ac-testid={`settings.agentRow.${i()}.containerWarning.image`}
                  data-ac-role="status"
                >
                  Saving this blank relies on AGENTSCOMMANDER_CONTAINER_IMAGE; launch
                  will fail if it is not set.
                </div>
              </Show>
              <Show when={!apiServerChecked()}>
                <div
                  class="settings-hint settings-hint-warning"
                  data-ac-testid={`settings.agentRow.${i()}.containerWarning.apiServer`}
                  data-ac-role="status"
                >
                  Container runtime needs the control-plane API server enabled.
                </div>
              </Show>
              <Show when={containerBindWarning()}>
                <div
                  class="settings-hint settings-hint-warning"
                  data-ac-testid={`settings.agentRow.${i()}.containerWarning.bind`}
                  data-ac-role="status"
                >
                  Docker cannot reach a loopback API bind; set a Docker-reachable bind
                  such as 0.0.0.0 with firewall rules.
                </div>
              </Show>
            </Show>
            <label class="settings-field">
              <span class="settings-label">Instructions file</span>
              <input
                class="settings-input"
                value={agent.instructionsFilename ?? ""}
                onInput={(e) =>
                  updateAgent(i(), "instructionsFilename", e.currentTarget.value)
                }
                placeholder={defaultInstructionsFilename(agent.command)}
                data-ac-testid={`settings.agentRow.${i()}.instructionsFilename`}
                data-ac-role="textbox"
              />
            </label>
            <div class="settings-hint">
              Filename AC writes into the agent root at launch (its AC context +
              Role.md). Leave blank to use the default shown.
            </div>
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                class="settings-checkbox"
                checked={!!agent.configSeed?.enabled}
                onChange={(e) =>
                  setAgentConfigSeedEnabled(i(), e.currentTarget.checked)
                }
                data-ac-testid={`settings.agentRow.${i()}.configSeedEnabled`}
                data-ac-role="checkbox"
                data-ac-state={agent.configSeed?.enabled ? "checked" : "unchecked"}
              />
              <span>Seed a config folder into the replica at spawn</span>
            </label>
            <Show when={agent.configSeed?.enabled}>
              <label class="settings-field">
                <span class="settings-label">Config folder</span>
                <input
                  class="settings-input"
                  value={agent.configSeed?.dest ?? ""}
                  onInput={(e) => setAgentConfigSeedDest(i(), e.currentTarget.value)}
                  placeholder=".claude"
                  data-ac-testid={`settings.agentRow.${i()}.configSeedDest`}
                  data-ac-role="textbox"
                />
              </label>
              <div class="settings-hint">
                {"AC seeds this folder from the first template present: " +
                  "<workspace>/default_profile_<letter><dest>, " +
                  "<workspace>/default<dest>, " +
                  "<matrix>/default_profile_<letter><dest>, then " +
                  "<matrix>/default<dest>. " +
                  "It is overwritten on every spawn. Leave the folder blank to disable."}
              </div>
            </Show>
            <label class="settings-field">
              <span class="settings-label">Color</span>
              <div class="settings-color-row">
                <input
                  type="color"
                  class="settings-color-picker"
                  value={agent.color}
                  onInput={(e) => updateAgent(i(), "color", e.currentTarget.value)}
                  data-ac-testid={`settings.agentRow.${i()}.colorPicker`}
                  data-ac-role="input"
                />
                <input
                  class="settings-input settings-input-sm"
                  value={agent.color}
                  onInput={(e) => updateAgent(i(), "color", e.currentTarget.value)}
                  data-ac-testid={`settings.agentRow.${i()}.color`}
                  data-ac-role="textbox"
                />
              </div>
            </label>
            {renderAgentEnvEditor(agent, i())}
            {/* #769 Phase 2 — re-seed this tool's default config folder to the
                shipped default. Only for built-ins that ship a re-seedable
                master (claude/codex/opencode today); gate is exact-basename. */}
            <Show when={reseedableDefForCommand(agent.command)}>
              {(def) => (
                <div class="settings-field">
                  <button
                    class="settings-row-btn"
                    onClick={() => setReseedTarget(def())}
                    data-ac-testid={`settings.agent.reseedDefault.${def().key}`}
                    data-ac-role="button"
                    title={`Restore the shipped default ${def().configSeed?.dest} config folder for new ${def().label} sessions`}
                  >
                    Re-seed default configuration
                  </button>
                </div>
              )}
            </Show>
          </div>
        </Show>
      </div>
    );
  };

  // #769 — quick-add buttons are driven by the backend-owned catalog
  // (`codingAgentsStore`) instead of a hand-maintained button per agent. Order,
  // per-command `hasAgentByCommand` guards, testids, and `addAgent` are preserved
  // verbatim; `+ Custom Agent` stays hardcoded last (it is not a catalog entry).
  const renderAgentPresets = () => (
    <div class="settings-agent-actions">
      <For each={codingAgentsStore.catalog()}>
        {(def) => (
          <button
            class="settings-preset-btn"
            onClick={() => addAgent(definitionToSeed(def))}
            disabled={hasAgentByCommand(def.command)}
            data-ac-testid={`settings.agentPreset.${def.key}`}
            data-ac-role="button"
            data-ac-state={hasAgentByCommand(def.command) ? "disabled" : "available"}
          >
            <span class="settings-color-dot" style={{ background: def.color }} />
            + {def.label}
          </button>
        )}
      </For>
      <button
        class="settings-add-btn"
        onClick={() => addAgent()}
        data-ac-testid="settings.agent.addCustom"
        data-ac-role="button"
      >
        + Custom Agent
      </button>
    </div>
  );

  const renderResourcesTab = () => (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Resource Monitor</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.resourceMonitorEnabled}
            onChange={(e) =>
              updateField("resourceMonitorEnabled", e.currentTarget.checked)
            }
            data-ac-testid="settings.resources.enabled"
            data-ac-role="checkbox"
          />
          <span>Enable resource monitor and watchdog</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.resourceKeepLastSnapshot}
            onChange={(e) =>
              updateField("resourceKeepLastSnapshot", e.currentTarget.checked)
            }
            data-ac-testid="settings.resources.keepLastSnapshot"
            data-ac-role="checkbox"
          />
          <span>Keep last successful snapshot when polling fails</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.resourceBackoffPolling}
            onChange={(e) =>
              updateField("resourceBackoffPolling", e.currentTarget.checked)
            }
            data-ac-testid="settings.resources.backoffPolling"
            data-ac-role="checkbox"
          />
          <span>Back off polling when no agent groups are active</span>
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Watchdog</div>
        <label class="settings-field">
          <span class="settings-label">Action</span>
          <select
            class="settings-input"
            value={settings.data!.resourceWatchdogAction}
            onChange={(e) =>
              updateField(
                "resourceWatchdogAction",
                e.currentTarget.value as AppSettings["resourceWatchdogAction"]
              )
            }
            data-ac-testid="settings.resources.watchdogAction"
            data-ac-role="combobox"
          >
            <option value="warn">Warn only</option>
            <option value="killGroup">Kill group</option>
          </select>
        </label>
        <label class="settings-field">
          <span class="settings-label">Max Concurrent Agent Processes</span>
          <input
            class="settings-input settings-input-sm"
            type="number"
            min="1"
            step="1"
            value={settings.data!.maxConcurrentAgentProcesses}
            onInput={(e) => {
              const value = parseInt(e.currentTarget.value, 10);
              if (!Number.isNaN(value)) {
                updateField("maxConcurrentAgentProcesses", value);
              }
            }}
            data-ac-testid="settings.resources.maxConcurrentAgentProcesses"
            data-ac-role="spinbutton"
          />
        </label>
        <div
          class="settings-hint settings-hint-warning"
          data-ac-testid="settings.resources.maxConcurrentAgentProcesses.warning"
          data-ac-role="status"
        >
          No upper limit (default 32). Higher values let more agent processes run
          at once, but can worsen stop / restart / assign latency — per-group
          cleanup on the destroy path gets more expensive as the cap grows.
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Memory Thresholds</div>
        <div class="settings-resource-grid">
          <label class="settings-field">
            <span class="settings-label">Group Warn GiB</span>
            <input
              class="settings-input settings-input-sm"
              type="number"
              min="0.1"
              step="0.1"
              value={bytesToGiBInput(settings.data!.agentGroupWarnPrivateBytes)}
              onInput={(e) => {
                const bytes = giBInputToBytes(e.currentTarget.value);
                if (Number.isFinite(bytes)) {
                  updateField("agentGroupWarnPrivateBytes", bytes);
                }
              }}
              data-ac-testid="settings.resources.groupWarnGiB"
              data-ac-role="spinbutton"
            />
          </label>
          <label class="settings-field">
            <span class="settings-label">Group Kill GiB</span>
            <input
              class="settings-input settings-input-sm"
              type="number"
              min="0.1"
              step="0.1"
              value={bytesToGiBInput(settings.data!.agentGroupKillPrivateBytes)}
              onInput={(e) => {
                const bytes = giBInputToBytes(e.currentTarget.value);
                if (Number.isFinite(bytes)) {
                  updateField("agentGroupKillPrivateBytes", bytes);
                }
              }}
              data-ac-testid="settings.resources.groupKillGiB"
              data-ac-role="spinbutton"
            />
          </label>
          <label class="settings-field">
            <span class="settings-label">Process Kill GiB</span>
            <input
              class="settings-input settings-input-sm"
              type="number"
              min="0.1"
              step="0.1"
              value={bytesToGiBInput(settings.data!.agentProcessKillPrivateBytes)}
              onInput={(e) => {
                const bytes = giBInputToBytes(e.currentTarget.value);
                if (Number.isFinite(bytes)) {
                  updateField("agentProcessKillPrivateBytes", bytes);
                }
              }}
              data-ac-testid="settings.resources.processKillGiB"
              data-ac-role="spinbutton"
            />
          </label>
        </div>
      </div>
    </>
  );

  const BADGE_LABEL: Record<ProfileBadge, string> = {
    match: "MATCH",
    configured: "CONFIGURED",
    fallback: "FALLBACK",
    missing: "MISSING",
    invalid: "invalid",
  };

  // The cell command holds the params appended to the agent base command (#597);
  // model/effort/sandbox are not split into separate fields (#384). The two
  // comparison rails render the slots A..Z of two coding agents side by side;
  // cards are addressed by rail index.
  const renderProfileCard = (agent: AgentConfig, railIndex: number, letter: string) => {
    const badge = () => profileCellBadge(agent.id, letter);
    const command = () => displayedProfileCellCommand(agent.id, letter);
    const cellError = () => profileCellErrors[profileCellKey(agent.id, letter)];
    const cardId = `settings.profileCard.${railIndex}.${letter}`;
    const expanded = () => isCellExpanded(agent.id, letter);
    const preview = () =>
      resolveProfilePreview(settings.data!.codingAgentProfiles, agent.id, letter);
    // #538: every cell always exposes its command editor (no "Add cell" / missing
    // box). The badge reports the real match/configured/fallback/missing state; the
    // sub-line shows the agent base binary the cell params append to (#597), or,
    // when no base command is set yet, the fallback/configured text, so it never
    // contradicts a MISSING or FALLBACK badge by claiming "Configured".
    const subLine = () => {
      if (cellError()) return "Command syntax error";
      // #597 - the binary comes from the agent base command; the cell holds only
      // params. Show the effective binary basename, not the params text.
      const baseExe = commandExecutableBasename(agent.command);
      if (baseExe) return baseExe;
      const resolved = preview();
      return resolved.fallbackApplied ? `Launches ${resolved.effectiveProfile}` : "Configured";
    };
    return (
      <article
        class="settings-profile-card"
        classList={{
          match: badge() === "match",
          configured: badge() === "configured",
          fallback: badge() === "fallback",
          missing: badge() === "missing",
          invalid: badge() === "invalid",
        }}
        data-ac-testid={cardId}
        data-ac-role="row"
        data-ac-state={badge()}
        data-ac-agent-id={agent.id}
        data-ac-profile-letter={letter}
      >
        <div class="settings-profile-card-head">
          <span class="settings-profile-letter-badge">{letter}</span>
          <div class="settings-profile-card-heading">
            <div class="settings-profile-name-line">
              <input
                class="settings-input settings-profile-label"
                value={settings.data!.codingAgentProfiles.profileLabelsByAgent[agent.id]?.[letter] ?? ""}
                onInput={(e) => updateProfileLabel(agent.id, letter, e.currentTarget.value)}
                placeholder={
                  resolveProfileLabel(
                    settings.data!.codingAgentProfiles,
                    settings.data!.agents,
                    agent.id,
                    letter,
                  ) || (letter === "A" ? "Baseline" : "Profile label")
                }
                data-ac-testid={`${cardId}.label`}
                data-ac-role="textbox"
              />
              <span
                class={`settings-profile-badge ${badge()}`}
                data-ac-testid={`${cardId}.badge`}
                data-ac-role="status"
                data-ac-state={badge()}
              >
                {BADGE_LABEL[badge()]}
              </span>
            </div>
            <div class="settings-profile-card-sub">{subLine()}</div>
          </div>
          <button
            class="settings-profile-chevron"
            onClick={() => toggleCell(agent.id, letter)}
            title={expanded() ? "Collapse" : "Expand"}
            aria-expanded={expanded()}
            data-ac-testid={`${cardId}.toggle`}
            data-ac-role="button"
          >
            {expanded() ? "▾" : "▸"}
          </button>
        </div>

        <Show when={expanded()}>
          <div class="settings-profile-field-label-row">
            <label class="settings-profile-field-label">Command</label>
            <button
              type="button"
              class="settings-field-help"
              title={AC_PLACEHOLDER_HELP}
              aria-label={AC_PLACEHOLDER_HELP}
              data-ac-testid={`${cardId}.placeholderHelp`}
              data-ac-role="button"
            >
              ?
            </button>
          </div>
          <div class="settings-profile-command-base" data-ac-testid={`${cardId}.commandBase`}>
            Runs <code>{agent.command || "(set the Coding Agent command first)"}</code> then your params:
          </div>
          <input
            class="settings-input settings-profile-command"
            classList={{ invalid: Boolean(cellError()) }}
            value={command()}
            onInput={(e) => updateProfileCellCommand(agent.id, letter, e.currentTarget.value)}
            placeholder="--sandbox workspace-write --model gpt-5-codex"
            data-ac-testid={`${cardId}.command`}
            data-ac-role="textbox"
            data-ac-state={cellError() ? "invalid" : "valid"}
          />
          <Show when={cellError()}>
            <div
              class="settings-profile-cell-error"
              data-ac-testid={`${cardId}.command.error`}
              data-ac-role="status"
            >
              {cellError()}
            </div>
          </Show>
          <Show when={hasAcPlaceholder(command())}>
            <div class="settings-profile-ph-hint" data-ac-testid={`${cardId}.command.placeholder`}>
              <span class="settings-profile-ph-token">%AC_REPLICA_ROOT%</span>,{" "}
              <span class="settings-profile-ph-token">%AC_WORKSPACE_ROOT%</span>, and{" "}
              <span class="settings-profile-ph-token">%AC_MATRIX_ROOT%</span> expand at
              launch; the backend validates the result.
            </div>
          </Show>

          <div
            class="settings-profile-env"
            data-ac-testid={`${cardId}.env`}
            data-ac-role="surface"
          >
            {/* #614 — <Index> (keyed by position) not <For> (keyed by row-object
                reference): updateCellEnvRow replaces the row object on every
                keystroke, so a reference-keyed <For> would dispose+recreate the
                edited <input> and drop focus after one character. <Index> keeps a
                stable DOM node per index and only updates the row signal. */}
            <Index each={cellEnvRows(agent.id, letter)}>
              {(row, rowIndex) => (
                <div
                  class="settings-profile-env-row"
                  data-ac-testid={`${cardId}.envRow.${rowIndex}`}
                  data-ac-role="row"
                >
                  <input
                    class="settings-input settings-env-key"
                    value={row().key}
                    onInput={(e) => updateCellEnvRow(agent.id, letter, rowIndex, "key", e.currentTarget.value)}
                    placeholder="KEY"
                    data-ac-testid={`${cardId}.envRow.${rowIndex}.key`}
                    data-ac-role="textbox"
                  />
                  <input
                    class="settings-input settings-env-value"
                    value={row().value}
                    onInput={(e) => updateCellEnvRow(agent.id, letter, rowIndex, "value", e.currentTarget.value)}
                    placeholder="value"
                    data-ac-testid={`${cardId}.envRow.${rowIndex}.value`}
                    data-ac-role="textbox"
                  />
                  {(() => {
                    // Accessor (not a once-computed const): <Index> keeps the row
                    // scope alive across edits, so the origin label must re-read
                    // row() to stay live as the user types.
                    const origin = () => profileEnvOrigin(row().key, row().value);
                    return (
                      <span
                        class={`settings-env-origin ${origin()}`}
                        data-ac-testid={`${cardId}.envRow.${rowIndex}.origin`}
                        data-ac-role="status"
                        data-ac-env-origin={origin()}
                        title={`Env value origin: ${ENV_ORIGIN_LABEL[origin()]}`}
                      >
                        {ENV_ORIGIN_LABEL[origin()]}
                      </span>
                    );
                  })()}
                  <button
                    class="settings-env-delete"
                    onClick={() => removeCellEnvRow(agent.id, letter, rowIndex)}
                    title="Delete environment row"
                    data-ac-testid={`${cardId}.envRow.${rowIndex}.delete`}
                    data-ac-role="button"
                  >
                    &#x2715;
                  </button>
                  <Show when={hasAcPlaceholder(row().value)}>
                    <div
                      class="settings-profile-ph-preview"
                      data-ac-testid={`${cardId}.envRow.${rowIndex}.placeholder`}
                      data-ac-role="status"
                    >
                      <span class="arrow">&rarr;</span>
                      <span>AC path placeholders expand at launch</span>
                    </div>
                  </Show>
                </div>
              )}
            </Index>
            <Show when={cellEnvError(agent.id, letter)}>
              <div
                class="settings-profile-cell-error"
                data-ac-testid={`${cardId}.env.error`}
                data-ac-role="status"
              >
                {cellEnvError(agent.id, letter)}
              </div>
            </Show>
            <button
              class="settings-profile-cell-btn"
              onClick={() => addCellEnvRow(agent.id, letter)}
              data-ac-testid={`${cardId}.env.add`}
              data-ac-role="button"
            >
              + Env
            </button>
          </div>

          <Show when={letter !== "A"}>
            <div class="settings-profile-card-footer">
              {/* #538: per-cell "Delete cell" removed — cells are never deleted
                  individually. Slot-level "Delete Profile" (drops the letter from
                  every agent; A is immutable) stays so added letters stay removable. */}
              <button
                class="settings-profile-cell-btn settings-profile-delete-profile"
                onClick={() => removeProfileLetter(letter)}
                title={`Delete the entire ${letter} profile slot from all coding agents`}
                data-ac-testid={`${cardId}.deleteProfile`}
                data-ac-role="button"
              >
                Delete Profile
              </button>
            </div>
          </Show>
        </Show>
      </article>
    );
  };

  // One comparison rail = the A..Z profile slots of one coding agent. The two
  // rails sit side by side (the comparison pair); each rail's agent is swappable
  // via its header select or the panel-level "Swap Rails".
  const renderRail = (agent: AgentConfig | null, railIndex: number, side: "left" | "right") => {
    if (!agent) {
      return (
        <section
          class="settings-profile-rail settings-rail-empty"
          data-ac-testid={`settings.profileRail.${railIndex}`}
          data-ac-role="surface"
          data-ac-rail-side={side}
        >
          <div class="settings-rail-empty-note">
            {side === "right"
              ? "Pick a second coding agent (click its row, or use the rail selector) to compare side by side."
              : "Add a coding agent to begin."}
          </div>
        </section>
      );
    }
    const setRailAgent = (id: string) =>
      side === "left" ? setLeftRailId(id) : setRightRailId(id);
    return (
      <section
        class="settings-profile-rail"
        style={{ "--agent-color": agent.color }}
        data-ac-testid={`settings.profileRail.${railIndex}`}
        data-ac-role="surface"
        data-ac-agent-id={agent.id}
        data-ac-rail-side={side}
      >
        <div class="settings-profile-rail-header">
          <div class="settings-profile-rail-heading">
            <div class="settings-profile-rail-title">{(agent.label || agent.id)} rail</div>
            <div
              class="settings-profile-rail-subtitle"
              data-ac-testid={`settings.profileRail.${railIndex}.subtitle`}
              data-ac-role="status"
            >
              {commandExecutableBasename(agent.command) || agent.command || "—"}
            </div>
          </div>
          <select
            class="settings-input settings-rail-select"
            value={agent.id}
            onChange={(e) => setRailAgent(e.currentTarget.value)}
            data-ac-testid={`settings.profileRail.${railIndex}.agentSelect`}
            data-ac-role="combobox"
          >
            <For each={agentList()}>
              {(a) => <option value={a.id}>{a.label || a.id}</option>}
            </For>
          </select>
          {/* #895 — emptying the comparison rail lives here rather than on the
              agent row: it sits next to what it clears, it no longer reflows the
              agent list, and it is no longer a red button 4px from the trash
              that deletes the agent outright. It selects nothing; it deletes
              nothing. Icon-only: a text button was a 42px unshrinkable third
              item in a header already at its shrink limit, so it overflowed and
              got clipped out of the rail below ~648px of effective width. */}
          <Show when={side === "right"}>
            <button
              class="settings-rail-clear"
              onClick={() => setRightRailId(null)}
              title="Clear the comparison rail (keeps the agent and its profiles)"
              aria-label="Clear the comparison rail"
              data-ac-testid={`settings.profileRail.${railIndex}.clear`}
              data-ac-role="button"
            >
              <XMarkIcon class="settings-rail-clear-icon" />
            </button>
          </Show>
        </div>
        <div class="settings-profile-rail-body">
          <For each={profileLetters()}>
            {(letter) => renderProfileCard(agent, railIndex, letter)}
          </For>
          <button
            class="settings-add-btn settings-profile-add"
            onClick={addProfileLetter}
            disabled={!nextAvailableProfileLetter(settings.data!.codingAgentProfiles)}
            data-ac-testid={railIndex === 0 ? "settings.profiles.add" : `settings.profileRail.${railIndex}.addProfile`}
            data-ac-role="button"
            data-ac-state={nextAvailableProfileLetter(settings.data!.codingAgentProfiles) ? "enabled" : "disabled"}
          >
            Add Profile
          </button>
        </div>
      </section>
    );
  };

  // #526 — the unified Coding Agents screen: compact agent list (left) + the two
  // comparison rails (right). Replaces the former split "Coding Agents" /
  // "Profiles" tabs. The `settings.profiles.*` test ids are kept so callers and
  // automation that opened the old "profiles" view still resolve here.
  const renderCodingAgentsScreen = () => {
    const activeIndex = () => agentList().findIndex((a) => a.id === activeAgentId());
    const canSwap = () => agentList().length > 1;
    return (
      <div
        class="settings-config-screen"
        data-ac-testid="settings.profiles.section"
        data-ac-role="region"
      >
        <aside class="settings-agents-panel">
          <div class="settings-agents-panel-header">
            <div class="settings-agents-panel-heading">
              <div class="settings-agents-panel-title">Coding Agents</div>
              <div class="settings-agents-panel-kicker">Colors and selected comparison pair</div>
            </div>
            <button
              class="settings-add-btn settings-agents-add"
              onClick={() => addAgent()}
              data-ac-testid="settings.agents.add"
              data-ac-role="button"
            >
              Add Agent
            </button>
          </div>
          <div class="settings-agents-panel-body">
            <Show
              when={agentList().length > 0}
              fallback={
                <div class="settings-empty-note" data-ac-testid="settings.profiles.empty" data-ac-role="status">
                  No coding agents configured. Add one below.
                </div>
              }
            >
              <For each={settings.data!.agents}>
                {(agent, i) => renderAgentRow(agent, i)}
              </For>
            </Show>

            <div class="settings-agents-actions-block">
              <div class="settings-agents-actions-title">Actions</div>
              <div class="settings-agents-actions-row">
                <button
                  class="settings-row-btn"
                  onClick={swapRails}
                  disabled={!canSwap()}
                  title="Swap the left and right comparison rails"
                  data-ac-testid="settings.agents.swapRails"
                  data-ac-role="button"
                  data-ac-state={canSwap() ? "enabled" : "disabled"}
                >
                  Swap Rails
                </button>
                <button
                  class="settings-row-btn danger"
                  onClick={() => {
                    const idx = activeIndex();
                    if (idx >= 0) removeAgent(idx);
                  }}
                  disabled={activeIndex() < 0}
                  title="Delete the expanded coding agent"
                  data-ac-testid="settings.agents.deleteActive"
                  data-ac-role="button"
                  data-ac-state={activeIndex() >= 0 ? "enabled" : "disabled"}
                >
                  Delete Agent
                </button>
              </div>
            </div>

            {renderAgentPresets()}
          </div>
        </aside>

        <div
          class="settings-compare-zone"
          data-ac-testid="settings.profiles.rails"
          data-ac-role="list"
        >
          {renderRail(leftRailAgent(), 0, "left")}
          {renderRail(rightRailAgent(), 1, "right")}
        </div>
      </div>
    );
  };

  const renderIntegrationsTab = () => (
    <>
      {/* Voice to Text */}
      <div class="settings-section">
        <div class="settings-section-title">Voice to Text</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.voiceToTextEnabled}
            onChange={(e) =>
              updateField("voiceToTextEnabled", e.currentTarget.checked)
            }
          />
          <span>Enable microphone button on sessions</span>
        </label>
        <Show when={settings.data!.voiceToTextEnabled}>
          <label class="settings-field">
            <span class="settings-label">Gemini API Key</span>
            <input
              class="settings-input"
              type="password"
              value={settings.data!.geminiApiKey}
              onInput={(e) =>
                updateField("geminiApiKey", e.currentTarget.value)
              }
              placeholder="AIza..."
            />
          </label>
          <label class="settings-field">
            <span class="settings-label">Gemini Model</span>
            <select
              class="settings-input"
              value={settings.data!.geminiModel}
              onChange={(e) =>
                updateField("geminiModel", e.currentTarget.value)
              }
            >
              <For each={GEMINI_MODELS}>
                {(m) => (
                  <option value={m.id}>{m.label}</option>
                )}
              </For>
            </select>
          </label>
          <label class="settings-checkbox-field">
            <input
              type="checkbox"
              class="settings-checkbox"
              checked={settings.data!.voiceAutoExecute}
              onChange={(e) =>
                updateField("voiceAutoExecute", e.currentTarget.checked)
              }
            />
            <span>Auto-execute after transcription</span>
          </label>
          <Show when={settings.data!.voiceAutoExecute}>
            <label class="settings-field">
              <span class="settings-label">Auto-execute delay (seconds)</span>
              <input
                class="settings-input settings-input-sm"
                type="number"
                min="1"
                max="120"
                value={settings.data!.voiceAutoExecuteDelay}
                onInput={(e) => {
                  const v = parseInt(e.currentTarget.value, 10);
                  if (!isNaN(v)) updateField("voiceAutoExecuteDelay", Math.max(1, Math.min(120, v)));
                }}
              />
            </label>
          </Show>
        </Show>
      </div>

      {/* Telegram Bots */}
      <div class="settings-section">
        <div class="settings-section-title">Telegram Bots</div>

      <For each={settings.data!.telegramBots || []}>
        {(bot, i) => (
          <div class="settings-button-card">
            <div class="settings-button-card-header">
              <div
                class="settings-color-dot"
                style={{ background: bot.color }}
              />
              <span>{bot.label || "New Bot"}</span>
              <button
                class="settings-agent-remove"
                onClick={() => removeBot(i())}
                title="Remove bot"
              >
                &#x2715;
              </button>
            </div>
            <label class="settings-field">
              <span class="settings-label">Label</span>
              <input
                class="settings-input"
                value={bot.label}
                onInput={(e) =>
                  updateBot(i(), "label", e.currentTarget.value)
                }
                placeholder="My Bot"
              />
            </label>
            <label class="settings-field">
              <span class="settings-label">Bot Token</span>
              <input
                class="settings-input"
                type="password"
                value={bot.token}
                onInput={(e) =>
                  updateBot(i(), "token", e.currentTarget.value)
                }
                placeholder="123456:ABC-DEF..."
              />
            </label>
            <Show when={bot.chatId}>
              <div class="settings-field">
                <span class="settings-label">Chat ID</span>
                <span class="settings-chat-id">{bot.chatId}</span>
              </div>
            </Show>
            <label class="settings-field">
              <span class="settings-label">Color</span>
              <div class="settings-color-row">
                <input
                  type="color"
                  class="settings-color-picker"
                  value={bot.color}
                  onInput={(e) =>
                    updateBot(i(), "color", e.currentTarget.value)
                  }
                />
                <input
                  class="settings-input settings-input-sm"
                  value={bot.color}
                  onInput={(e) =>
                    updateBot(i(), "color", e.currentTarget.value)
                  }
                />
              </div>
            </label>
            <div class="settings-bot-actions">
              <button
                class="settings-test-btn"
                onClick={() => handleTestBot(bot, i())}
                disabled={testingBot() === bot.id || !bot.token}
              >
                {testingBot() === bot.id ? "Testing..." : "Test"}
              </button>
              <Show when={testResult()?.id === bot.id}>
                <span
                  class={`settings-test-result ${testResult()!.ok ? "ok" : "fail"}`}
                >
                  {testResult()!.ok
                    ? "Connected"
                    : testResult()!.msg || "Failed"}
                </span>
              </Show>
            </div>
          </div>
        )}
      </For>

      <button class="settings-add-btn" onClick={addBot}>
        + Add Telegram Bot
      </button>
    </div>
    </>
  );

  return (
    <div
      class="modal-overlay"
      data-ac-testid="settings.overlay"
      data-ac-role="overlay"
    >
      <div
        class="modal-container modal-container-lg"
        classList={{ "modal-container-config": activeTab() === "agents" }}
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        data-ac-testid="settings.modal"
        data-ac-role="dialog"
      >
        <div class="modal-header">
          <span class="modal-title">Settings</span>
        </div>

        {/* Tab bar */}
        <div class="settings-tabs">
          <For each={TABS}>
            {(tab) => (
              <button
                class={`settings-tab ${activeTab() === tab.key ? "active" : ""}`}
                onClick={() => setActiveTab(tab.key)}
                role="tab"
                aria-selected={activeTab() === tab.key}
                data-ac-testid={`settings.tab.${tab.key}`}
                data-ac-role="tab"
                data-ac-state={activeTab() === tab.key ? "active" : "idle"}
              >
                {tab.label}
              </button>
            )}
          </For>
        </div>

        <Show
          when={settings.data}
          fallback={
            <div class="modal-body" style="display:flex;align-items:center;justify-content:center;min-height:200px;color:#555;font-size:13px">
              Loading...
            </div>
          }
        >
          <div
            class="modal-body"
            classList={{ "modal-body-config": activeTab() === "agents" }}
          >
            <Show when={activeTab() === "general"}>{renderGeneralTab()}</Show>
            <Show when={activeTab() === "agents"}>{renderCodingAgentsScreen()}</Show>
            <Show when={activeTab() === "resources"}>
              {renderResourcesTab()}
            </Show>
            <Show when={activeTab() === "integrations"}>
              {renderIntegrationsTab()}
            </Show>
          </div>
        </Show>

        <div class="modal-footer">
          <Show when={saveError() || currentValidationError()}>
            <span class="modal-save-error">
              {saveError() || currentValidationError()}
            </span>
          </Show>
          <button
            class="modal-btn modal-btn-cancel"
            onClick={closeSettings}
            data-ac-testid="settings.cancel"
            data-ac-role="button"
          >
            Cancel
          </button>
          <button
            class="modal-btn modal-btn-save"
            onClick={handleSave}
            disabled={saving() || !!currentValidationError()}
            data-ac-testid="settings.save"
            data-ac-role="button"
            data-ac-state={saving() ? "saving" : "ready"}
          >
            {saving() ? "Saving..." : "Save"}
          </button>
        </div>
      </div>

      {/* #769 Phase 2 — destructive-action confirm for re-seeding a default
          config folder (§14.7.5). Stacks over Settings via its own fixed overlay. */}
      <Show when={reseedTarget()}>
        {(def) => (
          <div
            class="modal-overlay"
            data-ac-testid="settings.reseedConfirm.overlay"
            data-ac-role="overlay"
          >
            <div
              class="agent-modal"
              role="dialog"
              aria-modal="true"
              aria-label="Re-seed default configuration"
              data-ac-testid="settings.reseedConfirm.modal"
              data-ac-role="dialog"
            >
              <div class="agent-modal-header">
                <span class="agent-modal-title">Re-seed default configuration</span>
              </div>
              <div class="wizard-body">
                <p class="settings-hint" data-ac-testid="settings.reseedConfirm.message">
                  Re-seed {def().label} default configuration? This replaces the default{" "}
                  <code>{def().configSeed?.dest}</code> config folder that AgentsCommander seeds for
                  new {def().label} sessions with the shipped default. Your current edits to that
                  default folder will be backed up to{" "}
                  <code>{def().configSeed?.dest}.bak-&lt;timestamp&gt;</code>. Running sessions and
                  their live config are not changed; new sessions will use the restored default.
                  Continue?
                </p>
              </div>
              <div class="new-agent-footer">
                <button
                  class="new-agent-cancel-btn"
                  onClick={() => setReseedTarget(null)}
                  disabled={reseeding()}
                  data-ac-testid="settings.reseedConfirm.cancel"
                  data-ac-role="button"
                >
                  Cancel
                </button>
                <button
                  class="new-agent-create-btn"
                  onClick={() => void confirmReseed()}
                  disabled={reseeding()}
                  data-ac-testid="settings.reseedConfirm.confirm"
                  data-ac-role="button"
                  data-ac-state={reseeding() ? "reseeding" : "ready"}
                >
                  {reseeding() ? "Re-seeding..." : "Re-seed"}
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
};

export default SettingsModal;
