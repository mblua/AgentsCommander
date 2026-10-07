# #2788 — New Room black-box testing, R2 opción A

Plan aprobado: .ac/plans/2788-new-room-team-filter-optional-task.md, SHA-256 371702415B18A1D427F3F489AF4591ACDD399C18AFB6EF8444B108478B82B57E. Requiere rebuild/evidencia nuevos; PASS anterior de B no acredita A.

## Decisiones visibles aprobadas

- Respuesta literal del usuario: **«D1-b y D2-b»**, comunicada por el coordinador el 2026-10-01.
- Fuente: D:/0_repos/AgentsCommander_iac/.ac/project-shared/i2788-visual-comparison/prototype-d1-d2-v1.html, SHA256 verificado 92708798c12d4527b2ed74cb7179260547f908859550bd9c37be89bae2056af9. D1-b define la confirmación inglesa; D2-b es mount('D2-b','es',true,true). El 'es' de esa comparación D2 solo aislaba el comportamiento Enter: el producto combina D1-b inglés con D2-b.
- Referencias de línea del prototipo D1/D2: 300 update/copy inglés, 306 focus/input/blur, 310 reopen y suppressFocus alrededor de search.focus(), 313 variantes mount. Esas reglas reemplazan únicamente copy y apertura inicial del HTML A original.
- D1-b: textos exactos «Selected team: <team>» y «No team selected.»; mantener la línea visible.
- D2-b: único team preconfirmado, lista cerrada durante foco inicial de montaje y primer Enter crea. La bandera suppressFocus existe solo alrededor de ese foco; focos posteriores, flechas, edición y Escape siguen A. Con varios teams el montaje abre lista y Enter abierto no crea.

No quedan decisiones visibles abiertas. Revisión e implementación siguen el circuito del coordinador/room-07.

## Testids y acciones semánticas cerradas

P=automationIdPart(proj.path), W=automationIdPart(wg.name), C=automationIdPart(rowContext), existentes. Mantener newRoom.teamSearch para el mismo input ahora combobox; evita renombrado gratuito. Retirar newRoom.team: ya no existe select nativo. Añadir targets a superficies visibles de A, sin nodos ocultos instrumentales.

| Superficie | data-ac-testid | Contrato/acción |
|---|---|---|
| Project header | project.header.${P} | Existente; query/contextClick |
| Acción New Room | project.action.newRoom.${P}.projectMenu | Existente; click |
| Modal | newRoom.modal | Existente role dialog; query |
| Combobox | newRoom.teamSearch | role combobox; detail="Search teams..."; state confirmed/unconfirmed según selectedTeam; expanded vía aria-expanded; query/click/setValue/typeText/key (ArrowDown). Set filtra, nunca selecciona. |
| Lista | newRoom.team.list | role listbox; detail JSON.stringify({options:filteredTeams().map(t=>t.name),active:activeIndex()}); query cuando abierta. |
| Fila | newRoom.team.option.${index} | Índice cero en orden filtrado; role=option + data-ac-role=text para snapshotText; detail nombre exacto; state active/inactive; aria-selected del activo; query/click. No setValue. |
| Confirmación | newRoom.team.confirmed | data-ac-role=text; state confirmed/unconfirmed; detail JSON.stringify({selected:selectedTeam()}); text literal definido arriba; query. |
| Status vacío | newRoom.team.empty | role status; query condicional |
| Título input | newRoom.taskTitle | detail="Task title (optional)"; setValue/query; input no expone valor por query |
| Ayuda | newRoom.taskTitle.hint | data-ac-role=text, texto exacto; query |
| Create | newRoom.create | query disabled + click |
| Cancel | newRoom.cancel | click/query |
| Título sala | workgroup.taskTitle.${P}.${C}.${W} | Existente role text/state clean o task; query |

Justificación de delta testids: mantener todos salvo newRoom.team porque desaparece el select; list/option permiten leer resultados y confirmar por click; confirmed refleja el texto nuevo aprobado y selección mientras lista cerrada. Cambiar detail del input por placeholder aprobado; estado/expanded hacen lectura de selección/apertura inequívoca. Docs/receta cambian las mismas acciones/targets para no prometer set de selección sobre input.

automation-bridge.ts: resolveSingleTarget exige único visible; query no hace hit-test. runFocusedAction hace focus y click DOM; setElementValue admite input/textarea/select y despacha input/change. Set en div retorna value_not_supported: no usarlo para confirmar. snapshotTarget expone expanded/selected vía ARIA; snapshotText permite data-ac-role=text, no role option/listbox solo. metadata.detail saneada/truncada120; no ampliar SAFE_METADATA_ATTRIBUTES/límites. Fixture de3 nombres tiene options/active JSON <120; test exige parse/límite en cada estado. Confirmación detail separado evita juntar lista+selección+abierto en proyección larga. Listas grandes truncadas: evidencia insuficiente, no enumeración completa. Filas se consultan explícitamente por índice/texto; no confiar en available truncada. No proyectar rutas/agentes/comandos/tokens/borrador.

Índice cambia al filtrar/refrescar: consultar lista y texto de fila inmediatamente antes de click. Sin identidad persistente por índice. dev-alpha con consulta vacía/DEV es option.0. Lista cerrada/fila bajo hidden: target_hidden esperado; abrir con key ArrowDown (activo0 si hay resultados; no confirma). Click input depende de focus y no garantiza apertura sin foreground. Lista abierta sin resultados: options:[],active:-1 y ninguna fila; missing_selector esperado de option.0. No afirmar que ui-click simula mousedown/blur OS: test separado despacha mousedown real en jsdom.

## Receta black-box canónica

Ejecutar después de implementación aprobada, rebuild oficial y autorización del tester. Receipt nuevo: commit final/SHA256/ruta/bytes/versión. No usar exe de B ni app del usuario. Antes de cualquier BIN incluso reset/help exigir CONFIG_DIR ausente (variante casing Windows también en Node); definida vacía bloquea, no unset/redirect. Antes del reset comprobar cero procesos GUI testeables activos/registrar observación. Validar path real/nombre/SHA contra receipt oficial. Reset exit0/receipt exitoso y luego Node sin NINGUNA CLI entre ambos. Crear fixture exclusivo, no sobrescribir. Guardar stdout/stderr/exit preflight/reset/Node, JSON fixture y receipt fuera del perfil reseteable.

```bash
set -euo pipefail
if [[ ${AGENTSCOMMANDER_CONFIG_DIR+x} ]]; then
  printf '%s\n' 'ABORT: AGENTSCOMMANDER_CONFIG_DIR is defined; do not unset or redirect it.' >&2
  exit 1
fi
cd /d/0_repos/AgentsCommander_iac/.ac/room-02-ac-dev-team-v4/repo-AgentsCommander
BIN="$PWD/target/release/agentscommander_testeable.exe"
RECEIPT_SHA256='REEMPLAZAR_CON_SHA256_DEL_RECEIPT_OFICIAL'
node --input-type=module - "$BIN" "$RECEIPT_SHA256" <<'PREFLIGHT'
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
if (Object.keys(process.env).some(key => key.toUpperCase() === 'AGENTSCOMMANDER_CONFIG_DIR'))
  throw Error('ABORT: config override is defined; do not unset or redirect it');
const expected = path.resolve('D:/0_repos/AgentsCommander_iac/.ac/room-02-ac-dev-team-v4/repo-AgentsCommander/target/release/agentscommander_testeable.exe');
const bin = fs.realpathSync(process.argv[2]);
if (bin.toLowerCase() !== expected.toLowerCase() || path.basename(bin).toLowerCase() !== 'agentscommander_testeable.exe')
  throw Error('ABORT: binary path/identity does not match the official receipt target');
const receiptSha = process.argv[3];
if (!/^[0-9a-f]{64}$/i.test(receiptSha)) throw Error('ABORT: official receipt SHA256 required');
const actualSha = createHash('sha256').update(fs.readFileSync(bin)).digest('hex');
if (actualSha !== receiptSha.toLowerCase()) throw Error('ABORT: binary digest differs from official receipt');
console.log(JSON.stringify({ binary: bin, sha256: actualSha, receiptVerified: true }));
PREFLIGHT
"$BIN" test-reset --confirm-testeable
# Continuar únicamente si exit code 0 y receipt final del reset es exitoso.
# Desde reset hasta Node: NINGUNA invocación CLI de BIN, ni siquiera --help.
node --input-type=module - "$BIN" <<'JS'
import fs from 'node:fs';
import path from 'node:path';
const bin = fs.realpathSync(process.argv[2]);
if (path.basename(bin).toLowerCase() !== 'agentscommander_testeable.exe') throw Error('wrong identity');
const root = path.join(path.dirname(bin), '.agentscommander_testeable');
if (fs.existsSync(root)) throw Error('reset did not leave a fresh config root');
fs.mkdirSync(root); // exclusive fresh child of the verified binary directory
const project = path.join(root, 'fixtures', 'i2788', 'project');
fs.mkdirSync(path.join(project, '.ac'), { recursive: true });
for (const name of ['dev-alpha', 'dev-beta', 'ops']) {
  const team = path.join(project, '.ac', '_team_' + name);
  fs.mkdirSync(team);
  fs.writeFileSync(path.join(team, 'config.json'),
    JSON.stringify({ agents: [], coordinator: '', repos: [], contextAlertPercentages: [] }) + '\n',
    { flag: 'wx' });
}
fs.writeFileSync(path.join(root, 'settings.30.instance.no-git.json'), JSON.stringify({
  defaultShell: 'cmd.exe', defaultShellArgs: [], agents: [],
  projectPath: project, projectPaths: [project], archivedProjectPaths: [],
  onboardingDismissed: true, roomNumberMask: '#', soundsEnabled: false,
  restoreCoordinatorWakeState: false, restartResumeWakeWorkingAgents: false,
  telegramBots: [], voiceToTextEnabled: false, coManagedEnabled: false,
  webServerEnabled: false, apiServerEnabled: false,
  npmUpdateNotificationsEnabled: false, remoteBlockingMenusEnabled: false,
  remoteAgentHelpEnabled: false
}, null, 2) + '\n', { flag: 'wx' });
console.log(JSON.stringify({ binary: bin, configRoot: root, project,
  projectTestIdPart: project.replace(/[^a-zA-Z0-9._-]+/g, '-').replace(/^-+|-+$/g, '') || 'unknown',
  teams: ['dev-alpha', 'dev-beta', 'ops'] }));
JS
```
Teams vacíos de agentes/repos evitan PTY/clones; no abrir sesiones. PROJECT solo procede del JSON fixture. Si root existe tras reset o preflight falla: parar/reportar, sin bootstrap improvisado ni perfil usuario. remoteAgentHelpEnabled:false se conserva. Al terminar cerrar solo GUI/PID testeable con ExecutablePath verificado, comprobar ausencia y test-reset; sin borrado recursivo manual. Evidencia fuera del root reseteable: room-shared/i2788-evidence/manual-r2-option-a/ (nueva carpeta; no sobrescribir corridas B).

### Lanzamiento, identidad y registro

Desde Git Bash, lanzamiento exacto `"$BIN" --app --ui-automation` con AC_TEST_WINDOW_PLACEMENT JSON en el entorno; no subcomando launch ni --window-maximized. Normal: placement {"x":450,"y":250,"width":1401,"height":902}. Mínima real autorizada: {"x":450,"y":250,"width":1200,"height":900}, DPR1.5. Relanzar solo instancia testeable para gate mínima, cerrar/verificar PID anterior. Ventana no debe salirse de pantalla autorizada. Nunca pedir tamaño menor que800x500 lógico (lib.rs:5240); placement opera físico y puede eludir mínimo. A DPR distinto no reutilizar 1200 como supuesto lógico: registrar bloqueo de geometría y escalar, no inventar dimensiones.

**Paso de readiness (obligatorio, entre el launch y la primera consulta `ui-*`).**
1. Confirmar que el PID lanzado sigue vivo y que su CommandLine contiene `--ui-automation`; ambas condiciones deben seguir válidas durante el sondeo.
2. Con un presupuesto total de 30 s medido con reloj monotónico desde el launch (incluye el tiempo de los comandos), repetir: `"$BIN" ui-wait --window main --selector main.root --timeout-ms <min(10000, presupuesto restante)>`. No iniciar un intento ni una espera que exceda el presupuesto.
   - exit 0 y `"ok":true` → puente listo; continuar.
   - exit 1 con `"error":"automation_not_enabled"` o `"error":"automation_session_missing"` → todavía arrancando; esperar 250 ms y repetir. Guardar cada intento (comando, salida, exit, duración) como evidencia.
   - cualquier otro resultado → TEST_BLOCKED.
3. Presupuesto agotado sin exit 0 → TEST_BLOCKED.
No relanzar la GUI ni cambiar variables de entorno durante el sondeo.
TEST_BLOCKED real: `automation_not_enabled` tras agotar el presupuesto o con un proceso sin `--ui-automation`; `timeout` (wait_condition_not_met); `automation_session_stale`, `automation_filesystem_error`, `refusing_non_testeable_binary`, `window_unavailable`; PID muerto.
Motivo: `daemon.pid` se escribe antes que `session.json`; en ese hueco el CLI devuelve `automation_not_enabled` y `ui-wait` sale sin esperar (comportamiento previo a #2788).

window-info tras arranque, poll bounded10s, acredita processPath/currentExe exacto/PID/HWND/DPR/viewport/rect real. La corrida B normal observó1383x893 físicos/920x593CSS; mínima1182x891/786x592CSS por marco. Son antecedentes, no expectativas a forzar: registrar medidas nuevas. Placement1200x900 corresponde a petición de ancho800 lógico, aunque viewport efectivo puede ser786; declarar esta diferencia, no llamar viewport800. No probar600x900 ni extrapolar por debajo del mínimo. No mover app usuario/monitor ni usar OS-input/foco/restauración como fallback. Lanzamiento puede tomar foreground en pantalla única; observar foreground/cursor sin atribuir causalidad a snapshots.

P impreso por Node es predicción. Después de Node y GUI: `"$BIN" ui-query --window main --selector "project.header.${P}"`. Exigir exit0, ok:true, UN solo target y testId exactamente igual; guardar respuesta confirmada junto al JSON del fixture. Solo entonces usar P en acciones/títulos. No enumeración `i2788.__enumerate__`: available limit8 ya causó bloqueo. Missing/duplicate/ambiguous esperado presente→TEST_BLOCKED; no inventar sufijo ni controlar otra ventana.

Cada invocación recibe prefijo único fase-acción-secuencia: setup-001-preflight, setup-002-reset, setup-003-node, cases-001-header-query…, visual-001…, narrow-001…, cleanup-001…; no reiniciar un contador dentro de una fase ni reutilizar nombre. Guardar por acción stdout/stderr/exit, comando/requestId, timestamp, target, asserts, PID/HWND/ruta; ledger commands.jsonl enlaza nombres únicos. Registrar también primera apertura, no empezar con modal ya abierto sin recibo. Ficheros exclusivos, manifest.sha256 final sobre evidencia; prefijos de capturas y copias TASK únicos. No reportar trazabilidad completa si se pisan recibos.

Comandos semánticos: BIN ui-query/ui-click/ui-set/ui-context-click/ui-key con --window main --selector; solo ui-set usa --value; ui-key recibe el CHORD posicional. Abrir modal: context-click project.header.${P}, click project.action.newRoom.${P}.projectMenu, query newRoom.modal. Antes de cualquier key registrar query newRoom.teamSearch/expanded y observación foreground/HWND propio.

F1 — Para el fixture de varios teams, si expanded:false después de abrir y la ventana testeable no tiene foreground, el intento «focus abre» es TEST_BLOCKED con receipt de query y foreground, NO FAIL del producto. Chromium puede no despachar focus/blur al element.focus() del bridge sin foco de ventana. No restaurar foco OS ni usar título-click→input-click como garantía. Sin evidencia de foreground, gate focus TEST_BLOCKED por evidencia insuficiente. Expanded:false con foreground propio se reporta para investigación, sin atribuir causalidad automática. Excepción D2-b: si hay único team al montar, expanded:false es el resultado correcto, no TEST_BLOCKED ni FAIL. El fixture canónico conserva tres equipos: no cambiarlo para probar D2-b ni añadir sexto caso. D2-b montaje/primer Enter queda probado en tests automatizados y pendiente nativo según matriz.

Camino canónico sin foco para los cinco casos: tras registrar ese estado, ejecutar `"$BIN" ui-key --window main --selector newRoom.teamSearch ArrowDown`, query input expanded:true y lista/opciones visibles. runKeyAction despacha keydown/keyup al input aun si focus no emitió eventos; no requiere OS-input. Desde activo-1 ArrowDown abre y activa0 si hay resultados, o conserva-1 sin filas; no confirma ni borra equipo/consulta. Registrar ese efecto. Repetir para reabrir tras confirmar (activo reseteado a-1). Confirmar con query fila/texto + ui-click. ui-set abre por input pero invalida confirmación: usar para filtrar, no para reabrir conservando equipo. Si key/query rechazado, TEST_BLOCKED sin fallback OS. Alternativa key permite continuar casos pero no acredita focus-abre: reportar gate aparte.

### Cinco casos adaptados a A

1. Abrir desde header confirmado. Query modal/teamSearch/team.list/team.confirmed/taskTitle/taskTitle.hint/create/cancel y las filas0,1,2. Input role combobox/detail Search teams.../expanded true/state unconfirmed. Tras paso canónico ArrowDown: lista detail {options:["dev-alpha","dev-beta","ops"],active:0}; fila0 selected:true/state active, filas1/2 selected:false/state inactive; textos/detail en orden. Antes de ArrowDown registrar activo-1 si focus abrió; expanded:false sin foreground sigue F1 para este fixture de varios equipos. Confirmado text «No team selected.»/detail {selected:""}/state unconfirmed; Create disabled:true. Ayuda/título placeholder exactos. Captura del orden Team/input/lista/confirmación inglesa/Task Title/ayudas/acciones. Cancel cierra; reabrir con paso canónico ArrowDown verifica mismos valores, activo0 y sin confirmación. No inferir texto del input por query (detail prueba contrato, imagen píxeles).
2. ui-set newRoom.teamSearch valor "  DEV ": expanded true, lista options dev-alpha/dev-beta/active-1, confirmado vacío/text «No team selected.»/Create disabled. Query option.0 text dev-alpha y ui-click esa fila: input expanded false/state confirmed, confirmado selected dev-alpha/text «Selected team: dev-alpha», Create enabled; lista y fila0 aún renderizadas bajo hidden: target_hidden esperado (las otras filas dependen del nuevo filtro dev-alpha). Reabrir con `"$BIN" ui-key --window main --selector newRoom.teamSearch ArrowDown`: expanded true, consulta exacta dev-alpha filtra solo ese nombre, active0/selected:true de fila; confirmado dev-alpha/text inglés se conserva. ui-set input "dev": borra confirmación aun si coincide, muestra alpha/beta/disabled y «No team selected.». ui-set "missing": lista options[]/active-1, status exacto «No teams match your search.», sin filas/Create disabled. ui-set "": recupera tres filas/orden sin confirmar. Captura lista abierta y confirmación cerrada en inglés; parse JSON entero<=120.
3. Reabrir modal limpio, registrar query newRoom.teamSearch/expanded y foreground/HWND propio; ejecutar `"$BIN" ui-key --window main --selector newRoom.teamSearch ArrowDown`, query expanded:true/lista visible/active:0; query fila0/text dev-alpha; query opción0/dev-alpha y ui-click, sin tocar título. Confirmado dev-alpha/expanded false/Create enabled. Inventario previo y ui-click Create: cierre y exactamente una sala nueva dev-alpha. Comparar TASK bytes Clean abajo; query workgroup.taskTitle.${P}.workgroups.${W} text Clean/state clean. No asumir room number.
4. Reabrir, registrar query newRoom.teamSearch/expanded y foreground/HWND propio; ejecutar `"$BIN" ui-key --window main --selector newRoom.teamSearch ArrowDown`, query expanded:true/lista visible/active:0; query fila0/text dev-alpha; click fila dev-alpha, ui-set título "   ", click Create con inventario previo. Una nueva sala y mismos bytes Clean/state clean.
5. Reabrir, registrar query newRoom.teamSearch/expanded y foreground/HWND propio; ejecutar `"$BIN" ui-key --window main --selector newRoom.teamSearch ArrowDown`, query expanded:true/lista visible/active:0; query fila0/text dev-alpha; click fila dev-alpha, ui-set título "  Fixture title  ", click Create. Una nueva sala, bytes USER exactos y título UI text «USER: Fixture title»/state task.

Antes/después de cada Create inventariar solo directorios room-* bajo PROJECT/.ac; exigir exactamente uno añadido del equipo seleccionado. Poll bounded10s para TASK/discovery UI; vencimiento es bloqueo concreto. Resolver realpath de sala nueva y comprobar bajo PROJECT/.ac antes de leer. Node fs.readFileSync como Buffer; Buffer.equals(Buffer.from("---\ntitle: 'Clean'\n---\n","utf8")) en3/4 (23bytes); en5 Buffer.from("---\ntitle: 'USER: Fixture title'\n---\n","utf8") (37bytes). Copiar bytes a case-3/4/5-TASK.md fuera del perfil y registrar ruta/hash/inventarios. Sin Get-Content/trim/CRLF normalizado ni task-clean/task-set-title.

Ausencia modal tras crear/Cancel: missing_selector esperado. Lista/fila hidden al cerrar: target_hidden esperado; cuando sin opciones fila missing_selector esperado. Un rechazo distinto o selector duplicado es TEST_BLOCKED. Query sola no prueba hit-test: click exitoso sí acredita dispatch/hit-test al centro de fila concreta, no ratón OS ni todos los controles.

### Capturas y geometría

En normal y mínima: modal abierto/lista abierta, después confirmado/lista cerrada; en ambos temas por acción semántica existente actionBar.theme y restaurar tema original del fixture. Capturas mediante control-plane configurado y autorización vigente del tester: reconciliar window-info processPath/PID/HWND con window-list AC [TESTEABLE] único; usar solo ese HWND. Verificar PNG estable/firma/dimensiones/SHA, preservar receipt. No capturar otra ventana.

Query todos controles y filas del fixture: visible:true, cajas x/y>=0 y right/bottom<=viewport medido; botones/hints/textos sin overflow. La lista180px scrolla con muchos equipos: tests DOM prueban navegación/scroll solicitado, no añadir fixture nuevo para simular30 equipos. Captura acredita forma/colores/orden/texto solo donde se ve; query exacta acredita texto/estados/cajas, no píxeles ni hit-testing. Si capturador recorta ~161px como B, registrar dimensión vs rect/faltante; no atribuirlo al producto ni declarar comparación visual completa. Nueva discrepancia visual se reporta; no heredar la aceptación del recorte de B para dispensar un requisito de A.

## Matriz de gates en SHA final

| Gate | Evidencia que lo cubre | Estado R2 hoy / límite |
|---|---|---|
| «Focus abre», con excepción inicial D2-b | Focus/blur reales despachados en jsdom; intento black-box de varios teams previo a ArrowDown con query/foreground | Solo jsdom prueba regla determinísticamente; Windows/Chromium NOT-RUN. Varios: expanded:false sin foreground = TEST_BLOCKED, no FAIL producto. Único al montar: expanded:false correcto. ArrowDown no acredita focus-abre. |
| D2-b único: montaje cerrado y primer Enter crea | Test focal KeyboardEvent bubbling/input enfocado/create_workgroup una llamada taskTitle:""; tests posteriores de foco/flechas/edición/Escape | NOT-RUN en SHA final hasta suite. No cubierto por fixture de tres teams/cinco casos; integración nativa Windows queda NOT-RUN, sin equivalencia con jsdom. |
| A filtro/confirmación/edición/refresh/único | Tests focales jsdom +5 casos semánticos para subset fixture | NOT-RUN R2; refresh/único solo automatizado, no afirmar black-box de esos casos. |
| Flechas/Enter selección sin creación/control positivo/Escape dos pasos/Tab/blur | KeyboardEvent bubbling + focus/mousedown/click reales despachados en jsdom | NOT-RUN R2; PASS solo tras suite en SHA final. No integración OS. |
| Composición lógica/guardas | CompositionEvent + KeyboardEvent isComposing/flag durante composición | NOT-RUN R2; no IME real. |
| Windows WebView teclado/ratón físico/foco por Tab/IME | Ensayo nativo autorizado con nuevo binario/receta | NOT-RUN. No dispensado por plan; consultar alcance al usuario mediante coordinador cuando existan binario y receta. Aceptación anterior fue para B. Sin instalar/cambiar IME ni fallback OS ahora. |
| ARIA/testids/bridge click opción | Tests con executeAutomationRequest/targets obligatorios y black-box filas visibles | NOT-RUN R2; bridge snapshot/dispatch, no accesibilidad lector pantalla ni teclado OS. |
| B1 Clean/IPC/validación/errores/retry | Rust B1 y contratos IPC conservados +suite modal | Regresión pendiente en SHA final; PASS B previo es antecedente, no nueva ejecución. |
| TASK vacío/espacios/explícito y reconocimiento UI | Casos3/4/5 con bytes23/23/37 y query state | NOT-RUN R2; cinco PASS previos no prueban nuevo control. |
| Tema y comparación A/layout normal/mínima | Capturas nuevas +bounds/identidad/DPR, diferencia visual declarada | NOT-RUN R2. Recorte no prueba píxeles ausentes; bloqueo/aceptación explícita del coordinador, no PASS inventado. |
| Aislamiento/trazabilidad/cleanup | Guard/preflight/reset→Node/receipt/ledger único/manifest/PID limpio | NOT-RUN R2; debe acreditarse nuevamente. |

Cinco PASS no completan teclado/IME nativo ni gate visual faltante. Si tooling impide ejecución, reportar TEST_BLOCKED concreto, no equivalencia con jsdom. El gate focus-abre no recibe PASS por apertura sintética mediante ArrowDown. D1-b/D2-b están decididos; no quedan decisiones de diseño pendientes ni TBD. No tomar decisiones visibles nuevas durante implementar/probar: elevar al coordinador. Sigue para fase futura el alcance del ensayo nativo/aceptación de limitaciones de captura. Implementar solo tras aprobación de la revisión por coordinador/room-07.
