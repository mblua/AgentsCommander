/**
 * Captura enfocada del prototipo v3 y audita la geometría de la maqueta.
 *
 * Delta v3: la lista informativa por tipo muestra solo las filas protegidas y la
 * nueva vista `resultado-unlock-room` muestra el estado posterior de quitar el
 * candado de una room entera. Este script NO repite la batería de capturas de v2:
 * por defecto captura solo la vista `tipo-preview` (donde vive la lista reducida)
 * y, con `--recorte-lista`, agrega un recorte ampliado de ese bloque;
 * con `--recorte-barra`, un recorte 2.5× de la barra del candado.
 *
 * Usa Chrome headless por CDP (sin instalar nada): abre `index-v3.html?vista=<id>`,
 * mide rectángulos reales y escribe `vistas-v3/<id>.png` (escala 1.5) más
 * `vistas-v3/<id>-lista.png` (recorte 3×) o `vistas-v3/<id>-barra.png` (2.5×).
 *
 * Uso:  node capturar-v3.mjs [--solo-auditar] [--vista=<id>[,<id>...]] [--recorte-lista] [--recorte-barra]
 *       (sin --vista captura solo tipo-preview)
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const outDir = join(here, "vistas-v3");
const auditOnly = process.argv.includes("--solo-auditar");

const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const PORT = 9334;
const VIEWPORT = { width: 1760, height: 1080 };
const SCALE = 1.5;

const ALL_VIEWS = ["entrada", "apply-lock", "replica-cerrado", "tipo-preview", "conflicto", "resultado-force", "resultado-unlocked", "futuro", "remove-scope", "resultado-unlock-room"];
const listCrop = process.argv.includes("--recorte-lista");
const barCrop = process.argv.includes("--recorte-barra");
const viewArg = process.argv.find((a) => a.startsWith("--vista="));
const selectedViews = viewArg
  ? viewArg.slice("--vista=".length).split(",").map((s) => s.trim()).filter(Boolean)
  : ["tipo-preview"];
const views = selectedViews.filter((v) => ALL_VIEWS.includes(v));
if (!views.length) {
  console.error(`Vistas válidas: ${ALL_VIEWS.join(", ")}`);
  process.exit(1);
}

const profileDir = join(tmpdir(), `ac-prototipo-candados-v3-perfil-${Date.now()}`);

const chrome = spawn(
  CHROME,
  [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=${profileDir}`,
    `--window-size=${VIEWPORT.width},${VIEWPORT.height}`,
    "about:blank",
  ],
  { stdio: "ignore" }
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForChrome() {
  for (let i = 0; i < 60; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/json/version`);
      if (res.ok) return;
    } catch {
      /* arrancando */
    }
    await sleep(250);
  }
  throw new Error("Chrome no respondió en el puerto de depuración");
}

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

const AUDIT = String.raw`(() => {
  const round = (n) => Math.round(n * 10) / 10;
  const rect = (el) => { const r = el.getBoundingClientRect(); return { x: round(r.x), y: round(r.y), w: round(r.width), h: round(r.height), right: round(r.right), bottom: round(r.bottom) }; };
  const win = document.querySelector('#acWindow');
  const winR = win.getBoundingClientRect();
  const modal = document.querySelector('#modalOverlay:not([hidden]) .agent-picker-modal');
  const lockBar = document.querySelector('.selection-lock-bar');
  const botonera = document.querySelector('.agent-picker-botonera');
  const problems = [];
  const notes = document.querySelectorAll('#explainList li').length;
  const pins = document.querySelectorAll('.demo-annot-pin').length;

  const inside = { top: winR.top - 1, left: winR.left - 1, right: winR.right + 1, bottom: winR.bottom + 1 };
  const skip = (el) => el.closest('.demo-annot-layer') || el.classList.contains('agent-scope-target-path');
  document.querySelectorAll('#acWindow *').forEach((el) => {
    if (skip(el)) return;
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return;
    if (r.right > inside.right || r.left < inside.left || r.bottom > inside.bottom || r.top < inside.top) {
      problems.push('desborda ventana: ' + el.className.toString().slice(0, 60));
    }
  });
  document.querySelectorAll('#acWindow .agent-modal *, #acWindow .session-context-menu *').forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return;
    if (el.scrollWidth > el.clientWidth + 2 && getComputedStyle(el).overflow !== 'visible' && (el.textContent || '').trim().length > 0) {
      problems.push('texto cortado: ' + el.className.toString().slice(0, 60));
    }
  });

  const bottomBar = document.querySelector('.demo-bottombar').getBoundingClientRect();
  if (winR.bottom > bottomBar.top + 1) problems.push('la ventana invade la barra inferior');
  if (bottomBar.bottom > innerHeight + 1) problems.push('la barra inferior queda fuera del viewport');

  const stateChip = document.querySelector('.selection-lock-state');
  const result = document.querySelector('.agent-scope-result');
  const toast = document.querySelector('#demoToast:not([hidden])');
  const kindBlock = document.querySelector('.selection-lock-kind');

  return {
    view: document.querySelector('.demo-viewbtn.active')?.dataset.view || '',
    viewport: { w: innerWidth, h: innerHeight },
    topbar: rect(document.querySelector('.demo-topbar')),
    bottomBar: rect(document.querySelector('.demo-bottombar')),
    window: rect(win),
    stageFits: winR.bottom <= bottomBar.top + 1 && winR.right <= innerWidth,
    modal: modal ? rect(modal) : null,
    modalFits: modal ? (modal.getBoundingClientRect().bottom <= winR.bottom + 1 && modal.getBoundingClientRect().right <= winR.right + 1) : null,
    lockBar: lockBar ? rect(lockBar) : null,
    lockState: stateChip ? stateChip.textContent.trim() : null,
    removeScope: document.querySelector('input[name=removeScope]:checked')?.value ?? null,
    removeOptions: [...document.querySelectorAll('.selection-lock-scope-opt')].map((el) => el.innerText.replace(/\n/g, ' ')),
    removeButton: document.querySelector('#lockRemoveBtn') ? { text: document.querySelector('#lockRemoveBtn').innerText.trim(), disabled: document.querySelector('#lockRemoveBtn').disabled } : null,
    kindBlock: kindBlock ? rect(kindBlock) : null,
    kindHead: document.querySelector('.selection-lock-kind-head')?.innerText.trim() ?? null,
    kindRows: [...document.querySelectorAll('.selection-lock-kind-row')].map((el) => el.innerText.replace(/\n/g, ' ')),
    kindNote: document.querySelector('.selection-lock-kind-note')?.innerText.trim() ?? null,
    botonera: botonera ? rect(botonera) : null,
    botoneraVisible: botonera ? botonera.getBoundingClientRect().bottom <= winR.bottom + 1 : null,
    targets: document.querySelectorAll('.agent-scope-target-row').length,
    result: result ? result.innerText.replace(/\n/g, ' | ') : null,
    toast: toast ? toast.textContent.trim() : null,
    lockBarText: lockBar ? lockBar.innerText.replace(/\n/g, ' | ') : null,
    targetRows: [...document.querySelectorAll('.agent-scope-target-row')].map((el) => el.innerText.replace(/\n/g, ' ')),
    applyLabel: document.querySelector('.agent-picker-apply')?.innerText.trim() ?? null,
    annots: { notes, pins },
    problems: [...new Set(problems)],
  };
})()`;

async function main() {
  await waitForChrome();
  const created = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: "PUT" })).json();
  const ws = new WebSocket(created.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res, { once: true });
    ws.addEventListener("error", rej, { once: true });
  });
  const cdp = new Cdp(ws);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  mkdirSync(outDir, { recursive: true });

  for (const view of views) {
    const url = `file:///${join(here, "index-v3.html").replace(/\\/g, "/")}?vista=${view}`;
    const loaded = cdp.once("Page.loadEventFired");
    await cdp.send("Page.navigate", { url });
    await loaded;
    await sleep(700);
    const { result } = await cdp.send("Runtime.evaluate", { expression: AUDIT, returnByValue: true });
    const report = result.value;
    console.log(JSON.stringify(report, null, 1));
    if (!auditOnly) {
      const { result: size } = await cdp.send("Runtime.evaluate", {
        expression: "({ w: innerWidth, h: innerHeight })",
        returnByValue: true,
      });
      const shot = await cdp.send("Page.captureScreenshot", {
        format: "png",
        captureBeyondViewport: false,
        clip: { x: 0, y: 0, width: size.value.w, height: size.value.h, scale: SCALE },
      });
      writeFileSync(join(outDir, `${view}.png`), Buffer.from(shot.data, "base64"));
      if (listCrop) {
        const { result: list } = await cdp.send("Runtime.evaluate", {
          expression: `(() => { const el = document.querySelector('.selection-lock-kind'); if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.max(0, r.x - 6), y: Math.max(0, r.y - 6), w: r.width + 12, h: r.height + 12 }; })()`,
          returnByValue: true,
        });
        if (list.value) {
          const crop = await cdp.send("Page.captureScreenshot", {
            format: "png",
            captureBeyondViewport: false,
            clip: { x: list.value.x, y: list.value.y, width: list.value.w, height: list.value.h, scale: 3 },
          });
          writeFileSync(join(outDir, `${view}-lista.png`), Buffer.from(crop.data, "base64"));
        }
      }
      if (barCrop) {
        const { result: bar } = await cdp.send("Runtime.evaluate", {
          expression: `(() => { const el = document.querySelector('.selection-lock-bar'); if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.max(0, r.x - 6), y: Math.max(0, r.y - 6), w: r.width + 12, h: r.height + 12 }; })()`,
          returnByValue: true,
        });
        if (bar.value) {
          const crop = await cdp.send("Page.captureScreenshot", {
            format: "png",
            captureBeyondViewport: false,
            clip: { x: bar.value.x, y: bar.value.y, width: bar.value.w, height: bar.value.h, scale: 2.5 },
          });
          writeFileSync(join(outDir, `${view}-barra.png`), Buffer.from(crop.data, "base64"));
        }
      }
    }
  }
  ws.close();
  chrome.kill();
  await sleep(400);
  try { rmSync(profileDir, { recursive: true, force: true }); } catch { /* opcional */ }
  console.log(auditOnly ? "Auditoría terminada." : `Vistas escritas en ${outDir}`);
}

main().catch((err) => {
  console.error(err);
  chrome.kill();
  process.exit(1);
});
