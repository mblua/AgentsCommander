// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import ContextHistoryCard, { formatMean, type HistoryDays } from "./ContextHistoryCard";
import type { SidebarBucket } from "../../shared/context-history";
import type { SidebarHistoryState } from "../context-history";

// #3532 P11 - the card only renders the state it is given.

const bucket = (days: number, mean: number | null, count: number, incomplete = false) =>
  ({
    days, fromUtcMs: 0, toUtcMs: 1, recordsCount: count, positiveSum: (mean ?? 0) * count,
    mean, sessionsCount: count, incomplete,
  }) satisfies SidebarBucket;

const ready = (month = bucket(30, 55.04, 9, true)): SidebarHistoryState => ({
  status: "ready",
  scope: { kind: "known", agentKey: "dev-rust", teamIds: ["team-a"] },
  generatedAtUtcMs: 1,
  windows: { day: bucket(1, null, 0), week: bucket(7, 35, 2), month },
});

function renderCard(
  state: SidebarHistoryState,
  percent: number | null = 35,
  colorDays: HistoryDays = 30,
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
        colorDays={colorDays}
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
    expect(rows.length).toBe(3);
    expect(rows[0].textContent).toContain("Último día");
    expect(rows[0].textContent).toContain("Sin datos · 0 registros");
    expect(rows[0].querySelector(".ctx-history-fill")).toBeNull();
    expect(rows[1].textContent).toContain("Últimos 7 días");
    expect(rows[1].textContent).toContain("35.0% · 2 registros");
    expect(rows[2].textContent).toContain("55.0% · 9 registros");
    expect(rows[2].textContent).toContain("cobertura parcial");
    expect(rows[2].classList.contains("selected")).toBe(true);
    expect(text).toContain("Color basado en: últimos 30 días");
    expect(text).toContain("Sesiones iniciadas en cada período; sesión entera.");
    expect(text).toContain("Live warning.");
    expect(text).toContain("una media alta no ordena reiniciar");
  });

  it("marks the saved period and says when the reading is missing", () => {
    const { card } = renderCard(ready(), null, 1);
    expect(card.textContent).toContain("Contexto actual de esta sesión: no disponible");
    expect(card.textContent).toContain("Color basado en: último día");
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
    expect(card.querySelectorAll(".ctx-history-row").length).toBe(0);
    expect(card.getAttribute("data-ac-state")).toBe(state.status);
  });

  it("is a tooltip that hides when closed", () => {
    const { card } = renderCard(ready());
    expect(card.getAttribute("role")).toBe("tooltip");
    expect(card.hidden).toBe(false);
  });
});
