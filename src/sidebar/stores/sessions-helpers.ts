import type { Session, SessionStatus } from "../../shared/types";

export function isRuntimeStringStatus(status: SessionStatus): boolean {
  return typeof status === "string";
}

export function upsertSessionList(prev: Session[], incoming: Session): Session[] {
  const idx = prev.findIndex((s) => s.id === incoming.id);
  if (idx === -1) return [...prev, incoming];
  const next = prev.slice();
  next[idx] = { ...prev[idx], ...incoming };
  return next;
}

export function preserveVisibleOrder<T>(
  next: T[],
  previous: T[] | undefined,
  keyFor: (item: T) => string,
): T[] {
  if (!previous || previous.length === 0) return next;

  const nextByKey = new Map(next.map((item) => [keyFor(item), item]));
  const used = new Set<string>();
  const ordered: T[] = [];

  for (const previousItem of previous) {
    const key = keyFor(previousItem);
    const nextItem = nextByKey.get(key);
    if (!nextItem) continue;
    ordered.push(nextItem);
    used.add(key);
  }

  for (const nextItem of next) {
    const key = keyFor(nextItem);
    if (used.has(key)) continue;
    ordered.push(nextItem);
  }

  return ordered;
}

export function reconcileVisibleOrderKeys(nextKeys: string[], frozenKeys: string[] | undefined): string[] {
  if (!frozenKeys || frozenKeys.length === 0) return nextKeys;

  const nextKeySet = new Set(nextKeys);
  const used = new Set<string>();
  const ordered: string[] = [];

  for (const key of frozenKeys) {
    if (!nextKeySet.has(key)) continue;
    ordered.push(key);
    used.add(key);
  }

  for (const key of nextKeys) {
    if (used.has(key)) continue;
    ordered.push(key);
  }

  return ordered;
}
