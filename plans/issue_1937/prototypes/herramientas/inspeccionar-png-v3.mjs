/* Inspección numérica de los PNG de v3 (lista informativa reducida) (sin dependencias).
   Decodifica PNG 8-bit RGBA/RGB no entrelazado y reporta color medio,
   proporción de acentos (ámbar del candado, verde de resultado, cian de la app)
   y la caja delimitadora del ámbar, para detectar pantallas vacías o mal
   renderizadas sin poder mirar la imagen. */
import { readFileSync, readdirSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { join } from "node:path";

function decodePng(buf) {
  let pos = 8;
  let width = 0, height = 0, bitDepth = 0, colorType = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      if (data[12] !== 0) throw new Error("PNG entrelazado no soportado");
    } else if (type === "IDAT") idat.push(data);
    else if (type === "IEND") break;
    pos += 12 + len;
  }
  if (bitDepth !== 8 || ![2, 6].includes(colorType)) throw new Error(`PNG no soportado: depth=${bitDepth} type=${colorType}`);
  const channels = colorType === 6 ? 4 : 3;
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = Buffer.alloc(stride * height);
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, y * (stride + 1) + 1 + stride);
    const cur = Buffer.from(line);
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? cur[i - channels] : 0;
      const b = prev[i];
      const c = i >= channels ? prev[i - channels] : 0;
      if (filter === 1) cur[i] = (cur[i] + a) & 255;
      else if (filter === 2) cur[i] = (cur[i] + b) & 255;
      else if (filter === 3) cur[i] = (cur[i] + ((a + b) >> 1)) & 255;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        cur[i] = (cur[i] + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)) & 255;
      }
    }
    cur.copy(out, y * stride);
    prev = cur;
  }
  return { width, height, channels, data: out };
}

const near = (r, g, b, tr, tg, tb, tol) => Math.abs(r - tr) <= tol && Math.abs(g - tg) <= tol && Math.abs(b - tb) <= tol;

function analyze(file) {
  const png = decodePng(readFileSync(file));
  const { width, height, channels, data } = png;
  let sum = 0, amber = 0, green = 0, cyan = 0, black = 0;
  let ax0 = 1e9, ay0 = 1e9, ax1 = -1, ay1 = -1;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * channels;
      const r = data[i], g = data[i + 1], b = data[i + 2];
      sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
      if (near(r, g, b, 227, 179, 65, 26)) {
        amber++;
        if (x < ax0) ax0 = x; if (x > ax1) ax1 = x;
        if (y < ay0) ay0 = y; if (y > ay1) ay1 = y;
      }
      if (near(r, g, b, 53, 242, 166, 30)) green++;
      if (near(r, g, b, 0, 212, 255, 24)) cyan++;
      if (r < 8 && g < 8 && b < 12) black++;
    }
  }
  const total = width * height;
  return {
    file: file.split(/[\\/]/).pop(),
    size: `${width}x${height}`,
    meanLum: +(sum / total).toFixed(1),
    blackPct: +((black / total) * 100).toFixed(1),
    amberPct: +((amber / total) * 100).toFixed(3),
    greenPct: +((green / total) * 100).toFixed(3),
    cyanPct: +((cyan / total) * 100).toFixed(3),
    amberBox: amber ? `${ax0},${ay0} → ${ax1},${ay1}` : "ausente",
  };
}

for (const f of readdirSync("vistas-v3").sort()) {
  console.log(JSON.stringify(analyze(join("vistas-v3", f))));
}
