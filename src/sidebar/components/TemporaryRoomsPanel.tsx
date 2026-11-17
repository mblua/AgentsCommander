// #4779 - the "Temporary Rooms" sidebar category: list, create, launch and delete
// temp rooms. Markup reuses existing classes only; the agent row's menu is the
// shared SessionRowMenu catalogue, so this host owns nothing but the data.
import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { Portal } from "solid-js/web";
import { SessionAPI, SettingsAPI, TelegramAPI, TempRoomAPI, WindowAPI } from "../../shared/ipc";
import type {
  SettingsWithTempRoom,
  TempRoomAgentChoice,
  TempRoomInfo,
} from "../../shared/temp-rooms";
import type { Session, TelegramBotConfig } from "../../shared/types";
import { launchErrorMessage } from "../../shared/launch-errors";
import { toastStore } from "../../shared/stores/toasts";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import { requestCoordinatorClose } from "../stores/coordinator-close";
import { sessionDotClass } from "./session-status";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";
import SessionRowMenu from "./context-menu/SessionRowMenu";
import {
  closeSpec,
  deleteAgentSpec,
  detachSpec,
  matrixFolderSpec,
  openFolderSpec,
  reposSpec,
  telegramSpec,
} from "./context-menu/session-row-menu-specs";

const UNLEASH_TITLE =
  "Not available in Temporary Rooms yet: the agent already runs without " +
  "AgentsCommander rules (stage 2, #4780)";
const NO_TASK_TITLE = "Temp rooms have no TASK.md";
const OPEN_ROOM_FOLDER = "Open Room's Folder";

type MenuPos = { x: number; y: number };
type RoomMenu = MenuPos & { room: TempRoomInfo };
type PickerRequest = {
  room: TempRoomInfo;
  session: Session | undefined;
  resolve: (selection: AgentPickerSelection | null) => void;
};

function findTempRoomSession(room: TempRoomInfo): Session | undefined {
  const path = normalizeProjectPathForCompare(room.path);
  return sessionsStore.sessions.find(
    (s) => s.name === room.name && normalizeProjectPathForCompare(s.workingDirectory) === path,
  );
}

const isLive = (session: Session | undefined): session is Session =>
  !!session && typeof session.status === "string";

async function readSettings(): Promise<SettingsWithTempRoom> {
  const settings: SettingsWithTempRoom = await SettingsAPI.get();
  return settings;
}

async function saveChoice(choice: TempRoomAgentChoice): Promise<void> {
  const current = await readSettings();
  const next: SettingsWithTempRoom = { ...current, tempRoomAgent: choice };
  await SettingsAPI.update(next);
}

const choiceOf = (selection: AgentPickerSelection): TempRoomAgentChoice => ({
  agentId: selection.agent.id,
  requestedProfile: selection.requestedProfile,
});

const TrashSvg: Component = () => (
  <svg
    width="14"
    height="14"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    style={{ "flex-shrink": "0" }}
  >
    <polyline points="3 6 5 6 21 6" />
    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
  </svg>
);

const TemporaryRoomsPanel: Component = () => {
  const [rooms, setRooms] = createSignal<TempRoomInfo[]>([]);
  const [collapsed, setCollapsed] = createSignal(false);
  const [categoryMenu, setCategoryMenu] = createSignal<MenuPos | null>(null);
  const [headerMenu, setHeaderMenu] = createSignal<RoomMenu | null>(null);
  const [agentMenu, setAgentMenu] = createSignal<RoomMenu | null>(null);
  const [menuBots, setMenuBots] = createSignal<TelegramBotConfig[] | null>(null);
  const [picker, setPicker] = createSignal<PickerRequest | null>(null);
  const [deleting, setDeleting] = createSignal<TempRoomInfo | null>(null);
  const [deleteError, setDeleteError] = createSignal<string | null>(null);
  const [deleteBusy, setDeleteBusy] = createSignal(false);

  const refresh = async () => {
    try {
      setRooms(await TempRoomAPI.list());
    } catch (e) {
      console.error("[TemporaryRoomsPanel] list_temp_rooms failed:", e);
    }
  };
  onMount(() => void refresh());

  // ProjectPanel handler pattern: dismiss on window click/contextmenu/Escape.
  let removeDismiss: (() => void) | null = null;
  const closeMenus = () => {
    removeDismiss?.();
    removeDismiss = null;
    setCategoryMenu(null);
    setHeaderMenu(null);
  };
  onCleanup(closeMenus);
  const openPortalMenu = (e: MouseEvent, open: (pos: MenuPos) => void) => {
    e.preventDefault();
    e.stopPropagation();
    closeMenus();
    closeAgentMenu();
    open({ x: e.clientX, y: e.clientY });
    const dismiss = (ev?: Event) => {
      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
      closeMenus();
    };
    setTimeout(() => {
      window.addEventListener("click", dismiss);
      window.addEventListener("contextmenu", dismiss);
      window.addEventListener("keydown", dismiss);
    }, 0);
    removeDismiss = () => {
      window.removeEventListener("click", dismiss);
      window.removeEventListener("contextmenu", dismiss);
      window.removeEventListener("keydown", dismiss);
    };
  };

  const closeAgentMenu = () => {
    setAgentMenu(null);
    setMenuBots(null);
  };

  const pickAgent = (room: TempRoomInfo, session: Session | undefined) =>
    new Promise<AgentPickerSelection | null>((resolve) => {
      picker()?.resolve(null); // a replaced request counts as cancelled
      setPicker({ room, session, resolve });
    });

  const resolveChoice = async (room: TempRoomInfo): Promise<TempRoomAgentChoice | null> => {
    const settings = await readSettings();
    const saved = settings.tempRoomAgent;
    if (saved && settings.agents.some((a) => a.id === saved.agentId)) return saved;
    const selection = await pickAgent(room, undefined);
    if (!selection) return null;
    const choice = choiceOf(selection);
    await saveChoice(choice);
    return choice;
  };

  const launch = async (room: TempRoomInfo, choice: TempRoomAgentChoice) => {
    try {
      const session = await SessionAPI.create({
        cwd: room.path,
        sessionName: room.name,
        agentId: choice.agentId,
        requestedProfile: choice.requestedProfile,
      });
      await SessionAPI.switch(session.id);
    } catch (e) {
      toastStore.error(launchErrorMessage(e));
    }
  };

  const launchWithSavedChoice = async (room: TempRoomInfo) => {
    try {
      const choice = await resolveChoice(room);
      if (choice) await launch(room, choice);
    } catch (e) {
      toastStore.error(launchErrorMessage(e));
    }
  };

  const createRoom = async () => {
    closeMenus();
    try {
      const room = await TempRoomAPI.create();
      await refresh();
      await launchWithSavedChoice(room);
    } catch (e) {
      toastStore.error(launchErrorMessage(e));
    }
  };

  const restart = async (room: TempRoomInfo, choice?: TempRoomAgentChoice) => {
    const session = findTempRoomSession(room);
    if (!session) {
      if (choice) await launch(room, choice);
      else await launchWithSavedChoice(room);
      return;
    }
    try {
      const next = await SessionAPI.restart(session.id, choice ?? undefined);
      await SessionAPI.switch(next.id);
    } catch (e) {
      toastStore.error(launchErrorMessage(e));
    }
  };

  const rowClick = async (room: TempRoomInfo) => {
    const session = findTempRoomSession(room);
    if (isLive(session)) {
      try {
        await SessionAPI.switch(session.id);
      } catch (e) {
        console.error("[TemporaryRoomsPanel] switch failed:", e);
      }
      return;
    }
    await restart(room);
  };

  const codingAgent = async (room: TempRoomInfo) => {
    const selection = await pickAgent(room, findTempRoomSession(room));
    if (!selection) return;
    const choice = choiceOf(selection);
    try {
      await saveChoice(choice);
    } catch (e) {
      toastStore.error(launchErrorMessage(e));
      return;
    }
    await restart(room, choice);
  };

  const openFolder = async (room: TempRoomInfo) => {
    try {
      await WindowAPI.openInExplorer(room.path);
    } catch (e) {
      console.error("[TemporaryRoomsPanel] open folder failed:", e);
    }
  };

  const toggleDetach = async (session: Session) => {
    try {
      if (sessionsStore.isDetached(session.id)) await WindowAPI.attach(session.id);
      else await WindowAPI.detach(session.id);
    } catch (e) {
      console.error("[TemporaryRoomsPanel] detach toggle failed:", e);
    }
  };

  // SessionItem.handleTelegramClick: bridged → detach; one bot → attach; more → list.
  const telegram = async (session: Session) => {
    try {
      if (bridgesStore.getBridge(session.id)) {
        closeAgentMenu();
        await TelegramAPI.detach(session.id);
        return;
      }
      const bots = (await SettingsAPI.get()).telegramBots || [];
      const onlyBot = bots[0];
      if (bots.length > 1) {
        setMenuBots(bots);
        return;
      }
      closeAgentMenu();
      if (onlyBot) await TelegramAPI.attach(session.id, onlyBot.id);
    } catch (e) {
      console.error("[TemporaryRoomsPanel] telegram action failed:", e);
    }
  };

  const selectBot = async (session: Session, botId: string) => {
    closeAgentMenu();
    try {
      await TelegramAPI.attach(session.id, botId);
    } catch (e) {
      console.error("[TemporaryRoomsPanel] telegram attach failed:", e);
    }
  };

  const confirmDelete = async () => {
    const room = deleting();
    if (!room || deleteBusy()) return;
    setDeleteBusy(true);
    setDeleteError(null);
    try {
      await TempRoomAPI.delete(room.name);
      setDeleting(null);
    } catch (e) {
      setDeleteError(launchErrorMessage(e));
    } finally {
      setDeleteBusy(false);
      await refresh();
    }
  };

  const agentCaps = (room: TempRoomInfo) => {
    const session = findTempRoomSession(room);
    const live = isLive(session);
    const bridge = session ? bridgesStore.getBridge(session.id) : undefined;
    const onOpen = () => void openFolder(room);
    return {
      restart: { onSelect: () => void restart(room) },
      codingAgent: { onSelect: () => void codingAgent(room) },
      openFolder:
        openFolderSpec(session, { label: OPEN_ROOM_FOLDER, onSelect: onOpen }) ||
        { label: OPEN_ROOM_FOLDER, onSelect: onOpen },
      repos: reposSpec(session?.gitRepos ?? [], {
        browseItems: () => [],
        onOpenRepo: (p: string) => void WindowAPI.openInExplorer(p),
        onOpenBrowse: () => {},
      }),
      matrixFolder: matrixFolderSpec(undefined),
      unleash: { disabled: true, title: UNLEASH_TITLE, onSelect: () => {} },
      close: closeSpec(session, {
        onSelect: () => {
          if (session) void requestCoordinatorClose(session);
        },
      }),
      deleteAgent: deleteAgentSpec(undefined),
      detach: detachSpec(live, !!session && sessionsStore.isDetached(session.id), () => {
        if (session) void toggleDetach(session);
      }),
      telegram: telegramSpec(live ? session : undefined, {
        on: !!bridge,
        bridgeColor: bridge?.color ?? null,
        bots: menuBots(),
        onSelect: () => {
          if (session) void telegram(session);
        },
        onSelectBot: (id: string) => {
          if (session) void selectBot(session, id);
        },
      }),
      addToGroup: { disabled: true as const, title: "Groups need a project room" },
      editTaskTitle: { disabled: true as const, title: NO_TASK_TITLE },
      clearTaskTitle: { disabled: true, title: NO_TASK_TITLE, onSelect: () => {} },
    };
  };

  return (
    <div class="project-panel" data-ac-testid="tempRooms.panel">
      <div
        class="project-header"
        data-ac-testid="tempRooms.header"
        onContextMenu={(e) => openPortalMenu(e, setCategoryMenu)}
      >
        <button
          type="button"
          class="project-header-main"
          aria-expanded={!collapsed()}
          onClick={() => setCollapsed(!collapsed())}
        >
          <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
            &#x25BE;
          </span>
          <span class="project-title">Temporary Rooms</span>
        </button>
        <span class="project-file-difference" data-ac-testid="tempRooms.count">
          {String(rooms().length)}
        </span>
      </div>
      <Show when={!collapsed()}>
        <For each={rooms()}>
          {(room) => {
            const session = () => findTempRoomSession(room);
            return (
              <>
                <div
                  class="ac-wg-header"
                  title={room.path}
                  data-ac-testid={`tempRoom.${room.name}.header`}
                  onContextMenu={(e) =>
                    openPortalMenu(e, (pos) => setHeaderMenu({ ...pos, room }))
                  }
                >
                  <span class="ac-wg-name">{room.name}</span>
                </div>
                <div
                  class="replica-item"
                  classList={{ active: !!session() && session()?.id === sessionsStore.activeId }}
                  title={room.path}
                  data-ac-testid={`tempRoom.${room.name}.row`}
                  onClick={() => void rowClick(room)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    closeMenus();
                    setMenuBots(null);
                    setAgentMenu({ x: e.clientX, y: e.clientY, room });
                  }}
                >
                  <div class={`session-item-status ${sessionDotClass(session())}`} />
                  <div class="replica-item-info">
                    <span class="replica-item-name">{room.name}</span>
                  </div>
                </div>
              </>
            );
          }}
        </For>
      </Show>

      <Show when={categoryMenu()}>
        {(pos) => (
          <Portal>
            <div
              class="session-context-menu"
              style={{ left: `${pos().x}px`, top: `${pos().y}px` }}
              onClick={(e) => e.stopPropagation()}
              data-ac-testid="tempRooms.menu"
            >
              <button
                class="session-context-option"
                data-ac-testid="tempRooms.menu.create"
                onClick={() => void createRoom()}
              >
                Create new Temp Room
              </button>
            </div>
          </Portal>
        )}
      </Show>

      <Show when={headerMenu()}>
        {(menu) => (
          <Portal>
            <div
              class="session-context-menu"
              style={{ left: `${menu().x}px`, top: `${menu().y}px` }}
              onClick={(e) => e.stopPropagation()}
              data-ac-testid="tempRoom.headerMenu"
            >
              <button
                class="session-context-option context-option-danger"
                data-ac-testid="tempRoom.headerMenu.delete"
                onClick={() => {
                  const room = menu().room;
                  closeMenus();
                  setDeleteError(null);
                  setDeleting(room);
                }}
              >
                <TrashSvg />
                Delete Temp Room
              </button>
            </div>
          </Portal>
        )}
      </Show>

      <Show when={agentMenu()}>
        {(menu) => (
          <SessionRowMenu
            open={true}
            x={menu().x}
            y={menu().y}
            testIdPrefix={`tempRoom.${menu().room.name}`}
            onDismiss={closeAgentMenu}
            caps={agentCaps(menu().room)}
          />
        )}
      </Show>

      <Show when={picker()}>
        {(req) => (
          <Portal>
            <AgentPickerModal
              sessionName={req().room.name}
              agentPath={req().room.path}
              currentAgentId={req().session?.agentId}
              currentRequestedProfile={req().session?.requestedProfile}
              onSelect={(selection) => {
                const { resolve } = req();
                setPicker(null);
                resolve(selection);
              }}
              onClose={() => {
                const { resolve } = req();
                setPicker(null);
                resolve(null);
              }}
            />
          </Portal>
        )}
      </Show>

      <Show when={deleting()}>
        {(room) => (
          <Portal>
            <div class="modal-overlay">
              <div
                class="agent-modal"
                style={{ "max-width": "360px" }}
                data-ac-testid="tempRoom.deleteModal"
              >
                <div class="agent-modal-header">
                  <span class="agent-modal-title">Delete Temp Room</span>
                </div>
                <div class="new-agent-form">
                  <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                    Delete temp room <strong>{room().name}</strong>? This will remove the folder
                    and all its contents. This action cannot be undone.
                  </p>
                  <Show when={deleteError()}>
                    <div class="new-agent-error" data-ac-testid="tempRoom.deleteModal.error">
                      {deleteError()}
                    </div>
                  </Show>
                </div>
                <div class="new-agent-footer">
                  <button
                    class="new-agent-cancel-btn"
                    data-ac-testid="tempRoom.deleteModal.cancel"
                    onClick={() => setDeleting(null)}
                  >
                    Cancel
                  </button>
                  <button
                    class="new-agent-create-btn"
                    style={{ background: "var(--danger, #c0392b)" }}
                    disabled={deleteBusy()}
                    data-ac-testid="tempRoom.deleteModal.confirm"
                    onClick={() => void confirmDelete()}
                  >
                    Delete
                  </button>
                </div>
              </div>
            </div>
          </Portal>
        )}
      </Show>
    </div>
  );
};

export default TemporaryRoomsPanel;
