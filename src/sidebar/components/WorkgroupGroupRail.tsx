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
  workgroupIsWorking,
} from "./workgroup-session";
import WorkgroupGroupsModal from "./WorkgroupGroupsModal";

interface WorkgroupGroupRailProps {
  projects: ProjectState[];
}

interface GroupButton {
  key: string;
  label: string;
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

function counterLabel(name: string, workgroups: AcWorkgroup[]): string {
  const working = workgroups.filter(workgroupIsWorking).length;
  return `${name}\n${working}/${workgroups.length}`;
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
        label: counterLabel("All", props.project.workgroups),
        selection: { kind: "all" },
        workgroups: props.project.workgroups,
        title: tooltipFor(props.project.workgroups),
      });
    }
    for (const group of config().groups) {
      const workgroups = props.project.workgroups.filter((wg) => groupMatches(group, wg));
      result.push({
        key: group.id,
        label: counterLabel(group.name, workgroups),
        selection: { kind: "group", id: group.id },
        workgroups,
        title: tooltipFor(workgroups),
      });
    }
    if (config().showUngrouped) {
      const workgroups = ungroupedWorkgroups();
      result.push({
        key: "ungrouped",
        label: counterLabel("Ungrouped", workgroups),
        selection: { kind: "ungrouped" },
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
            {button.label}
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
