// #4779 - the "Temporary Rooms" sidebar category: list, create, launch and delete
// temp rooms. Markup reuses existing classes only; the agent row's menu is the
// shared SessionRowMenu catalogue, so this host owns nothing but the data.
import { createMemo, createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { Portal } from "solid-js/web";
import {
  SessionAPI,
  SettingsAPI,
  TaskAPI,
  TelegramAPI,
  TempRoomAPI,
  WindowAPI,
} from "../../shared/ipc";
import {
  TEMP_ROOMS_GROUPS_KEY as KEY,
  tempRoomPlaceholderWarning,
  tempRoomUnavailablePlaceholders,
  type SettingsWithTempRoom,
  type TempRoomAgentChoice,
  type TempRoomInfo,
} from "../../shared/temp-rooms";
import type { Session, TelegramBotConfig } from "../../shared/types";
import { launchErrorMessage } from "../../shared/launch-errors";
import { toastStore } from "../../shared/stores/toasts";
import { sessionsStore } from "../stores/sessions";
import { tempRoomsStore, tempRoomWorkgroup } from "../stores/temp-rooms";
import {
  compileGroupRegex,
  compileWorkgroupGroups,
  DEFAULT_NON_STOP_NAME,
  groupMatchesWorkgroup,
  isUngroupedWorkgroup,
  nonStopMatchesWorkgroup,
  removeExactGroupToken,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";
import { projectCollapseStore } from "../stores/project-collapse";
import { automationIdPart } from "./replica-repo-badges";
import { bridgesStore } from "../stores/bridges";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import { requestCoordinatorClose } from "../stores/coordinator-close";
import { unleashedReplicasStore } from "../stores/unleashed-replicas";
import { applyUnleash } from "./ReplicaSessionActions";
import UnleashConfirmModal from "./UnleashConfirmModal";
import { sessionDotClass } from "./session-status";
import { RowKeyProxy, onRowKey } from "./RowKeyProxy";
import SidebarRegexFilter, { SidebarRegexFilterError } from "./SidebarRegexFilter";
import { sidebarFilter } from "../stores/sidebar-filter";
import { createSidebarFilterHost } from "./sidebar-filter-host";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";
import SessionRowMenu from "./context-menu/SessionRowMenu";
import type { AddToGroupSpec } from "./context-menu/session-row-menu-types";
import {
  closeSpec,
  deleteAgentSpec,
  detachSpec,
  matrixFolderSpec,
  openFolderSpec,
  reposSpec,
  telegramSpec,
} from "./context-menu/session-row-menu-specs";

const NONSTOP_CHOICE = "__nonstop__";
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

const messageOf = (e: unknown): string =>
  typeof e === "object" && e !== null && typeof (e as { message?: unknown }).message === "string"
    ? (e as { message: string }).message
    : String(e);

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
  const rooms = tempRoomsStore.rooms;
  const collapsed = () => projectCollapseStore.isProjectCollapsed(KEY);
  const [categoryMenu, setCategoryMenu] = createSignal<MenuPos | null>(null);
  const [headerMenu, setHeaderMenu] = createSignal<RoomMenu | null>(null);
  const [agentMenu, setAgentMenu] = createSignal<RoomMenu | null>(null);
  const [menuBots, setMenuBots] = createSignal<TelegramBotConfig[] | null>(null);
  const [picker, setPicker] = createSignal<PickerRequest | null>(null);
  const [deleting, setDeleting] = createSignal<TempRoomInfo | null>(null);
  const [deleteError, setDeleteError] = createSignal<string | null>(null);
  const [deleteBusy, setDeleteBusy] = createSignal(false);
  const [placeholderWarnings, setPlaceholderWarnings] = createSignal<string[]>([]);
  const [unleashAsk, setUnleashAsk] = createSignal<TempRoomInfo | null>(null);
  const [groupCreatePath, setGroupCreatePath] = createSignal<string | null>(null);
  const [groupDraft, setGroupDraft] = createSignal("");
  const [groupMenuError, setGroupMenuError] = createSignal<string | null>(null);
  const [titleEditPath, setTitleEditPath] = createSignal<string | null>(null);
  const [titleDraft, setTitleDraft] = createSignal("");
  const [titleBusy, setTitleBusy] = createSignal(false);
  const [titleError, setTitleError] = createSignal<string | null>(null);

  // #4805 - the Temporary Rooms group set (ProjectPanel `groupPredicate` mirror).
  const groupsConfig = () => workgroupGroupsStore.config(KEY);
  const compiled = createMemo(() => compileWorkgroupGroups(groupsConfig().groups));
  const inSelectedGroup = (room: TempRoomInfo): boolean => {
    const wg = tempRoomWorkgroup(room);
    const selected = workgroupGroupsStore.selection(KEY);
    if (selected.kind === "all") return true;
    if (selected.kind === "ungrouped")
      return isUngroupedWorkgroup(compiled(), groupsConfig().nonStop, wg);
    if (selected.kind === "nonstop") {
      const ns = groupsConfig().nonStop;
      return !!ns && nonStopMatchesWorkgroup(ns, wg);
    }
    return groupMatchesWorkgroup(compiled(), selected.id, wg);
  };

  // #4786 - red warning when the saved agent uses a placeholder temp rooms lack.
  const refreshWarnings = async () => {
    try {
      const snapshot = await readSettings();
      setPlaceholderWarnings(tempRoomUnavailablePlaceholders(snapshot, snapshot.tempRoomAgent));
    } catch (e) {
      console.error("[TemporaryRoomsPanel] placeholder check failed:", e);
      setPlaceholderWarnings([]);
    }
  };

  const refresh = async () => {
    try {
      await tempRoomsStore.refresh();
    } catch (e) {
      console.error("[TemporaryRoomsPanel] list_temp_rooms failed:", e);
    }
    await refreshWarnings();
  };
  onMount(() => {
    void refresh();
    void workgroupGroupsStore.ensureLoaded(KEY);
  });

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
      const target = ev?.target as Element | null | undefined;
      if (ev?.type === "click" && target?.closest?.(".session-context-menu")) return;
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
    setGroupCreatePath(null);
    setGroupDraft("");
    setGroupMenuError(null);
    setTitleEditPath(null);
    setTitleDraft("");
    setTitleBusy(false);
    setTitleError(null);
  };

  // #4805 - stale group entries of a reused trNN number.
  const forgetGroups = async (name: string) => {
    try {
      await workgroupGroupsStore.forgetWorkgroup(KEY, name);
    } catch (e) {
      toastStore.error(`Group entries of ${name} could not be removed: ${messageOf(e)}`);
    }
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
    await refreshWarnings();
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
      await forgetGroups(room.name);
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
    await refreshWarnings();
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
      await forgetGroups(room.name);
      setDeleting(null);
    } catch (e) {
      setDeleteError(launchErrorMessage(e));
    } finally {
      setDeleteBusy(false);
      await refresh();
    }
  };

  // #4780 - the replica Unleash flow (#4752) on the temp-room path; restart relaunches.
  const unleashed = (room: TempRoomInfo) =>
    unleashedReplicasStore.isUnleashed({ path: room.path, unleashed: room.unleashed });

  const groupAction = async (run: () => Promise<void>) => {
    try {
      await run();
    } catch (e) {
      setGroupMenuError(messageOf(e));
    }
  };

  const addToGroupFor = (room: TempRoomInfo): AddToGroupSpec => {
    const wg = tempRoomWorkgroup(room);
    const ns = groupsConfig().nonStop;
    const nonStopChecked = !!ns && nonStopMatchesWorkgroup(ns, wg);
    const groupChoices = groupsConfig().groups.map((g) => {
      const valid = !!compileGroupRegex(g);
      const checked = groupMatchesWorkgroup(compiled(), g.id, wg);
      const custom = checked && removeExactGroupToken(g.regex, room.name) === null;
      const title = !valid
        ? "Fix this group's regex before adding a room"
        : custom
          ? "Membership comes from a custom regex. Use Edit groups to change it."
          : checked
            ? "Remove this room from the group"
            : g.regex;
      return { id: g.id, name: g.name, checked, disabled: !valid || custom, title };
    });
    const choices = [
      {
        id: NONSTOP_CHOICE,
        name: DEFAULT_NON_STOP_NAME,
        checked: nonStopChecked,
        disabled: false,
        title: `Watch this room in the ${DEFAULT_NON_STOP_NAME} group`,
      },
      ...groupChoices,
    ];
    return {
      choices,
      onToggle: (id: string) => {
        setGroupMenuError(null);
        const checked = choices.find((c) => c.id === id)?.checked ?? false;
        void groupAction(() => {
          if (id === NONSTOP_CHOICE) {
            return checked
              ? workgroupGroupsStore.removeWorkgroupFromNonStop(KEY, room.name)
              : workgroupGroupsStore.addWorkgroupToNonStop(KEY, room.name);
          }
          return checked
            ? workgroupGroupsStore.removeWorkgroupFromGroup(KEY, id, room.name)
            : workgroupGroupsStore.addWorkgroupToGroup(KEY, id, room.name);
        });
      },
      emptyNote: "No groups yet",
      storeError: workgroupGroupsStore.error(KEY),
      menuError: groupMenuError(),
      create: {
        active: groupCreatePath() === room.path,
        draft: groupDraft(),
        onDraft: setGroupDraft,
        onStart: () => {
          setGroupCreatePath(room.path);
          setGroupDraft("");
          setGroupMenuError(null);
        },
        onSave: () => {
          const name = groupDraft().trim();
          if (!name) {
            setGroupMenuError("Group name cannot be blank.");
            return;
          }
          void groupAction(async () => {
            await workgroupGroupsStore.createGroupForWorkgroup(KEY, name, room.name);
            setGroupCreatePath(null);
          });
        },
      },
      testIds: {
        trigger: `tempRoom.${room.name}.groups.trigger`,
        flyout: `tempRoom.${room.name}.groups.flyout`,
        nonstop: `tempRoom.${room.name}.groups.nonstop`,
        choice: (id) => `tempRoom.${room.name}.groups.${automationIdPart(id)}`,
        create: `tempRoom.${room.name}.groups.create`,
        createInput: "tempRoom.groups.create.input",
        createSave: "tempRoom.groups.create.save",
        menuError: "tempRoom.groups.error",
      },
    };
  };

  const saveTitle = async (room: TempRoomInfo) => {
    const title = titleDraft().trim();
    if (!title) {
      setTitleError("Title cannot be empty.");
      return;
    }
    setTitleBusy(true);
    try {
      await TaskAPI.setTitleAt(room.path, title);
      await refresh();
      closeAgentMenu();
    } catch (e) {
      setTitleError(String(e));
    } finally {
      setTitleBusy(false);
    }
  };

  const clearTitle = async (room: TempRoomInfo) => {
    closeAgentMenu();
    try {
      await TaskAPI.cleanAt(room.path);
    } catch (e) {
      // A MutationCommitted error means the file already changed: refresh anyway.
      toastStore.error(messageOf(e));
    }
    await refresh();
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
      unleash: {
        label: unleashed(room) ? "Leash" : "Unleash",
        disabled: unleashedReplicasStore.busy(),
        onSelect: () => {
          if (unleashed(room)) {
            void applyUnleash(session?.id ?? "", room.path, false, () => restart(room));
          } else {
            setUnleashAsk(room);
          }
        },
      },
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
      addToGroup: addToGroupFor(room),
      editTaskTitle: {
        editing: titleEditPath() === room.path,
        draft: titleDraft(),
        busy: titleBusy(),
        error: titleError(),
        onDraft: setTitleDraft,
        onStart: () => {
          setTitleEditPath(room.path);
          setTitleDraft(room.taskTitle ?? "");
          setTitleError(null);
        },
        onSave: () => void saveTitle(room),
        onCancel: () => {
          setTitleEditPath(null);
          setTitleDraft("");
          setTitleError(null);
        },
      },
      clearTaskTitle: { onSelect: () => void clearTitle(room) },
    };
  };

  // #4785 - the one sidebar filter; a match on the label keeps every room.
  const visibleRooms = () =>
    !sidebarFilter.active() || sidebarFilter.matches("Temporary Rooms")
      ? rooms().filter(inSelectedGroup)
      : rooms()
          .filter(inSelectedGroup)
          .filter((r) => sidebarFilter.matches(r.name));

  let panelEl: HTMLDivElement | undefined;
  createSidebarFilterHost(() => panelEl?.parentElement);

  return (
    <div class="project-panel" data-ac-testid="tempRooms.panel" ref={panelEl}>
      <div
        class="project-header"
        classList={{
          "project-header--has-indicator": true,
        }}
        data-ac-testid="tempRooms.header"
        onContextMenu={(e) => {
          if ((e.target as HTMLElement).closest(".project-filter-row")) return;
          openPortalMenu(e, setCategoryMenu);
        }}
      >
        <button
          type="button"
          class="project-header-main"
          data-ac-testid="tempRooms.header.toggle"
          aria-expanded={!collapsed()}
          onClick={() => projectCollapseStore.toggleProjectCollapsed(KEY)}
        >
          <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
            &#x25BE;
          </span>
          <span class="project-title">Temporary Rooms</span>
        </button>
        <span
          class="project-file-difference project-header-indicator--last"
          data-ac-testid="tempRooms.count"
        >
          {String(visibleRooms().length)}
        </span>
        <SidebarRegexFilter />
        <div
          class="project-filter-error-slot"
          data-ac-testid="sidebar.regexFilter.errorSlot"
        >
          <SidebarRegexFilterError />
        </div>
      </div>
      <Show when={!collapsed()}>
        <Show when={placeholderWarnings().length > 0}>
          <div class="new-agent-error" data-ac-testid="tempRooms.placeholderWarning">
            <For each={placeholderWarnings()}>
              {(token) => <div>{tempRoomPlaceholderWarning(token)}</div>}
            </For>
          </div>
        </Show>
        <For each={visibleRooms()}>
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
                  <Show when={room.taskTitle?.trim()}>
                    {(text) => (
                      <span
                        class="ac-wg-task sidebar-task-label"
                        tabIndex={0}
                        data-ac-testid={`tempRoom.${room.name}.taskTitle`}
                        data-ac-role="text"
                        data-ac-state={text() === "Clean" ? "clean" : "task"}
                      >
                        {text()}
                      </span>
                    )}
                  </Show>
                </div>
                <div
                  class="replica-item"
                  classList={{
                    active: !!session() && session()?.id === sessionsStore.activeId,
                    unleashed: unleashed(room),
                  }}
                  title={room.path}
                  data-ac-testid={`tempRoom.${room.name}.row`}
                  onClick={() => void rowClick(room)}
                  onKeyDown={onRowKey(() => void rowClick(room))}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    closeMenus();
                    setMenuBots(null);
                    setAgentMenu({ x: e.clientX, y: e.clientY, room });
                  }}
                >
                  <RowKeyProxy label={room.name} />
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

      <Show when={unleashAsk()}>
        {(r) => (
          <Portal>
            <UnleashConfirmModal
              replicaName={r().name}
              busy={unleashedReplicasStore.busy()}
              onCancel={() => setUnleashAsk(null)}
              onConfirm={() => {
                const room = r();
                setUnleashAsk(null);
                void applyUnleash(findTempRoomSession(room)?.id ?? "", room.path, true, () =>
                  restart(room),
                );
              }}
            />
          </Portal>
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
