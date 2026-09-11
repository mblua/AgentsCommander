/**
 * Prueba de interacción del prototipo (Chrome headless por CDP, sin instalar nada).
 * Verifica que los elementos clave del mock funcionan de verdad:
 *   · clic derecho en una réplica abre el menú real y "Coding Agent" abre el modal
 *   · el interruptor del candado protege/desprotege y mantiene el par
 *   · "Remove lock" quita el candado y conserva la selección
 *   · "Set lock for → All replicas of this kind" canda el tipo con el par propio de cada réplica
 *   · "Apply" masivo omite protegidas y reporta 1 updated · 1 protected · 0 errors
 *
 * Uso:  node herramientas/probar-interacciones.mjs
 */
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { rmSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const indexUrl = `file:///${join(here, "..", "index.html").replace(/\\/g, "/")}`;
const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const PORT = 9334;
const profileDir = join(tmpdir(), `ac-prototipo-candados-test-${Date.now()}`);
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
  const exists = (sel) => evalJs(`Boolean(document.querySelector(${JSON.stringify(sel)}))`);

  /* 1 · entrada: menú contextual real → Coding Agent → modal */
  await go("entrada");
  await evalJs(`(() => {
    const row = document.querySelector('.replica-item[data-id="r12-ui"]');
    const r = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 60, clientY: r.top + 8 }));
  })()`);
  await sleep(200);
  check("clic derecho abre el menú contextual", await evalJs(`!document.querySelector('#ctxMenu').hidden`));
  check("el menú incluye 'Coding Agent'", (await text("#ctxMenu"))?.includes("Coding Agent"));
  await evalJs(`document.querySelector('#ctxMenu [data-action="coding-agent"]').click()`);
  await sleep(250);
  check("'Coding Agent' abre el modal real", await evalJs(`!document.querySelector('#modalOverlay').hidden`));
  check("el modal muestra el par y la barra del candado", (await text(".selection-lock-bar"))?.includes("Codex · Profile A"));

  /* 2 · candado abierto → cerrar y abrir conserva el par */
  await go("replica-abierto");
  const pairBefore = await text(".selection-lock-pair");
  await evalJs(`document.querySelector('#lockToggle').click()`);
  await sleep(200);
  check("el interruptor protege la réplica", (await text(".selection-lock-state")) === "Protected");
  check("aparece 'Remove lock'", await exists("#lockRemove"));
  check("activar avisa por toast", (await text("#demoToast"))?.includes("Selection lock set"));
  await evalJs(`document.querySelector('#lockRemove').click()`);
  await sleep(200);
  check("'Remove lock' desprotege", (await text(".selection-lock-state")) === "Unlocked");
  check("quitar el candado conserva la selección", (await text(".selection-lock-pair")) === pairBefore, `par después: ${await text(".selection-lock-pair")}`);
  check("quitar avisa por toast", (await text("#demoToast"))?.includes("Lock removed"));

  /* 3 · por tipo: una sola acción explícita, cada réplica conserva su par */
  await go("replica-abierto");
  await evalJs(`document.querySelector('input[name=lockScope][value=kind]').click()`);
  await sleep(200);
  check("'Set lock for' muestra el tipo y el par propio", (await evalJs(`document.querySelectorAll('.selection-lock-kind-row').length`)) === 2);
  check("en alcance tipo no compite el control de una réplica", !(await exists("#lockToggle")) && !(await exists("#lockRemove")));
  check("el botón de tipo dice la acción exacta", (await text("#lockKindAction")) === "Lock 2 replicas", await text("#lockKindAction"));
  const pillsBefore = await evalJs(`[...document.querySelectorAll('.selection-lock-pill')].map(e => e.innerText).join(' | ')`);
  check("las píldoras muestran el estado actual, no una acción pendiente", pillsBefore === "Not protected | Not protected", pillsBefore);
  const pairsBefore = await evalJs(`[...document.querySelectorAll('.selection-lock-kind-row .pair')].map(e => e.innerText).join(' | ')`);
  await evalJs(`document.querySelector('#lockKindAction').click()`);
  await sleep(200);
  const pillsAfter = await evalJs(`[...document.querySelectorAll('.selection-lock-pill')].map(e => e.innerText).join(' | ')`);
  const pairsAfter = await evalJs(`[...document.querySelectorAll('.selection-lock-kind-row .pair')].map(e => e.innerText).join(' | ')`);
  check("el tipo entero queda candado", pillsAfter === "Protected | Protected", pillsAfter);
  check("el botón pasa a quitar el candado", (await text("#lockKindAction")) === "Remove lock from 2 replicas");
  check("cada réplica conserva su par propio", pairsBefore === pairsAfter && pairsAfter.includes("Codex") && pairsAfter.includes("OpenCode"), pairsAfter);
  check("toast del candado por tipo", (await text("#demoToast"))?.includes("each replica kept its own pair"));
  await evalJs(`document.querySelector('#lockKindAction').click()`);
  await sleep(200);
  check("quitar el candado de tipo conserva los pares", (await text("#lockKindAction")) === "Lock 2 replicas" && (await evalJs(`[...document.querySelectorAll('.selection-lock-kind-row .pair')].map(e => e.innerText).join(' | ')`)) === pairsBefore);
  check("toast al quitar el candado de tipo", (await text("#demoToast"))?.includes("locks removed — selections kept"));

  /* 4 · masivo: preview, armado y resultado */
  await go("tipo-preview");
  check("el tipo muestra qué falta por candar", (await text("#lockKindAction")) === "Lock 1 remaining replica", await text("#lockKindAction"));
  check("estado mixto coherente en las píldoras", (await evalJs(`[...document.querySelectorAll('.selection-lock-pill')].map(e => e.innerText).join(' | ')`)) === "Protected | Not protected");
  check("preview marca la protegida", (await text(".agent-scope-target-row.protected"))?.includes("Protected · skipped"));
  check("resumen separa elegibles de protegidas", (await text(".agent-scope-lock-summary"))?.includes("1 protected · skipped, not restarted"));
  await evalJs(`document.querySelector('#mpApply').click()`);
  await sleep(250);
  const result = await text(".agent-scope-result");
  check("resultado: 1 updated · 1 protected · 0 errors", result?.startsWith("1 updated · 1 protected · 0 errors"), result?.split("\n")[0]);
  check("resultado aclara que la protegida no se reinició", result?.includes("not restarted"));

  /* 5 · candidata actualizada, protegida intacta */
  const rows = await evalJs(`[...document.querySelectorAll('.replica-item')].map(e => e.innerText.replace(/\\n/g, ' ')).join(' || ')`);
  check("la réplica elegible recibió el par", rows.includes("ac-dev-webpage-ui-v4 Codex A 1 live") || rows.includes("ac-dev-webpage-ui-v4 Codex A"), rows.slice(0, 200));
  check("la protegida conserva su par y candado", rows.includes("ac-dev-webpage-ui-v4 Codex A KEEP"));

  ws.close();
  chrome.kill();
  await sleep(400);
  try { rmSync(profileDir, { recursive: true, force: true }); } catch { /* opcional */ }
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} comprobaciones OK`);
  process.exit(failed ? 1 : 0);
}

main().catch((err) => { console.error(err); chrome.kill(); process.exit(1); });
