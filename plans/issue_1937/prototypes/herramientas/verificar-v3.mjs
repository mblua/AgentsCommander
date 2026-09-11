/**
 * Verificación enfocada del delta v3 (Chrome headless por CDP, sin instalar nada).
 * NO repite la batería de 58 de v2: cubre solo lo que cambia y lo que debe quedar intacto.
 *
 * Delta v3: en la lista informativa por tipo (`.selection-lock-kind`) solo se pintan
 * las filas protegidas; el recuento N of M protected, los destinos, el cartel de
 * conflictos y los datos elegibles siguen calculados sobre el total completo.
 *
 * Comprueba:
 *   1. fila protegida de room-12 presente; fila no protegida de room-15 ausente
 *   2. recuentos intactos: 1 of 2 protected, total 2 replicas · 2 rooms
 *   3. preview de destinos y aplicar no cambian: los elegibles siguen disponibles
 *      (el cartel de conflictos y "Apply only to unlocked" siguen actuando sobre la
 *      réplica no protegida que la lista informativa ya no muestra)
 *   4. cero protegidas: resumen (head) visible y cero filas, sin cambios de datos
 *   5. vista `resultado-unlock-room`: ejecutar la acción real (Remove lock from →
 *      Entire room) deja 0 of 4 protected, acción inactiva, pares conservados,
 *      room-15 con su candado y el default de futuras réplicas intacto
 *
 * Uso:  node herramientas/verificar-v3.mjs
 */
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { rmSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const indexUrl = `file:///${join(here, "..", "index-v3.html").replace(/\\/g, "/")}`;
const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const PORT = 9336;
const profileDir = join(tmpdir(), `ac-prototipo-candados-v3-test-${Date.now()}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const chrome = spawn(
  CHROME,
  ["--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profileDir}`, "--window-size=1760,1080", "about:blank"],
  { stdio: "ignore" }
);

class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.events = new Map();
    ws.addEventListener("message", (e) => {
      const msg = JSON.parse(e.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
      } else if (msg.method && this.events.has(msg.method)) {
        for (const fn of this.events.get(msg.method)) fn(msg.params);
        this.events.delete(msg.method);
      }
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  once(method) {
    return new Promise((resolve) => {
      const list = this.events.get(method) || [];
      list.push(resolve);
      this.events.set(method, list);
    });
  }
}

const results = [];
function check(name, ok, detail) {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
}

async function main() {
  for (let i = 0; i < 60; i++) {
    try { if ((await fetch(`http://127.0.0.1:${PORT}/json/version`)).ok) break; } catch { /* arrancando */ }
    await sleep(250);
  }
  const created = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: "PUT" })).json();
  const ws = new WebSocket(created.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.addEventListener("open", res, { once: true }); ws.addEventListener("error", rej, { once: true }); });
  const cdp = new Cdp(ws);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");

  const evalJs = async (expression) => (await cdp.send("Runtime.evaluate", { expression, returnByValue: true })).result.value;
  const go = async (view) => {
    const loaded = cdp.once("Page.loadEventFired");
    await cdp.send("Page.navigate", { url: `${indexUrl}?vista=${view}` });
    await loaded;
    await sleep(500);
  };
  const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText.trim() ?? null`);
  const flat = async (sel) => ((await text(sel)) || "").replace(/\n/g, " ");
  const kindRows = () => evalJs(`[...document.querySelectorAll('.selection-lock-kind-row')].map(e => e.innerText.replace(/\\n/g, ' '))`);
  const targetRows = () => evalJs(`[...document.querySelectorAll('.agent-scope-target-row')].map(e => e.innerText.replace(/\\n/g, ' '))`);
  const sidebarRows = () => evalJs(`[...document.querySelectorAll('.replica-item')].map(e => e.innerText.replace(/\\n/g, ' ')).join(' || ')`);

  /* 1-3 · lista reducida, recuentos y datos intactos (vista 4 · Por tipo + lock) */
  await go("tipo-preview");
  const rows = await kindRows();
  check("la lista informativa muestra solo 1 fila (antes 2)", rows.length === 1, JSON.stringify(rows));
  check("la fila protegida de room-12 sigue visible", rows[0]?.includes("room-12-ac-dev-team-v4") && rows[0]?.includes("ac-dev-webpage-ui-v4") && rows[0]?.includes("Protected"), rows[0]);
  check("la fila no protegida de room-15 ya no se lista", !rows.some((r) => r.includes("room-15-dev-team")) && !rows.some((r) => r.includes("Not protected")));
  check("el chip conserva el recuento sobre el total: 1 of 2 protected", (await text(".selection-lock-state")) === "1 of 2 protected", await text(".selection-lock-state"));
  const scopes = await flat(".selection-lock-remove-scopes");
  check("el alcance por tipo conserva su recuento: 1 of 2 protected", scopes.includes("All replicas of this kind 1 of 2 protected"), scopes);
  const head = await flat(".selection-lock-kind-head");
  check("el resumen conserva el total completo: 2 replicas · 2 rooms", head.includes("2 replica(s) of this kind") && head.includes("2 room(s)"), head);
  const targets = await targetRows();
  check("el preview de destinos sigue listando las 2 réplicas elegibles/bloqueadas", targets.length === 2 && targets.some((r) => r.includes("room-15-dev-team") && r.includes("Will update + lock")) && targets.some((r) => r.includes("Protected · skipped")), JSON.stringify(targets));
  check("el destino + lock sigue contando la elegible oculta: Overwrite 1 + lock", (await text(".agent-picker-apply")) === "Overwrite 1 + lock of this kind", await text(".agent-picker-apply"));

  /* 4 · aplicar sigue funcionando sobre la réplica que la lista oculta */
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(250);
  const conflict = await flat(".lock-conflict-card");
  check("el cartel de conflictos sigue apareciendo", (await evalJs(`Boolean(document.querySelector('.lock-conflict-card'))`)) === true);
  check("el cartel nombra la bloqueada y el par pedido", conflict.includes("room-12-ac-dev-team-v4") && conflict.includes("Protected") && conflict.includes("Claude Code · Profile B"), conflict.slice(0, 200));
  await evalJs(`document.querySelector('#conflictUnlockedOnly').click()`);
  await sleep(250);
  check("solo libres: resultado 1 updated + locked · 1 protected", (await text(".agent-scope-result"))?.startsWith("1 updated + locked · 1 protected · 0 errors"), await text(".agent-scope-result"));
  const sidebarAfter = await sidebarRows();
  check("la elegible oculta (room-15) recibió el par y el candado", sidebarAfter.includes("ac-dev-webpage-ui-v4 Claude Code B KEEP"), sidebarAfter.slice(0, 260));
  check("la protegida (room-12) conservó su par y su candado", sidebarAfter.includes("ac-dev-webpage-ui-v4 Codex A KEEP"), sidebarAfter.slice(0, 260));
  const rowsAfter = await kindRows();
  check("con las 2 protegidas, la lista vuelve a mostrar las 2 filas", rowsAfter.length === 2 && rowsAfter.every((r) => r.includes("Protected")), JSON.stringify(rowsAfter));

  /* 5 · cero protegidas: solo resumen, cero filas, sin cambios de datos */
  await go("tipo-preview");
  await evalJs(`document.querySelector('input[name=removeScope][value="kind"]').click()`);
  await sleep(200);
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(250);
  check("cero protegidas: la lista informativa queda con 0 filas", (await kindRows()).length === 0);
  const headZero = await flat(".selection-lock-kind-head");
  check("cero protegidas: el resumen sigue mostrando el total completo", (await evalJs(`Boolean(document.querySelector('.selection-lock-kind-head'))`)) === true && headZero.includes("2 replica(s) of this kind"), headZero);
  check("cero protegidas: el recuento queda en 0 of 2 protected", (await text(".selection-lock-state")) === "0 of 2 protected", await text(".selection-lock-state"));
  const targetsZero = await targetRows();
  check("cero protegidas: los destinos elegibles siguen intactos (2 filas)", targetsZero.length === 2 && targetsZero.every((r) => !r.includes("Protected · skipped")), JSON.stringify(targetsZero));
  check("cero protegidas: la acción de quitar queda deshabilitada", (await evalJs(`document.querySelector('#lockRemoveBtn').disabled`)) === true && (await text("#lockRemoveBtn")) === "Nothing to remove");

  /* 6 · vista nueva: resultado real de quitar el candado de una room entera */
  await go("resultado-unlock-room");
  const worldAfter = await evalJs(`({ locked: allReplicas().filter((r) => r.locked).map((r) => r.id), removal: state.modal.lastRemoval })`);
  check("vista nueva: el setup ejecutó la acción real del mock (Entire room)", worldAfter?.removal?.scope === "workgroup" && worldAfter?.removal?.count === 3, JSON.stringify(worldAfter));
  check("vista nueva: solo queda candado fuera de la room (room-15)", JSON.stringify(worldAfter?.locked) === JSON.stringify(["r15-ui"]), JSON.stringify(worldAfter?.locked));
  check("vista nueva: chip del alcance en 0 of 4 protected", (await text(".selection-lock-state")) === "0 of 4 protected", await text(".selection-lock-state"));
  const scopesAfter = await flat(".selection-lock-remove-scopes");
  check("vista nueva: recuentos por alcance intactos (tipo sigue en 1 of 2)", scopesAfter.includes("This replica 0 protected") && scopesAfter.includes("All replicas of this kind 1 of 2 protected") && scopesAfter.includes("Entire room 0 of 4 protected"), scopesAfter);
  check("vista nueva: toast de cantidad desbloqueada", (await text("#demoToast")) === "3 locks removed — pairs kept, no restart.", await text("#demoToast"));
  const doneAfter = await flat(".selection-lock-remove-done");
  check("vista nueva: aviso de éxito con cantidad y par conservado", doneAfter.includes("Lock removed from 3 replicas") && doneAfter.includes("Coding Agent + Profile kept") && doneAfter.includes("no restart"), doneAfter);
  check("vista nueva: acción inactiva (Nothing to remove deshabilitado)", (await evalJs(`document.querySelector('#lockRemoveBtn').disabled`)) === true && (await text("#lockRemoveBtn")) === "Nothing to remove" && (await flat(".selection-lock-remove-note")).includes("No protected replicas in this scope"));
  const rowsUI = await sidebarRows();
  check("vista nueva: pares de room-12 conservados y sin KEEP", rowsUI.includes("ac-dev-webpage-ui-v4 Codex A") && !rowsUI.includes("ac-dev-webpage-ui-v4 Codex A KEEP") && rowsUI.includes("ac-dev-rust-core-v4 OpenCode B") && !rowsUI.includes("ac-dev-rust-core-v4 OpenCode B KEEP"), rowsUI.slice(0, 300));
  const keeps = (rowsUI.match(/KEEP/g) || []).length;
  check("vista nueva: afuera intacto, un solo KEEP (room-15)", keeps === 1 && rowsUI.includes("ac-dev-webpage-ui-v4 OpenCode C KEEP"), rowsUI.slice(0, 300));
  const futureAfter = await flat(".selection-lock-future");
  check("vista nueva: default de futuras réplicas intacto (Start locked, par de creación)", (await evalJs(`document.querySelector('.selection-lock-future input[type=checkbox]')?.checked === true`)) && futureAfter.includes("Start locked") && futureAfter.includes("Codex · Profile A"), futureAfter);
  /* Re-ejecutar la acción real desde la UI sobre lo que queda (el candado de afuera). */
  await evalJs(`document.querySelector('input[name=removeScope][value="kind"]').click()`);
  await sleep(150);
  check("vista nueva: el alcance de tipo conserva la protegida de afuera (1 of 2)", (await text(".selection-lock-state")) === "1 of 2 protected" && (await text("#lockRemoveBtn")) === "Remove lock from 1 replica", await text(".selection-lock-state"));
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(200);
  check("vista nueva: la acción real deja todo en cero (0 of 2, deshabilitado)", (await text(".selection-lock-state")) === "0 of 2 protected" && (await evalJs(`document.querySelector('#lockRemoveBtn').disabled`)) === true);

  ws.close();
  chrome.kill();
  await sleep(400);
  try { rmSync(profileDir, { recursive: true, force: true }); } catch { /* opcional */ }
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} comprobaciones OK`);
  process.exit(failed ? 1 : 0);
}

main().catch((err) => { console.error(err); chrome.kill(); process.exit(1); });
