// #4248 renderer fixture: test-only, standalone; no app source, IPC or filesystem.
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

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
  document.title = `ac4248:error:${JSON.stringify({ generation, message, actualRenderer })}`;
}

function rectOf(element) {
  const { x, y, width, height } = element.getBoundingClientRect();
  return { x, y, width, height };
}

function nextFrames(count) {
  return new Promise((resolve) => {
    const step = (remaining) => {
      if (remaining === 0) resolve();
      else requestAnimationFrame(() => step(remaining - 1));
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
  document.title = `ac4248:${JSON.stringify(ready)}`;
}

async function ac4248Frame(request) {
  let parsed;
  try {
    parsed = validate(request);
  } catch (error) {
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
    document.title = `ac4248:${JSON.stringify(ready)}`;
  } catch (error) {
    if (renderer === "webgl") actualRenderer = "none";
    setErrorTitle(generation, error?.message ?? String(error), actualRenderer);
  }
}

window.ac4248Frame = ac4248Frame;
