# Prototipo v2 — asignar + candado en un paso (candados de selección)

Iteración visual sobre la maqueta v1. **No la reemplaza**: `index.html` y todos los artefactos de v1
quedan intactos (SHA256 verificado al cierre: `ab7a6977223830ff1bcbad8c4a356703fbf24c9e6a5cb5beb51b080129f61c05`).
V2 es un archivo separado, navegable y autocontenido: se abre con doble clic, sin servidor ni build.

> **Iteración 2 (vigente)**: el usuario pidió que **quitar el candado** presentara también los alcances
> `This replica` / `All replicas of this kind` / `Entire room` en la barra superior. `index-v2.html`
> sigue siendo la v2 vigente (no hay v3); el snapshot anterior quedó en `index-v2-prev-e1c882bc.html`.
> Detalle en la sección *Iteración 2 — alcance propio de `Remove lock`*.

Pedido del usuario (2026-09-11):

> «… justo debajo de esos radio button sería perfecto el lugar para poner los mismos radiobuttons
> pero que tengan el + lock. Entonces, cuando seteamos, podemos al mismo tiempo forzar.»

Y sobre las réplicas ya bloqueadas:

> «Debe presentar cartel indicando con cuales conflictua y ahí ofrecer "Forzar de todas formas",
> "Forzar solo las no previamente lockeadas" o "Cancelar". Las opciones estas deben ser en ingles…
> Y que el cancelar esté a izquierda/derecha según como generalmente esté el cancelar en la app.»

Requisito registrado en `room-shared/candados-decision-usuario.md`. La decisión vigente del usuario
manda para las opciones masivas + lock: **reemplaza** la suposición provisional de omitir siempre las
bloqueadas. El informe previo de arquitectura no invalida este comportamiento de v2; v2 es alcance de
maqueta y no autoriza implementación real.

## Iteración 2 — alcance propio de `Remove lock`

El usuario señaló la barra superior del candado (`SELECTION LOCK` · `Protected` · `Remove lock`) y
pidió que quitar el candado presente también los conceptos **`This replica`**, **`All replicas of
this kind`** y **`Entire room`** («inclusive podría estar presentado ahí arriba como ya figura»).

Lo agregado:

- `Remove lock from` en la barra, con los tres alcances y el recuento de protegidas de cada uno
  (`This replica · 0 protected`, `All replicas of this kind · 1 of 2 protected`,
  `Entire room · 1 of 4 protected`).
- **Alcance independiente** de `Apply to` / `+ lock` de la botonera: cambiar uno no cambia el otro.
- El alcance masivo **sigue disponible aunque la réplica enfocada no tenga candado**, siempre que
  haya protegidas de ese tipo/room.
- Quitar candado **solo cambia protección**: conserva el par `Coding Agent + Profile`, no reinicia
  sesiones y no toca el default de futuras réplicas (esa es otra opción).
- **Cero candidatas** en el alcance elegido ⇒ acción deshabilitada
  (`No protected replicas in this scope — nothing to remove`) y ningún cambio.
- **Post estado claro**: chip de la barra, recuentos por alcance, toast y aviso persistente
  (`Lock removed from 1 replica · Coding Agent + Profile kept · no restart`).

Evidencia de esta iteración: `vistas-v2/remove-scope.png` (vista completa con los tres alcances arriba)
y `vistas-v2/remove-scope-barra.png` (recorte 2.5× de la barra para leer recuentos y copy).

Snapshot previo de la v2 (por si la revisión pide comparar o volver atrás):
`index-v2-prev-e1c882bc.html`, SHA256 `e1c882bc08f1e05840d680405965686c746d43a9afbf6e4b41e6e1f5cefa07d2`.

## Entregables v2

| Artefacto | Ruta | Qué es |
|---|---|---|
| Prototipo v2 | `room-shared/prototipos-candados/index-v2.html` | Un solo archivo autocontenido (309.7 KB). Mismo link de siempre, v2 con la iteración 2 |
| Capturas v2 | `room-shared/prototipos-candados/vistas-v2/` | 9 PNG 2607×1473 + `remove-scope-barra.png` (recorte 2.5× de la barra) |
| Snapshot v2 previo | `room-shared/prototipos-candados/index-v2-prev-e1c882bc.html` | Copia intacta de la v2 anterior (`e1c882bc…`), por si la revisión pide comparar o volver atrás |
| Reconstruir | `room-shared/prototipos-candados/construir-v2.mjs` | `node construir-v2.mjs` (lee `piezas-v2/`, escribe `index-v2.html`) |
| Capturar + auditar | `room-shared/prototipos-candados/capturar-v2.mjs` | `node capturar-v2.mjs [--solo-auditar] [--vista=<id>] [--recorte-barra]` (Chrome headless por CDP) |
| Prueba de interacción v2 | `room-shared/prototipos-candados/herramientas/probar-interacciones-v2.mjs` | `node herramientas/probar-interacciones-v2.mjs`: 58 comprobaciones reales |
| QA de PNG v2 | `room-shared/prototipos-candados/herramientas/inspeccionar-png-v2.mjs` | Decodifica `vistas-v2/` y detecta pantallas vacías |

## El cambio

- **Fila nueva**: debajo de `Apply to` se repiten los tres destinos con `+ lock`
  (`This replica + lock`, `All replicas of this kind + lock`, `Entire room + lock`).
  Es una única elección: marcar una fila desmarca la otra. Elegir `+ lock` escribe el par y pone el
  candado en el mismo paso.
- **La barra dejó de competir**: ya no tiene los radios `Set lock for`. Arriba quedan el estado
  (`Protected` / `Unlocked` / `N of M protected`) y el control `Remove lock from` con sus tres alcances
  y recuentos; el candado se pone abajo, junto a la asignación.
- **Quitar candado con alcance propio**: `This replica` / `All replicas of this kind` / `Entire room`
  se eligen en la barra, separados de `Apply to`. Solo cambia protección: conserva el par, no reinicia
  y no toca el default de futuras réplicas; sin protegidas en el alcance, la acción queda deshabilitada
  y no hay cambios.
- **Sin conflictos** (ninguna réplica del alcance está bloqueada) el camino es directo: `Apply` escribe
  y canda sin preguntar nada.
- **Con conflictos** (lote + lock con réplicas ya bloqueadas) aparece el cartel:
  - título con cuántas réplicas están bloqueadas y el par pedido;
  - lista de conflictos: `Now: <par actual> · Protected` → `Requested: <par pedido>` por réplica;
  - efectos explicados y tres acciones, con **Cancel a la izquierda** como en la botonera real:
    1. `Cancel` — no cambia pares, candados ni reinicios.
    2. `Apply only to unlocked` — solo las libres reciben par y candado; las bloqueadas quedan intactas.
    3. `Force all, including locked` — también las bloqueadas reciben el par y **conservan su candado**.
- **Individual deliberado**: `This replica + lock` no abre cartel; cambia el par y mantiene el candado.

Lo que no cambia: pasos 1 y 2, comparación, preview de destinos, confirmación de sobrescritura,
`Restart sessions after apply`, la herencia confirmada a futuras réplicas y quitar candado (ahora con
alcance propio en la barra).
La asignación masiva **sin** `+ lock` sigue como en v1: omite protegidas sin cartel.

## Vistas de v2

| Vista | PNG | Qué se ve |
|---|---|---|
| 1 · Entrada | `vistas-v2/entrada.png` | El candado en la fila del sidebar y el camino real: clic derecho → **Coding Agent** |
| 2 · Fila + lock | `vistas-v2/apply-lock.png` | La fila nueva completa con sus tres destinos; barra en `Unlocked` sin controles competidores |
| 3 · Candado cerrado | `vistas-v2/replica-cerrado.png` | Réplica `Protected`, los tres alcances de `Remove lock` en la barra y cambio individual permitido |
| 4 · Por tipo + lock | `vistas-v2/tipo-preview.png` | `All replicas of this kind + lock` elegido; cada par listado, la protegida en `Protected · skipped`, `Remove lock from 1 replica` con alcance de tipo |
| 5 · Cartel de conflictos | `vistas-v2/conflicto.png` | El cartel: lista `Now` vs `Requested` y las tres acciones con **Cancel** a la izquierda |
| 6 · Resultado: forzar todo | `vistas-v2/resultado-force.png` | `2 updated + locked · 0 protected · 0 errors`; la bloqueada fue sobrescrita y conservó el candado |
| 7 · Resultado: solo libres | `vistas-v2/resultado-unlocked.png` | `1 updated + locked · 1 protected · 0 errors`; la bloqueada intacta |
| 8 · Futuras réplicas | `vistas-v2/futuro.png` | Herencia confirmada por el usuario, con excepción por réplica y sin propagación retroactiva |
| 9 · Quitar por alcance | `vistas-v2/remove-scope.png` | Los tres alcances de la barra con su recuento; la enfocada está `Unlocked` y el alcance masivo sigue disponible |
| Barra ampliada (recorte) | `vistas-v2/remove-scope-barra.png` | Recorte 2.5× de la barra del candado para leer alcances, recuentos, acción y nota |

## Revisión visual sugerida (corta)

1. Abrir `index-v2.html` y elegir **9 · Quitar por alcance**: los tres alcances con su recuento están en
   la barra y la acción dice cuántas va a quitar.
2. Probar el botón: chip y recuentos pasan a `0 of 2 protected`, la acción queda en
   `Nothing to remove` y aparece el aviso de par conservado (no reinicio).
3. Elegir **3 · Candado cerrado**: con `This replica` quita 1; con **4 · Por tipo + lock** el alcance
   de tipo muestra `1 of 2 protected` aunque la réplica enfocada no cambie de alcance.
4. Verificar que elegir un alcance de quitar no mueve los radios de `Apply to` / `+ lock` y viceversa.

## Verificación hecha

- **Interacción** (`node herramientas/probar-interacciones-v2.mjs`): **58/58 OK**. Cubre la fila nueva
  (una sola opción activa), el camino directo sin conflictos, el cartel con la réplica en conflicto y
  sus tres acciones, `Cancel` sin cambios, `Apply only to unlocked` (bloqueada intacta),
  `Force all, including locked` (bloqueada sobrescrita con su candado), y además quitar candado por
  réplica/tipo/room, pares intactos, targets ajenos intactos, independencia entre el alcance de la
  barra y `Apply to`, cero candidatas sin cambios y default de futuras réplicas conservado.
- **Auditoría de geometría** (`node capturar-v2.mjs`): en las 9 vistas la ventana, el modal y el cartel
  caben, la botonera queda visible y hay **0 desbordes**.
- **Inspección de PNG** (`node herramientas/inspeccionar-png-v2.mjs`): 9 vistas 2607×1473 más el
  recorte `remove-scope-barra.png` (2975×220), con acentos ámbar (barra/candado) y cian; verde en los
  resultados y en el chip `confirmed` de la vista 8.
- **V1 intacto**: `sha256sum index.html` =
  `ab7a6977223830ff1bcbad8c4a356703fbf24c9e6a5cb5beb51b080129f61c05`; sin cambios en `piezas/`,
  `vistas/`, `README.md`, `construir.mjs` ni `capturar.mjs`.
- **Snapshot v2 previo**: `index-v2-prev-e1c882bc.html` conserva el SHA256
  `e1c882bc08f1e05840d680405965686c746d43a9afbf6e4b41e6e1f5cefa07d2` de la iteración 1.
- Sin captura de la app real, sin input físico ni ventanas visibles: solo HTML de demostración en
  Chrome headless con perfil temporal.

## Limitaciones

- Maqueta sin backend: pares, candados, reinicios y conteos se simulan en el cliente.
- No se modelan concurrencia, espera del lote, timeouts, resultados parciales, fingerprint ni persistencia real.
- El «no reinicia» al quitar candado se expresa en el estado y el copy de la maqueta: no hay un log de
  reinicios que inspeccionar.
- El cartel ya refleja la decisión del usuario, pero sigue siendo una maqueta: no es una aprobación técnica.
