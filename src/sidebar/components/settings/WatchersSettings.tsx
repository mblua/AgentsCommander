import { Component, For, Show, createEffect, createSignal, onCleanup } from "solid-js";
import { PtyAPI } from "../../../shared/ipc";
import type {
  Session, WatcherConfig, WatcherEntry, WatcherPatternPreview,
  WatcherReachEntry, WatcherReachRow,
} from "../../../shared/types";
import { sessionsStore } from "../../stores/sessions";
import TrashIcon from "../TrashIcon";
import {
  canEnableWatcher, isWatcherConfig, selectorMode, toggleCommandStem, validateWatcherId,
  watcherBudgetNotice, watcherReachSummary, withSelectorMode,
} from "../settings-watchers";

interface Props {
  watcherIds: string[];
  unreadableWatcherIds: string[];
  watcherMap: Record<string, WatcherEntry>;
  commandStems: string[];
  agentSessions: Session[];
  reachAnswer: WatcherReachRow[] | null;
  reachByWatcherId: Map<string, WatcherReachEntry[]>;
  reachError: string;
  onChange: (id: string, config: WatcherConfig) => void;
  onRename: (fromId: string, toId: string) => void;
  onRemove: (id: string) => void;
  onAdd: () => void;
}

const errorMessage = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

/** #1171 - how long the pattern and reach previews wait before asking the backend. */
export const WATCHER_PREVIEW_DEBOUNCE_MS = 300;

/** #1171 - the sentinel of the "no session" option, which exercises the compile-only path. */
const WATCHER_NO_SESSION = "";

/**
 * #1171 - one row of the Watchers tab.
 *
 * A component rather than a `render*` helper because each row owns a debounced pattern
 * preview and its timer; inside `<For>` every row gets its own reactive scope, so the cleanup
 * is the framework's job instead of a bookkeeping map keyed by watcher id.
 *
 * Reach is NOT owned here. Whether a row holds one of an agent's 8 slots depends on every
 * other row of the draft and on the draft's agents, so one call for the whole section answers
 * every row at once and each row renders the `entries` its `id` indexes.
 */
const WatcherRow: Component<{
  id: string;
  config: WatcherConfig;
  otherIds: string[];
  stems: string[];
  sessions: Session[];
  /** This row's half of the section's answer; `null` while none is displayed. */
  reach: WatcherReachEntry[] | null;
  reachError: string;
  onChange: (next: WatcherConfig) => void;
  onRename: (nextId: string) => void;
  onRemove: () => void;
}> = (props) => {
  const [renameDraft, setRenameDraft] = createSignal(props.id);
  const [renaming, setRenaming] = createSignal(false);
  const [preview, setPreview] = createSignal<WatcherPatternPreview | null>(null);
  const [previewError, setPreviewError] = createSignal("");
  const [testSessionId, setTestSessionId] = createSignal(WATCHER_NO_SESSION);

  // Preselect the active session when it qualifies, i.e. when it is an agent session. A
  // plain shell has a screen but nothing a watcher would ever be configured against.
  //
  // Reactive rather than read once on mount: the modal is opened by `emitOpenSettings` and by
  // tests before the session store has hydrated, and a one-shot read would then preselect
  // nothing for the rest of the modal's life. It stops the moment the user picks, so a later
  // hydration never overrides a deliberate choice.
  let sessionPickedByUser = false;
  createEffect(() => {
    if (sessionPickedByUser) return;
    const active = props.sessions.find((s) => s.id === sessionsStore.activeId);
    if (active) setTestSessionId(active.id);
  });

  /** An empty pattern is a valid regex that matches every row, so enabling is gated on one. */
  const enableBlocked = () => !props.config.enabled && !canEnableWatcher(props.config);

  const renameError = () =>
    renameDraft() === props.id ? null : validateWatcherId(renameDraft(), props.otherIds);

  const applyRename = () => {
    if (renameError() || renameDraft() === props.id) return;
    props.onRename(renameDraft());
    setRenaming(false);
  };

  // Debounced and generation-guarded: the user types, and a slow answer for an older pattern
  // must never overwrite the answer for the current one.
  let previewGeneration = 0;
  createEffect(() => {
    const pattern = props.config.pattern;
    const sessionId = testSessionId();
    // Taken BEFORE the empty-pattern branch. Clearing the preview without advancing the
    // generation leaves an answer for the previous pattern still entitled to paint, so a
    // slow "Compiles. Matches 30 of 30 rows." lands on a row that no longer has a pattern.
    const generation = (previewGeneration += 1);
    if (!pattern) {
      setPreview(null);
      setPreviewError("");
      return;
    }
    const timer = setTimeout(() => {
      PtyAPI.previewWatcherPattern(pattern, sessionId || undefined)
        .then((result) => {
          if (generation !== previewGeneration) return;
          setPreview(result);
          setPreviewError("");
        })
        .catch((err: unknown) => {
          if (generation !== previewGeneration) return;
          setPreview(null);
          setPreviewError(errorMessage(err));
        });
    }, WATCHER_PREVIEW_DEBOUNCE_MS);
    onCleanup(() => clearTimeout(timer));
  });

  const update = <K extends keyof WatcherConfig>(key: K, value: WatcherConfig[K]) =>
    props.onChange({ ...props.config, [key]: value });

  return (
    <div
      class="settings-section"
      data-ac-testid={`settings.watchers.row.${props.id}`}
      data-ac-role="group"
      data-ac-state={props.config.enabled ? "enabled" : "disabled"}
    >
      <div class="settings-color-row">
        <span class="settings-section-title">{props.id}</span>
        <button
          class="settings-agent-remove"
          onClick={props.onRemove}
          title={`Delete "${props.id}"`}
          aria-label={`Delete watcher ${props.id}`}
          data-ac-testid={`settings.watchers.remove.${props.id}`}
          data-ac-role="button"
        >
          <TrashIcon />
        </button>
      </div>

      {/* Refused rather than merely discouraged: an empty pattern matches every row, so
          enabling one would fill the per-tick caps, turn the ring over, go degraded and
          displace a useful watcher out of an agent's budget. Disabling stays available even
          when the pattern is empty, which is the state a hand-written file can arrive in. */}
      <label class="settings-checkbox-field">
        <input
          type="checkbox"
          class="settings-checkbox"
          checked={props.config.enabled}
          disabled={enableBlocked()}
          onChange={(e) => {
            if (e.currentTarget.checked && !canEnableWatcher(props.config)) {
              e.currentTarget.checked = false;
              return;
            }
            update("enabled", e.currentTarget.checked);
          }}
          data-ac-testid={`settings.watchers.enabled.${props.id}`}
          data-ac-role="checkbox"
          data-ac-state={enableBlocked() ? "blocked" : "available"}
        />
        <span>Enabled</span>
      </label>

      <label class="settings-field">
        <span class="settings-label">Mode</span>
        <select
          class="settings-input"
          value={props.config.mode}
          onChange={(e) =>
            update("mode", e.currentTarget.value as WatcherConfig["mode"])
          }
          data-ac-testid={`settings.watchers.mode.${props.id}`}
          data-ac-role="combobox"
        >
          <option value="occurrence">Occurrence — every match is an event</option>
          <option value="state">State — a reading, re-reported only when it changes</option>
        </select>
      </label>

      <label class="settings-field">
        <span class="settings-label">Pattern</span>
        <input
          class="settings-input"
          type="text"
          value={props.config.pattern}
          onInput={(e) => update("pattern", e.currentTarget.value)}
          placeholder="Read \((.+)\)"
          spellcheck={false}
          data-ac-testid={`settings.watchers.pattern.${props.id}`}
          data-ac-role="textbox"
        />
      </label>
      {/* The pattern is handed to the engine verbatim, never trimmed: a leading space is an
          anchor, and the pattern is the only thing keeping a watcher narrow. */}
      <div class="settings-label-hint">
        Taken exactly as written, including leading and trailing spaces.
      </div>

      <div class="settings-field">
        <span class="settings-label">Applies to</span>
        <select
          class="settings-input"
          value={selectorMode(props.config)}
          onChange={(e) =>
            props.onChange(
              withSelectorMode(
                props.config,
                e.currentTarget.value === "all" ? "all" : "selected"
              )
            )
          }
          data-ac-testid={`settings.watchers.selectorMode.${props.id}`}
          data-ac-role="combobox"
        >
          <option value="all">All agents</option>
          <option value="selected">Selected commands</option>
        </select>
      </div>

      <Show when={selectorMode(props.config) === "selected"}>
        <div
          class="settings-api-client-scopes"
          data-ac-testid={`settings.watchers.stems.${props.id}`}
          data-ac-role="group"
        >
          <For each={props.stems}>
            {(stem) => (
              <label class="settings-api-client-scope">
                <input
                  type="checkbox"
                  class="settings-checkbox"
                  checked={(props.config.commands ?? []).includes(stem)}
                  onChange={() => props.onChange(toggleCommandStem(props.config, stem))}
                  data-ac-testid={`settings.watchers.stem.${props.id}.${stem}`}
                  data-ac-role="checkbox"
                />
                <span>{stem}</span>
              </label>
            )}
          </For>
        </div>
        {/* A stem no agent currently has is not an error: it simply reaches nobody until
            such an agent exists. Free text is how that case is expressible at all. */}
        <label class="settings-field">
          <span class="settings-label">Commands</span>
          <input
            class="settings-input"
            type="text"
            value={(props.config.commands ?? []).join(", ")}
            onInput={(e) =>
              update(
                "commands",
                e.currentTarget.value
                  .split(",")
                  .map((entry) => entry.trim())
                  .filter(Boolean)
              )
            }
            placeholder="claude, codex"
            spellcheck={false}
            data-ac-testid={`settings.watchers.commands.${props.id}`}
            data-ac-role="textbox"
          />
        </label>
      </Show>

      {/* Reach and allocation answer different questions, so they are worded differently:
          `entries` is what this selector reaches, `allocated` is whether the row holds a slot
          after Save. A rejected call renders the error rather than the previous answer, which
          would belong to a draft that no longer exists. */}
      <div
        class="settings-label-hint"
        data-ac-testid={`settings.watchers.reach.${props.id}`}
        data-ac-role="status"
      >
        <Show
          when={props.reachError}
          fallback={
            <Show when={props.reach} fallback="Resolving reach...">
              {(entries) => (
                <>
                  {watcherReachSummary(props.config, entries())}
                  {watcherBudgetNotice(props.config, entries())}
                </>
              )}
            </Show>
          }
        >
          {`Reach unknown: ${props.reachError}`}
        </Show>
      </div>

      <Show when={props.config.mode === "occurrence"}>
        <label class="settings-field">
          <span class="settings-label">Deduplicate by</span>
          <select
            class="settings-input"
            value={props.config.dedupe}
            onChange={(e) =>
              update("dedupe", e.currentTarget.value as WatcherConfig["dedupe"])
            }
            data-ac-testid={`settings.watchers.dedupe.${props.id}`}
            data-ac-role="combobox"
          >
            <option value="row">Row text</option>
            <option value="capture">Captured groups</option>
            <option value="none">Nothing — count every match</option>
          </select>
        </label>
        <label class="settings-field">
          <span class="settings-label">Dedupe window (ms)</span>
          <input
            class="settings-input settings-input-sm"
            type="number"
            min="0"
            step="100"
            value={props.config.dedupeWindowMs}
            onInput={(e) => {
              const value = Number.parseInt(e.currentTarget.value, 10);
              // The editor must not be able to write a value the Rust decoder would reject:
              // a negative or unrepresentable one reclassifies this very row as unrecognised,
              // which would take it out of the editor in the middle of being edited and leave
              // the user with a warning naming an id they can no longer reach.
              if (Number.isSafeInteger(value) && value >= 0) {
                update("dedupeWindowMs", value);
              }
            }}
            data-ac-testid={`settings.watchers.dedupeWindow.${props.id}`}
            data-ac-role="spinbutton"
          />
        </label>
      </Show>

      <label class="settings-field">
        <span class="settings-label">Captured against</span>
        <input
          class="settings-input"
          type="text"
          value={props.config.capturedAgainst ?? ""}
          onInput={(e) => update("capturedAgainst", e.currentTarget.value || null)}
          placeholder="claude 2.1.212"
          data-ac-testid={`settings.watchers.capturedAgainst.${props.id}`}
          data-ac-role="textbox"
        />
      </label>
      <div class="settings-label-hint">
        Free text. A TUI's layout changes between versions, and this is where the version
        the pattern was written against is recorded.
      </div>

      <label class="settings-field">
        <span class="settings-label">Test against</span>
        <select
          class="settings-input"
          value={testSessionId()}
          onChange={(e) => {
            sessionPickedByUser = true;
            setTestSessionId(e.currentTarget.value);
          }}
          data-ac-testid={`settings.watchers.testSession.${props.id}`}
          data-ac-role="combobox"
        >
          <option value={WATCHER_NO_SESSION}>No session (check syntax only)</option>
          <For each={props.sessions}>
            {(session) => <option value={session.id}>{session.name}</option>}
          </For>
        </select>
      </label>

      <div
        class="settings-label-hint"
        data-ac-testid={`settings.watchers.preview.${props.id}`}
        data-ac-role="status"
      >
        <Show when={previewError()}>{`Preview failed: ${previewError()}`}</Show>
        <Show when={!previewError() && preview()}>
          {(result) => (
            <Show
              when={result().compiles}
              fallback={`Does not compile: ${result().error ?? "unknown error"}`}
            >
              <Show
                when={result().sampled}
                fallback="Compiles. No session read, so nothing was matched against."
              >
                {`Compiles. Matches ${result().matchedRows} of ${result().totalRows} rows.`}
                <Show when={result().capturesVolatile}>
                  {" The capture changes between samples: in state mode this fires constantly."}
                </Show>
                <For each={result().samples}>
                  {(sample) => <div class="settings-hint">{sample}</div>}
                </For>
              </Show>
            </Show>
          )}
        </Show>
      </div>

      <Show
        when={renaming()}
        fallback={
          <button
            class="settings-add-btn"
            onClick={() => {
              setRenameDraft(props.id);
              setRenaming(true);
            }}
            data-ac-testid={`settings.watchers.renameStart.${props.id}`}
            data-ac-role="button"
          >
            Rename
          </button>
        }
      >
        <label class="settings-field">
          <span class="settings-label">New id</span>
          <input
            class="settings-input"
            type="text"
            value={renameDraft()}
            onInput={(e) => setRenameDraft(e.currentTarget.value)}
            spellcheck={false}
            data-ac-testid={`settings.watchers.renameInput.${props.id}`}
            data-ac-role="textbox"
          />
        </label>
        {/* Stated rather than implied: the id is the map key AND the grouping key the
            activity window and the history counters use, so this really is delete plus
            create and already-recorded activations keep the old id. */}
        <div class="settings-label-hint">
          Renaming deletes this watcher and creates a new one. Activations already recorded
          keep the old id.
        </div>
        <Show when={renameError()}>
          <div class="settings-api-client-error">{renameError()}</div>
        </Show>
        <div class="settings-color-row">
          <button
            class="settings-add-btn"
            onClick={applyRename}
            disabled={!!renameError() || renameDraft() === props.id}
            data-ac-testid={`settings.watchers.renameConfirm.${props.id}`}
            data-ac-role="button"
          >
            Rename
          </button>
          <button
            class="settings-add-btn"
            onClick={() => setRenaming(false)}
            data-ac-testid={`settings.watchers.renameCancel.${props.id}`}
            data-ac-role="button"
          >
            Cancel
          </button>
        </div>
      </Show>
    </div>
  );
};

const WatchersSettings: Component<Props> = (props) => (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Watchers</div>
        <div class="settings-label-hint">
          Regular expressions run over what a coding agent prints, every 200 ms. Matches
          appear in the watcher activity window. Best-effort: activations can be missed, so
          this is an indicator and not an audit log.
        </div>
      </div>

      <Show when={props.unreadableWatcherIds.length > 0}>
        {/* Named rather than hidden, and deliberately not offered for editing: the backend
            preserves what it could not parse, and so does this editor. */}
        <div
          class="settings-section"
          data-ac-testid="settings.watchers.unreadable"
          data-ac-role="status"
        >
          <div class="settings-api-client-warning">
            {"Not shown because this version could not read them: "
              + `${props.unreadableWatcherIds.join(", ")}`
              + ". They are left exactly as written in settings.30.instance.no-git.json."}
          </div>
        </div>
      </Show>

      <For each={props.watcherIds}>
        {(id) => {
          const entry = () => props.watcherMap[id];
          return (
            <Show when={isWatcherConfig(entry()) ? (entry() as WatcherConfig) : null}>
              {(config) => (
                <WatcherRow
                  id={id}
                  config={config()}
                  otherIds={props.watcherIds.filter((other) => other !== id)}
                  stems={props.commandStems}
                  sessions={props.agentSessions}
                  // Indexed by id, not by position: the request leaves unrecognised rows out,
                  // so response positions do not match table positions.
                  reach={props.reachAnswer ? props.reachByWatcherId.get(id) ?? [] : null}
                  reachError={props.reachError}
                  onChange={(next) => props.onChange(id, next)}
                  onRename={(nextId) => props.onRename(id, nextId)}
                  onRemove={() => props.onRemove(id)}
                />
              )}
            </Show>
          );
        }}
      </For>

      <Show when={props.watcherIds.length === 0}>
        <div class="settings-label-hint" data-ac-testid="settings.watchers.empty">
          No watchers configured. Nothing is evaluated and nothing costs anything until you
          add one.
        </div>
      </Show>

      <button
        class="settings-add-btn"
        onClick={props.onAdd}
        data-ac-testid="settings.watchers.add"
        data-ac-role="button"
      >
        Add Watcher
      </button>
    </>
  );

export default WatchersSettings;
