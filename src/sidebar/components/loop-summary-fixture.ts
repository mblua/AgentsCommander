import type { AcLoopSummary } from "../../shared/types";

const projectPath = "C:\\Project";

/**
 * Shared `AcLoopSummary` fixture for the ProjectPanel loop tests.
 *
 * Both `ProjectPanel.collapse-state.test.tsx` and
 * `ProjectPanel.regex-filter.test.tsx` carried an identical copy of this
 * factory, which the test-code duplication gate flags as a cross-file clone.
 * See `docs/testing/test-code-duplication.md`.
 */
export function loopSummaryFixture(overrides: Partial<AcLoopSummary> = {}): AcLoopSummary {
  return {
    id: "loop-standup",
    name: "Weekday standup",
    enabled: true,
    expr: "0 9 * * 1-5",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: "wg-2-dev-team",
    promptPreview: "scheduled run",
    busyCoordinator: "skip",
    sessionStart: "fresh",
    path: `${projectPath}\\.ac\\_loop_standup`,
    configPath: `${projectPath}\\.ac\\_loop_standup\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
    ...overrides,
  };
}
