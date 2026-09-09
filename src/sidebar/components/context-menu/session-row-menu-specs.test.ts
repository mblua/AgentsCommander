// #1871 section 10.6 - pure unit tests over the *Spec helpers. No DOM, no
// jsdom, no store, no harness. The catalogue tests drive finished synthetic
// records and bypass every helper, and phase 1's only live host passes
// `undefined` to five of the ten, so without this file addToGroupSpec and
// taskTitleSpec would ship with one exercised branch each.
import { describe, expect, it, vi } from "vitest";
import type {
  AcAgentReplica,
  AcWorkgroup,
  Session,
  SessionRepo,
  TelegramBotConfig,
} from "../../../shared/types";
import {
  addToGroupSpec,
  clearTaskTitleSpec,
  closeSpec,
  deleteAgentSpec,
  detachSpec,
  matrixFolderSpec,
  openFolderSpec,
  reposSpec,
  taskTitleSpec,
  telegramSpec,
  type AddToGroupData,
  type ReposHandlers,
  type TaskTitleData,
  type TelegramHandlers,
} from "./session-row-menu-specs";
import type { AddToGroupTestIds, GroupChoice } from "./session-row-menu-types";

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: "root-1",
    name: "Agent's Commander",
    shell: "pwsh",
    shellArgs: [],
    effectiveShellArgs: [],
    createdAt: "2026-06-13T00:00:00.000Z",
    workingDirectory: "C:\\Project",
    status: "running",
    waitingForInput: false,
    communication: null,
    pendingReview: false,
    lastPrompt: null,
    agentId: null,
    agentLabel: null,
    gitRepos: [],
    workgroupTask: null,
    isCoordinator: false,
    isRootAgent: true,
    token: "",
    agentKind: null,
    requestedProfile: null,
    effectiveProfile: null,
    profileFallbackChain: [],
    profileFallbackApplied: false,
    ...overrides,
  };
}

const repo = (sourcePath: string, label = "repo"): SessionRepo => ({
  label,
  sourcePath,
  branch: null,
  dirty: null,
});

const noop = (): void => {};
const nullSession = null as unknown as undefined;

const wg: AcWorkgroup = { name: "wg-1-dev-team", path: "C:\\Project\\.ac\\wg-1-dev-team", task: null, agents: [] };
const replica: AcAgentReplica = {
  name: "dev-webpage-ui",
  path: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui",
  repoPaths: [],
  isCoordinator: true,
};
const bot: TelegramBotConfig = { id: "b1", label: "Ops", token: "t", chatId: 1, color: "#123456" };

const reposHandlers: ReposHandlers = { browseItems: () => [], onOpenRepo: noop, onOpenBrowse: noop };
const telegramHandlers: TelegramHandlers = {
  on: true,
  bridgeColor: "#abcdef",
  bots: [bot],
  onSelect: noop,
  onSelectBot: noop,
};
const testIds: AddToGroupTestIds = {
  trigger: "replica.wg-1-dev-team.groups.trigger",
  flyout: "replica.wg-1-dev-team.groups.flyout",
  nonstop: "replica.wg-1-dev-team.groups.nonstop",
  choice: (id) => `replica.wg-1-dev-team.groups.${id}`,
  create: "replica.wg-1-dev-team.groups.create",
  createInput: "replica.groups.create.input",
  createSave: "replica.groups.create.save",
  menuError: "replica.groups.error",
};
const choice: GroupChoice = { id: "g1", name: "Group 1", checked: true, disabled: false, title: "regex" };
const groupData: AddToGroupData = {
  choices: [choice],
  onToggle: noop,
  emptyNote: "No groups yet",
  storeError: "store boom",
  menuError: "menu boom",
  create: { active: false, draft: "", onDraft: noop, onStart: noop, onSave: noop },
  testIds,
};
const titleData: TaskTitleData = {
  editing: true,
  draft: "Title",
  busy: false,
  error: "boom",
  onDraft: noop,
  onStart: noop,
  onSave: noop,
  onCancel: noop,
};

describe("session-row-menu-specs", () => {
  // 1. The absent rule, table-driven over all ten helpers. `toBe(false)`, not a
  //    falsy check: an `undefined` or a `null` return would be a different bug.
  it.each([
    ["reposSpec([])", () => reposSpec([], reposHandlers)],
    ["openFolderSpec(undefined)", () => openFolderSpec(undefined, { onSelect: noop })],
    ["openFolderSpec(null)", () => openFolderSpec(nullSession, { onSelect: noop })],
    ["closeSpec(undefined)", () => closeSpec(undefined, { onSelect: noop })],
    ["closeSpec(null)", () => closeSpec(nullSession, { onSelect: noop })],
    ["detachSpec(false)", () => detachSpec(false, true, noop)],
    ["telegramSpec(undefined)", () => telegramSpec(undefined, telegramHandlers)],
    ["telegramSpec(null)", () => telegramSpec(nullSession, telegramHandlers)],
    ["matrixFolderSpec(undefined)", () => matrixFolderSpec(undefined)],
    ["matrixFolderSpec(null)", () => matrixFolderSpec(null)],
    ["deleteAgentSpec(undefined)", () => deleteAgentSpec(undefined)],
    ["deleteAgentSpec(null)", () => deleteAgentSpec(null)],
    ["addToGroupSpec(undefined)", () => addToGroupSpec(undefined)],
    ["addToGroupSpec(null)", () => addToGroupSpec(null)],
    ["taskTitleSpec(undefined)", () => taskTitleSpec(undefined)],
    ["taskTitleSpec(null)", () => taskTitleSpec(null)],
    ["clearTaskTitleSpec(undefined)", () => clearTaskTitleSpec(undefined)],
    ["clearTaskTitleSpec(null)", () => clearTaskTitleSpec(null)],
  ])("absent rule: %s returns exactly false", (_name, call) => {
    expect(call()).toBe(false);
  });

  // 2. The present rule: the minimum real input yields a spec whose fields equal
  //    the inputs, named individually.
  it("present rule: every helper returns a spec carrying exactly its inputs", () => {
    const live = session();

    const repos = reposSpec([repo("C:\\a", "A")], reposHandlers);
    expect(repos).not.toBe(false);
    if (repos === false) throw new Error("unreachable");
    expect(repos.repos).toEqual([repo("C:\\a", "A")]);
    expect(repos.browseItems).toBe(reposHandlers.browseItems);
    expect(repos.onOpenRepo).toBe(reposHandlers.onOpenRepo);
    expect(repos.onOpenBrowse).toBe(reposHandlers.onOpenBrowse);

    const onOpen = vi.fn();
    const openFolder = openFolderSpec(live, { onSelect: onOpen, label: "Open Root", title: "tip" });
    expect(openFolder).not.toBe(false);
    if (openFolder === false) throw new Error("unreachable");
    expect(openFolder.onSelect).toBe(onOpen);
    expect(openFolder.label).toBe("Open Root");
    expect(openFolder.title).toBe("tip");

    const onClose = vi.fn();
    const close = closeSpec(live, { onSelect: onClose });
    expect(close).not.toBe(false);
    if (close === false) throw new Error("unreachable");
    expect(close.onSelect).toBe(onClose);

    const onDetach = vi.fn();
    const detach = detachSpec(true, true, onDetach);
    expect(detach).not.toBe(false);
    if (detach === false) throw new Error("unreachable");
    expect(detach.on).toBe(true);
    expect(detach.onSelect).toBe(onDetach);
    const attached = detachSpec(true, false, onDetach);
    expect(attached).not.toBe(false);
    if (attached === false) throw new Error("unreachable");
    expect(attached.on).toBe(false);

    const telegram = telegramSpec(live, telegramHandlers);
    expect(telegram).not.toBe(false);
    if (telegram === false) throw new Error("unreachable");
    expect(telegram.on).toBe(true);
    expect(telegram.bridgeColor).toBe("#abcdef");
    expect(telegram.bots).toEqual([bot]);
    expect(telegram.onSelect).toBe(telegramHandlers.onSelect);
    expect(telegram.onSelectBot).toBe(telegramHandlers.onSelectBot);

    const onMatrix = vi.fn();
    const matrix = matrixFolderSpec("C:\\Project\\.ac\\_agent_dev", { onSelect: onMatrix });
    expect(matrix.onSelect).toBe(onMatrix);
    expect(matrix.title).toBe("C:\\Project\\.ac\\_agent_dev");

    const onDelete = vi.fn();
    const del = deleteAgentSpec(replica, { onSelect: onDelete, testId: "agent.action.delete.dev" });
    expect(del.onSelect).toBe(onDelete);
    expect(del.testId).toBe("agent.action.delete.dev");

    const group = addToGroupSpec(wg, groupData);
    expect(group.choices).toEqual([choice]);
    expect(group.onToggle).toBe(groupData.onToggle);
    expect(group.emptyNote).toBe("No groups yet");
    expect(group.storeError).toBe("store boom");
    expect(group.menuError).toBe("menu boom");
    expect(group.create).toBe(groupData.create);
    expect(group.testIds).toBe(testIds);

    const title = taskTitleSpec(wg, titleData);
    expect(title.editing).toBe(true);
    expect(title.draft).toBe("Title");
    expect(title.busy).toBe(false);
    expect(title.error).toBe("boom");
    expect(title.onDraft).toBe(titleData.onDraft);
    expect(title.onStart).toBe(titleData.onStart);
    expect(title.onSave).toBe(titleData.onSave);
    expect(title.onCancel).toBe(titleData.onCancel);

    const onClear = vi.fn();
    const clear = clearTaskTitleSpec(wg, { onSelect: onClear, disabled: true, title: "clean" });
    expect(clear.onSelect).toBe(onClear);
    expect(clear.disabled).toBe(true);
    expect(clear.title).toBe("clean");
  });

  // 3. reposSpec's filter, the one helper with real logic. Matches
  //    SessionItem's repoMenuEntries filter (section 5.5 parity).
  describe("reposSpec filter", () => {
    it("[] -> false", () => {
      expect(reposSpec([], reposHandlers)).toBe(false);
    });
    it('[{sourcePath: ""}] -> false', () => {
      expect(reposSpec([repo("")], reposHandlers)).toBe(false);
    });
    it('[{sourcePath: "   "}] -> false (whitespace is blank)', () => {
      expect(reposSpec([repo("   ")], reposHandlers)).toBe(false);
    });
    it("[{sourcePath: undefined}] -> false", () => {
      const broken = { label: "x", sourcePath: undefined, branch: null, dirty: null } as unknown as SessionRepo;
      expect(reposSpec([broken], reposHandlers)).toBe(false);
    });
    it("two blank and one real -> a spec whose repos has length 1 and is the real one", () => {
      const real = repo("C:\\real", "real");
      const spec = reposSpec([repo(""), real, repo("  ")], reposHandlers);
      expect(spec).not.toBe(false);
      if (spec === false) throw new Error("unreachable");
      expect(spec.repos).toHaveLength(1);
      expect(spec.repos[0]).toBe(real);
    });
    it("two real -> length 2 in input order", () => {
      const first = repo("C:\\first", "first");
      const second = repo("C:\\second", "second");
      const spec = reposSpec([first, second], reposHandlers);
      expect(spec).not.toBe(false);
      if (spec === false) throw new Error("unreachable");
      expect(spec.repos).toHaveLength(2);
      expect(spec.repos[0]).toBe(first);
      expect(spec.repos[1]).toBe(second);
    });
  });

  // 4. openFolderSpec and closeSpec are NOT live-gated; telegramSpec does NOT
  //    re-check liveness (its host applies that gate before calling it).
  it("openFolderSpec, closeSpec and telegramSpec accept a dormant session", () => {
    const dormantSession = session({ status: null as unknown as Session["status"] });
    expect(typeof dormantSession.status).not.toBe("string");
    expect(openFolderSpec(dormantSession, { onSelect: noop })).not.toBe(false);
    expect(closeSpec(dormantSession, { onSelect: noop })).not.toBe(false);
    expect(telegramSpec(dormantSession, telegramHandlers)).not.toBe(false);
  });

  // 5. The overload set: the off call takes one argument and returns false; the
  //    on call cannot forget its bag. `@ts-expect-error` is a real assertion
  //    here because AC 1 typechecks the test files too.
  it("overloads: one-argument off calls compile and return false; on calls require the bag", () => {
    expect(addToGroupSpec(undefined)).toBe(false);
    // @ts-expect-error - a workgroup without its data bag matches no overload
    expect(addToGroupSpec(wg)).toBe(false);

    expect(taskTitleSpec(undefined)).toBe(false);
    // @ts-expect-error - a workgroup without its data bag matches no overload
    expect(taskTitleSpec(wg)).toBe(false);

    expect(clearTaskTitleSpec(undefined)).toBe(false);
    // @ts-expect-error - a workgroup without its options bag matches no overload
    expect(clearTaskTitleSpec(wg)).toBe(false);

    expect(matrixFolderSpec(undefined)).toBe(false);
    // @ts-expect-error - a path without its handler matches no overload
    expect(matrixFolderSpec("C:\\matrix")).toBe(false);

    expect(deleteAgentSpec(undefined)).toBe(false);
    // @ts-expect-error - a replica without its handlers matches no overload
    expect(deleteAgentSpec(replica)).toBe(false);
  });

  // 6. addToGroupSpec carries both errors and all eight testids, distinctly.
  it("addToGroupSpec carries both error rows and all eight testids", () => {
    const spec = addToGroupSpec(wg, groupData);
    expect(spec.storeError).toBe("store boom");
    expect(spec.menuError).toBe("menu boom");
    expect(spec.storeError).not.toBe(spec.menuError);
    expect(spec.testIds.trigger).toBe("replica.wg-1-dev-team.groups.trigger");
    expect(spec.testIds.flyout).toBe("replica.wg-1-dev-team.groups.flyout");
    expect(spec.testIds.nonstop).toBe("replica.wg-1-dev-team.groups.nonstop");
    expect(spec.testIds.choice("g1")).toBe("replica.wg-1-dev-team.groups.g1");
    expect(spec.testIds.create).toBe("replica.wg-1-dev-team.groups.create");
    expect(spec.testIds.createInput).toBe("replica.groups.create.input");
    expect(spec.testIds.createSave).toBe("replica.groups.create.save");
    expect(spec.testIds.menuError).toBe("replica.groups.error");
  });

  // 7. taskTitleSpec carries all four callbacks and all four data fields.
  it("taskTitleSpec carries four callbacks and four data fields", () => {
    const onDraft = vi.fn();
    const onStart = vi.fn();
    const onSave = vi.fn();
    const onCancel = vi.fn();
    const spec = taskTitleSpec(wg, { ...titleData, onDraft, onStart, onSave, onCancel });
    expect(spec.editing).toBe(true);
    expect(spec.draft).toBe("Title");
    expect(spec.busy).toBe(false);
    expect(spec.error).toBe("boom");
    spec.onDraft("new");
    expect(onDraft).toHaveBeenCalledTimes(1);
    expect(onDraft).toHaveBeenCalledWith("new");
    expect(onStart).not.toHaveBeenCalled();
    spec.onStart();
    expect(onStart).toHaveBeenCalledTimes(1);
    expect(onSave).not.toHaveBeenCalled();
    spec.onSave();
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
    spec.onCancel();
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  // 8. No helper reads anything but its arguments, and none mutates them.
  it("helpers are pure: equal outputs for equal inputs, frozen inputs untouched", () => {
    const live = session();
    const frozenRepos = Object.freeze([Object.freeze(repo("C:\\a", "A")), Object.freeze(repo("", "blank"))]) as SessionRepo[];
    const frozenHandlers = Object.freeze({ ...reposHandlers }) as ReposHandlers;
    const frozenGroup = Object.freeze({ ...groupData }) as AddToGroupData;
    const frozenTitle = Object.freeze({ ...titleData }) as TaskTitleData;
    const frozenTelegram = Object.freeze({ ...telegramHandlers }) as TelegramHandlers;
    const frozenWg = Object.freeze({ ...wg }) as AcWorkgroup;

    const calls: Array<() => unknown> = [
      () => reposSpec(frozenRepos, frozenHandlers),
      () => openFolderSpec(live, { onSelect: noop, label: "L", title: "T" }),
      () => closeSpec(live, { onSelect: noop }),
      () => detachSpec(true, false, noop),
      () => telegramSpec(live, frozenTelegram),
      () => matrixFolderSpec("C:\\matrix", { onSelect: noop }),
      () => deleteAgentSpec(replica, { onSelect: noop, testId: "t" }),
      () => addToGroupSpec(frozenWg, frozenGroup),
      () => taskTitleSpec(frozenWg, frozenTitle),
      () => clearTaskTitleSpec(frozenWg, { onSelect: noop, disabled: false, title: "x" }),
    ];
    for (const call of calls) {
      expect(call()).toEqual(call());
    }

    expect(Object.isFrozen(frozenRepos)).toBe(true);
    expect(frozenRepos).toHaveLength(2);
    expect(frozenRepos[0].sourcePath).toBe("C:\\a");
    expect(frozenRepos[1].sourcePath).toBe("");
    expect(Object.isFrozen(frozenGroup)).toBe(true);
    expect(frozenGroup.storeError).toBe("store boom");
    expect(Object.isFrozen(frozenTitle)).toBe(true);
    expect(frozenTitle.draft).toBe("Title");
    expect(Object.isFrozen(frozenTelegram)).toBe(true);
    expect(frozenTelegram.bridgeColor).toBe("#abcdef");
    expect(Object.isFrozen(frozenWg)).toBe(true);
    expect(frozenWg.name).toBe("wg-1-dev-team");
  });
});
