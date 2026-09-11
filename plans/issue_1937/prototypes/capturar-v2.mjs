/**
 * Captura las vistas del prototipo v2 y audita la geometría de la maqueta.
 *
 * Usa Chrome headless por CDP (sin instalar nada): abre cada `index-v2.html?vista=<id>`,
 * mide rectángulos reales (ventana, modal, barra del candado, botonera, listas, cartel),
 * detecta desbordes/textos cortados y escribe `vistas-v2/<id>.png` (escala 1.5).
 *
 * Uso:  node capturar-v2.mjs [--solo-auditar] [--vista=<id>[,<id>...]] [--recorte-barra]
 *       (sin --vista captura las 9; con --vista=conflicto solo la pedida;
 *        --recorte-barra agrega vistas-v2/<id>-barra.png con la barra del candado ampliada)
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const outDir = join(here, "vistas-v2");
const auditOnly = process.argv.includes("--solo-auditar");

const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const PORT = 9333;
const VIEWPORT = { width: 1760, height: 1080 };
const SCALE = 1.5;

const VIEWS = ["entrada", "apply-lock", "replica-cerrado", "tipo-preview", "conflicto", "resultado-force", "resultado-unlocked", "futuro", "remove-scope"];
const barCrop = process.argv.includes("--recorte-barra");
const viewArg = process.argv.find((a) => a.startsWith("--vista="));
const selectedViews = viewArg
  ? viewArg.slice("--vista=".length).split(",").map((s) => s.trim()).filter(Boolean)
  : VIEWS;
const views = selectedViews.filter((v) => VIEWS.includes(v));
if (!views.length) {
  console.error(`Vistas válidas: ${VIEWS.join(", ")}`);
  process.exit(1);
}

const profileDir = join(tmpdir(), `ac-prototipo-candados-perfil-${Date.now()}`);

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
  const visible = (el) => el && el.offsetParent !== null && el.getBoundingClientRect().width > 0;
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

  const lockChip = document.querySelector('.replica-item[data-id="r12-lead"] .selection-lock-chip');
  const stateChip = document.querySelector('.selection-lock-state');
  const result = document.querySelector('.agent-scope-result');
  const protectedRows = document.querySelectorAll('.agent-scope-target-row.protected').length;
  const eligibleRows = document.querySelectorAll('.agent-scope-target-row:not(.protected)').length;
  const toast = document.querySelector('#demoToast:not([hidden])');

  return {
    view: document.querySelector('.demo-viewbtn.active')?.dataset.view || '',
    viewport: { w: innerWidth, h: innerHeight },
    window: rect(win),
    stageFits: winR.bottom <= bottomBar.top + 1 && winR.right <= innerWidth,
    modal: modal ? rect(modal) : null,
    modalFits: modal ? (modal.getBoundingClientRect().bottom <= winR.bottom + 1 && modal.getBoundingClientRect().right <= winR.right + 1) : null,
    lockBar: lockBar ? rect(lockBar) : null,
    lockState: stateChip ? stateChip.textContent.trim() : null,
    removeScope: document.querySelector('input[name=removeScope]:checked')?.value ?? null,
    removeOptions: [...document.querySelectorAll('.selection-lock-scope-opt')].map((el) => el.innerText.replace(/\n/g, ' ')),
    removeButton: document.querySelector('#lockRemoveBtn') ? { text: document.querySelector('#lockRemoveBtn').innerText.trim(), disabled: document.querySelector('#lockRemoveBtn').disabled } : null,
    removeNote: document.querySelector('.selection-lock-remove-note')?.innerText.trim() ?? null,
    removeDone: document.querySelector('.selection-lock-remove-done')?.innerText.replace(/\n/g, ' | ') ?? null,
    botonera: botonera ? rect(botonera) : null,
    botoneraVisible: botonera ? botonera.getBoundingClientRect().bottom <= winR.bottom + 1 : null,
    sidebarLockChip: lockChip ? rect(lockChip) : null,
    protectedRows, eligibleRows,
    targets: document.querySelectorAll('.agent-scope-target-row').length,
    result: result ? result.innerText.replace(/\n/g, ' | ') : null,
    toast: toast ? toast.textContent.trim() : null,
    lockBarText: lockBar ? lockBar.innerText.replace(/\n/g, ' | ') : null,
    kindRows: [...document.querySelectorAll('.selection-lock-kind-row')].map((el) => el.innerText.replace(/\n/g, ' ')),
    future: document.querySelector('.selection-lock-future')?.innerText.replace(/\n/g, ' | ') ?? null,
    applyRows: [...document.querySelectorAll('.agent-scope-picker')].map((el) => el.innerText.replace(/\n/g, ' ')),
    choice: document.querySelector('input[name=assignChoice]:checked')?.value ?? null,
    conflictText: document.querySelector('.lock-conflict-card')?.innerText.replace(/\n/g, ' | ') ?? null,
    conflictActions: [...document.querySelectorAll('.lock-conflict-actions .modal-btn')].map((el) => el.innerText.trim()),
    targetRows: [...document.querySelectorAll('.agent-scope-target-row')].map((el) => el.innerText.replace(/\n/g, ' ')),
    sidebarRows: [...document.querySelectorAll('.replica-item')].map((el) => el.innerText.replace(/\n/g, ' ')),
    summary: document.querySelector('.agent-scope-lock-summary')?.innerText.replace(/\n/g, ' | ') ?? null,
    armLabel: document.querySelector('.agent-scope-arm')?.innerText.replace(/\n/g, ' ') ?? null,
    applyLabel: document.querySelector('.agent-picker-apply')?.innerText.trim() ?? null,
    annots: { notes, pins },
    bodies: {
      providers: document.querySelector('#mpProviders')?.children.length ?? 0,
      profiles: document.querySelector('#mpProfiles')?.children.length ?? 0,
      comparison: document.querySelectorAll('#mpComparison .agent-comparison-row').length,
    },
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
    const url = `file:///${join(here, "index-v2.html").replace(/\\/g, "/")}?vista=${view}`;
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
