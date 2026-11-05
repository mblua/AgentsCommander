// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import {
  PreflightTargetPicker, type PreflightTargetPickerControl,
} from "./PreflightTargetPicker";
import type { PreflightTestTarget } from "../../../shared/preflightTypes";

// #3832 - the controlled "Test on" combobox: search, keyboard, running rows, IME.

const ROOM = "C:\\Project\\.ac\\room-1-dev";
const DEV = `${ROOM}\\__agent_dev`;
const OPS = `${ROOM}\\__agent_ops`;
const QA = "C:/Project/.ac/room-1-dev/__agent_qa";

function row(path: string, running = false): PreflightTestTarget {
  return {
    replicaPath: path, requestedProfile: "A", effectiveProfile: "A", fallbackApplied: false,
    running,
  };
}

interface Picker {
  host: HTMLElement;
  chosen: string[];
  edits: () => number;
  control: () => PreflightTargetPickerControl;
  setRows: (rows: PreflightTestTarget[]) => void;
  setSelected: (path: string) => void;
  setDisabled: (disabled: boolean) => void;
  dispose: () => void;
}

function mount(rows: PreflightTestTarget[]): Picker {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const [list, setRows] = createSignal(rows);
  const [selected, setSelected] = createSignal("");
  const [disabled, setDisabled] = createSignal(false);
  const chosen: string[] = [];
  let edits = 0;
  let control: PreflightTargetPickerControl | undefined;
  const dispose = render(() => (
    <PreflightTargetPicker
      rows={list()}
      selected={selected()}
      disabled={disabled()}
      onChoose={(path) => { chosen.push(path); setSelected(path); }}
      onSearchEdit={() => { edits++; setSelected(""); }}
      onControl={(c) => { control = c; }}
    />
  ), host);
  return {
    host, chosen, edits: () => edits, control: () => control!, setRows, setSelected,
    setDisabled, dispose,
  };
}

const input = (p: Picker) =>
  p.host.querySelector<HTMLInputElement>('[data-ac-testid="settings.preflightTest.target"]')!;
const listbox = (p: Picker) => p.host.querySelector<HTMLElement>("[role=listbox]")!;
const options = (p: Picker) => [...p.host.querySelectorAll<HTMLElement>("[role=option]")];
const key = (p: Picker, name: string, over: KeyboardEventInit = {}) => {
  const event = new KeyboardEvent("keydown", {
    key: name, bubbles: true, cancelable: true, ...over,
  });
  input(p).dispatchEvent(event);
  return event;
};
const type = (p: Picker, text: string) => {
  input(p).value = text;
  input(p).dispatchEvent(new InputEvent("input", { bubbles: true }));
};
const activeId = (p: Picker) => input(p).getAttribute("aria-activedescendant");

describe("PreflightTargetPicker (#3832)", () => {
  let p: Picker | null = null;

  afterEach(() => {
    p?.dispose();
    p = null;
    document.body.replaceChildren();
  });

  it("is an accessible combobox that opens on focus and closes on blur", () => {
    p = mount([row(DEV)]);
    const el = input(p);
    expect(el.getAttribute("role")).toBe("combobox");
    expect(el.getAttribute("aria-autocomplete")).toBe("list");
    expect(el.getAttribute("aria-controls")).toBe(listbox(p).id);
    expect(el.placeholder).toBe("Search agents...");
    expect(p.host.querySelector("label")!.textContent).toBe("Test on");
    expect(el.getAttribute("aria-expanded")).toBe("false");
    el.focus();
    expect(el.getAttribute("aria-expanded")).toBe("true");
    expect(p.control().isOpen()).toBe(true);
    el.blur();
    expect(listbox(p).hidden).toBe(true);
    expect(p.control().isOpen()).toBe(false);
  });

  it("shows basename, dimmed full path and launch Profile for both slash styles", () => {
    p = mount([row(DEV), row(QA)]);
    input(p).focus();
    const [dev, qa] = options(p);
    expect(dev.querySelector(".preflight-target-name")!.textContent).toBe("__agent_dev");
    expect(dev.querySelector(".preflight-target-path")!.textContent).toBe(DEV);
    expect(dev.querySelector(".preflight-target-profile")!.textContent)
      .toBe("Launches with Profile A");
    expect(qa.querySelector(".preflight-target-name")!.textContent).toBe("__agent_qa");
  });

  it("searches case-insensitively over the whole path and keeps every row reachable", () => {
    p = mount([row(DEV), row(OPS), row(QA)]);
    input(p).focus();
    type(p, "AGENT_O");
    expect(options(p).map((o) => o.dataset.acDetail)).toEqual([OPS]);
    type(p, "room-1-dev\\__agent_d");
    expect(options(p).map((o) => o.dataset.acDetail)).toEqual([DEV]);
    type(p, "nothing");
    expect(options(p)).toHaveLength(0);
    expect(p.host.textContent).toContain("No agents match your search.");
    type(p, "");
    expect(options(p)).toHaveLength(3);
    expect(p.edits()).toBe(4);
  });

  it("moves over enabled rows only, clamps at the ends and confirms with Enter", () => {
    p = mount([row(DEV), row(OPS, true), row(QA)]);
    input(p).focus();
    key(p, "ArrowUp");
    expect(activeId(p)).toBe(options(p)[0].id);
    key(p, "ArrowDown");
    expect(activeId(p)).toBe(options(p)[2].id);
    key(p, "ArrowDown");
    expect(activeId(p)).toBe(options(p)[2].id);
    key(p, "ArrowUp");
    expect(activeId(p)).toBe(options(p)[0].id);
    expect(options(p)[0].getAttribute("aria-selected")).toBe("true");
    const enter = key(p, "Enter");
    expect(enter.defaultPrevented).toBe(true);
    expect(p.chosen).toEqual([DEV]);
    expect(input(p).value).toBe(DEV);
    expect(listbox(p).hidden).toBe(true);
  });

  it("never chooses a running row by click or Enter", () => {
    p = mount([row(OPS, true)]);
    input(p).focus();
    const running = options(p)[0];
    expect(running.getAttribute("aria-disabled")).toBe("true");
    expect(running.textContent).toContain("__agent_ops (running)");
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    running.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    running.click();
    key(p, "ArrowDown");
    key(p, "Enter");
    expect(p.chosen).toEqual([]);
  });

  it("chooses an enabled row by click and keeps it independent of a later query", () => {
    p = mount([row(DEV), row(QA)]);
    input(p).focus();
    options(p)[1].click();
    expect(p.chosen).toEqual([QA]);
    expect(input(p).dataset.acState).toBe("confirmed");
    type(p, "dev");
    expect(input(p).dataset.acState).toBe("unconfirmed");
    expect(p.edits()).toBe(1);
  });

  it("closes the list on Escape and ignores keys during IME composition", () => {
    p = mount([row(DEV)]);
    input(p).focus();
    input(p).dispatchEvent(new CompositionEvent("compositionstart"));
    key(p, "ArrowDown");
    const enter = key(p, "Enter");
    expect(enter.defaultPrevented).toBe(false);
    expect(activeId(p)).toBeNull();
    input(p).dispatchEvent(new CompositionEvent("compositionend"));
    key(p, "Enter", { isComposing: true });
    expect(p.chosen).toEqual([]);
    const esc = key(p, "Escape");
    expect(esc.defaultPrevented).toBe(true);
    expect(listbox(p).hidden).toBe(true);
  });

  it("lets the panel close the list and locks while disabled", () => {
    p = mount([row(DEV)]);
    input(p).focus();
    p.control().setOpen(false);
    expect(listbox(p).hidden).toBe(true);
    p.control().setOpen(true);
    p.setDisabled(true);
    expect(input(p).disabled).toBe(true);
    expect(p.control().isOpen()).toBe(false);
    options(p)[0].click();
    expect(p.chosen).toEqual([]);
  });

  it("shows a chosen path set by the panel and resets the active row on new rows", () => {
    p = mount([row(DEV), row(QA)]);
    p.setSelected(QA);
    expect(input(p).value).toBe(QA);
    input(p).focus();
    type(p, "");
    key(p, "ArrowDown");
    expect(activeId(p)).not.toBeNull();
    p.setRows([row(DEV)]);
    expect(activeId(p)).toBeNull();
  });
});
