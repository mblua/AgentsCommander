import { Component } from "solid-js";
import type { AppSettings } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import CodingAgentQuickConfiguration from "./CodingAgentQuickConfiguration";

// #975 — the first-run Welcome flow is now a thin wrapper over the reusable
// CodingAgentQuickConfiguration. Every `onboardingDismissed` write lives here:
// the reusable component never touches first-run state, so other consumers can
// mount it without dismissing onboarding as a side effect.
const WELCOME_TITLE = "Welcome";
const WELCOME_MESSAGE = "Welcome to AgentsCommander! Let's set up your first Coding Agent.";

const OnboardingModal: Component<{ onClose: () => void }> = (props) => {
  // Cancel path: persist dismissal, then close. Escape resolves to this same
  // callback, so keyboard and pointer dismissal stay identical.
  const dismissAndClose = async () => {
    try {
      const settings = await SettingsAPI.get();
      await SettingsAPI.update({ ...settings, onboardingDismissed: true });
      settingsStore.refresh();
    } catch {}
    props.onClose();
  };

  // Success path: dismissal rides along in the same single settings write that
  // persists the new agent, preserving the pre-#975 atomic update.
  const withDismissal = (settings: AppSettings): AppSettings => ({
    ...settings,
    onboardingDismissed: true,
  });

  return (
    <CodingAgentQuickConfiguration
      title={WELCOME_TITLE}
      message={WELCOME_MESSAGE}
      ariaLabel="Set up your first Coding Agent"
      onCancel={() => void dismissAndClose()}
      onBeforeSave={withDismissal}
      onClose={props.onClose}
    />
  );
};

export default OnboardingModal;
