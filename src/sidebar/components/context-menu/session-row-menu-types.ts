// The session-row context-menu catalogue's public types.
// Types only, zero runtime code. The catalogue (SessionRowMenu) owns item order,
// labels, icons, markup and separators; a host owns nothing but the data it
// hands in through SessionRowMenuCaps, one `false | <spec>` per item id.
import type { SessionRepo, TelegramBotConfig } from "../../../shared/types";

export type SessionRowMenuItemId =
  | "restart"
  | "codingAgent"
  | "openFolder"
  | "repos"
  | "matrixFolder"
  | "close"
  | "deleteAgent"
  | "detach"
  | "telegram"
  | "addToGroup"
  | "editTaskTitle"
  | "clearTaskTitle";

export interface RepoBrowseItem {
  id: "main" | "branch";
  label: string;
  url: string;
}

export interface ActionSpec {
  onSelect: () => void;
  disabled?: boolean;
  title?: string;
}

export interface OpenFolderSpec extends ActionSpec {
  label?: string;
}

export interface ToggleSpec {
  on: boolean;
  onSelect: () => void;
}

export interface ReposSpec {
  /** Never empty; `reposSpec` returns `false` instead of an empty list. */
  repos: SessionRepo[];
  browseItems: (repo: SessionRepo) => RepoBrowseItem[];
  onOpenRepo: (sourcePath: string) => void;
  onOpenBrowse: (url: string) => void;
}

export interface TelegramSpec extends ToggleSpec {
  // `on` is inherited from ToggleSpec and means BRIDGED: true renders
  // "Detach Telegram", false renders "Attach Telegram". There is no `attached`.
  /** null => the catalogue tints the icon #0088cc. */
  bridgeColor: string | null;
  /** non-null => the bot list renders inline under the toggle, now. */
  bots: TelegramBotConfig[] | null;
  onSelectBot: (botId: string) => void;
}

export interface DeleteAgentSpec {
  onSelect: () => void;
  testId: string;
}

export interface GroupChoice {
  id: string;
  name: string;
  checked: boolean;
  disabled: boolean;
  title: string;
  pinned?: boolean;
}

/** Every testid the live Add to Group surface emits. Three of the eight are NOT
 *  derivable from one base, so the spec carries each field. */
export interface AddToGroupTestIds {
  trigger: string; // replica.<wgPath>.groups.trigger
  flyout: string; // replica.<wgPath>.groups.flyout
  nonstop: string; // replica.<wgPath>.groups.nonstop
  choice: (choiceId: string) => string; // replica.<wgPath>.groups.<automationIdPart(id)>
  create: string; // replica.<wgPath>.groups.create
  createInput: string; // replica.<wgPath>.groups.create.input
  createSave: string; // replica.<wgPath>.groups.create.save
  menuError: string; // replica.<wgPath>.groups.error
}

export interface AddToGroupSpec {
  /** choices[0] is the pinned Non-stop slot. */
  choices: GroupChoice[];
  onToggle: (id: string) => void;
  emptyNote: string | null;
  /** TWO independent error rows. Both render, in this order, when both are
   *  non-null. storeError has no testid today; menuError carries
   *  testIds.menuError. */
  storeError: string | null;
  menuError: string | null;
  create: {
    active: boolean;
    draft: string;
    onDraft: (v: string) => void;
    onStart: () => void;
    onSave: () => void;
  };
  testIds: AddToGroupTestIds;
}

export interface EditTaskTitleSpec {
  editing: boolean;
  draft: string;
  busy: boolean;
  error: string | null;
  onDraft: (v: string) => void;
  onStart: () => void;
  onSave: () => void;
  onCancel: () => void;
}

/** A row shown dimmed and inert, with a `title` that says why. */
export interface DisabledItemSpec {
  disabled: true;
  title: string;
}

export type SessionRowMenuCaps = {
  restart?: false | ActionSpec;
  codingAgent?: false | ActionSpec;
  openFolder?: false | OpenFolderSpec;
  repos?: false | ReposSpec;
  matrixFolder?: false | ActionSpec;
  /** Shown only by hosts that pass it. */
  unleash?: false | (ActionSpec & { label?: string });
  close?: false | ActionSpec;
  deleteAgent?: false | DeleteAgentSpec;
  detach?: false | ToggleSpec;
  telegram?: false | TelegramSpec;
  addToGroup?: false | AddToGroupSpec | DisabledItemSpec;
  editTaskTitle?: false | EditTaskTitleSpec | DisabledItemSpec;
  clearTaskTitle?: false | ActionSpec;
};

export interface SessionRowMenuProps {
  open: boolean;
  x: number;
  y: number;
  testIdPrefix: string;
  caps: SessionRowMenuCaps;
  onDismiss: () => void;
}
