# Prototipo v3 — lista informativa solo protegidas + resultado de quitar el candado de una room

Iteración **mínima** sobre la v2 vigente, derivada de ella. **No la reemplaza**: v1 e v2 quedan
intactas (SHA256 al cierre: v1 `ab7a6977223830ff1bcbad8c4a356703fbf24c9e6a5cb5beb51b080129f61c05`,
v2 `d385fcbc18a197572db03d780025de5a949ecfb75bd1dd14b6870da3ee762de6`). V3 es un archivo
separado, navegable y autocontenido: se abre con doble clic, sin servidor ni build.

Snapshot de la v3 anterior (por si la revisión pide comparar o volver atrás):
`index-v3-prev-d1cae46c.html`, SHA256 `d1cae46c997c647a66f873a37ec82bfe66bc7350dda09dd1ee2185c51fd279a4`.

## Iteración 2 (vigente) — «aceptamos desbloquear, ¿cómo se ve?»

Pedido del usuario (2026-09-11, vía tech lead):

> «En el prototipo te falta mostrar como se vería cuando aceptamos desbloquear, por ejemplo, el
> entire room».

Lo agregado, sin diálogo nuevo (es el resultado de la acción que ya existe):

- **Vista 10 · Resultado: room desbloqueada** (`resultado-unlock-room`), después de la vista 9.
  Su `setup` **ejecuta la acción real de la maqueta** —`Remove lock from → Entire room` y el mismo
  `removeLocks()` que dispara el botón— sobre una room con **3 de sus 4 réplicas protegidas**
  (`r12-lead`, `r12-ui`, `r12-rust`) y muestra el estado que deja:
  - aviso verde de éxito y cantidad: `Lock removed from 3 replicas · Coding Agent + Profile kept ·
    no restart`, más el toast `3 locks removed — pairs kept, no restart.`;
  - chip del alcance en **`0 of 4 protected`** (las réplicas de la room quedan sin chip `KEEP`,
    es decir `Unlocked`) y cada opción con su recuento: `This replica 0 protected`,
    `All replicas of this kind 1 of 2 protected`, `Entire room 0 of 4 protected`;
  - **acción inactiva**: `Nothing to remove` deshabilitado, con la nota
    `No protected replicas in this scope — nothing to remove`;
  - **pares conservados**: cada réplica mantiene su `Coding Agent + Profile` (la enfocada,
    `Codex · Profile A`) y no hay reinicio;
  - **afuera sin cambios**: `room-15-dev-team` conserva candado y par (por eso el alcance de tipo
    sigue en `1 of 2 protected`), y el default de futuras réplicas sigue en `Start locked`.
- **Marco de demo para 10 escenarios**: el nav del topbar envuelve a dos filas; el `body` pasó a
  columna flex y el stage toma la altura restante, así la barra inferior (Anterior/Siguiente) nunca
  queda fuera del viewport. El producto (ventana, modal, barra del candado) no cambia.
- **Delta de la iteración 1 intacto**: la lista informativa por tipo sigue mostrando **solo las
  filas protegidas** y sigue ocultando `Not protected`; los recuentos, destinos, cartel de
  conflictos y elegibles siguen calculados sobre el total completo (lo re-verifica la batería).

## Iteración 1 — lista informativa por tipo: solo filas protegidas

Pedido del usuario:

> «LOS not protected no hace falta mostrarlos. solo con el 1 of 2 ya alcanza como dato. y así
> entonces hay menos lineas. en el screenshot, el room-15-dev-team no apareceria con el cambio
> que te pido. --- aplica el cambio en un index-v3».

- **Lista informativa por tipo** (`.selection-lock-kind`, vista 4 · Por tipo + lock): pinta **solo
  las filas protegidas**. En el caso del screenshot, `room-12-ac-dev-team-v4` sigue visible y
  `room-15-dev-team` desaparece de esa lista; hay menos líneas.
- La filtración es **solo de render**: `kindTargets` sigue completo, así que el recuento
  **`1 of 2 protected`** (chip y alcances de `Remove lock`), el total del resumen
  (`2 replica(s) of this kind · 2 room(s)`), los destinos de `Apply to` / `+ lock` / `Remove lock`,
  el cartel de conflictos y las réplicas elegibles quedan intactos y siguen disponibles para aplicar.
- **Cero protegidas**: la lista queda con **cero filas** y solo el resumen (head); no cambia ningún dato.

## Entregables v3

| Artefacto | Ruta | Qué es |
|---|---|---|
| Prototipo v3 | `room-shared/prototipos-candados/index-v3.html` | Un solo archivo autocontenido (313.5 KB) con 10 escenarios |
| Captura del resultado | `room-shared/prototipos-candados/vistas-v3/resultado-unlock-room.png` | Vista 10 completa, 2607×1473: barra con `0 of 4 protected`, aviso de 3 candados, `Nothing to remove`, bloque de default y toast |
| Recorte de la barra | `room-shared/prototipos-candados/vistas-v3/resultado-unlock-room-barra.png` | Recorte 2.5× de `.selection-lock-bar` (2975×265) para leer recuentos, acción y aviso |
| Capturas de la iteración 1 | `room-shared/prototipos-candados/vistas-v3/tipo-preview.png`, `tipo-preview-lista.png` | Vista 4 completa y recorte 3× de la lista reducida |
| Snapshot v3 previo | `room-shared/prototipos-candados/index-v3-prev-d1cae46c.html` | Copia intacta de la v3 anterior (`d1cae46c…`) |
| Reconstruir | `room-shared/prototipos-candados/construir-v3.mjs` | `node construir-v3.mjs` (lee `piezas-v3/`, escribe `index-v3.html`) |
| Capturar + auditar | `room-shared/prototipos-candados/capturar-v3.mjs` | `node capturar-v3.mjs [--solo-auditar] [--vista=<id>[,<id>...]] [--recorte-lista] [--recorte-barra]` |
| Verificación enfocada | `room-shared/prototipos-candados/herramientas/verificar-v3.mjs` | `node herramientas/verificar-v3.mjs`: 31/31 OK |
| QA de PNG v3 | `room-shared/prototipos-candados/herramientas/inspeccionar-png-v3.mjs` | Decodifica `vistas-v3/` y confirma que las capturas no están vacías |
| Fuentes v3 | `room-shared/prototipos-candados/piezas-v3/` | `app.js` con la vista 10 (el setup ejecuta la acción real) y `prototipo.css` con el marco de 10 escenarios; `cuerpo.html` sin cambios |

## Revisión visual sugerida (corta)

1. Abrir `index-v3.html` y elegir **9 · Quitar por alcance** (el *antes*: la barra con
   `Remove lock from` y sus tres recuentos).
2. Pasar a **10 · Resultado: room desbloqueada** (o abrir directamente
   `index-v3.html?vista=resultado-unlock-room`): el *después* real de ejecutar `Entire room`.
3. Comprobar en la barra: `0 of 4 protected`, aviso verde de 3 candados, `Nothing to remove`
   deshabilitado, y el bloque azul del default en `Start locked`.
4. Probar el recorrido a mano en una vista con protegidas: elegir `Entire room` en `Remove lock from`
   y pulsar el botón. La barra queda con la misma forma que la vista 10 (`0 of N protected` para esa
   room, `Nothing to remove` deshabilitado, aviso de candados quitados y pares conservados); la
   cantidad depende de las protegidas que tenga la room de partida. La vista 10 parte de 3
   protegidas para que la cantidad también sea visible.

## Verificación hecha (enfocada, sin repetir la batería de 58 de v2)

`node herramientas/verificar-v3.mjs` → **31/31 OK**:

- **Iteración 1 (19 comprobaciones, intactas)**: fila protegida de room-12 presente; fila no
  protegida de room-15 ausente; recuentos `1 of 2 protected` en chip, alcance de tipo y resumen;
  preview de destinos y cartel de conflictos sin cambios; aplicar sigue actuando sobre la réplica
  que la lista ya no muestra; cero protegidas con resumen, `0 of 2 protected` y `Remove lock`
  deshabilitado.
- **Iteración 2 (12 comprobaciones nuevas)**:
  - el setup de la vista 10 ejecutó la **acción real** (`state.modal.lastRemoval` = `workgroup`/3) y
    en el mundo del mock solo queda candado en `r15-ui` (fuera de la room);
  - chip `0 of 4 protected`; recuentos por alcance `0 protected` / `1 of 2 protected` /
    `0 of 4 protected`;
  - toast `3 locks removed — pairs kept, no restart.` y aviso verde
    `Lock removed from 3 replicas · Coding Agent + Profile kept · no restart`;
  - acción inactiva (`Nothing to remove` deshabilitado, nota de cero candidatas);
  - pares de room-12 conservados y sin `KEEP`; un solo `KEEP`, el de room-15 (afuera intacto);
  - default de futuras réplicas intacto (`Start locked`, par de creación `Codex · Profile A`);
  - re-ejecutar la acción real desde la UI sobre lo que queda: `1 of 2 protected` → `0 of 2` y
    acción deshabilitada.

Geometría: auditoría de las **10 vistas** con `node capturar-v3.mjs --solo-auditar --vista=...`:
en todas `stageFits`, `modalFits` y `botoneraVisible` en `true` y **0 desbordes** (`problems: []`);
la barra inferior entra justo en el viewport con el topbar de dos filas. PNG inspeccionados
(`node herramientas/inspeccionar-png-v3.mjs`): acentos ámbar y verde presentes, ninguna pantalla
vacía. Sin captura de la app real, sin input físico ni ventanas visibles: solo HTML en Chrome
headless con perfil temporal.

## Hashes previos intactos

- `index.html` (v1): `ab7a6977223830ff1bcbad8c4a356703fbf24c9e6a5cb5beb51b080129f61c05`
- `index-v2.html` (v2 vigente): `d385fcbc18a197572db03d780025de5a949ecfb75bd1dd14b6870da3ee762de6`
- `index-v2-prev-e1c882bc.html`: `e1c882bc08f1e05840d680405965686c746d43a9afbf6e4b41e6e1f5cefa07d2`
- `index-v3-prev-d1cae46c.html` (v3 anterior): `d1cae46c997c647a66f873a37ec82bfe66bc7350dda09dd1ee2185c51fd279a4`
- `piezas-v2/` (`9dd84a95…`, `c2be4ca2…`, `5c6256e7…`), `README-v2.md` (`5f54b080…`),
  `construir-v2.mjs`, `capturar-v2.mjs`, `vistas/`, `vistas-v2/`: sin cambios.
- `piezas-v3/cuerpo.html`: `e6dceedc…` (sin cambios desde la iteración 1).

## Hashes de esta entrega

- `index-v3.html` (nuevo): `7901780e9d936c603ba0a704b9cf002197a8bdf5cf687b795b2c0fe37ae7f7b0`
- `piezas-v3/app.js`: `ea1eb3d145a5a925ca0477f7b6e8a539acba93ee8c0bd51bcddd6e7f0a92b6aa`
- `piezas-v3/prototipo.css`: `f45c8e3f9ea7d408bfc6d85d570b04a5bde3f0b054fcad759026539e12e0d71f`
- `capturar-v3.mjs`: `1bdd97f255feae65d87cca2dfe7ed01fb72afe0016242e3be23398fd40f0af6e`
- `herramientas/verificar-v3.mjs`: `6755c514b63059d1627a4928b8fcd69b69ae927e9bde66a8db4ff3c29789e8b6`

## Limitaciones

- Maqueta sin backend: pares, candados, conteos, reinicios y el aviso posterior se simulan en el
  cliente (igual que v1/v2). No hay log de reinicios que inspeccionar: «no reinicia» se expresa en
  el estado y el copy.
- La batería de 58 comprobaciones de v2 no se repitió (pedido explícito de delta mínimo); v3 tiene
  su propia verificación enfocada de 31 comprobaciones, que incluye las 19 de su iteración 1.
- El topbar de la demo ahora usa dos filas con 10 escenarios; es marco de revisión, no producto.
- No es producto ni aprobación técnica: es una maqueta para revisión visual.
