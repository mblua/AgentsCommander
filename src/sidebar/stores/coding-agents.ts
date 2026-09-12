import { createSignal } from "solid-js";
import type {
  CatalogDiagnostic,
  CatalogReport,
  CodingAgentDefinition,
} from "../../shared/types";
import { CodingAgentsAPI } from "../../shared/ipc";
import { normalizeProjectPathForCompare } from "./project-refresh";

// #1965 — the catalog is served as a report (primary identity, source path,
// warnings and an unavailable diagnostic) instead of a bare array. The store
// keeps one request per primary-identity generation so a superseded read can
// never publish stale presets, diagnostics, or re-seed eligibility, and it
// never falls back to the bundled presets: a failure disables catalog
// registrations instead of silently presenting selectable embedded defaults.

const [catalog, setCatalog] = createSignal<CodingAgentDefinition[]>([]);
const [reseedableCommands, setReseedableCommands] = createSignal<string[]>([]);
const [loaded, setLoaded] = createSignal(false);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<CatalogDiagnostic | null>(null);
const [warnings, setWarnings] = createSignal<CatalogDiagnostic[]>([]);
const [sourcePath, setSourcePath] = createSignal<string | null>(null);
const [generation, setGeneration] = createSignal(0);

// The requested primary identity. `initialized: false` means nothing has been
// requested or adopted yet, so the first completed report supplies the
// identity; `null` is a real no-project identity and stays distinct from it.
let primary: { initialized: boolean; root: string | null } = {
  initialized: false,
  root: null,
};

interface InFlightLoad {
  generation: number;
  promise: Promise<void>;
}
let inFlight: InFlightLoad | null = null;

// #1965 — Windows-shaped roots compare by shape (slash/case folding, ordinary
// verbatim prefix removal, trailing-separator trimming), POSIX roots keep case
// and literal backslashes; null never aliases a path. Reuses the project-refresh
// comparator unchanged.
function samePrimaryIdentity(a: string | null, b: string | null): boolean {
  if (a === null || b === null) return a === b;
  return (
    normalizeProjectPathForCompare(a.trim()) === normalizeProjectPathForCompare(b.trim())
  );
}

function diagnostic(code: string, path: string, reason: unknown): CatalogDiagnostic {
  return { code, path, reason: String(reason) };
}

function describeRoot(root: string | null): string {
  return root === null ? "no selected project" : `"${root}"`;
}

/** Discards everything selectable: definitions and master re-seed eligibility. */
function clearSelectableState(): void {
  setCatalog([]);
  setReseedableCommands([]);
}

function clearDiagnostics(): void {
  setError(null);
  setWarnings([]);
  setSourcePath(null);
}

/** Starts a new generation: outstanding completions can no longer publish. */
function beginGeneration(): void {
  setGeneration(generation() + 1);
  clearSelectableState();
  clearDiagnostics();
  setLoaded(false);
}

type LoadMode =
  // No identity check: the report supplies the primary identity (first read,
  // or an explicit reload that may legitimately follow a project switch).
  | "adopt"
  // The report must match the requested identity; a mismatch is a
  // source-changed state and its data is discarded.
  | "match";

/**
 * The single caught runner behind `ensureLoaded`, `refresh` and
 * `setPrimaryProject`. It never rejects: synchronous invocation throws and
 * asynchronous rejections are converted into published diagnostics, so a void
 * mount/effect call cannot raise an unhandled rejection.
 */
function startLoad(mode: LoadMode): Promise<void> {
  const gen = generation();
  if (inFlight && inFlight.generation === gen) return inFlight.promise;

  setLoading(true);
  const promise = (async () => {
    try {
      // `Promise.resolve().then(...)` also catches a synchronous throw from the
      // API wrapper (for example a test mock without a report handler).
      const [reportRes, reseedRes] = await Promise.allSettled([
        Promise.resolve().then(() => CodingAgentsAPI.getCatalogReport()),
        Promise.resolve().then(() => CodingAgentsAPI.listReseedableCommands()),
      ]);

      // Only the owning generation may publish, and a superseded rejection must
      // not overwrite the current state.
      if (gen !== generation()) return;

      // A master-list failure is a diagnostic only: a successful catalog still
      // publishes, while an empty command set disables the master re-seed
      // controls.
      const reseedDiagnostic =
        reseedRes.status === "rejected"
          ? diagnostic("reseedable-unavailable", "", reseedRes.reason)
          : null;
      const warningsForLoad = (report: CatalogDiagnostic[]): CatalogDiagnostic[] =>
        reseedDiagnostic ? [...report, reseedDiagnostic] : [...report];

      if (reportRes.status === "rejected") {
        clearSelectableState();
        setLoaded(false);
        setSourcePath(null);
        setWarnings(reseedDiagnostic ? [reseedDiagnostic] : []);
        setError(diagnostic("transport-error", "", reportRes.reason));
        return;
      }

      const report = reportRes.value;

      if (mode === "match" && !samePrimaryIdentity(primary.root, report.primaryProjectRoot)) {
        clearSelectableState();
        setLoaded(false);
        clearDiagnostics();
        setError(
          diagnostic(
            "primary-project-changed",
            report.primaryProjectRoot ?? "",
            `Catalog report is for ${describeRoot(report.primaryProjectRoot)}, expected ` +
              `${describeRoot(primary.root)}. Reload catalog to retry.`,
          ),
        );
        return;
      }

      if (mode === "adopt") {
        primary = { initialized: true, root: report.primaryProjectRoot };
      }

      setSourcePath(report.sourcePath ?? null);
      setWarnings(warningsForLoad(report.warnings ?? []));

      if (report.unavailable) {
        // Backend unavailable disables catalog registrations; the report itself
        // stays the diagnostic source.
        clearSelectableState();
        setLoaded(false);
        setError(report.unavailable);
        return;
      }

      setCatalog(Array.isArray(report.catalog) ? report.catalog : []);
      setReseedableCommands(reseedRes.status === "fulfilled" ? reseedRes.value : []);
      setError(null);
      // A valid empty catalog is a successful load: loaded=true, no fallback.
      setLoaded(true);
    } catch (caught) {
      // Last-resort guard for anything the two allSettled branches above did
      // not absorb, so the returned promise always resolves.
      if (gen !== generation()) return;
      clearSelectableState();
      setLoaded(false);
      setSourcePath(null);
      setWarnings([]);
      setError(diagnostic("transport-error", "", caught));
    } finally {
      // Only the owning generation may clear the loading state or in-flight slot.
      if (gen === generation()) {
        setLoading(false);
        if (inFlight && inFlight.generation === gen) inFlight = null;
      }
    }
  })();

  inFlight = { generation: gen, promise };
  return promise;
}

export const codingAgentsStore = {
  catalog,
  loaded,
  loading,
  error,
  warnings,
  sourcePath,
  generation,
  reseedableCommands,

  async ensureLoaded(): Promise<void> {
    if (loaded()) return;
    // Multiple calls within one generation join the single in-flight request;
    // after a failure `loaded` stays false, so the next mount retries.
    if (inFlight && inFlight.generation === generation()) return inFlight.promise;
    return startLoad(primary.initialized ? "match" : "adopt");
  },

  /** Explicit reload: clears selectable data immediately, starts a new
   *  generation and adopts the backend's latest primary identity. */
  refresh(): Promise<void> {
    beginGeneration();
    return startLoad("adopt");
  },

  /** Primary-project change: no-op when the initialized identity is unchanged;
   *  otherwise discards selectable/diagnostic state and fetches the new
   *  project's report (a mismatched report becomes a source-changed state). */
  setPrimaryProject(root: string | null): Promise<void> {
    if (primary.initialized && samePrimaryIdentity(primary.root, root)) {
      return Promise.resolve();
    }
    primary = { initialized: true, root };
    beginGeneration();
    return startLoad("match");
  },

  /** Invalidates outstanding completions as well as clearing every signal. */
  resetForTests(): void {
    setGeneration(generation() + 1);
    primary = { initialized: false, root: null };
    inFlight = null;
    clearSelectableState();
    clearDiagnostics();
    setLoading(false);
    setLoaded(false);
  },
};
