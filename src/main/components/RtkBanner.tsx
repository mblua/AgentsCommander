import { Component, createSignal, onCleanup, onMount, Show } from "solid-js";
import type { UnlistenFn } from "../../shared/transport";
import { SettingsAPI, onRtkStartupStatus } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import type { RtkStartupMode } from "../../shared/ipc";

const RtkBanner: Component = () => {
  const [mode, setMode] = createSignal<RtkStartupMode>("silent");
  // Disables both buttons during the in-flight sweep so a rapid double-click
  // cannot queue two concurrent setInjectRtkHook+sweepRtkHook pairs.
  const [busy, setBusy] = createSignal(false);
  let unlisten: UnlistenFn | null = null;

  onMount(async () => {
    if (!isTauri) return;

    // Subscribe BEFORE snapshotting. The Rust setup task may emit
    // rtk_startup_status during our mount, and a snapshot-then-listen order
    // would drop that emit. If the listener fires before the snapshot
    // resolves, the snapshot's setMode() is idempotent at worst (same value).
    unlisten = await onRtkStartupStatus((next) => setMode(next));

    try {
      const initial = await SettingsAPI.getRtkStartupStatus();
      setMode(initial);
    } catch (err) {
      console.error("[rtk-banner] getRtkStartupStatus failed:", err);
    }
  });

  onCleanup(() => {
    if (unlisten) unlisten();
  });

  const onEnable = async () => {
    if (busy()) return;
    setBusy(true);
    try {
      // Narrow setter - holds the Rust SettingsState write lock through
      // save_settings, avoiding the IPC-level read-modify-write race that a
      // get+update would have against a concurrent SettingsModal save.
      await SettingsAPI.setInjectRtkHook(true);
      const result = await SettingsAPI.sweepRtkHook(true);
      if (result.errors.length > 0) {
        console.error(
          `[rtk-banner] sweep partial failure: ${result.errors.length}/${result.total} dirs failed`,
          result.errors,
        );
      }
      setMode("active");
    } catch (err) {
      console.error("[rtk-banner] enable failed:", err);
    } finally {
      setBusy(false);
    }
  };

  const onDismissPrompt = async () => {
    if (busy()) return;
    setBusy(true);
    try {
      await SettingsAPI.setRtkPromptDismissed(true);
      setMode("silent");
    } catch (err) {
      console.error("[rtk-banner] dismiss failed:", err);
    } finally {
      setBusy(false);
    }
  };

  // Auto-disabled banner is UI-only dismiss - does NOT persist. If the
  // condition (rtk missing AND injectRtkHook=true was the trigger that
  // already auto-flipped) holds again on next boot, the banner reappears.
  const onDismissAutoDisabled = () => setMode("silent");

  return (
    <Show when={mode() === "prompt-enable" || mode() === "auto-disabled"}>
      <Show when={mode() === "prompt-enable"}>
        <div class="rtk-banner rtk-banner-prompt">
          <span class="rtk-banner-text">
            RTK is installed. Inject the RTK hook into agent replicas to compress
            Bash output and save tokens?
          </span>
          <button
            class="rtk-banner-btn rtk-banner-btn-primary"
            disabled={busy()}
            onClick={onEnable}
          >
            Enable
          </button>
          <button
            class="rtk-banner-btn rtk-banner-btn-secondary"
            disabled={busy()}
            onClick={onDismissPrompt}
          >
            Don't ask again
          </button>
        </div>
      </Show>
      <Show when={mode() === "auto-disabled"}>
        <div class="rtk-banner rtk-banner-warning">
          <span class="rtk-banner-text">
            RTK was disabled because the binary is no longer in PATH. Hooks were
            removed from all replicas. Re-install RTK and re-enable the toggle in
            Settings to restore.
          </span>
          <button
            class="rtk-banner-btn rtk-banner-btn-secondary"
            onClick={onDismissAutoDisabled}
          >
            Dismiss
          </button>
        </div>
      </Show>
    </Show>
  );
};

export default RtkBanner;
