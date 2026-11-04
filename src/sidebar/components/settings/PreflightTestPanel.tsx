// #3516 - the shared preflight test panel. Runs the SAVED chain on a stopped agent
// folder, polls its progress (chained, one `get` in flight) and cancels on close.
import {
  type Component, For, Index, Show, createEffect, createMemo, createSignal, onCleanup, onMount,
} from "solid-js";
import { PreflightTestAPI, SettingsAPI } from "../../../shared/ipc";
import type {
  AppSettings, PreflightStageReport, PreflightTargetDescription, PreflightTestSnapshot,
} from "../../../shared/types";
import { projectStore } from "../../stores/project";
import {
  activeLine, formatDuration, isStale, preflightFingerprint, stageStatusText,
} from "./preflightModel";

const POLL_MS = 500;
const PROMPT = "Which stopped agent should run this test?";
const NO_TARGETS = "No compatible stopped agent is available. Create or stop an agent "
  + "before testing.";
const UNSAVED = "Save your changes before testing preflights.";
const TARGET_RUNNING = "Stop this agent before testing its preflights.";
const EMPTY_CHAIN = "No preflight commands configured. Nothing to test.";
const STALE = "Previous result — settings or test context changed. Run the test again.";
const NOTES = [
  "Runs both saved preflights in launch order. Does not start the Coding Agent.",
  "Preparation and commands may change files or external systems.",
  "The test uses a temporary AgentsCommander token, so scripts that call the "
    + "AgentsCommander CLI may behave differently than at launch.",
  "The test does not copy the agent configuration first. If a preflight needs files that "
    + "a launch would copy, the result can differ from a real launch.",
];

export interface PreflightTestPanelProps {
  agentId: string;
  letter: string | null;
  draft: AppSettings;
  timeoutDraftPending: boolean;
  onClose: () => void;
}

interface ResultContext {
  snapshot: PreflightTestSnapshot;
  cwd: string;
  fingerprint: string;
}

const errorText = (err: unknown): string => (err instanceof Error ? err.message : String(err));

const StageRow: Component<{ report: PreflightStageReport }> = (props) => {
  const [open, setOpen] = createSignal(false);
  const r = () => props.report;
  const done = () => r().status !== "pending" && r().status !== "running";
  const bad = () => ["failed", "timedOut", "spawnFailed", "signaled"].includes(r().status);
  const stream = (name: string, text: string, truncated: boolean) => (
    <Show when={text}>
      <div>{name}</div>
      <pre>{text}</pre>
      <Show when={truncated}><div>Output truncated (first 4 KiB kept)</div></Show>
    </Show>
  );
  return (
    <div
      class="settings-preflight-stage"
      classList={{ passed: r().status === "passed", failed: bad() }}
      data-ac-testid={`settings.preflightTest.stage.${r().source}`}
      data-ac-role="status"
      data-ac-state={r().status}
    >
      <div class="settings-preflight-stage-label">{r().label}</div>
      <For each={stageStatusText(r())}>{(line) => <div>{line}</div>}</For>
      <Show when={done()}>
        <button
          type="button"
          class="settings-profile-cell-btn"
          aria-expanded={open()}
          onClick={() => setOpen(!open())}
          data-ac-testid={`settings.preflightTest.stage.${r().source}.output`}
          data-ac-role="button"
        >
          {open() ? "Hide output" : "Show output"}
        </button>
        <Show when={open()}>
          <div class="settings-preflight-output">
            <Show when={r().durationMs !== null}>
              <div>Duration: {formatDuration(r().durationMs ?? 0)}</div>
            </Show>
            <Show when={r().exitCode !== null}><div>Exit code: {r().exitCode}</div></Show>
            {stream("stdout", r().stdout, r().stdoutTruncated)}
            {stream("stderr", r().stderr, r().stderrTruncated)}
            <Show when={!r().stdout && !r().stderr}><div>No output</div></Show>
          </div>
        </Show>
      </Show>
    </div>
  );
};

export const PreflightTestPanel: Component<PreflightTestPanelProps> = (props) => {
  let root: HTMLElement | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let exited = false;
  let describeSeq = 0;
  const [savedFingerprint, setSavedFingerprint] = createSignal<string | null>(null);
  const [selected, setSelected] = createSignal("");
  const [description, setDescription] = createSignal<PreflightTargetDescription | null>(null);
  const [runId, setRunId] = createSignal<string | null>(null);
  const [runCwd, setRunCwd] = createSignal("");
  const [live, setLive] = createSignal<PreflightTestSnapshot | null>(null);
  const [cancelSent, setCancelSent] = createSignal(false);
  const [resultLine, setResultLine] = createSignal<string | null>(null);
  const [result, setResult] = createSignal<ResultContext | null>(null);
  const [edited, setEdited] = createSignal(false);

  const fingerprint = createMemo(() => preflightFingerprint(props.draft, props.agentId));
  const dirty = () => props.timeoutDraftPending || fingerprint() !== savedFingerprint();
  const targets = createMemo(() => {
    const paths = projectStore.projects.flatMap((project) =>
      project.workgroups.flatMap((wg) => wg.agents.map((agent) => agent.path)));
    return [...new Set(paths)];
  });
  const active = () => runId() !== null;
  const disabledReason = (): string | null => {
    const d = description();
    if (!selected()) return targets().length ? PROMPT : NO_TARGETS;
    if (dirty()) return UNSAVED;
    if (!d) return null;
    if (d.unavailableReason) return d.unavailableReason;
    if (!d.stopped) return TARGET_RUNNING;
    if (d.stages.length === 0) return EMPTY_CHAIN;
    return null;
  };
  const stale = () => {
    const res = result();
    if (!res) return false;
    const d = description();
    const current = d?.settingsRevision ?? res.snapshot.settingsRevision;
    const contextMoved = !!d
      && (d.effectiveProfile !== res.snapshot.effectiveProfile || d.cwd !== res.cwd);
    return isStale(res.snapshot, current, edited()) || contextMoved;
  };
  const statusText = () => {
    const snap = live();
    if (active()) return snap ? activeLine(snap, cancelSent()) : activeLine(PENDING, cancelSent());
    return resultLine() ?? disabledReason() ?? "";
  };

  createEffect(() => {
    const res = result();
    if (res && fingerprint() !== res.fingerprint) setEdited(true);
  });

  const stopPolling = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  const finish = (snap: PreflightTestSnapshot) => {
    stopPolling();
    setRunId(null);
    setLive(null);
    setCancelSent(false);
    setEdited(false);
    setResult({ snapshot: snap, cwd: runCwd(), fingerprint: fingerprint() });
  };
  const fail = (text: string) => {
    stopPolling();
    setRunId(null);
    setLive(null);
    setCancelSent(false);
    setResultLine(text);
    void describe(selected());
  };
  /** Applies one snapshot of run `id`; true when the run is still unfinished. */
  const apply = (id: string, snap: PreflightTestSnapshot): boolean => {
    if (exited || runId() !== id || snap.runId !== id) return false;
    if (snap.phase === "finished") {
      finish(snap);
      return false;
    }
    setLive(snap);
    return true;
  };
  const poll = (id: string) => {
    timer = undefined;
    if (exited || runId() !== id) return;
    PreflightTestAPI.get(id).then(
      (snap) => {
        if (apply(id, snap)) timer = setTimeout(() => poll(id), POLL_MS);
      },
      (err) => {
        if (!exited && runId() === id) fail(errorText(err));
      },
    );
  };
  const attach = (id: string, cwd: string) => {
    stopPolling();
    setResultLine(null);
    setCancelSent(false);
    setLive(null);
    setRunCwd(cwd);
    setRunId(id);
    poll(id);
  };
  const describe = async (path: string) => {
    if (!path) return;
    const seq = ++describeSeq;
    try {
      const d = await PreflightTestAPI.describe(props.agentId, props.letter, path);
      if (exited || seq !== describeSeq || selected() !== path) return;
      setDescription(d);
      if (d.activeRunId && !active()) attach(d.activeRunId, d.cwd);
    } catch (err) {
      if (exited || seq !== describeSeq) return;
      setDescription(null);
      setResultLine(errorText(err));
    }
  };
  const run = async () => {
    const d = description();
    const path = selected();
    if (!d || active() || disabledReason()) return;
    setResultLine(null);
    try {
      const id = await PreflightTestAPI.start(props.agentId, props.letter, path,
        d.settingsRevision);
      if (exited) {
        // AC-3516-021 - closed while start was in flight: never leak the run.
        void PreflightTestAPI.cancel(id).catch(() => {});
        return;
      }
      attach(id, d.cwd);
    } catch (err) {
      if (exited) return;
      setResultLine(errorText(err));
      void describe(path);
    }
  };
  const cancel = () => {
    const id = runId();
    if (!id) return;
    setCancelSent(true);
    PreflightTestAPI.cancel(id).then(
      (snap) => { apply(id, snap); },
      (err) => {
        if (!exited && runId() === id) fail(errorText(err));
      },
    );
  };
  /** Close, Escape and unmount: cancel an unfinished run once, then stop polling. */
  const exit = () => {
    if (exited) return;
    exited = true;
    stopPolling();
    const id = runId();
    if (id) void PreflightTestAPI.cancel(id).catch(() => {});
  };
  const close = () => {
    exit();
    props.onClose();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    close();
  };

  window.addEventListener("keydown", onKeyDown, true);
  onCleanup(() => {
    window.removeEventListener("keydown", onKeyDown, true);
    exit();
  });
  onMount(() => {
    root?.focus();
    SettingsAPI.get().then(
      (saved) => {
        if (!exited) setSavedFingerprint(preflightFingerprint(saved, props.agentId));
      },
      () => {},
    );
  });

  const shownStages = () => (active() ? live()?.stages : result()?.snapshot.stages) ?? [];
  return (
    <section
      ref={root}
      class="settings-preflight-panel"
      tabIndex={-1}
      aria-label="Test preflights"
      data-ac-testid="settings.preflightTest"
      data-ac-role="dialog"
    >
      <div class="settings-preflight-panel-header">
        <span class="settings-label">Test preflights</span>
        <button
          type="button"
          class="settings-profile-cell-btn"
          onClick={close}
          data-ac-testid="settings.preflightTest.close"
          data-ac-role="button"
        >
          Close
        </button>
      </div>
      <Show when={targets().length > 0}>
        <label class="settings-field">
          <span class="settings-label">Test on</span>
          <select
            class="settings-input"
            value={selected()}
            onChange={(e) => {
              setSelected(e.currentTarget.value);
              setDescription(null);
              setResultLine(null);
              void describe(e.currentTarget.value);
            }}
            data-ac-testid="settings.preflightTest.target"
            data-ac-role="combobox"
          >
            <option value="" disabled>{PROMPT}</option>
            <For each={targets()}>{(path) => <option value={path}>{path}</option>}</For>
          </select>
        </label>
      </Show>
      <Show when={description()}>
        {(d) => (
          <div class="settings-preflight-target" data-ac-testid="settings.preflightTest.info">
            <div>Folder: {d().cwd}</div>
            <Show when={d().fallbackApplied}>
              <div>
                Profile {d().requestedProfile} falls back to Profile {d().effectiveProfile}.
              </div>
            </Show>
            <For each={d().stages}>
              {(stage) => (
                <div>
                  <div>{stage.label}: {stage.commandText}</div>
                  <div>Saved timeout: {stage.timeoutSeconds} seconds</div>
                </div>
              )}
            </For>
            <For each={NOTES}>{(note) => <div class="settings-label-hint">{note}</div>}</For>
          </div>
        )}
      </Show>
      <div
        class="settings-preflight-status"
        role="status"
        aria-live="polite"
        data-ac-testid="settings.preflightTest.status"
      >
        {statusText()}
      </div>
      <Show
        when={active()}
        fallback={
          <button
            type="button"
            class="settings-profile-cell-btn"
            disabled={!!disabledReason() || !description()}
            onClick={() => void run()}
            data-ac-testid="settings.preflightTest.run"
            data-ac-role="button"
          >
            Run test
          </button>
        }
      >
        <button
          type="button"
          class="settings-profile-cell-btn"
          disabled={cancelSent()}
          onClick={cancel}
          data-ac-testid="settings.preflightTest.cancel"
          data-ac-role="button"
        >
          Cancel
        </button>
      </Show>
      <Show when={!active() && result()}>
        {(res) => (
          <div
            class="settings-preflight-stage"
            classList={{
              passed: res().snapshot.outcome === "passed",
              failed: res().snapshot.outcome !== "passed",
              stale: stale(),
            }}
            data-ac-testid="settings.preflightTest.result"
            data-ac-role="status"
            data-ac-state={stale() ? "stale" : res().snapshot.outcome ?? "none"}
          >
            <Show when={stale()}><div>{STALE}</div></Show>
            <div>{res().snapshot.message ?? ""}</div>
          </div>
        )}
      </Show>
      <div classList={{ stale: !active() && stale() }}>
        <Index each={shownStages()}>{(report) => <StageRow report={report()} />}</Index>
      </div>
    </section>
  );
};

/** A run whose first snapshot has not arrived yet reads as "preparing". */
const PENDING: PreflightTestSnapshot = {
  runId: "", agentId: "", replicaPath: "", effectiveProfile: "", settingsRevision: "",
  phase: "preparing", outcome: null, message: null, stages: [],
};
