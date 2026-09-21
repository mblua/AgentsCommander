// #2245 - PID filter grammar for the Resource Monitor integral view.
//
// Pure and dependency-free on purpose: it is the one place the typed text is
// turned into an applied PID set, and keeping it importable without a DOM is
// what lets `pid-filter.test.ts` pin the grammar (and the debounce constant)
// without rendering anything.

export interface PidFilterParse {
  /** Applied PIDs, in first-appearance order, deduplicated by numeric value. */
  pids: number[];
  /** Rejected tokens, verbatim, in appearance order, deduplicated. */
  rejected: string[];
  /** True when more than MAX_PIDS distinct valid PIDs were given. */
  truncated: boolean;
}

/** Upper bound on applied PIDs. Beyond it the chip row stops being readable. */
export const MAX_PIDS = 32;

/** Debounce window for both filter inputs. Exported so its value is assertable
 *  without a DOM: a rendered test cannot tell 200 ms from 20 ms without
 *  sleeping, so the number is a contract here rather than a comment there. */
export const FILTER_DEBOUNCE_MS = 200;

// The wire contract, not an invented rule: `pid: u32` and `root_pid: u32` in
// `src-tauri/src/resource_monitor/types.rs`. `0` is representable, so it parses;
// it simply matches nothing.
const MAX_U32 = 4294967295;

export function parsePidFilter(text: string): PidFilterParse {
  const pids: number[] = [];
  const seenPids = new Set<number>();
  const rejected: string[] = [];
  const seenRejected = new Set<string>();
  let truncated = false;

  for (const token of text.split(/[,;\s]+/)) {
    if (token === "") continue;

    // Digits only, and inside u32. A leading `-`, a `.`, an exponent or any
    // other non-digit fails the first half; an overlong number fails the second.
    if (!/^\d+$/.test(token) || Number(token) > MAX_U32) {
      if (!seenRejected.has(token)) {
        seenRejected.add(token);
        rejected.push(token);
      }
      continue;
    }

    // Number() is what makes `04242` and `4242` the same PID.
    const pid = Number(token);
    if (seenPids.has(pid)) continue;
    seenPids.add(pid);

    if (pids.length >= MAX_PIDS) {
      truncated = true;
      continue;
    }
    pids.push(pid);
  }

  return { pids, rejected, truncated };
}
