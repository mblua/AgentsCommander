/**
 * Prueba de interacción del prototipo v2 (Chrome headless por CDP, sin instalar nada).
 * Enfocada en lo nuevo de v2 y en no romper lo anterior:
 *   · la fila "+ lock" debajo de "Apply to": mismos destinos, una sola elección
 *   · sin conflictos, "Apply to + lock" va directo (sin cartel)
 *   · con réplicas bloqueadas, el cartel las lista (Now vs Requested) y ofrece las 3 salidas
 *   · Cancel no cambia pares, candados ni reinicios
 *   · Apply only to unlocked: la bloqueada queda intacta; la libre se escribe y se canda
 *   · Force all, including locked: la bloqueada se sobrescribe y conserva su candado
 *   · quitar candado en la barra con alcance propio (This replica / All replicas of this kind /
 *     Entire room), independiente de "Apply to", con recuento, pares intactos, targets ajenos
 *     intactos, cero candidatas sin cambios y default futuro conservado
 *
 * Uso:  node herramientas/probar-interacciones-v2.mjs
 */
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { rmSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const indexUrl = `file:///${join(here, "..", "index-v2.html").replace(/\\/g, "/")}`;
const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const PORT = 9335;
const profileDir = join(tmpdir(), `ac-prototipo-candados-v2-test-${Date.now()}`);
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
  results.push({ name, ok, detail });
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

  const go = async (view) => {
    const loaded = cdp.once("Page.loadEventFired");
    await cdp.send("Page.navigate", { url: `${indexUrl}?vista=${view}` });
    await loaded;
    await sleep(500);
  };
  const evalJs = async (expression) => (await cdp.send("Runtime.evaluate", { expression, returnByValue: true })).result.value;
  const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText.trim() ?? null`);
  const flat = async (sel) => (((await text(sel)) || "").replace(/\n/g, " "));
  const exists = (sel) => evalJs(`Boolean(document.querySelector(${JSON.stringify(sel)}))`);
  const sidebarRows = () => evalJs(`[...document.querySelectorAll('.replica-item')].map(e => e.innerText.replace(/\\n/g, ' ')).join(' || ')`);

  /* 1 · fila nueva: mismos destinos con "+ lock", una sola elección */
  await go("apply-lock");
  check("hay dos filas de destinos (Apply to y + lock)", (await evalJs(`document.querySelectorAll('.agent-scope-picker').length`)) === 2);
  const lockRow = await text(".agent-scope-picker--lock");
  check("la fila + lock repite los tres destinos", lockRow?.includes("This replica + lock") && lockRow?.includes("All replicas of this kind + lock") && lockRow?.includes("Entire room + lock"), lockRow);
  check("la barra ya no tiene radios de candado", !(await exists("input[name=lockScope]")) && !(await exists("#lockToggle")));
  check("la barra ofrece los tres alcances de quitar candado", JSON.stringify(await evalJs(`[...document.querySelectorAll('input[name=removeScope]')].map(e => e.value)`)) === JSON.stringify(["replica", "kind", "workgroup"]));
  check("la barra indica estado y acción", (await text(".selection-lock-state")) === "Unlocked" && (await text(".selection-lock-remove-head"))?.toLowerCase() === "remove lock from");
  check("cero candidatas en el alcance: acción deshabilitada", (await evalJs(`document.querySelector('#lockRemoveBtn')?.disabled === true`)) && (await flat(".selection-lock-remove-note")).includes("No protected replicas in this scope"), await text("#lockRemoveBtn"));

  await evalJs(`document.querySelector('input[name=assignChoice][value="kind+lock"]').click()`);
  await sleep(200);
  check("elegir + lock marca una sola opción", (await evalJs(`document.querySelectorAll('.agent-scope-opt.active').length`)) === 1);
  check("la opción activa es la de + lock", (await evalJs(`document.querySelector('input[name=assignChoice]:checked')?.value`)) === "kind+lock");
  check("Apply to no cambia el alcance de quitar candado", (await evalJs(`document.querySelector('input[name=removeScope]:checked')?.value`)) === "replica" && (await text(".selection-lock-state")) === "Unlocked");
  await evalJs(`document.querySelector('input[name=removeScope][value="kind"]').click()`);
  await sleep(200);
  check("quitar candado no cambia el alcance de Apply to", (await evalJs(`document.querySelector('input[name=assignChoice]:checked')?.value`)) === "kind+lock");
  check("el chip describe el alcance de quitar candado", (await text(".selection-lock-state")) === "0 of 2 protected");

  /* 2 · sin conflictos: camino directo (sin cartel) */
  await evalJs(`document.querySelector('#armToggle').click()`);
  await sleep(150);
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(250);
  check("sin conflictos no aparece cartel", !(await exists(".lock-conflict-card")));
  check("sin conflictos aplica + canda directo", (await text(".agent-scope-result"))?.startsWith("2 updated + locked · 0 protected · 0 errors"), await text(".agent-scope-result"));
  const rowsDirect = await sidebarRows();
  check("las dos libres quedan candadas", (rowsDirect.match(/KEEP/g) || []).length === 3, rowsDirect);

  /* 3 · réplica individual: + lock directo y quitar candado conserva el par */
  await go("replica-cerrado");
  const pairBefore = await text(".selection-lock-pair");
  check("réplica protegida muestra Remove lock", (await text("#lockRemoveBtn")) === "Remove lock");
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(200);
  check("Remove lock desprotege", (await text(".selection-lock-state")) === "Unlocked");
  check("Remove lock conserva la selección", (await text(".selection-lock-pair")) === pairBefore, `par después: ${await text(".selection-lock-pair")}`);
  check("Remove lock avisa por toast", (await text("#demoToast"))?.includes("Lock removed"));
  check("quitar por réplica deja el aviso posterior", (await flat(".selection-lock-remove-done")).includes("Lock removed from 1 replica") && (await flat(".selection-lock-remove-done")).includes("no restart"));
  const rowsReplicaRemoved = await sidebarRows();
  check("quitar por réplica no toca las demás de la room", (rowsReplicaRemoved.match(/KEEP/g) || []).length === 1 && rowsReplicaRemoved.includes("ac-tech-lead-v4"), rowsReplicaRemoved.slice(0, 180));
  await evalJs(`document.querySelector('input[name=assignChoice][value="replica+lock"]').click()`);
  await sleep(150);
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(250);
  check("réplica individual + lock va directo (sin cartel)", !(await exists(".lock-conflict-card")));
  check("réplica individual + lock deja el candado", (await text(".selection-lock-state")) === "Protected" && (await sidebarRows()).includes("ac-dev-webpage-ui-v4 Codex A KEEP"));

  /* 4 · conflicto: el cartel lista la bloqueada y ofrece las tres salidas */
  await go("tipo-preview");
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(250);
  const card = await text(".lock-conflict-card");
  check("el cartel aparece con bloqueadas", Boolean(card) && card.includes("1 replica is already locked"));
  check("el cartel nombra la réplica en conflicto", card?.includes("room-12-ac-dev-team-v4 · ac-dev-webpage-ui-v4"));
  check("el cartel muestra par actual y solicitado", card?.includes("Now: Codex · Profile A") && card?.includes("Requested: Claude Code · Profile B"));
  check("las acciones están en orden con Cancel a la izquierda", JSON.stringify(await evalJs(`[...document.querySelectorAll('.lock-conflict-actions .modal-btn')].map(e => e.innerText.trim())`)) === JSON.stringify(["Cancel", "Apply only to unlocked", "Force all, including locked"]));
  check("el cartel explica el efecto de forzar", card?.includes("keeps its lock"));

  /* 5 · Cancel: no cambia pares, candados ni reinicios */
  const rowsBeforeCancel = await sidebarRows();
  await evalJs(`document.querySelector('#conflictCancel').click()`);
  await sleep(200);
  check("Cancel cierra el cartel", !(await exists(".lock-conflict-card")));
  check("Cancel no deja resultado", !(await exists(".agent-scope-result")));
  check("Cancel no cambia pares ni candados", (await sidebarRows()) === rowsBeforeCancel, (await sidebarRows()).slice(0, 140));

  /* 6 · Apply only to unlocked: la bloqueada queda intacta */
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(200);
  await evalJs(`document.querySelector('#conflictUnlockedOnly').click()`);
  await sleep(250);
  const unlockedResult = await text(".agent-scope-result");
  check("solo libres: 1 updated + locked · 1 protected · 0 errors", unlockedResult?.startsWith("1 updated + locked · 1 protected · 0 errors"), unlockedResult?.split("\n")[0]);
  const rowsUnlocked = await sidebarRows();
  check("solo libres: la bloqueada conserva par y candado", rowsUnlocked.includes("ac-dev-webpage-ui-v4 Codex A KEEP") || rowsUnlocked.includes("ac-dev-webpage-ui-v4 Codex A 1 live"), rowsUnlocked.slice(0, 160));
  check("solo libres: la libre queda con el par pedido y candada", (await flat(".selection-lock-kind")).includes("Claude Code · Profile B Protected"), await flat(".selection-lock-kind"));
  const targetText = await text("#mpTargets");
  check("solo libres: la fila de la bloqueada dice Protected · skipped", targetText?.includes("Protected · skipped") && targetText?.includes("Updated + locked"));

  /* 7 · Force all: la bloqueada se sobrescribe y conserva su candado */
  await go("tipo-preview");
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(200);
  await evalJs(`document.querySelector('#conflictForceAll').click()`);
  await sleep(250);
  const forceResult = await text(".agent-scope-result");
  check("forzar todo: 2 updated + locked · 0 protected · 0 errors", forceResult?.startsWith("2 updated + locked · 0 protected · 0 errors"), forceResult?.split("\n")[0]);
  check("forzar todo: aclara que la bloqueada conservó el candado", forceResult?.includes("overwritten and kept the lock"));
  check("forzar todo: las dos filas del tipo quedan con el par pedido", (await flat(".selection-lock-kind")).includes("Claude Code · Profile B Protected"), await flat(".selection-lock-kind"));
  const forceRows = await sidebarRows();
  check("forzar todo: la ex bloqueada sigue candada (KEEP) y con el par nuevo", forceRows.includes("ac-dev-webpage-ui-v4 Claude Code B KEEP"), forceRows.slice(0, 200));
  check("forzar todo: quitar candado del alcance sigue disponible", (await text("#lockRemoveBtn")) === "Remove lock from 2 replicas");
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(200);
  check("quitar candado del tipo conserva los pares nuevos", (await text(".selection-lock-state")) === "0 of 2 protected" && (await text(".selection-lock-kind"))?.includes("Claude Code · Profile B"));

  /* 8 · quitar candado con alcance propio: réplica / tipo / room, independiente de Apply to */
  await go("remove-scope");
  check("alcance masivo disponible aunque la enfocada no tenga candado", (await text(".selection-lock-state")) === "1 of 2 protected" && (await evalJs(`document.querySelector('input[name=removeScope]:checked')?.value`)) === "kind" && !(await sidebarRows()).includes("ac-dev-webpage-ui-v4 Codex A KEEP"));
  check("alcance de quitar independiente de Apply to", (await evalJs(`document.querySelector('input[name=assignChoice]:checked')?.value`)) === "replica");
  const scopesText = await flat(".selection-lock-remove-scopes");
  check("los tres alcances muestran su recuento", scopesText.includes("This replica 0 protected") && scopesText.includes("All replicas of this kind 1 of 2 protected") && scopesText.includes("Entire room 1 of 4 protected"), scopesText);
  const pairBeforeKindRemoval = await text(".selection-lock-pair");
  check("la acción dice cuántas va a quitar", (await text("#lockRemoveBtn")) === "Remove lock from 1 replica" && (await exists("#lockRemoveBtn:not([disabled])")));
  check("la nota aclara el efecto", (await flat(".selection-lock-remove-note")).includes("Keeps Coding Agent + Profile") && (await flat(".selection-lock-remove-note")).includes("No restart"));
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(250);
  const rowsKindRemoved = await sidebarRows();
  check("quitar por tipo desprotege solo esa réplica del tipo", !rowsKindRemoved.includes("ac-dev-webpage-ui-v4 OpenCode C KEEP") && rowsKindRemoved.includes("ac-dev-webpage-ui-v4 OpenCode C"), rowsKindRemoved.slice(0, 220));
  check("target ajeno intacto: misma room, otro tipo", (rowsKindRemoved.match(/KEEP/g) || []).length === 1 && rowsKindRemoved.includes("ac-tech-lead-v4"), rowsKindRemoved.slice(0, 220));
  check("pares intactos: el par del panel no cambió", (await text(".selection-lock-pair")) === pairBeforeKindRemoval);
  check("estado posterior: 0 of 2 protected y acción deshabilitada", (await text(".selection-lock-state")) === "0 of 2 protected" && (await text("#lockRemoveBtn")) === "Nothing to remove" && (await evalJs(`document.querySelector('#lockRemoveBtn').disabled`)) === true);
  check("aviso posterior de quitar candado", (await flat(".selection-lock-remove-done")).includes("Lock removed from 1 replica") && (await flat(".selection-lock-remove-done")).includes("no restart"));

  await go("remove-scope");
  await evalJs(`document.querySelector('input[name=removeScope][value="workgroup"]').click()`);
  await sleep(200);
  check("alcance Entire room con recuento y acción", (await text(".selection-lock-state")) === "1 of 4 protected" && (await text("#lockRemoveBtn")) === "Remove lock from 1 replica");
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(250);
  const rowsRoomRemoved = await sidebarRows();
  check("quitar por room desprotege la protegida de la room", (rowsRoomRemoved.match(/KEEP/g) || []).length === 1 && !rowsRoomRemoved.includes("ac-tech-lead-v4 ORCHESTRATOR Claude Code A KEEP"), rowsRoomRemoved.slice(0, 220));
  check("target ajeno intacto: otra room", rowsRoomRemoved.includes("ac-dev-webpage-ui-v4 OpenCode C KEEP"));

  await go("remove-scope");
  await evalJs(`document.querySelector('input[name=removeScope][value="replica"]').click()`);
  await sleep(200);
  check("cero candidatas: acción deshabilitada y nota", (await evalJs(`document.querySelector('#lockRemoveBtn').disabled`)) === true && (await flat(".selection-lock-remove-note")).includes("No protected replicas in this scope"));
  const rowsBeforeZero = await sidebarRows();
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(200);
  check("cero candidatas no cambia nada", (await sidebarRows()) === rowsBeforeZero && (await text(".selection-lock-state")) === "Unlocked" && !(await exists(".selection-lock-remove-done")), rowsBeforeZero.slice(0, 180));

  await go("futuro");
  await evalJs(`document.querySelector('input[name=assignChoice][value="replica+lock"]').click()`);
  await sleep(150);
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(200);
  check("futuro: el default está visible antes de quitar", (await flat(".selection-lock-future")).includes("Start locked"));
  await evalJs(`document.querySelector('input[name=removeScope][value="kind"]').click()`);
  await sleep(150);
  await evalJs(`document.querySelector('#lockRemoveBtn').click()`);
  await sleep(250);
  check("quitar candado conserva el default de futuras réplicas", (await evalJs(`document.querySelector('.selection-lock-future input[type=checkbox]')?.checked === true`)) && (await flat(".selection-lock-future")).includes("Start locked") && (await text(".selection-lock-state")) === "0 of 2 protected");

  /* 9 · Escape cierra el cartel sin aplicar */
  await go("tipo-preview");
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(200);
  await evalJs(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
  await sleep(200);
  check("Escape cierra el cartel sin dejar resultado", !(await exists(".lock-conflict-card")) && !(await exists(".agent-scope-result")));

  ws.close();
  chrome.kill();
  await sleep(400);
  try { rmSync(profileDir, { recursive: true, force: true }); } catch { /* opcional */ }
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} comprobaciones OK`);
  process.exit(failed ? 1 : 0);
}

main().catch((err) => { console.error(err); chrome.kill(); process.exit(1); });
