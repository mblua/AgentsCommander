import { Component, For, Show, createSignal, onCleanup } from "solid-js";
import { SessionAPI } from "../../shared/ipc";
import { sessionsStore } from "../stores/sessions";

const HungNotification: Component = () => {
  const notifications = () => sessionsStore.hungNotifications;

  // Reactive timer for elapsed time display
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 60000);
  onCleanup(() => clearInterval(timer));

  return (
    <Show when={notifications().length > 0}>
      <div class="hung-notification-container">
        <For each={notifications()}>
          {(notif) => {
            const elapsed = () => Math.floor((now() - notif.timestamp) / 60000);
            return (
              <div class="hung-notification">
                <div class="hung-notification-header">
                  <span class="hung-notification-icon">!</span>
                  <span>Agent may be hung</span>
                </div>
                <div class="hung-notification-body">
                  <strong>{notif.sessionName}</strong> has been idle for {elapsed()}+ minutes
                  without completing its task.
                </div>
                <div class="hung-notification-actions">
                  <button onClick={() => {
                    SessionAPI.switch(notif.sessionId);
                    sessionsStore.dismissHungNotification(notif.sessionId);
                  }}>Switch to session</button>
                  <button onClick={() => sessionsStore.dismissHungNotification(notif.sessionId)}>
                    Dismiss
                  </button>
                </div>
              </div>
            );
          }}
        </For>
      </div>
    </Show>
  );
};

export default HungNotification;
