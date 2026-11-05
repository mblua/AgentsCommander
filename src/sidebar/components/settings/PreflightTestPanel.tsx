// #3516/#3832 - the shared preflight test panel. Runs the visible draft preflight
// commands and timeouts on a stopped agent folder, polls its progress (chained,
// one `get` in flight) and cancels on close. Nothing is saved.
import {
  type Component, For, Index, Show, createEffect, createMemo, createSignal, on, onCleanup,
  onMount,
} from "solid-js";
import { PreflightTestAPI, SettingsAPI } from "../../../shared/ipc";
import type {
  AppSettings, PreflightStageReport, PreflightTargetDescription, PreflightTestSnapshot,
  PreflightTestTarget,
} from "../../../shared/types";
import { projectStore } from "../../stores/project";
import {
  INVALID_TIMEOUT_TEXT, activeLine, buildPreflightDraft, draftKey, formatDuration, isStale,
  preflightFingerprint, rowsForLetter, stageStatusText, targetListText,
} from "./preflightModel";
import { PreflightTargetPicker, type PreflightTargetPickerControl } from "./PreflightTargetPicker";
import "./preflightTest.css";

const POLL_MS = 500;
const PROMPT = "Which stopped agent should run this test?";
const LOADING = "Loading agents...";
const TARGET_RUNNING = "Stop this agent before testing its preflights.";
const EMPTY_CHAIN = "No preflight commands configured. Nothing to test.";
const STALE = "Previous result — settings or test context changed. Run the test again.";
const UNSAVED_LINE = "Testing unsaved changes. Nothing is saved.";
const NOTES = [
  "Runs both preflights in launch order, as shown here. Does not start the Coding Agent.",
  "Preparation and commands may change files or external systems.",
  "The test uses a temporary AgentsCommander token, so scripts that call the "
    + "AgentsCommander CLI may behave differently than at launch.",
  "The test does not copy the agent configuration first. If a preflight needs files that "
    + "a launch would copy, the result can differ from a real launch.",
  "Only the preflight commands and timeouts use your unsaved changes. Other settings "
    + "use the saved values.",
];

export interface PreflightTestPanelProps {
  agentId: string;
  letter: string | null;
  draft: AppSettings;
  timeoutDraftPending: boolean;
  onClose: () => void;
}

/** What a run was started (or reattached) against; captured once, never relabelled. */
interface RunContext {
  path: string;
  cwd: string;
  letter: string | null;
  fingerprint: string;
  /** A reattached run started elsewhere: its draft provenance is unknown. */
  unknown: boolean;
}

interface ResultContext {
  snapshot: PreflightTestSnapshot;
  ctx: RunContext;
}

interface Described {
  path: string;
  key: string;
  d: PreflightTargetDescription;
}

type ListState = "loading" | "ready" | "error";

const errorText = (err: unknown): string => (err instanceof Error ? err.message : String(err));
const samePaths = (a: string[], b: string[]) =>
  a.length === b.length && a.every((path, i) => path === b[i]);

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

/** The draft chain the latest matching describe returned, with the notes. */
const TargetInfo: Component<{ d: PreflightTargetDescription }> = (props) => (
  <div class="settings-preflight-target" data-ac-testid="settings.preflightTest.info">
    <div>Folder: {props.d.cwd}</div>
    <Show when={props.d.fallbackApplied}>
      <div>
        Profile {props.d.requestedProfile} falls back to Profile {props.d.effectiveProfile}.
      </div>
    </Show>
    <For each={props.d.stages}>
      {(stage) => (
        <div>
          <div>{stage.label}: {stage.commandText}</div>
          <div>Timeout: {stage.timeoutSeconds} seconds</div>
        </div>
      )}
    </For>
    <For each={NOTES}>{(note) => <div class="settings-label-hint">{note}</div>}</For>
  </div>
);

const RetryLine: Component<{ text: string; testId: string; onRetry: () => void }> = (props) => (
  <div class="preflight-test-retry" data-ac-testid={`settings.preflightTest.${props.testId}`}>
    <span>{props.text}</span>
    <button
      type="button"
      class="settings-profile-cell-btn"
      onClick={() => props.onRetry()}
      data-ac-testid={`settings.preflightTest.${props.testId}.retry`}
      data-ac-role="button"
    >
      Retry
    </button>
  </div>
);

export const PreflightTestPanel: Component<PreflightTestPanelProps> = (props) => {
  let root: HTMLElement | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let exited = false;
  let describeSeq = 0;
  let listSeq = 0;
  let picker: PreflightTargetPickerControl | undefined;
  const [savedFingerprint, setSavedFingerprint] = createSignal<string | null>(null);
  const [savedError, setSavedError] = createSignal<string | null>(null);
  const [rows, setRows] = createSignal<PreflightTestTarget[]>([]);
  const [listState, setListState] = createSignal<ListState>("loading");
  const [listError, setListError] = createSignal("");
  const [selected, setSelected] = createSignal("");
  const [described, setDescribed] = createSignal<Described | null>(null);
  const [starting, setStarting] = createSignal(false);
  const [runId, setRunId] = createSignal<string | null>(null);
  const [runCtx, setRunCtx] = createSignal<RunContext | null>(null);
  const [live, setLive] = createSignal<PreflightTestSnapshot | null>(null);
  const [cancelSent, setCancelSent] = createSignal(false);
  const [resultLine, setResultLine] = createSignal<string | null>(null);
  const [result, setResult] = createSignal<ResultContext | null>(null);
  const [edited, setEdited] = createSignal(false);

  const fingerprint = createMemo(() => preflightFingerprint(props.draft, props.agentId));
  const payload = createMemo(() => buildPreflightDraft(props.draft, props.agentId));
  const payloadKey = createMemo(() => draftKey(payload()));
  // While the saved comparison loads (or failed), never claim unsaved state.
  const dirty = () => savedFingerprint() !== null
    && (props.timeoutDraftPending || fingerprint() !== savedFingerprint());
  const paths = createMemo(() => [...new Set(projectStore.projects.flatMap((project) =>
    project.workgroups.flatMap((wg) => wg.agents.map((agent) => agent.path))))],
  undefined, { equals: samePaths });
  const profileRows = createMemo(() => rowsForLetter(rows(), props.letter));
  const active = () => runId() !== null;
  const busy = () => active() || starting();
  /** The description only while it matches the current selection and draft payload. */
  const description = () => {
    const c = described();
    return c && c.path === selected() && c.key === payloadKey() ? c.d : null;
  };
  const disabledReason = (): string | null => {
    if (listState() === "loading") return LOADING;
    if (listState() === "error") return listError();
    const listText = targetListText(profileRows(), props.letter);
    if (listText) return listText;
    if (!selected()) return PROMPT;
    if (props.timeoutDraftPending) return INVALID_TIMEOUT_TEXT;
    const d = description();
    if (!d) return null;
    if (d.unavailableReason) return d.unavailableReason;
    if (!d.stopped) return TARGET_RUNNING;
    if (d.stages.length === 0) return EMPTY_CHAIN;
    return null;
  };
  const shownResult = () => {
    const res = result();
    return res && res.ctx.path === selected() ? res : null;
  };
  const stale = () => {
    const res = result();
    if (!res) return false;
    const c = res.ctx;
    if (c.unknown || edited() || c.letter !== props.letter || c.path !== selected()) return true;
    const d = description();
    const current = d?.settingsRevision ?? res.snapshot.settingsRevision;
    const contextMoved = !!d
      && (d.effectiveProfile !== res.snapshot.effectiveProfile || d.cwd !== c.cwd);
    return isStale(res.snapshot, current, false) || contextMoved;
  };
  const statusText = () => {
    const snap = live();
    if (active()) return snap ? activeLine(snap, cancelSent()) : activeLine(PENDING, cancelSent());
    return resultLine() ?? disabledReason() ?? "";
  };

  // Sticky: any edit after the run's start marks its result stale, even if reverted.
  createEffect(() => {
    const ctx = runCtx();
    if (ctx && fingerprint() !== ctx.fingerprint) setEdited(true);
  });

  const stopPolling = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  const finish = (snap: PreflightTestSnapshot) => {
    stopPolling();
    const ctx = runCtx();
    setRunId(null);
    setLive(null);
    setCancelSent(false);
    if (ctx) setResult({ snapshot: snap, ctx });
  };
  const fail = (text: string) => {
    stopPolling();
    setRunId(null);
    setLive(null);
    setCancelSent(false);
    setResultLine(text);
    void describe();
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
  const attach = (id: string, ctx: RunContext) => {
    stopPolling();
    setResultLine(null);
    setCancelSent(false);
    setLive(null);
    setEdited(false);
    setRunCtx(ctx);
    setRunId(id);
    poll(id);
  };
  const loadList = async () => {
    const seq = ++listSeq;
    setListState("loading");
    try {
      const list = await PreflightTestAPI.listTargets(props.agentId, paths());
      if (exited || seq !== listSeq) return;
      setRows(list);
      setListState("ready");
    } catch (err) {
      if (exited || seq !== listSeq) return;
      setListError(errorText(err));
      setListState("error");
    }
  };
  const describe = async () => {
    const seq = ++describeSeq;
    const path = selected();
    if (!path || props.timeoutDraftPending) return;
    const key = payloadKey();
    try {
      const d = await PreflightTestAPI.describe(props.agentId, props.letter, path, payload());
      if (exited || seq !== describeSeq) return;
      setDescribed({ path, key, d });
      if (d.activeRunId && !busy()) {
        attach(d.activeRunId, {
          path, cwd: d.cwd, letter: props.letter, fingerprint: fingerprint(), unknown: true,
        });
      }
    } catch (err) {
      if (exited || seq !== describeSeq) return;
      setDescribed(null);
      setResultLine(errorText(err));
    }
  };
  const loadSaved = () => {
    setSavedError(null);
    SettingsAPI.get().then(
      (saved) => {
        if (!exited) setSavedFingerprint(preflightFingerprint(saved, props.agentId));
      },
      (err) => {
        if (!exited) setSavedError(`Could not load saved settings: ${errorText(err)}`);
      },
    );
  };
  const choose = (path: string) => {
    if (busy()) return;
    setResultLine(null);
    if (path === selected()) void describe();
    else setSelected(path);
  };
  const clearSelection = () => {
    describeSeq++;
    setSelected("");
    setDescribed(null);
    setResultLine(null);
  };
  const runTest = async () => {
    const d = description();
    const path = selected();
    if (!d || busy() || disabledReason()) return;
    // Snapshot everything at click; later edits never relabel this run.
    const draft = payload();
    const ctx: RunContext = {
      path, cwd: d.cwd, letter: props.letter, fingerprint: fingerprint(), unknown: false,
    };
    setResultLine(null);
    setStarting(true);
    try {
      const id = await PreflightTestAPI.start(props.agentId, props.letter, path,
        d.settingsRevision, draft);
      if (exited) {
        // AC-3516-021 - closed while start was in flight: never leak the run.
        void PreflightTestAPI.cancel(id).catch(() => {});
        return;
      }
      attach(id, ctx);
    } catch (err) {
      if (exited) return;
      setResultLine(errorText(err));
      void describe();
      void loadList();
    } finally {
      setStarting(false);
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
  /** Window capture: an open picker list takes the first Escape; IME keys pass through. */
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.isComposing || e.keyCode === 229) return;
    e.preventDefault();
    if (picker?.isOpen()) {
      e.stopImmediatePropagation();
      picker.setOpen(false);
      return;
    }
    e.stopPropagation();
    close();
  };

  createEffect(on([() => props.agentId, paths], () => { void loadList(); }));
  createEffect(on(
    [selected, payloadKey, () => props.timeoutDraftPending, () => props.letter],
    () => { void describe(); },
  ));
  // Removed or newly running choices clear, but only while no run is starting or active.
  createEffect(() => {
    const path = selected();
    if (!path || busy() || listState() !== "ready") return;
    const row = profileRows().find((r) => r.replicaPath === path);
    if (!row || row.running) clearSelection();
  });

  window.addEventListener("keydown", onKeyDown, true);
  onCleanup(() => {
    window.removeEventListener("keydown", onKeyDown, true);
    exit();
  });
  onMount(() => {
    root?.focus();
    loadSaved();
  });

  const shownStages = () =>
    (active() ? live()?.stages : shownResult()?.snapshot.stages) ?? [];
  const filterHint = () => (props.letter === null ? undefined
    : `Showing only agents that launch with Profile ${props.letter} (saved settings).`);
  return (
    <section
      ref={root}
      class="settings-preflight-panel"
      classList={{ "preflight-test-unsaved": dirty() }}
      tabIndex={-1}
      aria-label="Test preflights"
      data-ac-testid="settings.preflightTest"
      data-ac-role="dialog"
      data-ac-state={dirty() ? "unsaved" : "saved"}
    >
      <div class="settings-preflight-panel-header">
        <span class="settings-label">Test preflights</span>
        <span class="preflight-test-header-actions">
          <Show when={dirty()}>
            <span
              class="preflight-test-unsaved-badge"
              data-ac-testid="settings.preflightTest.unsaved"
            >
              Unsaved changes
            </span>
          </Show>
          <button
            type="button"
            class="settings-profile-cell-btn"
            onClick={close}
            data-ac-testid="settings.preflightTest.close"
            data-ac-role="button"
          >
            Close
          </button>
        </span>
      </div>
      <Show when={dirty()}>
        <div
          class="preflight-test-unsaved-line"
          data-ac-testid="settings.preflightTest.unsavedLine"
        >
          {UNSAVED_LINE}
        </div>
      </Show>
      <Show when={savedError()}>
        {(text) => <RetryLine text={text()} testId="savedError" onRetry={loadSaved} />}
      </Show>
      <Show when={profileRows().length > 0}>
        <PreflightTargetPicker
          rows={profileRows()}
          selected={selected()}
          disabled={busy()}
          hint={filterHint()}
          onChoose={choose}
          onSearchEdit={clearSelection}
          onControl={(control) => { picker = control; }}
        />
      </Show>
      <Show when={description()}>{(d) => <TargetInfo d={d()} />}</Show>
      <div
        class="settings-preflight-status"
        role="status"
        aria-live="polite"
        data-ac-testid="settings.preflightTest.status"
      >
        {statusText()}
      </div>
      <Show when={!active() && listState() === "error"}>
        <button
          type="button"
          class="settings-profile-cell-btn"
          onClick={() => void loadList()}
          data-ac-testid="settings.preflightTest.listRetry"
          data-ac-role="button"
        >
          Retry
        </button>
      </Show>
      <Show
        when={active()}
        fallback={
          <button
            type="button"
            class="settings-profile-cell-btn"
            disabled={!!disabledReason() || !description() || starting()}
            onClick={() => void runTest()}
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
      <Show when={!active() && shownResult()}>
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
