// @vitest-environment jsdom
import { describe, expect } from "vitest";
import SessionItem from "./SessionItem";
import { baseSettings, click, contextMenu, session, waitFor } from "../../shared/testing/ui-harness";
import { describeRestartToast } from "./restart-toast-harness";

const sessionId = "sess-1";
const restartSelector = `[data-ac-testid="session.${sessionId}.restart"]`;

describe("SessionItem Restart Session toast (#2573)", () => {
  describeRestartToast({
    failName: "restarting_a_session_row_shows_the_plain_error_toast",
    okName: "a_successful_session_row_restart_shows_no_toast",
    setup: (fake) => fake.resolve("get_settings", baseSettings()),
    ui: () => <SessionItem session={session({ id: sessionId })} isActive={false} />,
    trigger: async (root) => {
      contextMenu(root.firstElementChild!);
      await waitFor(() => expect(document.body.querySelector(restartSelector)).toBeTruthy());
      click(document.body.querySelector(restartSelector)!);
    },
  });
});
