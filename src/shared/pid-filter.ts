export interface PidFilterParse {
  pids: number[];
  rejected: string[];
  truncated: boolean;
}

export const MAX_PIDS = 32;
export const FILTER_DEBOUNCE_MS = 200;

// The wire contract is `pid: u32` / `root_pid: u32`, so the only bound here is
// the u32 maximum; nothing else is invented.
const MAX_U32 = 4294967295;

export function parsePidFilter(text: string): PidFilterParse {
  const tokens = text.split(/[,;\s]+/).filter((token) => token.length > 0);
  const pids: number[] = [];
  const seenPids = new Set<number>();
  const rejected: string[] = [];
  const seenRejected = new Set<string>();
  let truncated = false;

  for (const token of tokens) {
    const reject = () => {
      if (!seenRejected.has(token)) {
        seenRejected.add(token);
        rejected.push(token);
      }
    };

    if (!/^\d+$/.test(token)) {
      reject();
      continue;
    }

    const value = Number(token);
    if (value > MAX_U32) {
      reject();
      continue;
    }

    if (seenPids.has(value)) continue;
    seenPids.add(value);
    if (pids.length < MAX_PIDS) {
      pids.push(value);
    } else {
      truncated = true;
    }
  }

  return { pids, rejected, truncated };
}
