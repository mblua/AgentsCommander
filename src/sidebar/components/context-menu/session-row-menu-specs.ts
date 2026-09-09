// #1871 phase 1 - the *Spec helpers every session-row menu host builds its
// capability record from. Each helper returns `false` when its data is absent
// and a spec otherwise. A helper never inspects a store, never reads a signal,
// and never decides whether an item belongs in a host; it decides only whether
// the data for it exists. That is what keeps a future data change (the root
// gaining repos) a data change and not a code change.
import type {
  AcAgentReplica,
  AcWorkgroup,
  Session,
  SessionRepo,
  TelegramBotConfig,
} from "../../../shared/types";
import type {
  ActionSpec,
  AddToGroupSpec,
  AddToGroupTestIds,
  DeleteAgentSpec,
  EditTaskTitleSpec,
  GroupChoice,
  OpenFolderSpec,
  RepoBrowseItem,
  ReposSpec,
  TelegramSpec,
  ToggleSpec,
} from "./session-row-menu-types";

// ---- argument bags ----

export interface ReposHandlers {
  browseItems: (repo: SessionRepo) => RepoBrowseItem[];
  /** The CLICK action: a repo entry opens the folder, never the flyout. */
  onOpenRepo: (sourcePath: string) => void;
  onOpenBrowse: (url: string) => void;
}
export interface SelectHandler {
  onSelect: () => void;
}
export interface OpenFolderOptions {
  onSelect: () => void;
  label?: string;
  title?: string;
}
export interface TelegramHandlers {
  /** bridged; NOT `attached` */
  on: boolean;
  bridgeColor: string | null;
  bots: TelegramBotConfig[] | null;
  onSelect: () => void;
  onSelectBot: (botId: string) => void;
}
export interface DeleteAgentHandlers {
  onSelect: () => void;
  testId: string;
}
export interface AddToGroupData {
  choices: GroupChoice[];
  onToggle: (id: string) => void;
  emptyNote: string | null;
  storeError: string | null;
  menuError: string | null;
  create: AddToGroupSpec["create"];
  testIds: AddToGroupTestIds;
}
export interface TaskTitleData {
  editing: boolean;
  draft: string;
  busy: boolean;
  error: string | null;
  onDraft: (v: string) => void;
  onStart: () => void;
  onSave: () => void;
  onCancel: () => void;
}
export interface ClearTaskTitleOptions {
  onSelect: () => void;
  disabled?: boolean;
  title?: string;
}

// ---- the five helpers a host always feeds: one signature each ----

/** `false` iff no repo has a non-blank `sourcePath`; the kept subset is what
 *  goes into `ReposSpec.repos` (parity with SessionItem's repoMenuEntries). */
export function reposSpec(repos: SessionRepo[], h: ReposHandlers): false | ReposSpec {
  const kept = repos.filter(
    (repo) => typeof repo.sourcePath === "string" && repo.sourcePath.trim() !== "",
  );
  if (kept.length === 0) return false;
  return {
    repos: kept,
    browseItems: h.browseItems,
    onOpenRepo: h.onOpenRepo,
    onOpenBrowse: h.onOpenBrowse,
  };
}

/** Not live-gated: a dormant session still has a folder to open. */
export function openFolderSpec(
  session: Session | undefined,
  o: OpenFolderOptions,
): false | OpenFolderSpec {
  if (!session) return false;
  return { onSelect: o.onSelect, label: o.label, title: o.title };
}

/** Not live-gated: a dormant session can still be closed. */
export function closeSpec(session: Session | undefined, h: SelectHandler): false | ActionSpec {
  if (!session) return false;
  return { onSelect: h.onSelect };
}

export function detachSpec(
  live: boolean,
  detached: boolean,
  onSelect: () => void,
): false | ToggleSpec {
  if (live === false) return false;
  return { on: detached, onSelect };
}

/** The host applies its own liveness gate before calling this; the helper
 *  must NOT re-check it. */
export function telegramSpec(
  session: Session | undefined,
  h: TelegramHandlers,
): false | TelegramSpec {
  if (!session) return false;
  return {
    on: h.on,
    onSelect: h.onSelect,
    bridgeColor: h.bridgeColor,
    bots: h.bots,
    onSelectBot: h.onSelectBot,
  };
}

// ---- the five a host may switch off by passing `undefined`: three overloads
//      each, so the "off" call needs no dummy bag and the "on" call cannot
//      forget one ----

export function matrixFolderSpec(path: undefined | null): false;
export function matrixFolderSpec(path: string, h: SelectHandler): ActionSpec;
export function matrixFolderSpec(
  path: string | null | undefined,
  h: SelectHandler,
): false | ActionSpec;
export function matrixFolderSpec(
  path: string | null | undefined,
  h?: SelectHandler,
): false | ActionSpec {
  // Absent iff undefined or null. There is no value rule: "" is a present input.
  if (path == null || !h) return false;
  return { onSelect: h.onSelect, title: path };
}

export function deleteAgentSpec(replica: undefined | null): false;
export function deleteAgentSpec(replica: AcAgentReplica, h: DeleteAgentHandlers): DeleteAgentSpec;
export function deleteAgentSpec(
  replica: AcAgentReplica | undefined | null,
  h: DeleteAgentHandlers,
): false | DeleteAgentSpec;
export function deleteAgentSpec(
  replica: AcAgentReplica | undefined | null,
  h?: DeleteAgentHandlers,
): false | DeleteAgentSpec {
  if (!replica || !h) return false;
  return { onSelect: h.onSelect, testId: h.testId };
}

export function addToGroupSpec(wg: undefined | null): false;
export function addToGroupSpec(wg: AcWorkgroup, d: AddToGroupData): AddToGroupSpec;
export function addToGroupSpec(
  wg: AcWorkgroup | undefined | null,
  d: AddToGroupData,
): false | AddToGroupSpec;
export function addToGroupSpec(
  wg: AcWorkgroup | undefined | null,
  d?: AddToGroupData,
): false | AddToGroupSpec {
  if (!wg || !d) return false;
  return {
    choices: d.choices,
    onToggle: d.onToggle,
    emptyNote: d.emptyNote,
    storeError: d.storeError,
    menuError: d.menuError,
    create: d.create,
    testIds: d.testIds,
  };
}

export function taskTitleSpec(wg: undefined | null): false;
export function taskTitleSpec(wg: AcWorkgroup, d: TaskTitleData): EditTaskTitleSpec;
export function taskTitleSpec(
  wg: AcWorkgroup | undefined | null,
  d: TaskTitleData,
): false | EditTaskTitleSpec;
export function taskTitleSpec(
  wg: AcWorkgroup | undefined | null,
  d?: TaskTitleData,
): false | EditTaskTitleSpec {
  if (!wg || !d) return false;
  return {
    editing: d.editing,
    draft: d.draft,
    busy: d.busy,
    error: d.error,
    onDraft: d.onDraft,
    onStart: d.onStart,
    onSave: d.onSave,
    onCancel: d.onCancel,
  };
}

export function clearTaskTitleSpec(wg: undefined | null): false;
export function clearTaskTitleSpec(wg: AcWorkgroup, o: ClearTaskTitleOptions): ActionSpec;
export function clearTaskTitleSpec(
  wg: AcWorkgroup | undefined | null,
  o: ClearTaskTitleOptions,
): false | ActionSpec;
export function clearTaskTitleSpec(
  wg: AcWorkgroup | undefined | null,
  o?: ClearTaskTitleOptions,
): false | ActionSpec {
  if (!wg || !o) return false;
  return { onSelect: o.onSelect, disabled: o.disabled, title: o.title };
}
