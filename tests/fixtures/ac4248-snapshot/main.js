// #4248 renderer fixture: test-only, standalone; no app source, IPC or filesystem.
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

// AC-4248-008 beacon: proves the module ran; the harness reads it after a deadline.
// docId tells documents apart; trace is the bounded page-side link log.
const beacon = {
  version: 1,
  moduleUtc: new Date().toISOString(),
  phase: "module",
  frameCalls: 0,
  lastGeneration: null,
  rafRequested: 0,
  rafTicks: 0,
  docId: Math.random().toString(36).slice(2, 10),
  href: String(location.href),
  trace: [],
  traceDropped: 0,
};
window.ac4248Beacon = beacon;

// Page trace mark: kept in the beacon (at most 64) and written to the console.
function mark(event, detail = null) {
  const t = Math.round(performance.now());
  if (beacon.trace.length < 64) beacon.trace.push({ t, event, detail });
  else beacon.traceDropped += 1;
  console.log("ac4248trace " + JSON.stringify({ docId: beacon.docId, t, event, detail }));
}

mark("module", beacon.href);
document.addEventListener("DOMContentLoaded", () => mark("domcontentloaded"));
window.addEventListener("load", () => mark("load"));
window.addEventListener("pagehide", () => mark("pagehide"));
document.addEventListener("visibilitychange", () => {
  mark("visibilitychange", document.visibilityState);
});

const COLS = 40;
const ROWS = 12;
const BLOCK_CELLS = 10;
const FRAMES = {
  A: { canvas: "#00ffff", ansiBackground: 41, text: "A" },
  B: { canvas: "#ffff00", ansiBackground: 42, text: "B" },
};
const ZOOMS = new Set([1, 1.1]);
const RENDERERS = new Set(["default", "webgl"]);
const GENERATION_PATTERN = /^[A-Za-z0-9]{1,32}$/;

const CONTROL_SQUARE = 32;

const stage = document.getElementById("stage");
const control = document.getElementById("control");
const canvas = document.getElementById("canvas");
const label = document.getElementById("label");
const terminalHost = document.getElementById("terminal");

let currentGeneration = null;
let terminal = null;
let webglAddon = null;

function setErrorTitle(generation, message, actualRenderer) {
  if (generation !== null && generation !== currentGeneration) return;
  mark("error-title", message);
  beacon.phase = "error-title-set";
  document.title = `ac4248:error:${JSON.stringify({ generation, message, actualRenderer })}`;
}

function rectOf(element) {
  const { x, y, width, height } = element.getBoundingClientRect();
  return { x, y, width, height };
}

function nextFrames(count) {
  beacon.phase = "waiting-frames";
  mark("frames-wait", count);
  return new Promise((resolve) => {
    const step = (remaining) => {
      if (remaining === 0) {
        beacon.phase = "frames-done";
        resolve();
        return;
      }
      beacon.rafRequested += 1;
      requestAnimationFrame(() => {
        beacon.rafTicks += 1;
        mark("raf-tick", beacon.rafTicks);
        step(remaining - 1);
      });
    };
    step(count);
  });
}

function disposeTerminal() {
  webglAddon?.dispose();
  webglAddon = null;
  terminal?.dispose();
  terminal = null;
}

function createTerminal(renderer, generation) {
  disposeTerminal();
  terminal = new Terminal({
    cols: COLS,
    rows: ROWS,
    fontSize: 14,
    fontFamily: "monospace",
    lineHeight: 1,
    cursorBlink: false,
    cursorInactiveStyle: "none",
    allowTransparency: false,
    theme: { background: "#000000", foreground: "#ffffff" },
  });
  terminal.open(terminalHost);
  if (renderer !== "webgl") return "default";
  webglAddon = new WebglAddon();
  webglAddon.onContextLoss(() => {
    setErrorTitle(generation, "webgl context lost", "none");
  });
  terminal.loadAddon(webglAddon);
  return "webgl";
}

function writeTerminal(data) {
  return new Promise((resolve) => terminal.write(data, resolve));
}

function drawCanvas(color) {
  const context = canvas.getContext("2d");
  context.fillStyle = color;
  context.fillRect(0, 0, canvas.width, canvas.height);
}

function terminalBlockRect() {
  const screen = rectOf(terminalHost.querySelector(".xterm-screen"));
  const cellWidth = screen.width / COLS;
  const cellHeight = screen.height / ROWS;
  return { x: screen.x, y: screen.y, width: cellWidth * BLOCK_CELLS, height: cellHeight };
}

function validate(request) {
  const { generation, frame, zoom, renderer } = request ?? {};
  if (typeof generation !== "string" || !GENERATION_PATTERN.test(generation)) {
    throw new Error("invalid generation");
  }
  if (frame === "control") {
    if (zoom !== 1) throw new Error("invalid control zoom");
    if (renderer !== "default") throw new Error("invalid control renderer");
    return { generation, frame, zoom, renderer };
  }
  if (!Object.hasOwn(FRAMES, frame)) throw new Error("invalid frame");
  if (!ZOOMS.has(zoom)) throw new Error("invalid zoom");
  if (!RENDERERS.has(renderer)) throw new Error("invalid renderer");
  return { generation, frame, zoom, renderer };
}

function viewportInfo() {
  return {
    viewport: { width: window.innerWidth, height: window.innerHeight },
    devicePixelRatio: window.devicePixelRatio,
  };
}

function drawControl() {
  disposeTerminal();
  stage.style.display = "none";
  control.style.display = "block";
  control.width = window.innerWidth;
  control.height = window.innerHeight;
  const context = control.getContext("2d");
  for (let y = 0; y < control.height; y += CONTROL_SQUARE) {
    for (let x = 0; x < control.width; x += CONTROL_SQUARE) {
      const even = (x / CONTROL_SQUARE + y / CONTROL_SQUARE) % 2 === 0;
      context.fillStyle = even ? "#ff00ff" : "#000000";
      context.fillRect(x, y, CONTROL_SQUARE, CONTROL_SQUARE);
    }
  }
}

async function showControl(generation) {
  drawControl();
  await nextFrames(2);
  if (generation !== currentGeneration) return;
  const ready = {
    generation,
    frame: "control",
    zoom: 1,
    renderer: "default",
    marker: "AC4248_CONTROL",
    scriptExecuted: true,
    ...viewportInfo(),
  };
  mark("ready-title");
  beacon.phase = "ready-title-set";
  document.title = `ac4248:${JSON.stringify(ready)}`;
}

async function ac4248Frame(request) {
  mark("frame-enter", String(request?.generation).slice(0, 32));
  beacon.frameCalls += 1;
  beacon.phase = "frame-called";
  beacon.lastGeneration =
    typeof request?.generation === "string" ? request.generation.slice(0, 32) : null;
  let parsed;
  try {
    parsed = validate(request);
  } catch (error) {
    mark("frame-invalid", error.message);
    currentGeneration = null;
    setErrorTitle(null, error.message, "none");
    return;
  }
  const { generation, frame, zoom, renderer } = parsed;
  currentGeneration = generation;
  document.title = "ac4248:loading";
  let actualRenderer = "none";
  try {
    if (frame === "control") {
      await showControl(generation);
      return;
    }
    control.style.display = "none";
    stage.style.display = "block";
    actualRenderer = createTerminal(renderer, generation);
    const spec = FRAMES[frame];
    drawCanvas(spec.canvas);
    label.textContent = `Frame ${spec.text}`;
    const blank = " ".repeat(BLOCK_CELLS);
    await writeTerminal(`\x1b[?25l\x1b[${spec.ansiBackground}m${blank}\x1b[0m\r\n\r\n${spec.text}`);
    await nextFrames(2);
    if (generation !== currentGeneration) return;
    const ready = {
      generation,
      frame,
      zoom,
      renderer: actualRenderer,
      scriptExecuted: true,
      ...viewportInfo(),
      rects: {
        canvas: rectOf(canvas),
        overlay: rectOf(document.getElementById("overlay")),
        terminal: terminalBlockRect(),
      },
    };
    mark("ready-title");
    beacon.phase = "ready-title-set";
    document.title = `ac4248:${JSON.stringify(ready)}`;
  } catch (error) {
    if (renderer === "webgl") actualRenderer = "none";
    setErrorTitle(generation, error?.message ?? String(error), actualRenderer);
  }
}

window.ac4248Frame = ac4248Frame;

// AC-4248-008: the native load signal can come before this module runs, so the
// first frame call may arrive early; frame_script parks it in ac4248Pending.
// Drain through the window name so the stall-module probe still sees no call.
const pending = window.ac4248Pending;
window.ac4248Pending = undefined;
if (pending !== undefined) {
  mark("frame-drain", String(pending?.generation).slice(0, 32));
  window.ac4248Frame?.(pending);
}
