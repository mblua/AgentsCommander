// #4785 - the one sidebar regex filter. Module-level state shared by the
// Temporary Rooms header (which renders the control) and every project panel.
import { createMemo, createRoot, createSignal } from "solid-js";

type SearchPart = string | null | undefined | false;

const [open, setOpen] = createSignal(false);
const [pattern, setPattern] = createSignal("");

const state = createRoot(() =>
  createMemo(() => {
    const p = pattern().trim();
    if (!p) return { pattern: p, regex: null, error: "" };
    try {
      return { pattern: p, regex: new RegExp(p, "i"), error: "" };
    } catch {
      return { pattern: p, regex: null, error: "Invalid regex" };
    }
  }),
);

export function joinSearchText(...parts: SearchPart[]): string {
  return parts
    .filter((part): part is string => typeof part === "string" && part.trim().length > 0)
    .join(" ");
}

export const sidebarFilter = {
  open,
  pattern,
  active: () => state().regex !== null,
  error: () => state().error,
  setPattern: (value: string) => setPattern(value),
  toggle: () => {
    if (open()) {
      setOpen(false);
      setPattern("");
      return;
    }
    setOpen(true);
  },
  escape: (): "cleared" | "closed" => {
    if (pattern()) {
      setPattern("");
      return "cleared";
    }
    setOpen(false);
    return "closed";
  },
  matches: (...parts: SearchPart[]) => {
    const regex = state().regex;
    if (!regex) return true;
    return regex.test(joinSearchText(...parts));
  },
  resetForTests: () => {
    setOpen(false);
    setPattern("");
  },
};
