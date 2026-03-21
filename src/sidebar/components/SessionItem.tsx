import { Component, createSignal, Show, For } from "solid-js";
import type { Session, SessionStatus, TelegramBotConfig } from "../../shared/types";
import { SessionAPI, TelegramAPI, SettingsAPI, WindowAPI } from "../../shared/ipc";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { bridgesStore } from "../stores/bridges";

function statusClass(status: SessionStatus): string {
  if (typeof status === "string") return status;
  return "exited";
}

const SessionItem: Component<{
  session: Session;
  isActive: boolean;
}> = (props) => {
  const [editing, setEditing] = createSignal(false);
  const [editValue, setEditValue] = createSignal("");
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);
  let inputRef!: HTMLInputElement;

  const bridge = () => bridgesStore.getBridge(props.session.id);

  const handleTelegramClick = async (e: MouseEvent) => {
    e.stopPropagation();
    const b = bridge();
    if (b) {
      // Detach existing bridge
      await TelegramAPI.detach(props.session.id);
    } else {
      // Load bots and show menu (or auto-attach if only one)
      const settings = await SettingsAPI.get();
      const bots = settings.telegramBots || [];
      if (bots.length === 1) {
        await TelegramAPI.attach(props.session.id, bots[0].id);
      } else if (bots.length > 1) {
        setAvailableBots(bots);
        setShowBotMenu(true);
      }
    }
  };

  const handleBotSelect = async (botId: string) => {
    setShowBotMenu(false);
    await TelegramAPI.attach(props.session.id, botId);
  };

  const handleClick = async () => {
    if (!editing()) {
      SessionAPI.switch(props.session.id);
      (await WebviewWindow.getByLabel("terminal"))?.setFocus();
    }
  };

  const handleDoubleClick = (e: MouseEvent) => {
    e.stopPropagation();
    setEditValue(props.session.name);
    setEditing(true);
    // Focus input after it renders
    requestAnimationFrame(() => {
      inputRef?.focus();
      inputRef?.select();
    });
  };

  const confirmRename = () => {
    const val = editValue().trim();
    if (val && val !== props.session.name) {
      SessionAPI.rename(props.session.id, val);
    }
    setEditing(false);
  };

  const cancelRename = () => {
    setEditing(false);
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      confirmRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelRename();
    }
  };

  const handleDetach = (e: MouseEvent) => {
    e.stopPropagation();
    WindowAPI.detach(props.session.id);
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    SessionAPI.destroy(props.session.id);
  };

  return (
    <div
      class={`session-item session-item-enter ${props.isActive ? "active" : ""}`}
      onClick={handleClick}
    >
      <div
        class={`session-item-status ${props.session.waitingForInput ? "waiting" : statusClass(props.session.status)}`}
      />
      <div class="session-item-info">
        <Show
          when={editing()}
          fallback={
            <div class="session-item-name" onDblClick={handleDoubleClick}>
              {props.session.name}
            </div>
          }
        >
          <input
            ref={inputRef!}
            class="session-item-rename-input"
            value={editValue()}
            onInput={(e) => setEditValue(e.currentTarget.value)}
            onKeyDown={handleKeyDown}
            onBlur={confirmRename}
            maxLength={50}
            onClick={(e) => e.stopPropagation()}
          />
        </Show>
        <div class="session-item-shell">{props.session.shell}</div>
      </div>
      <button
        class="session-item-detach"
        onClick={handleDetach}
        title="Detach to own window"
      >
        &#x29C9;
      </button>
      <Show when={bridge()}>
        <div
          class="session-item-bridge-dot"
          style={{ background: bridge()!.color }}
          title={`Telegram: ${bridge()!.botLabel}`}
        />
      </Show>
      <button
        class={`session-item-telegram ${bridge() ? "active" : ""}`}
        onClick={handleTelegramClick}
        title={bridge() ? "Detach Telegram" : "Attach Telegram"}
        style={bridge() ? { color: bridge()!.color } : {}}
      >
        T
      </button>
      <Show when={showBotMenu()}>
        <div class="session-item-bot-menu" onClick={(e) => e.stopPropagation()}>
          <For each={availableBots()}>
            {(bot) => (
              <button
                class="session-item-bot-option"
                onClick={() => handleBotSelect(bot.id)}
              >
                <span class="settings-color-dot" style={{ background: bot.color }} />
                {bot.label}
              </button>
            )}
          </For>
        </div>
      </Show>
      <button class="session-item-close" onClick={handleClose} title="Close session">
        &#x2715;
      </button>
    </div>
  );
};

export default SessionItem;
