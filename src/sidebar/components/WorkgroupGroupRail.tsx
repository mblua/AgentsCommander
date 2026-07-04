import { Component, For, Show, createEffect, createMemo, createSignal } from "solid-js";
import type { AcWorkgroup, WorkgroupGroup } from "../../shared/types";
import type { ProjectState } from "../stores/project";
import {
  MAX_GROUP_MATCH_ID_LENGTH,
  compileGroupRegex,
  groupMatchId,
  workgroupGroupsStore,
  type WorkgroupGroupSelection,
} from "../stores/workgroup-groups";
import {
  isWorkingReplicaDot,
  replicaDotClass,
  workgroupHasRaisedHand,
  workgroupIsWorking,
} from "./workgroup-session";
import WorkgroupGroupsModal from "./WorkgroupGroupsModal";

interface WorkgroupGroupRailProps {
  projects: ProjectState[];
}

interface GroupButton {
  key: string;
  name: string;
  counter: string;
  /** #746 — true when the button's workgroup set has >=1 working agent
   *  (same predicate as the counter's X: running/active, not waiting/pending). */
  working: boolean;
  /** #763 — true when >=1 coordinator in the button's workgroup set has a
   *  raised hand (mirrors ProjectPanel's per-row `showRaiseHand`). */
  raiseHand: boolean;
  selection: WorkgroupGroupSelection;
  workgroups: AcWorkgroup[];
  title: string;
}

function wgNumber(name: string): number {
  const match = name.match(/^wg-(\d+)/i);
  return match ? Number.parseInt(match[1], 10) : Number.MAX_SAFE_INTEGER;
}

function wgTooltipLabel(wgName: string): string {
  const match = wgName.match(/^wg-(\d+)/i);
  return match ? `WG${match[1]}` : wgName;
}

function groupMatches(group: WorkgroupGroup, wg: AcWorkgroup): boolean {
  const id = groupMatchId(wg);
  if (id.length > MAX_GROUP_MATCH_ID_LENGTH) return false;
  const regex = compileGroupRegex(group);
  return !!regex?.test(id);
}

function tooltipFor(workgroups: AcWorkgroup[]): string {
  const rows = workgroups
    .flatMap((wg) =>
      wg.agents
        .filter((replica) => isWorkingReplicaDot(replicaDotClass(wg, replica)))
        .map((replica) => ({ wg, replica }))
    )
    .sort((a, b) => {
      const wgDelta = wgNumber(a.wg.name) - wgNumber(b.wg.name);
      if (wgDelta !== 0) return wgDelta;
      return a.replica.name.localeCompare(b.replica.name, "en", { sensitivity: "base", numeric: true });
    })
    .map(({ wg, replica }) => `${wgTooltipLabel(wg.name)}:(${replica.name})`);
  return rows.length > 0 ? rows.join("\n") : "No running agents";
}

function buttonContent(
  name: string,
  workgroups: AcWorkgroup[]
): Pick<GroupButton, "name" | "counter" | "working" | "raiseHand"> {
  const working = workgroups.filter(workgroupIsWorking).length;
  return {
    name,
    counter: `${working}/${workgroups.length}`,
    working: working > 0,
    raiseHand: workgroups.some(workgroupHasRaisedHand),
  };
}

const ProjectRailSection: Component<{
  project: ProjectState;
  showProjectLabel: boolean;
}> = (props) => {
  const [editing, setEditing] = createSignal(false);

  createEffect(() => {
    void workgroupGroupsStore.ensureLoaded(props.project.path);
  });

  const config = () => workgroupGroupsStore.config(props.project.path);
  const selection = () => workgroupGroupsStore.selection(props.project.path);
  const ungroupedWorkgroups = createMemo(() =>
    props.project.workgroups.filter(
      (wg) => !config().groups.some((group) => groupMatches(group, wg))
    )
  );
  const buttons = createMemo<GroupButton[]>(() => {
    const result: GroupButton[] = [];
    if (config().showAll) {
      result.push({
        key: "all",
        ...buttonContent("All", props.project.workgroups),
        // #775 — the built-in "All" group never shows the raise-hand indicator,
        // under any condition (even when a member workgroup's coordinator has a
        // raised hand). Gated here on the statically-built "all" entry, not on
        // the display name, so a user group coincidentally named "All" is
        // unaffected. Ungrouped + dynamic groups keep their aggregation below.
        raiseHand: false,
        selection: { kind: "all" },
        workgroups: props.project.workgroups,
        title: tooltipFor(props.project.workgroups),
      });
    }
    if (config().showUngrouped) {
      const workgroups = ungroupedWorkgroups();
      result.push({
        key: "ungrouped",
        ...buttonContent("Ungrouped", workgroups),
        selection: { kind: "ungrouped" },
        workgroups,
        title: tooltipFor(workgroups),
      });
    }
    for (const group of config().groups) {
      const workgroups = props.project.workgroups.filter((wg) => groupMatches(group, wg));
      result.push({
        key: group.id,
        ...buttonContent(group.name, workgroups),
        selection: { kind: "group", id: group.id },
        workgroups,
        title: tooltipFor(workgroups),
      });
    }
    return result;
  });
  const selected = (button: GroupButton) => {
    const current = selection();
    if (current.kind !== button.selection.kind) return false;
    return current.kind !== "group" || button.selection.kind !== "group" || current.id === button.selection.id;
  };

  return (
    <div class="workgroup-group-rail-project" data-ac-testid={`workgroupGroups.rail.${props.project.folderName}`}>
      <Show when={props.showProjectLabel}>
        <div class="workgroup-group-rail-project-label" title={props.project.path}>
          {props.project.folderName}
        </div>
      </Show>

      <For each={buttons()}>
        {(button) => (
          <button
            class="workgroup-group-rail-button"
            classList={{ selected: selected(button) }}
            aria-pressed={selected(button)}
            title={button.title}
            onClick={() => workgroupGroupsStore.select(props.project.path, button.selection)}
            data-ac-testid={`workgroupGroups.button.${button.key}`}
          >
            <span class="workgroup-group-rail-title-line">
              <Show when={button.raiseHand}>
                <span
                  class="workgroup-group-rail-raise-hand"
                  data-ac-testid={`workgroupGroups.raiseHand.${button.key}`}
                  title="A coordinator raised its hand"
                  aria-label="A coordinator raised its hand"
                >
                  <svg
                    class="workgroup-group-rail-raise-hand-icon"
                    viewBox="0 0 24 24"
                    fill="currentColor"
                    aria-hidden="true"
                  >
                    <path d="M10.5 1.875a1.125 1.125 0 0 1 2.25 0v8.219c.517.162 1.02.382 1.5.659V3.375a1.125 1.125 0 0 1 2.25 0v10.937a4.505 4.505 0 0 0-3.25 2.373 8.963 8.963 0 0 1 4-.935A.75.75 0 0 0 18 15v-2.266a3.368 3.368 0 0 1 .988-2.37 1.125 1.125 0 0 1 1.591 1.59 1.118 1.118 0 0 0-.329.79v3.006h-.005a6 6 0 0 1-1.752 4.007l-1.736 1.736a6 6 0 0 1-4.242 1.757H10.5a7.5 7.5 0 0 1-7.5-7.5V6.375a1.125 1.125 0 0 1 2.25 0v5.519c.46-.452.965-.832 1.5-1.141V3.375a1.125 1.125 0 0 1 2.25 0v6.526c.495-.1.997-.151 1.5-.151V1.875Z" />
                  </svg>
                </span>
              </Show>
              <span class="workgroup-group-rail-title">{button.name}</span>
            </span>
            <span class="workgroup-group-rail-counter-line">
              <Show when={button.working}>
                <span
                  class="session-item-status running workgroup-group-rail-dot"
                  data-ac-testid={`workgroupGroups.dot.${button.key}`}
                />
              </Show>
              {button.counter}
            </span>
          </button>
        )}
      </For>

      <Show when={workgroupGroupsStore.error(props.project.path)}>
        {(error) => (
          <div class="workgroup-group-rail-error" title={error()}>
            !
          </div>
        )}
      </Show>

      <button
        class="workgroup-group-rail-button edit"
        onClick={() => setEditing(true)}
        data-ac-testid="workgroupGroups.edit"
      >
        Edit
      </button>

      <Show when={editing()}>
        <WorkgroupGroupsModal
          projectPath={props.project.path}
          projectName={props.project.folderName}
          onClose={() => setEditing(false)}
        />
      </Show>
    </div>
  );
};

const WorkgroupGroupRail: Component<WorkgroupGroupRailProps> = (props) => {
  const showProjectLabels = () => props.projects.length > 1;

  return (
    <aside class="workgroup-group-rail" data-ac-testid="workgroupGroups.rail">
      <For each={props.projects}>
        {(project) => (
          <ProjectRailSection project={project} showProjectLabel={showProjectLabels()} />
        )}
      </For>
    </aside>
  );
};

export default WorkgroupGroupRail;
