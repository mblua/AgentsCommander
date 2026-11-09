// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import ContextHistoryCard, {
  GAP,
  formatMean,
  type CardHistoryState as SidebarHistoryState,
} from "./ContextHistoryCard";
import type { SessionWindowBucket } from "../../shared/context-history";

// #3532 P11 - the card only renders the state it is given.

/** recordsPerSession = records / actual sessions; null without sessions. */
const bucket = (
  requestedSessions: number,
  mean: number | null,
  count: number,
  sessions: number,
  incomplete = false,
) =>
  ({
    requestedSessions, recordsCount: count, positiveSum: (mean ?? 0) * count, mean,
    sessionsCount: sessions, recordsPerSession: sessions === 0 ? null : count / sessions,
    incomplete,
  }) satisfies SessionWindowBucket;

const readyWith = (sessionWindows: SessionWindowBucket[]): SidebarHistoryState => ({
  status: "ready",
  scope: { kind: "known", agentKey: "dev-rust", teamIds: ["team-a"] },
  generatedAtUtcMs: 1,
  sessionWindows,
});

const ready = (last = bucket(300, 55.04, 9, 3, true)): SidebarHistoryState =>
  readyWith([bucket(25, null, 0, 0), bucket(100, 35, 2, 2), last]);

function renderCard(
  state: SidebarHistoryState,
  percent: number | null = 35,
  colorSessions: number | null = 300,
) {
  const host = document.createElement("div");
  document.body.append(host);
  const dispose = render(
    () => (
      <ContextHistoryCard
        id="card-1"
        open
        anchor={() => host}
        state={state}
        percent={percent}
        projectPath="C:/proj"
        colorSessions={colorSessions}
        warning="Live warning."
      />
    ),
    host,
  );
  const card = document.getElementById("card-1")!;
  return { card, dispose };
}

describe("ContextHistoryCard (#3532 P11)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("formats a mean with one decimal", () => {
    expect(formatMean(55.04)).toBe("55.0%");
    expect(formatMean(0)).toBe("0.0%");
  });

  it("shows the current reading, identity, three bars and the basis", () => {
    const { card } = renderCard(ready());
    const text = card.textContent ?? "";
    expect(text).toContain("Contexto actual de esta sesión: 35%");
    expect(text).toContain("dev-rust");
    expect(text).toContain("team-a");
    expect(text).toContain("C:/proj");
    const rows = card.querySelectorAll(".ctx-history-row");
    expect(rows).toHaveLength(3);
    expect(rows[0].textContent).toContain("Últimas 25 sesiones (0 disponibles)");
    expect(rows[0].textContent).toContain("Sin datos · 0 registros · — registros/sesión");
    expect(rows[0].querySelector(".ctx-history-fill")).toBeNull();
    expect(rows[1].textContent).toContain("Últimas 100 sesiones (2 disponibles)");
    expect(rows[1].textContent).toContain("35.0% · 2 registros · 1.0 registros/sesión");
    expect(rows[2].textContent).toContain("Últimas 300 sesiones (3 disponibles)");
    expect(rows[2].textContent).toContain("55.0% · 9 registros · 3.0 registros/sesión");
    expect(rows[2].textContent).toContain("cobertura parcial");
    expect(rows[2].classList.contains("selected")).toBe(true);
    expect(text).toContain("Color basado en: últimas 300 sesiones");
    expect(text).toContain("Sesiones más recientes de este agente y team; sesión entera.");
    expect(text).toContain("Live warning.");
    expect(text).toContain("una media alta no ordena reiniciar");
  });

  it("marks the saved count and says when the reading is missing", () => {
    const { card } = renderCard(ready(), null, 25);
    expect(card.textContent).toContain("Contexto actual de esta sesión: no disponible");
    expect(card.textContent).toContain("Color basado en: últimas 25 sesiones");
    const rows = card.querySelectorAll(".ctx-history-row");
    expect(rows[0].classList.contains("selected")).toBe(true);
    expect(rows[2].classList.contains("selected")).toBe(false);
  });

  it.each([
    [{ status: "loading" } as SidebarHistoryState, "Cargando medias de contexto…"],
    [{ status: "error", message: "x" } as SidebarHistoryState, "No se pudieron leer las medias"],
    [
      {
        status: "ambiguous", scope: { kind: "ambiguous", agentKey: null, teamIds: [] },
        generatedAtUtcMs: 1, text: "",
      } as SidebarHistoryState,
      "Datos sin atribución segura a este team",
    ],
    [
      {
        status: "unknown", scope: { kind: "unknown", agentKey: null, teamIds: [] },
        generatedAtUtcMs: 1, text: "",
      } as SidebarHistoryState,
      "Identidad histórica no disponible",
    ],
  ])("renders the %s state without bars", (state, expected) => {
    const { card } = renderCard(state);
    expect(card.textContent).toContain(expected);
    expect(card.querySelectorAll(".ctx-history-row")).toHaveLength(0);
    expect(card.getAttribute("data-ac-state")).toBe(state.status);
  });

  it("renders exactly the returned custom [2,5] buckets with full availability", () => {
    const { card } = renderCard(readyWith([bucket(2, 40, 4, 2), bucket(5, 30, 10, 5)]), 35, 5);
    const rows = card.querySelectorAll(".ctx-history-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("Últimas 2 sesiones");
    expect(rows[0].textContent).not.toContain("disponibles");
    expect(rows[1].getAttribute("data-ac-sessions")).toBe("5");
    expect(rows[1].classList.contains("selected")).toBe(true);
    expect(card.textContent).toContain("Color basado en: últimas 5 sesiones");
  });

  it("shows backend records per session after the record count (criterion 8)", () => {
    const { card } = renderCard(readyWith([
      bucket(25, 20, 1560, 100),
      bucket(100, 30, 21, 2),
      bucket(200, null, 0, 0),
      bucket(300, null, 0, 1),
    ]), 35, 25);
    const rows = card.querySelectorAll(".ctx-history-row");
    expect(rows[0].textContent).toContain("20.0% · 1560 registros · 15.6 registros/sesión");
    expect(rows[1].textContent).toContain("Últimas 100 sesiones (2 disponibles)");
    expect(rows[1].textContent).toContain("30.0% · 21 registros · 10.5 registros/sesión");
    expect(rows[2].textContent).toContain("Sin datos · 0 registros · — registros/sesión");
    // A boundary-only session: zero records, null context mean, 0.0 per session.
    expect(rows[3].textContent).toContain("Sin datos · 0 registros · 0.0 registros/sesión");
    expect(rows[3].querySelector(".ctx-history-fill")).toBeNull();
  });

  it("is a tooltip that hides when closed", () => {
    const { card } = renderCard(ready());
    expect(card.getAttribute("role")).toBe("tooltip");
    expect(card.hidden).toBe(false);
  });
});

// AC-3532-030 - viewport placement with stubbed measures (jsdom has no layout).
describe("ContextHistoryCard placement (#3532 P11)", () => {
  let cardH = 100;
  const CARD_W = 260;
  const restore: (() => void)[] = [];
  const stub = (obj: object, key: string, desc: PropertyDescriptor) => {
    const prev = Object.getOwnPropertyDescriptor(obj, key);
    Object.defineProperty(obj, key, { configurable: true, ...desc });
    restore.push(() => {
      if (prev) Object.defineProperty(obj, key, prev);
      else delete (obj as Record<string, unknown>)[key];
    });
  };
  const viewport = (w: number, h: number) => {
    stub(window, "innerWidth", { value: w, writable: true });
    stub(window, "innerHeight", { value: h, writable: true });
  };

  beforeEach(() => {
    cardH = 100;
    stub(HTMLElement.prototype, "offsetHeight", {
      get(this: HTMLElement) { return this.id === "card-1" ? cardH : 0; },
    });
    stub(HTMLElement.prototype, "offsetWidth", {
      get(this: HTMLElement) { return this.id === "card-1" ? CARD_W : 0; },
    });
  });
  afterEach(() => {
    while (restore.length) restore.pop()!();
    document.body.innerHTML = "";
  });

  function mount(anchorTop: number, initial: SidebarHistoryState = ready()) {
    const anchor = document.createElement("span");
    anchor.getBoundingClientRect = () =>
      ({ left: 20, top: anchorTop, bottom: anchorTop + 16, right: 60 }) as DOMRect;
    document.body.append(anchor);
    const [state, setState] = createSignal(initial);
    const host = document.createElement("div");
    document.body.append(host);
    const dispose = render(
      () => (
        <ContextHistoryCard
          id="card-1" open anchor={() => anchor} state={state()} percent={35}
          projectPath="C:/proj" colorSessions={300} warning="w"
        />
      ),
      host,
    );
    const card = document.getElementById("card-1")!;
    const top = () => parseFloat(card.style.top);
    return { card, top, setState, dispose };
  }

  it("opens below the anchor when it fits", () => {
    viewport(800, 600);
    const { top } = mount(100);
    expect(top()).toBe(100 + 16 + GAP);
  });

  it("flips above an anchor near the bottom", () => {
    viewport(800, 600);
    const { top } = mount(560);
    expect(top()).toBe(560 - 100 - GAP);
  });

  it("stays inside a viewport lower than the card", () => {
    viewport(800, 120);
    cardH = 400;
    const { top } = mount(60);
    expect(top()).toBe(8);
  });

  it("re-places on loading -> ready and on resize", () => {
    viewport(800, 600);
    cardH = 40;
    const { top, setState } = mount(400, { status: "loading" });
    expect(top()).toBe(400 + 16 + GAP);
    cardH = 300;
    setState(ready());
    expect(top()).toBe(400 - 300 - GAP);
    cardH = 40;
    window.dispatchEvent(new Event("resize"));
    expect(top()).toBe(400 + 16 + GAP);
  });

  it("updates label, mean and records per session on ready -> ready", () => {
    viewport(800, 600);
    const { card, setState } = mount(100, readyWith([bucket(100, 35, 21, 2)]));
    expect(card.textContent).toContain("Últimas 100 sesiones (2 disponibles)");
    expect(card.textContent).toContain("35.0% · 21 registros · 10.5 registros/sesión");
    setState(readyWith([bucket(100, 50, 1560, 100)]));
    expect(card.textContent).toContain("Últimas 100 sesiones");
    expect(card.textContent).not.toContain("disponibles");
    expect(card.textContent).toContain("50.0% · 1560 registros · 15.6 registros/sesión");
  });

  it("removes its window listeners when it closes", () => {
    viewport(800, 600);
    const remove = vi.spyOn(window, "removeEventListener");
    const { dispose } = mount(100);
    dispose();
    const types = remove.mock.calls.map((c) => c[0]);
    expect(types).toContain("resize");
    expect(types).toContain("scroll");
    remove.mockRestore();
  });
});

// #3532 P18 - native card listeners (Sonar S6847/S1082) and explicit legend spacing (S6772).
describe("ContextHistoryCard P18 native listeners", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  function mountWithParent() {
    const [handlers, setHandlers] = createSignal({ enter: vi.fn(), leave: vi.fn() });
    const parentClick = vi.fn();
    const parentPointer = vi.fn();
    const host = document.createElement("div");
    document.body.append(host);
    const dispose = render(
      () => (
        // The logical parent: Solid re-targets delegated events through the Portal.
        <div onClick={parentClick} onPointerDown={parentPointer} data-testid="parent">
          <ContextHistoryCard
            id="card-1" open anchor={() => host} state={ready()} percent={35}
            projectPath="C:/proj" colorSessions={300} warning="w"
            onEnter={() => handlers().enter()}
            onLeave={(related) => handlers().leave(related)}
          />
        </div>
      ),
      host,
    );
    const card = document.getElementById("card-1")!;
    return { card, dispose, handlers, setHandlers, parentClick, parentPointer };
  }

  it("forwards hover with relatedTarget and reads the current props", () => {
    const { card, handlers, setHandlers } = mountWithParent();
    const first = handlers();
    card.dispatchEvent(new MouseEvent("mouseenter"));
    card.dispatchEvent(new MouseEvent("mouseleave", { relatedTarget: document.body }));
    expect(first.enter).toHaveBeenCalledTimes(1);
    expect(first.leave).toHaveBeenCalledWith(document.body);
    const next = { enter: vi.fn(), leave: vi.fn() };
    setHandlers(next);
    card.dispatchEvent(new MouseEvent("mouseenter"));
    expect(next.enter).toHaveBeenCalledTimes(1);
    expect(first.enter).toHaveBeenCalledTimes(1);
  });

  it("stops click and pointerdown from the card and its children", () => {
    const { card, parentClick, parentPointer } = mountWithParent();
    const domSpy = vi.fn();
    document.body.addEventListener("click", domSpy);
    document.body.addEventListener("pointerdown", domSpy);
    const child = card.querySelector<HTMLElement>(".ctx-history-row")!;
    for (const target of [card, child]) {
      target.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      target.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    }
    expect(parentClick).not.toHaveBeenCalled();
    expect(parentPointer).not.toHaveBeenCalled();
    expect(domSpy).not.toHaveBeenCalled();
    document.body.removeEventListener("click", domSpy);
    document.body.removeEventListener("pointerdown", domSpy);
  });

  it("leaves no listener on the retained node after dispose and works after remount", () => {
    const a = mountWithParent();
    const enter = a.handlers().enter;
    a.dispose();
    a.card.dispatchEvent(new MouseEvent("mouseenter"));
    expect(enter).not.toHaveBeenCalled();
    document.body.innerHTML = "";
    const b = mountWithParent();
    b.card.dispatchEvent(new MouseEvent("mouseenter"));
    expect(b.handlers().enter).toHaveBeenCalledTimes(1);
    expect(enter).not.toHaveBeenCalled();
  });

  it("separates each legend swatch from its label", () => {
    const { card } = mountWithParent();
    const legend = card.querySelector(".ctx-history-legend")!;
    expect(legend.textContent).toBe(" menor media mayor media");
    expect(legend.querySelector(".ctx-history-swatch.low")!.nextSibling?.textContent)
      .toMatch(/^ menor media/);
    expect(legend.querySelector(".ctx-history-swatch.high")!.nextSibling?.textContent)
      .toMatch(/^ mayor media/);
  });
});
