// #3832 - controlled, searchable "Test on" picker for the preflight test panel.
// Follows the NewWorkgroupModal Team combobox. No IPC and no stores: the panel
// owns the rows, the chosen path and the disabled state.
import {
  type Component, For, Show, createEffect, createSignal, createUniqueId, on, onMount,
} from "solid-js";
import type { PreflightTestTarget } from "../../../shared/preflightTypes";

const NO_MATCH = "No agents match your search.";

/** #3832 - the last path segment, for either slash style. */
export function targetBasename(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** #3832 - the Profile line of one picker row; fallback replaces the plain line. */
export function targetProfileText(row: PreflightTestTarget): string {
  return row.fallbackApplied
    ? `Asks for Profile ${row.requestedProfile}, launches with Profile ${row.effectiveProfile}.`
    : `Launches with Profile ${row.effectiveProfile}`;
}

/** #3832 - case-insensitive substring match on the complete path; empty keeps all. */
export function searchTargets(
  rows: readonly PreflightTestTarget[], query: string,
): PreflightTestTarget[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [...rows];
  return rows.filter((row) => row.replicaPath.toLowerCase().includes(needle));
}

/** #3832 - next enabled row after `from` in `step` direction, clamped at the ends.
 *  With no active row, either key starts at the first enabled row. */
export function nextEnabledIndex(
  rows: readonly PreflightTestTarget[], from: number, step: 1 | -1,
): number {
  const dir = from < 0 ? 1 : step;
  for (let i = from < 0 ? 0 : from + step; i >= 0 && i < rows.length; i += dir) {
    if (!rows[i]?.running) return i;
  }
  return from >= 0 && from < rows.length && !rows[from]?.running ? from : -1;
}

/** Lets the panel read and set the list's open state (Escape arbitration). */
export interface PreflightTargetPickerControl {
  isOpen: () => boolean;
  setOpen: (open: boolean) => void;
}

export interface PreflightTargetPickerProps {
  rows: readonly PreflightTestTarget[];
  selected: string;
  disabled: boolean;
  hint?: string;
  onChoose: (path: string) => void;
  onSearchEdit: () => void;
  onControl?: (control: PreflightTargetPickerControl) => void;
}

const optionState = (p: { chosen: boolean; active: boolean }) => {
  if (p.chosen) return "chosen";
  return p.active ? "active" : "inactive";
};

const TargetOption: Component<{
  id: string;
  row: PreflightTestTarget;
  active: boolean;
  chosen: boolean;
  index: number;
  onChoose: (row: PreflightTestTarget) => void;
}> = (props) => (
  <div
    id={props.id}
    class="preflight-target-option"
    role="option"
    aria-selected={props.active}
    aria-disabled={props.row.running ? "true" : undefined}
    data-ac-testid={`settings.preflightTest.target.option.${props.index}`}
    data-ac-role="text"
    data-ac-detail={props.row.replicaPath}
    data-ac-state={optionState(props)}
    onMouseDown={(e) => e.preventDefault()}
    onClick={() => props.onChoose(props.row)}
  >
    <div class="preflight-target-name">
      {targetBasename(props.row.replicaPath)}{props.row.running ? " (running)" : ""}
    </div>
    <div class="preflight-target-path">{props.row.replicaPath}</div>
    <div class="preflight-target-profile">{targetProfileText(props.row)}</div>
  </div>
);

export const PreflightTargetPicker: Component<PreflightTargetPickerProps> = (props) => {
  const listId = createUniqueId();
  const optionId = (index: number) => `${listId}-option-${index}`;
  const [query, setQuery] = createSignal(props.selected);
  const [open, setOpenSignal] = createSignal(false);
  const [activeIndex, setActiveIndex] = createSignal(-1);
  let composing = false;
  let listElement: HTMLDivElement | undefined;

  const matches = () => searchTargets(props.rows, query());
  const setOpen = (value: boolean) => {
    setOpenSignal(value && !props.disabled);
    setActiveIndex(-1);
  };
  const choose = (row: PreflightTestTarget) => {
    if (row.running || props.disabled) return;
    setQuery(row.replicaPath);
    setOpen(false);
    props.onChoose(row.replicaPath);
  };
  const move = (step: 1 | -1) => {
    if (!open()) setOpenSignal(!props.disabled);
    const index = nextEnabledIndex(matches(), activeIndex(), step);
    setActiveIndex(index);
    if (index >= 0) listElement?.children[index]?.scrollIntoView?.({ block: "nearest" });
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.isComposing || composing) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      move(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.key === "Enter" && open()) {
      e.preventDefault();
      e.stopPropagation();
      const row = matches()[activeIndex()];
      if (row) choose(row);
    } else if (e.key === "Escape" && open()) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    } else if (e.key === "Tab") {
      setOpen(false);
    }
  };
  const onInput = (e: InputEvent & { currentTarget: HTMLInputElement }) => {
    setQuery(e.currentTarget.value);
    setOpenSignal(!props.disabled);
    setActiveIndex(-1);
    props.onSearchEdit();
  };

  // A chosen path shows in the input; a cleared choice keeps the typed query.
  createEffect(on(() => props.selected, (path) => { if (path) setQuery(path); }, { defer: true }));
  createEffect(on(() => props.disabled, (disabled) => { if (disabled) setOpen(false); }));
  createEffect(on(() => props.rows, () => setActiveIndex(-1), { defer: true }));
  onMount(() => props.onControl?.({ isOpen: open, setOpen }));

  return (
    <div class="settings-field">
      <label class="settings-label" for={`${listId}-input`}>Test on</label>
      <Show when={props.hint}>
        <div class="settings-label-hint" data-ac-testid="settings.preflightTest.filter">
          {props.hint}
        </div>
      </Show>
      <input
        id={`${listId}-input`}
        type="text"
        class="settings-input"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open()}
        aria-controls={listId}
        aria-activedescendant={activeIndex() >= 0 ? optionId(activeIndex()) : undefined}
        autocomplete="off"
        placeholder="Search agents..."
        value={query()}
        disabled={props.disabled}
        onInput={onInput}
        onFocus={() => { if (!open()) setOpen(true); }}
        onBlur={() => { setOpen(false); composing = false; }}
        on:keydown={onKeyDown}
        onCompositionStart={() => { composing = true; }}
        onCompositionEnd={() => { composing = false; }}
        data-ac-testid="settings.preflightTest.target"
        data-ac-role="combobox"
        data-ac-state={props.selected ? "confirmed" : "unconfirmed"}
      />
      <div
        ref={listElement}
        id={listId}
        class="preflight-target-list"
        role="listbox"
        aria-label="Test on"
        hidden={!open()}
        data-ac-testid="settings.preflightTest.target.list"
      >
        <For each={matches()}>
          {(row, index) => (
            <TargetOption
              id={optionId(index())}
              row={row}
              index={index()}
              active={index() === activeIndex()}
              chosen={row.replicaPath === props.selected}
              onChoose={choose}
            />
          )}
        </For>
        <Show when={matches().length === 0}>
          <div class="preflight-target-empty" role="status"
            data-ac-testid="settings.preflightTest.target.empty">
            {NO_MATCH}
          </div>
        </Show>
      </div>
    </div>
  );
};
