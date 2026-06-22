import { Component } from "solid-js";

/**
 * #592 - the drift "Reload" affordance, shown when a session's loaded profile
 * cell no longer matches its current configuration (the backend-computed
 * profileOutdated flag). Clicking relaunches the session with the current
 * config, which re-stamps the content hash and clears the flag.
 *
 * Shared by SessionItem, the ProjectPanel replica rows, and RootAgentBanner so
 * the badge looks and behaves identically everywhere a coding-agent session can
 * drift. The caller supplies the reload action because each surface restarts
 * through a different path (sidebar restart vs replica restart vs root wake);
 * stopPropagation is handled here so a badge click never selects the row.
 */
const ProfileOutdatedBadge: Component<{
  onReload: () => void;
  testId?: string;
}> = (props) => {
  return (
    <button
      type="button"
      class="profile-outdated-badge"
      title="Loaded profile no longer matches its configuration. Reload to relaunch with the current profile."
      data-ac-testid={props.testId}
      onClick={(e) => {
        e.stopPropagation();
        props.onReload();
      }}
    >
      &#x27F3; outdated
    </button>
  );
};

export default ProfileOutdatedBadge;
