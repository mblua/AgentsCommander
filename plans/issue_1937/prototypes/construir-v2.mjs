/**
 * Construye `index-v2.html`: un único archivo autocontenido (sin build de Vite,
 * sin servidor, sin dependencias) que reproduce las pantallas reales del
 * sidebar/modal e incorpora la UI propuesta del candado.
 *
 * Fuentes de estilo (SOLO lectura del repo autorizado):
 *   src/sidebar/styles/variables.css   · tokens
 *   src/sidebar/styles/sidebar.css     · UI real del sidebar y del AgentPickerModal
 *   src/shared/styles/toast.css        · toasts
 *
 * V2 (iteración visual): piezas propias en `piezas-v2/`; escribe `index-v2.html`.
 * No toca `index.html` ni los artefactos de v1.
 *
 * Uso:  node construir-v2.mjs
 */
import { readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../repo-AgentsCommander");

const readRepo = (rel) => readFileSync(join(repo, rel), "utf8");
const piece = (rel) => readFileSync(join(here, "piezas-v2", rel), "utf8");
const stripImports = (css) => css.replace(/@import[^;]+;/g, "/* @import eliminado: los estilos van inline */");

let sha = "sin-git";
try {
  sha = execSync("git rev-parse HEAD", { cwd: repo }).toString().trim();
} catch {
  /* opcional: solo provenance */
}

const style = [
  `/* ============================================================================
     ORIGEN VISUAL: repo-AgentsCommander @ ${sha}
     Estilos copiados tal cual de la app (solo se quitó el @import).
     ========================================================================== */
/* ---- src/sidebar/styles/variables.css ---- */
${stripImports(readRepo("src/sidebar/styles/variables.css"))}

/* ---- src/sidebar/styles/sidebar.css ---- */
${stripImports(readRepo("src/sidebar/styles/sidebar.css"))}

/* ---- src/shared/styles/toast.css ---- */
${stripImports(readRepo("src/shared/styles/toast.css"))}

/* ---- piezas/prototipo.css (solo maqueta) ---- */
${piece("prototipo.css")}`,
].join("\n");

const html = `<!doctype html>
<html lang="es" data-sidebar-style="noir-minimal">
<head>
<meta charset="utf-8" />
<title>Prototipo v2 · Aplicar + candado (Coding Agent + Profile)</title>
<meta name="viewport" content="width=device-width, initial-scale=1" />
<link rel="icon" href="data:," />
<!--
  PROTOTIPO V2 DE MAQUETAS — no es producto.
  Generado por construir-v2.mjs desde repo-AgentsCommander @ ${sha}.
  Autocontenido: CSS real inline + demo inline. Abrir con doble clic (file://).
  Datos ficticios. El texto del producto va en inglés; la guía, en español.
  index.html (v1) no se toca: este es un artefacto separado.
-->
<style>
${style}
</style>
</head>
<body>
${piece("cuerpo.html")}
<script>
${piece("app.js")}
</script>
</body>
</html>
`;

const out = join(here, "index-v2.html");
writeFileSync(out, html, "utf8");
const kb = (Buffer.byteLength(html, "utf8") / 1024).toFixed(1);
console.log(`index-v2.html generado (${kb} KB) — repo @ ${sha}`);
