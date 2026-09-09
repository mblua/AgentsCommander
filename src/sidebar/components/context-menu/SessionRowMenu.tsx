// #1871 phase 1 - the session-row context-menu CATALOGUE. This component owns
// the complete item set, its order, its labels, its icons, its markup and
// where the separators fall. A host turns items on by handing in a per-item
// capability record (SessionRowMenuCaps) built from *Spec helpers; it cannot
// choose order, spelling, icon or dimming. Imports no store, no ipc, and no
// other component except the five icon modules below.
//
// Four rules this file owns:
//   1. Never destructure or cache props.caps; read props.caps.<id> inside a
//      tracking scope only, or the record is captured once at mount and stops
//      following the host's data.
//   2. Dismiss, then select: props.onDismiss() and then the spec's callback,
//      as two statements in that order, for every control except five that
//      keep the menu open (the telegram toggle, a telegram bot row, an Add to
//      Group choice, the Add to Group create row, and the three task-title
//      editor controls). For those five the callback still fires; only the
//      dismiss is withheld.
//   3. Reclamp on expansion: one effect over two booleans (bot list shown,
//      title editor shown) drives the Surface's reclamp().
//   4. The two flyout items do not share click behaviour, and each panel
//      renders behind its own EXACT-KEY guard. Neither guard may be weakened
//      to a non-null test such as flyoutKey() !== null.
import { createEffect, createMemo, For, on, Show, type Component, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import ContextMenuSurface, { type ContextMenuSurfaceApi } from "./ContextMenuSurface";
import type {
  ActionSpec,
  AddToGroupSpec,
  EditTaskTitleSpec,
  ReposSpec,
  SessionRowMenuCaps,
  SessionRowMenuProps,
  TelegramSpec,
} from "./session-row-menu-types";
import { TelegramIcon } from "../TelegramIcon";
import DetachIcon from "../DetachIcon";
import ReattachIcon from "../ReattachIcon";
import UserPlusIcon from "../UserPlusIcon";
import TrashIcon from "../TrashIcon";

/** The unbridged telegram tint is a catalogue constant, like the labels. */
const UNBRIDGED_TELEGRAM_TINT = "#0088cc";

const FOLDER_ICON_PATH =
  "M1.75 4.25A1.75 1.75 0 0 1 3.5 2.5h3.1c.46 0 .9.18 1.22.5l.9.9h3.78A1.75 1.75 0 0 1 14.25 5.65v5.1a1.75 1.75 0 0 1-1.75 1.75h-9A1.75 1.75 0 0 1 1.75 10.75v-6.5Z";

// #1871 phase 1 - the copies at ProjectPanel.tsx:297-310/1842-1851 and
// SessionItem.tsx:554-563 are deleted when those hosts migrate (phases 2 and 3).
const RepoFolderIcon: Component = () => (
  <svg class="session-context-repo-icon" viewBox="0 0 16 16" aria-hidden="true">
    <path fill="currentColor" d={FOLDER_ICON_PATH} />
  </svg>
);

// #1871 phase 1 - the copies at ProjectPanel.tsx:297-310/1842-1851 and
// SessionItem.tsx:554-563 are deleted when those hosts migrate (phases 2 and 3).
const MatrixFolderIcon: Component = () => (
  <svg class="session-context-matrix-icon" viewBox="0 0 16 16" aria-hidden="true">
    <path fill="currentColor" d={FOLDER_ICON_PATH} />
  </svg>
);

const deferToFrame = (fn: () => void): void => {
  if (typeof window.requestAnimationFrame === "function") {
    window.requestAnimationFrame(fn);
    return;
  }
  window.setTimeout(fn, 0);
};

const optionClass = (danger: boolean, dim: boolean): string =>
  ["session-context-option", danger ? "context-option-danger" : "", dim ? "context-option-disabled" : ""]
    .filter((c) => c !== "")
    .join(" ");

/** The `action` shape. `dim` is the catalogue's per-item "Dim when disabled?"
 *  column: context-option-disabled is added only where it says yes. */
const ActionItem: Component<{
  spec: ActionSpec;
  testId: string;
  label: string;
  danger: boolean;
  dim: boolean;
  onDismiss: () => void;
  children: JSX.Element;
}> = (props) => (
  <button
    class={optionClass(props.danger, props.dim && !!props.spec.disabled)}
    disabled={props.spec.disabled}
    title={props.spec.title}
    onClick={() => {
      props.onDismiss();
      props.spec.onSelect();
    }}
    data-ac-testid={props.testId}
    data-ac-role="menuitem"
  >
    <span class="session-context-option-icon" aria-hidden="true">
      {props.children}
    </span>{" "}
    {props.label}
  </button>
);

interface CatalogueBodyProps {
  surface: ContextMenuSurfaceApi;
  caps: SessionRowMenuCaps;
  testIdPrefix: string;
  onDismiss: () => void;
}

const CatalogueBody: Component<CatalogueBodyProps> = (props) => {
  const id = (suffix: string): string => `${props.testIdPrefix}${suffix}`;

  // Five groups; a separator renders between two groups iff both sides have at
  // least one rendered item. Disabled is not absent.
  const g1 = createMemo(
    () =>
      !!(
        props.caps.restart ||
        props.caps.codingAgent ||
        props.caps.openFolder ||
        props.caps.repos ||
        props.caps.matrixFolder ||
        props.caps.close
      ),
  );
  const g2 = createMemo(() => !!props.caps.deleteAgent);
  const g3 = createMemo(() => !!(props.caps.detach || props.caps.telegram));
  const g4 = createMemo(() => !!props.caps.addToGroup);
  const g5 = createMemo(() => !!(props.caps.editTaskTitle || props.caps.clearTaskTitle));
  const sepBefore2 = createMemo(() => g2() && g1());
  const sepBefore3 = createMemo(() => g3() && (g1() || g2()));
  const sepBefore4 = createMemo(() => g4() && (g1() || g2() || g3()));
  const sepBefore5 = createMemo(() => g5() && (g1() || g2() || g3() || g4()));

  // Rule 3: exactly two booleans, each a memo so only a real change reclamps.
  const botsShown = createMemo(() => {
    const telegram = props.caps.telegram;
    return !!(telegram && telegram.bots);
  });
  const editorShown = createMemo(() => {
    const editor = props.caps.editTaskTitle;
    return !!(editor && editor.editing);
  });
  createEffect(on([botsShown, editorShown], () => props.surface.reclamp(), { defer: true }));

  const stop = (e: Event): void => e.stopPropagation();

  const repoEntries = (spec: () => ReposSpec): JSX.Element => (
    <For each={spec().repos}>
      {(repo, index) => {
        const key = (): string => `repo:${index()}`;
        const browseItems = () => spec().browseItems(repo);
        return (
          <>
            <button
              class="session-context-option session-context-repo-option"
              title={repo.sourcePath}
              onClick={() => {
                props.onDismiss();
                spec().onOpenRepo(repo.sourcePath);
              }}
              onMouseEnter={(e) => {
                if (browseItems().length > 0) {
                  props.surface.openFlyout(key(), e.currentTarget);
                } else {
                  props.surface.closeFlyout();
                }
              }}
              onMouseLeave={() => props.surface.scheduleFlyoutClose()}
              onFocus={(e) => {
                if (browseItems().length > 0) props.surface.openFlyout(key(), e.currentTarget);
              }}
              onKeyDown={(e) => {
                if (e.key === "ArrowRight" && browseItems().length > 0) {
                  e.preventDefault();
                  e.stopPropagation();
                  props.surface.openFlyout(key(), e.currentTarget);
                  props.surface.focusFirstFlyoutItem();
                  return;
                }
                if (e.key === "Escape" && props.surface.flyoutKey() !== null) {
                  e.preventDefault();
                  e.stopPropagation(); // close the submenu only, keep the menu
                  props.surface.closeFlyout();
                }
              }}
              data-ac-testid={id(`.menu.repo.${index()}`)}
              data-ac-role="menuitem"
            >
              <span class="session-context-option-icon" aria-hidden="true">
                <RepoFolderIcon />
              </span>
              <span class="session-context-repo-label">{repo.label}</span>
              <Show when={browseItems().length > 0}>
                <span
                  class="session-context-submenu-arrow"
                  data-ac-testid={id(`.menu.repo.${index()}.browse.arrow`)}
                >
                  &rsaquo;
                </span>
              </Show>
            </button>
            <Portal>
              {/* Rule 4: an EXACT-KEY guard. Do not weaken to flyoutKey() !== null. */}
              <Show when={props.surface.flyoutKey() === key()}>
                <div
                  class="session-context-flyout"
                  ref={(el) => props.surface.setFlyoutEl(el)}
                  style={{
                    left: `${props.surface.flyoutPos()?.x ?? 0}px`,
                    top: `${props.surface.flyoutPos()?.y ?? 0}px`,
                  }}
                  onMouseEnter={() => props.surface.cancelFlyoutClose()}
                  onMouseLeave={() => props.surface.scheduleFlyoutClose()}
                  onClick={stop}
                  onContextMenu={stop}
                  onKeyDown={(e) => {
                    if (e.key !== "Escape") return;
                    e.preventDefault();
                    e.stopPropagation(); // close the submenu only, keep the menu
                    props.surface.closeFlyout();
                  }}
                  data-ac-testid={id(`.menu.repo.${index()}.browse.flyout`)}
                >
                  <For each={browseItems()}>
                    {(item) => (
                      <button
                        class="session-context-option"
                        title={item.url}
                        onClick={() => {
                          props.onDismiss();
                          spec().onOpenBrowse(item.url);
                        }}
                        data-ac-testid={id(`.menu.repo.${index()}.browse.${item.id}`)}
                        data-ac-role="menuitem"
                      >
                        {item.label}
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </Portal>
          </>
        );
      }}
    </For>
  );

  const telegramItem = (spec: () => TelegramSpec): JSX.Element => (
    <>
      {/* Exempt from the dismiss: the host decides, because the toggle may need
          to expand its bot list inline. */}
      <button
        class="session-context-option"
        onClick={() => spec().onSelect()}
        data-ac-testid={id(".menu.telegram")}
        data-ac-role="menuitem"
        data-ac-state={spec().on ? "bridged" : "unbridged"}
      >
        <span
          class="session-context-option-icon"
          aria-hidden="true"
          style={{ color: spec().bridgeColor ?? UNBRIDGED_TELEGRAM_TINT }}
        >
          <TelegramIcon />
        </span>{" "}
        {spec().on ? "Detach Telegram" : "Attach Telegram"}
      </button>
      <Show when={spec().bots}>
        {(bots) => (
          <For each={bots()}>
            {(bot) => (
              // Exempt from the dismiss: the host closes, and only after its
              // own guard has run; a component-side dismiss would clear the
              // list the guard reads and the attach would never be reached.
              <button
                class="session-context-option"
                onClick={() => spec().onSelectBot(bot.id)}
                data-ac-testid={id(`.menu.telegram.bot.${bot.id}`)}
                data-ac-role="menuitem"
              >
                <span class="session-context-option-icon" aria-hidden="true">
                  <span class="settings-color-dot" style={{ background: bot.color }} />
                </span>{" "}
                {bot.label}
              </button>
            )}
          </For>
        )}
      </Show>
    </>
  );

  const addToGroupItem = (spec: () => AddToGroupSpec): JSX.Element => {
    const KEY = "addToGroup";
    const activate = (anchor: HTMLElement): void => props.surface.openFlyout(KEY, anchor);
    return (
      <>
        <button
          class="session-context-option session-context-submenu-trigger"
          onMouseEnter={(e) => activate(e.currentTarget)}
          onMouseLeave={() => props.surface.scheduleFlyoutClose()}
          onFocus={(e) => activate(e.currentTarget)}
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            activate(e.currentTarget);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter" && e.key !== " ") return;
            e.preventDefault();
            e.stopPropagation();
            activate(e.currentTarget);
          }}
          data-ac-testid={spec().testIds.trigger}
          data-ac-role="menuitem"
        >
          <span class="session-context-option-icon" aria-hidden="true">
            <UserPlusIcon class="session-context-group-add-icon" />
          </span>
          <span>Add to Group</span>
          <span class="session-context-submenu-arrow">&rsaquo;</span>
        </button>
        <Portal>
          {/* Rule 4: an EXACT-KEY guard. Do not weaken to flyoutKey() !== null. */}
          <Show when={props.surface.flyoutKey() === KEY}>
            <div
              class="session-context-flyout"
              ref={(el) => props.surface.setFlyoutEl(el)}
              style={{
                left: `${props.surface.flyoutPos()?.x ?? 0}px`,
                top: `${props.surface.flyoutPos()?.y ?? 0}px`,
              }}
              onMouseEnter={() => props.surface.cancelFlyoutClose()}
              onMouseLeave={() => props.surface.scheduleFlyoutClose()}
              onClick={stop}
              onContextMenu={stop}
              data-ac-testid={spec().testIds.flyout}
            >
              <For each={spec().choices}>
                {(choice, index) => (
                  // Exempt from the dismiss: group membership is a multi-select.
                  <button
                    class={
                      "session-context-option session-context-group-option" +
                      (index() === 0 ? " session-context-group-option-nonstop" : "") +
                      (choice.disabled ? " context-option-disabled" : "")
                    }
                    disabled={choice.disabled}
                    title={choice.title}
                    onClick={() => {
                      if (choice.disabled) return;
                      spec().onToggle(choice.id);
                    }}
                    data-ac-testid={
                      index() === 0 ? spec().testIds.nonstop : spec().testIds.choice(choice.id)
                    }
                    data-ac-role="menuitem"
                  >
                    <span class="session-context-option-check">{choice.checked ? "✓" : ""}</span>
                    <span>{choice.name}</span>
                  </button>
                )}
              </For>
              <Show when={spec().choices.length === 0 && spec().emptyNote}>
                {(note) => <div class="session-context-note">{note()}</div>}
              </Show>
              <Show when={spec().storeError}>
                {(error) => <div class="session-context-error">{error()}</div>}
              </Show>
              <Show when={spec().menuError}>
                {(error) => (
                  <div class="session-context-error" data-ac-testid={spec().testIds.menuError}>
                    {error()}
                  </div>
                )}
              </Show>
              <Show
                when={spec().create.active}
                fallback={
                  // Exempt from the dismiss: the menu IS the editor.
                  <button
                    class="session-context-option"
                    onClick={() => spec().create.onStart()}
                    data-ac-testid={spec().testIds.create}
                    data-ac-role="menuitem"
                  >
                    <span class="session-context-option-icon" aria-hidden="true">
                      {"\u{1F465}"}
                    </span>
                    <span>Create new group</span>
                  </button>
                }
              >
                <div class="session-context-inline-create">
                  <input
                    class="session-context-inline-input"
                    value={spec().create.draft}
                    onInput={(e) => spec().create.onDraft(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        spec().create.onSave();
                      }
                    }}
                    placeholder="Group name"
                    data-ac-testid={spec().testIds.createInput}
                  />
                  <button
                    class="session-context-option"
                    onClick={() => spec().create.onSave()}
                    data-ac-testid={spec().testIds.createSave}
                    data-ac-role="menuitem"
                  >
                    Create
                  </button>
                </div>
              </Show>
            </div>
          </Show>
        </Portal>
      </>
    );
  };

  const editTaskTitleItem = (spec: () => EditTaskTitleSpec): JSX.Element => (
    <>
      {/* Exempt from the dismiss: the editor lives inside the menu. */}
      <button
        class="session-context-option"
        title="Edit TASK title"
        onClick={(e) => {
          e.stopPropagation();
          spec().onStart();
        }}
        data-ac-testid={id(".menu.editTaskTitle")}
        data-ac-role="menuitem"
      >
        <span class="session-context-option-icon session-context-task-icon" aria-hidden="true">
          {"✎"}
        </span>{" "}
        Edit TASK title
      </button>
      <Show when={spec().editing}>
        <div class="session-context-title-edit" onClick={stop}>
          <input
            ref={(el) =>
              deferToFrame(() => {
                el.focus();
                el.select();
              })
            }
            class="session-context-title-input"
            value={spec().draft}
            onInput={(e) => spec().onDraft(e.currentTarget.value)}
            onKeyDown={(e) => {
              // Unconditional: the window keydown dismissal fires on Escape,
              // and Escape must cancel the editor, not close the whole menu.
              e.stopPropagation();
              if (e.key === "Enter") {
                e.preventDefault();
                if (!spec().busy) spec().onSave();
              } else if (e.key === "Escape") {
                e.preventDefault();
                spec().onCancel();
              }
            }}
            placeholder="Title"
            disabled={spec().busy}
          />
          <button
            class="session-context-title-btn save"
            onClick={(e) => {
              e.stopPropagation();
              spec().onSave();
            }}
            disabled={spec().busy || !spec().draft.trim()}
            type="button"
          >
            Save
          </button>
          <button
            class="session-context-title-btn cancel"
            onClick={(e) => {
              e.stopPropagation();
              spec().onCancel();
            }}
            disabled={spec().busy}
            type="button"
          >
            Cancel
          </button>
        </div>
      </Show>
      {/* Outside the editing <Show>, so an error survives the editor closing. */}
      <Show when={spec().error}>
        {(error) => <div class="session-context-title-error">{error()}</div>}
      </Show>
    </>
  );

  return (
    <>
      {/* G1 */}
      <Show when={props.caps.restart}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={id(".restart")}
            label="Restart Session"
            danger={true}
            dim={false}
            onDismiss={props.onDismiss}
          >
            {"↺"}
          </ActionItem>
        )}
      </Show>
      <Show when={props.caps.codingAgent}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={id(".codingAgent")}
            label="Coding Agent"
            danger={false}
            dim={false}
            onDismiss={props.onDismiss}
          >
            {"\u{1F916}"}
          </ActionItem>
        )}
      </Show>
      <Show when={props.caps.openFolder}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={id(".openFolder")}
            label={spec().label ?? "Open Folder"}
            danger={false}
            dim={false}
            onDismiss={props.onDismiss}
          >
            {"\u{1F4C2}"}
          </ActionItem>
        )}
      </Show>
      <Show when={props.caps.repos}>{(spec) => repoEntries(spec)}</Show>
      <Show when={props.caps.matrixFolder}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={id(".menu.matrixFolder")}
            label="Open Matrix folder"
            danger={false}
            dim={false}
            onDismiss={props.onDismiss}
          >
            <MatrixFolderIcon />
          </ActionItem>
        )}
      </Show>
      <Show when={props.caps.close}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={id(".close")}
            label="Close Session (Ctrl+Shift+W)"
            danger={true}
            dim={false}
            onDismiss={props.onDismiss}
          >
            {"✕"}
          </ActionItem>
        )}
      </Show>

      {/* G2 */}
      <Show when={sepBefore2()}>
        <div class="context-separator" />
      </Show>
      <Show when={props.caps.deleteAgent}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={spec().testId}
            label="Delete"
            danger={true}
            dim={false}
            onDismiss={props.onDismiss}
          >
            <TrashIcon />
          </ActionItem>
        )}
      </Show>

      {/* G3 */}
      <Show when={sepBefore3()}>
        <div class="context-separator" />
      </Show>
      <Show when={props.caps.detach}>
        {(spec) => (
          <button
            class="session-context-option"
            onClick={() => {
              props.onDismiss();
              spec().onSelect();
            }}
            data-ac-testid={id(".menu.detachToggle")}
            data-ac-role="menuitem"
            data-ac-state={spec().on ? "detached" : "attached"}
          >
            <span class="session-context-option-icon" aria-hidden="true">
              {spec().on ? (
                <ReattachIcon class="session-context-detach-icon" />
              ) : (
                <DetachIcon class="session-context-detach-icon" />
              )}
            </span>{" "}
            {spec().on ? "Re-attach session" : "Detach session"}
          </button>
        )}
      </Show>
      <Show when={props.caps.telegram}>{(spec) => telegramItem(spec)}</Show>

      {/* G4 */}
      <Show when={sepBefore4()}>
        <div class="context-separator" />
      </Show>
      <Show when={props.caps.addToGroup}>{(spec) => addToGroupItem(spec)}</Show>

      {/* G5 */}
      <Show when={sepBefore5()}>
        <div class="context-separator" />
      </Show>
      <Show when={props.caps.editTaskTitle}>{(spec) => editTaskTitleItem(spec)}</Show>
      <Show when={props.caps.clearTaskTitle}>
        {(spec) => (
          <ActionItem
            spec={spec()}
            testId={id(".menu.clearTaskTitle")}
            label="Clear task title"
            danger={false}
            dim={true}
            onDismiss={props.onDismiss}
          >
            {"\u{1F9F9}"}
          </ActionItem>
        )}
      </Show>
    </>
  );
};

const SessionRowMenu: Component<SessionRowMenuProps> = (props) => (
  <ContextMenuSurface
    open={props.open}
    x={props.x}
    y={props.y}
    testId={`${props.testIdPrefix}.menu`}
    onDismiss={props.onDismiss}
  >
    {(surface) => (
      <CatalogueBody
        surface={surface}
        caps={props.caps}
        testIdPrefix={props.testIdPrefix}
        onDismiss={props.onDismiss}
      />
    )}
  </ContextMenuSurface>
);

export default SessionRowMenu;
