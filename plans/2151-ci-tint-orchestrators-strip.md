# 2151 — El tinte azul de CI no alcanza la tira ORCHESTRATORS

Status: READY_FOR_IMPLEMENTATION
Issue: https://github.com/mblua/AgentsCommander/issues/2151
Branch: `fix/2151-ci-tint-orchestrators-strip` (base `origin/main` = 7887c46a438d5dd2c1895ccbcc824ed2be8b6144)

## Objetivo

El tinte azul derivado de CI debe aplicarse en la tira ORCHESTRATORS en las mismas
ocasiones en que hoy se aplica en el árbol de la room. Un mismo orquestador no puede
quedar teñido de un lado y sin teñir del otro en el mismo frame.

## Causa (evidencia verificada contra el código de la branch)

- `src/sidebar/components/ProjectPanel.tsx:2450-2454` — `orchestratorCiRunning()` =
  `isCoord() && repoBadges().some(repo => remoteActivityStore.forPath(repo.sourcePath)?.ci === "running")`.
- `src/sidebar/components/ProjectPanel.tsx:2464-2467` — `rowIsWorking()` suma
  `orchestratorCiRunning()` solo en la rama no-`quick`:
  `rowContext === "quick" ? workgroupIsWorking(wg) : isReplicaWorking(wg, replica) || orchestratorCiRunning()`.
  Esa asimetría es la causa directa.
- `src/sidebar/components/ProjectPanel.tsx:2442-2449` y `2455-2463` — comentarios de
  #2131 y #1783 que declaran el límite actual como deliberado.
- `src/sidebar/components/ProjectPanel.tsx:2977` — único sitio que pasa `"quick"`,
  dentro de `<div class="coord-quick-access">` bajo el header `Orchestrators` (2966).
- `src/sidebar/components/ProjectPanel.tsx:2542` — único consumidor de `rowIsWorking()`
  (`classList={{ working: rowIsWorking() }}`).

Consumidores de `workgroupIsWorking` (que NO deben cambiar):
`src/sidebar/components/workgroup-session.ts:41` (`splitWorkgroupsByWorking`, orden de
rooms), `src/sidebar/components/ProjectPanel.tsx:2730` (`.ac-wg-subgroup` wash) y
`src/sidebar/components/WorkgroupGroupRail.tsx:83` (dot/contador del rail, vía
`isReplicaWorking`). Ninguno lee `rowIsWorking`.

## In-scope

- La expresión `rowIsWorking` en `ProjectPanel.tsx:2464-2467`.
- Los comentarios #1783 / #2131 en `ProjectPanel.tsx:2442-2463`.
- Tests en `src/sidebar/components/ProjectPanel.working-tint.test.tsx`.

## Out-of-scope

- `workgroupIsWorking`, `isReplicaWorking`, `splitWorkgroupsByWorking`
  (`workgroup-session.ts`) — sin cambios.
- `WorkgroupGroupRail.tsx`, la clase `.ac-wg-subgroup` y el orden de rooms — sin cambios.
- `orchestratorCiRunning` y la clase `ci-running` del chip — sin cambios.
- CSS y cualquier otro `rowContext`.

## Solución decidida

En `src/sidebar/components/ProjectPanel.tsx`, agregar el término de CI como OR en la
rama `quick`:

```ts
const rowIsWorking = () =>
  rowContext === "quick"
    ? workgroupIsWorking(wg) || orchestratorCiRunning()
    : isReplicaWorking(wg, replica) || orchestratorCiRunning();
```

Es la única edición de lógica. `orchestratorCiRunning` ya exige `isCoord()` y lee el
mismo `repoBadges()` / `remoteActivityStore` que el chip, así que fila y anillo naranja
no pueden discrepar. El límite que #2142 fijó ("no debe alcanzar `workgroupIsWorking`")
se respeta: esa función queda intacta y los tres efectos que protege se alimentan solo
de ella.

### Comentarios a reescribir (2442-2463)

El bloque debe quedar diciendo el contrato nuevo, sin dejar texto que afirme el límite
viejo:

- Bloque #2131 (2442-2449): mantener la justificación del gate `isCoord()` y de leer la
  misma entrada publicada que el chip. Reemplazar la frase "It must NOT reach
  workgroupIsWorking: room ordering, the group-rail dot and the quick-access row stay
  session-only" por: no debe alcanzar `workgroupIsWorking` — orden de rooms y dot del
  group-rail siguen siendo session-only; la fila de quick-access ya NO (ver #2151).
- Bloque #1783 (2455-2463): el panel de quick-access ahora responde "¿trabaja alguien de
  esta room, o su repo tiene CI en curso?". Los demás `rowContext` (`"workgroups"`,
  `"selected"`) conservan el significado per-row: sesión propia, más CI. Borrar el
  párrafo #2131 que dice que el término de CI se agrega SOLO en la rama no-`quick` y que
  la discrepancia entre árbol y tira es el límite aceptado; reemplazarlo por la
  referencia a #2151 que cierra esa discrepancia.

## Comportamiento requerido y casos borde

1. CI `running` en el repo del orquestador + todas las sesiones idle → la fila `quick`
   del orquestador y su fila del árbol llevan `working` en el mismo render.
2. Ese mismo estado NO cambia clasificación de room, wash de `.ac-wg-subgroup`, dot ni
   contador del rail, ni el orden de rooms.
3. `isCoord()` falso (fila no-orquestador) → sin tinte por CI, aunque comparta
   `repoPath`. La tira solo renderiza orquestadores, así que el gate sigue siendo el que
   protege el árbol.
4. CI `idle`, `unknown` o ausente → sin término de CI; la rama `quick` cae de nuevo en
   `workgroupIsWorking(wg)` solo.
5. Otra room con otro `repoPath` → sin tinte; el predicado es por repo del propio
   orquestador, no "hay CI en algún lado".
6. Si alguien de la room trabaja, la fila `quick` sigue tiñéndose aunque no haya CI
   (#1783 intacto): el OR solo agrega casos, no quita ninguno.

## Tests (`src/sidebar/components/ProjectPanel.working-tint.test.tsx`)

- **Test 15** (línea 715, "does not tint a non-orchestrator row, the quick strip, or
  another room, from the same CI state"): partirlo. Conservar las aserciones de control
  positivo (fila `workgroups` del ORCHESTRATOR teñida), fila WORKER sin teñir, fila de
  `IDLE_ORCHESTRATOR` en `IDLE_ROOM` sin teñir, y la aserción de que el WORKER no tiene
  chip. Quitar el bloque de la tira (`row(root, "quick", ORCHESTRATOR)` sin `working`) y
  el comentario D-B6 que lo justifica. Renombrar el test a que ya no prometa la tira,
  p. ej. "15. does not tint a non-orchestrator row or another room from the same CI state".
- **Test 16** (línea 766): sin cambios. Es la red que prueba clasificación, rail y orden.
- **Nuevo test 17** — "the CI tint reaches the Orchestrators strip and the room tree in
  the same render": `mountCiPanel()`, sesiones ORCHESTRATOR y WORKER en `idle`,
  `publishCi("running")`; esperar `ci-running` en el chip; luego afirmar en el mismo
  render que `row(root, "quick", ORCHESTRATOR)` y `row(root, "workgroups", ORCHESTRATOR)`
  llevan ambas `working`. Gate previo con `waitFor` sobre
  `[data-ac-testid="${rowTestId("quick", ORCHESTRATOR)}"]` para que la fila exista antes
  de asertar.
- **Nuevo test 18** — "the strip tint alone changes no room classification, no rail dot
  and no order": `mountCiPanel({ withRail: true })`, sesiones idle, capturar
  `subgroupRowOrder(root)` antes; `publishCi("running")`; esperar `working` en la fila
  `quick` como control positivo in-run; luego afirmar
  `splitWorkgroupsByWorking(projectStore.projects[0].workgroups).working === []`,
  `anySubgroupWorking(root) === false`, `railDots(root)` vacío, `railButton(root,"all")`
  con `0/2`, y `subgroupRowOrder(root)` igual al de antes.
- **Tests 7, 8, 9, 13, 14** no cambian.

## Criterios de aceptación

- `rowIsWorking` es la única expresión de lógica modificada en `ProjectPanel.tsx`.
- `git diff` no toca `workgroup-session.ts`, `WorkgroupGroupRail.tsx` ni CSS.
- Los comentarios 2442-2463 no contienen ninguna afirmación de que la tira queda fuera
  del tinte de CI.
- Suite `ProjectPanel.working-tint.test.tsx` verde, con tests 17 y 18 nuevos y el 16
  intacto.
- Typecheck y lint del frontend en verde.
