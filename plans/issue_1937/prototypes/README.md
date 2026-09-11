# Prototipos visuales navegables — candados de selección (Coding Agent + Profile)

Maquetas de **cómo cambian las pantallas** al poner y quitar el candado a una réplica o a un tipo
de Matrix. Es producción de prototipo: **no implementa funcionalidad, no toca producto, no hay
issue/branch/PR ni build de la aplicación**.

La semántica no se inventa aquí: sale de `room-shared/candados-propuesta.md` (investigación del
arquitecto, `NOT READY_FOR_IMPLEMENTATION`). Si este prototipo y ese informe difieren, vale el informe.
El alcance de futuras réplicas está ratificado por el usuario en `room-shared/candados-decision-usuario.md`
(«Sí, con excepciones por réplica»): las réplicas que se creen en futuras rooms heredan el candado del
tipo, con excepción por réplica.

## Entregables

| Artefacto | Ruta | Qué es |
|---|---|---|
| Prototipo autocontenido | `room-shared/prototipos-candados/index.html` | Un solo archivo (292.3 KB): CSS real de la app inline + demo. Se abre con doble clic, sin servidor ni build |
| Vistas PNG | `room-shared/prototipos-candados/vistas/` | 6 capturas legibles, 2607×1473 (escala 1.5), una por pantalla principal |
| Reconstruir el HTML | `room-shared/prototipos-candados/construir.mjs` | `node construir.mjs` |
| Recapturar PNG + auditoría | `room-shared/prototipos-candados/capturar.mjs` | `node capturar.mjs` (Chrome headless por CDP; `--solo-auditar` mide geometría sin guardar; `--vista=<id>[,<id>]` limita la captura a las vistas indicadas) |
| QA de PNG sin visión | `room-shared/prototipos-candados/herramientas/inspeccionar-png.mjs` | Decodifica los PNG y reporta color medio y acentos (ámbar/verde/cian), para detectar pantallas vacías |
| Prueba de interacción | `room-shared/prototipos-candados/herramientas/probar-interacciones.mjs` | `node herramientas/probar-interacciones.mjs`: 28 comprobaciones reales sobre el HTML (menú, candado, tipo, apply, resultado) |

## Cómo recorrerlo

1. Doble clic en `index.html` (funciona con `file://`; no hay recursos remotos).
2. Arriba hay 6 vistas. **Anterior/Siguiente** o los botones numerados cambian de pantalla.
3. **Anotaciones: sí/no** muestra u oculta los recuadros ámbar; los números de la maqueta
   corresponden a la lista «¿Qué mirar?» de la derecha (en español).
4. Elementos que funcionan de verdad en el mock:

   - **Clic derecho sobre una réplica** del sidebar → menú contextual real → **Coding Agent** abre el modal.
   - **Interruptor del candado** (`Keep across bulk changes`) activa/desactiva la protección de la réplica.
   - **Set lock for → All replicas of this kind** cambia el control por un **botón de acción explícito**
     cuyo rótulo coincide con el efecto (`Lock 1 remaining replica` → `Remove lock from 2 replicas`).
   - **Remove lock** quita el candado de una réplica y **conserva la selección** (aparece un aviso).
   - Pasos 1 y 2, **Apply to** (réplica / tipo / room), **Restart sessions after apply**, el
     armado del alcance y **Apply** recalculan el preview y el resultado.
   - `Esc` cierra el menú o el modal. **Reiniciar** vuelve a los datos de demo.

Los datos (rooms, réplicas, pares) son **ficticios**: dentro de la ventana hay una etiqueta
`datos ficticios · demo`.

## Qué demuestra cada vista

| Vista | PNG | Qué se ve |
|---|---|---|
| 1 · Entrada | `vistas/entrada.png` | El candado visible junto al par en la fila protegida del sidebar y el camino real: clic derecho → **Coding Agent** |
| 2 · Candado abierto | `vistas/replica-abierto.png` | Réplica concreta sin protección: la barra del candado dice `Unlocked`; el cambio masivo podría escribir el par |
| 3 · Candado cerrado | `vistas/replica-cerrado.png` | Réplica concreta protegida (`Protected`), par materializado, cambio individual permitido y botón **Remove lock** |
| 4 · Por tipo: preview | `vistas/tipo-preview.png` | Estado mixto coherente: `1 of 2 protected`, cada par listado, botón `Lock 1 remaining replica`, y preview masivo con **Protected · skipped** y conteos |
| 5 · Resultado | `vistas/resultado.png` | `1 updated · 1 protected · 0 errors`; la protegida no se escribió ni se reinició |
| 6 · Futuras réplicas | `vistas/futuro.png` | Default para réplicas nuevas: heredan el candado del tipo al crearse, con excepción por réplica y sin propagación retroactiva |

## Origen visual

Repo de referencia: `repo-AgentsCommander` en **main, `f16edd976f9861648d04d137b3bb960189d4548e`** (árbol limpio).
Estilos copiados **tal cual** dentro de `index.html` (solo se eliminó el `@import`):

- `src/sidebar/styles/variables.css` — tokens (tema Noir por defecto).
- `src/sidebar/styles/sidebar.css` — sidebar, rows y **todo el AgentPickerModal**.
- `src/shared/styles/toast.css` — avisos.

Clases reales reutilizadas en el mock: `.agent-modal.agent-picker-modal`, `.agent-profile-assignment-body`,
`.agent-profile-provider-card`, `.agent-profile-card` (+ pills `MATCH/CONFIGURED/FALLBACK/MISSING`,
`Declared env`), `.agent-comparison-*`, `.agent-scope-picker`, `.agent-scope-opt`, `.agent-scope-targets`,
`.agent-picker-bar`, `.agent-scope-switch`, `.agent-scope-arm`, `.modal-btn`, `.session-context-menu`,
`.replica-item`, `.ac-wg-header`, `.ac-discovery-badge`, `.agent-name-chip`, `.profile-badge`.

**Única UI nueva propuesta**: `.selection-lock-*` (chip de fila + barra del candado + preview por tipo).
No se rediseña otro producto: mismas clases, tokens, tipografías y textos en inglés.

### Idioma

El producto real está en inglés, así que el texto **dentro** de las pantallas va en inglés
(ej. `Keep across bulk changes`). La guía, las anotaciones, el marco y este README van en español.
La frase de la propuesta «Conservar ante cambios masivos» se muestra como **`Keep across bulk changes`**.

## Semántica cubierta

| Operación (según la propuesta) | Dónde se ve en el prototipo |
|---|---|
| Candado protege Coding Agent + Profile **conjuntamente** | Barra del candado, junto al par; chip en la fila del sidebar |
| Activar en réplica | Vista 2 → interruptor → estado `Protected` |
| Activar **por tipo** (réplicas de la misma Matrix), conservando el par propio de cada una | Vista 4 → `Set lock for: All replicas of this kind` + botón `Lock 1 remaining replica`; píldoras `Protected`/`Not protected` y cada par listado |
| Asignación masiva omite protegidas (escritura y reinicio) | Vista 4: fila `Protected · skipped`, resumen `1 protected · skipped, not restarted`, botón con solo elegibles |
| Desactivar **conserva la selección** | Vista 3: `Remove lock` + aviso «Lock removed — Coding Agent + Profile kept» |
| Cambio individual deliberado permitido, conserva el candado | Vista 3 (texto del hint) y paso 1/2 editable con el candado activo |
| Resultado `N actualizadas, M protegidas, K errores` | Vista 5 (franja en el modal + toast) |
| Nuevas réplicas heredan el default de la Matrix, con excepción por réplica | Vista 6 (`Default for new replicas of this Matrix`) y nota del bloque por tipo (vistas 4-5) |
| Candado **no** congela el contenido del profile | Fuera de alcance de la maqueta; sólo se protege el par, no el `Command`/`env` de la celda |
| Default para futuras réplicas: **alcance confirmado por el usuario**, heredado al crear la réplica, con excepción por réplica y sin propagación retroactiva | Vista 6, chip `confirmed · user decision`; requisito en `candados-decision-usuario.md` |

## Decisiones de composición visual

- El candado vive en una **barra propia entre el encabezado y los tres paneles** del modal real: el
  par (`Coding Agent · Profile`) queda a la izquierda y el control a la derecha. Los pasos 1 y 2 no cambian.
- Hay **dos alcances distintos y separados** a propósito, porque son operaciones distintas:
  `Apply to` (a qué destinos se escribe el par) y `Set lock for` (a qué réplicas alcanza el candado).
  El bloque por tipo lo repite en texto: «Apply to changes the pairing; this section only sets locks».
- **Un solo control por alcance, nunca contradictorio.** En `This replica` el control es el interruptor
  (`Keep across bulk changes`) y, si está protegida, el botón `Remove lock`. En `All replicas of this kind`
  el interruptor desaparece y queda **un solo botón de acción** que dice exactamente lo que va a pasar:
  `Lock 2 replicas`, `Lock 1 remaining replica` o `Remove lock from 2 replicas`. Las píldoras de la lista
  describen **estado actual** (`Protected` / `Not protected`), no una acción pendiente; el chip de la barra
  agrega el estado del alcance (`1 of 2 protected`).
- Estado con color ámbar (protección, no error), verde para «se actualizará/actualizado» y rojo
  (ya existente en la app) para los alcances peligrosos.
- El resumen de un lote nunca mezcla protegidas con errores: son columnas distintas.
- La opción de futuras réplicas queda **fuera del flujo principal** y ya no se presenta como pendiente:
  el chip dice `confirmed · user decision` porque el usuario ratificó la herencia con excepciones por réplica.

### Estados deshabilitados y cuándo se habilitan

| Control | Estado inicial | Se habilita / aparece cuando |
|---|---|---|
| `Apply` en alcance réplica | Habilitado | Siempre (salvo selección redundante o carga en curso, lógica real del modal) |
| `Apply` en alcance tipo/room | **Deshabilitado** | Al marcar la confirmación `I understand this overwrites 1 replica of this kind (1 protected is skipped)` |
| Botón de tipo (`Lock…` / `Remove lock from…`) | Habilitado | Siempre: su rótulo ya describe la acción del estado mixto actual |
| `Remove lock` (una réplica) | No existe | **Aparece** cuando el candado de la réplica está activo; nunca está deshabilitado |
| `Restart sessions after apply` | Marcado | Solo visible en alcances masivos (`Apply to` tipo/room) |
| `Keep across bulk changes` | Según la réplica | Solo en alcance `This replica`; no convive con el botón de tipo |

## Verificación hecha

- **Prueba de interacción** (`node herramientas/probar-interacciones.mjs`): **28/28 OK**. Cubre menú
  contextual → modal, interruptor de réplica, `Remove lock` conservando el par, acción por tipo en estado
  mixto (rótulo ↔ efecto ↔ pares conservados), preview con protegida omitida y resultado
  `1 updated · 1 protected · 0 errors` sin reinicio de la protegida.
- **Auditoría de geometría** con Chrome headless por CDP (`node capturar.mjs --solo-auditar`):
  en las 6 vistas la ventana y el modal caben, la botonera queda visible, hay 3 agentes / 3
  profiles / 3 filas de comparación, 1 fila elegible + 1 protegida en vistas 4-5, y **0 desbordes**.
- **Cambio de etiquetas (2026-09-11, alcance de futuras réplicas)**: sin cambio de lógica. Se
  regeneró `index.html` y se recapturaron las 6 vistas (`node capturar.mjs`): **0 problemas** de
  geometría o desborde en las seis. El bloque renderizado dice
  `Default for new replicas of this Matrix | confirmed · user decision | Start locked | …`, y ya no
  aparece `proposal`, `pending decision` ni `New rooms start unlocked` en `piezas/`, `index.html`
  ni este README. No se repitió la batería de interacción: no cambió ninguna lógica.
- **Inspección de PNG** (`node herramientas/inspeccionar-png.mjs`): 2607×1473, contenido con
  acentos cian de la app y ámbar del candado en todas las vistas; verde de resultado en 4 y 5, y del
  chip `confirmed` en 6 (0.014 % de verde).
- Sin captura de la app real, sin input físico, sin ventanas visibles: solo el HTML de demostración
  en Chrome headless con perfil temporal.

## Limitaciones

- Datos ficticios: no hay backend; las escrituras, reinicios y conteos se simulan en el cliente.
- La ventana simulada (1280 px) es más ancha que un sidebar típico, porque el modal real mide hasta 1180 px.
- Tipografías: la app usa Geist/Outfit; el prototipo cae al fallback declarado (`Segoe UI`).
- No se modelan: concurrencia, espera del lote, timeouts, resultados parciales, fingerprint ni
  persistencia real. Son decisiones de implementación, no de maqueta.
- La herencia a futuras réplicas está **confirmada como alcance de producto** (`candados-decision-usuario.md`);
  este prototipo la muestra, pero **no implementa** la funcionalidad ni aprueba un plan técnico.
