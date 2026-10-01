# #2788 — New Room black-box testing

Approved R1 procedure. Execute only after the official rebuild receipt and tester authorization. jsdom checks do not establish black-box PASS.

### Selectores cerrados

Conservar el patrón de prefijos semánticos separados por puntos y automationIdPart existente de replica-repo-badges.ts para identidades de ProjectPanel. P = automationIdPart(proj.path); W = automationIdPart(wg.name); C = automationIdPart(rowContext). Estos son los únicos targets nuevos:

| Elemento existente | data-ac-testid exacto | Instrumentación adicional / acción |
|---|---|---|
| .project-header que recibe contextmenu | project.header.${P} | contextClick; no cambiar handlers |
| New Room en menú del proyecto | project.action.newRoom.${P}.projectMenu | click; no instrumentar la segunda entrada del menú Rooms para estos casos |
| .agent-modal interior de New Room | newRoom.modal | query; data-ac-role="dialog" |
| input Search teams | newRoom.teamSearch | setValue/typeText; data-ac-detail="Type to filter teams..." |
| select nativo | newRoom.team | setValue con nombre exacto; data-ac-detail={JSON.stringify({options: filteredTeams().map(team => team.name), selected: selectedTeam(), size: 6})} |
| input de título | newRoom.taskTitle | setValue; data-ac-detail="Task title (optional)" |
| ayuda existente Leave empty... | newRoom.taskTitle.hint | query; data-ac-role="text" |
| estado existente sin equipos/resultados | newRoom.team.empty | query; conservar role="status" y montaje condicional |
| botón Create | newRoom.create | query/click; conservar disabled y texto dinámico |
| botón Cancel | newRoom.cancel | click |
| span existente .ac-wg-task | workgroup.taskTitle.${P}.${C}.${W} | query; data-ac-role="text"; data-ac-state={isTaskClean(wg.taskTitle) ? "clean" : "task"} |

La última fila es necesaria para el caso 3: lectura semántica del título y del reconocimiento Clean usando el predicado existente, sin cambiarlo ni ejecutar task-clean. C evita duplicados si una sala se proyecta en más de una sección. Solo se añaden atributos al span ya montado. Inputs no exponen valores por query: metadata.detail identifica su placeholder, screenshots prueban el texto visible; no ampliar SAFE_METADATA_ATTRIBUTES ni usar data-ac-value. La proyección del select incluye solo nombres ya visibles, orden, selección y size; excluye agentes, rutas de repos, comandos, credenciales y borrador de tarea. Para este fixture su JSON completo queda por debajo de 120 caracteres incluso seleccionado; la prueba focal debe exigir JSON.parse de metadata.detail y longitud <=120. El presupuesto/redacción existente se conserva: si otros nombres/listas truncan la proyección, el tester reporta evidencia insuficiente, sin cambiar límites ni interpretarlo como lista completa. No añadir testids a options ni nodos ocultos. El select sigue siendo el único target de selección.

### Fixture exacto, aislado y sin cuentas

Documentar en docs/testing/2788-new-room-black-box.md la siguiente receta canónica, también fijada aquí, ejecutable desde Git Bash. BIN es exclusivamente el artefacto oficial reconstruido y recibido del shipper en este repo; no el ejecutable de la sesión ni el de usuario. ANTES de cualquier invocación de BIN, incluido --help/test-reset, exigir AGENTSCOMMANDER_CONFIG_DIR no definida (incluso definida vacía bloquea); si existe, ABORTAR sin des-setearla ni elegir root alternativo y reportar al coordinador. Antes de reset, tester registra que no está activa la GUI testeable y valida ruta exacta/identidad/SHA-256 contra el receipt oficial del rebuild. El placeholder RECEIPT_SHA256 se sustituye solo por ese digest, nunca por el hash calculado como sustituto del receipt. Después de reset exitoso crear únicamente estos archivos bajo la identidad recién vacía; si existen, parar sin sobrescribir. No copiar settings, tokens, agentes ni proyectos del usuario.

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

remoteAgentHelpEnabled está confirmado contra AppSettings.remote_agent_help_enabled. Si el reset no deja la raíz vacía, parar y reportar; no improvisar bootstrap ni recurrir a la app del usuario. El fixture requiere cero decisiones visibles y cero datos/cuentas del usuario. Teams sin miembros no arrancan PTY ni clones; no abrir sesiones durante estos casos. Todo TASK.md generado queda dentro de este project aislado. Al terminar, cerrar solo la GUI testeable y usar su test-reset; no borrados manuales recursivos. Guardar evidencia fuera de la identidad reseteable, en room-shared/i2788-evidence/manual/.

### Ejecución de los cinco casos y lectura de TASK.md

Tras rebuild oficial con nuevos commit/SHA256/bytes y completar preflight/reset/Node en ese orden, tester mantiene las reglas de colocación/captura asignadas por el usuario. Lanzamiento exacto: `"$BIN" --app --ui-automation` (flags, no existe subcomando launch); añadir solo flags de colocación ya autorizados. window-info verifica currentExe/processPath exacto, HWND/PID. No hay fallback OS-input. Antes del caso 1, P impreso por Node se trata como PREDICCIÓN: ejecutar `"$BIN" ui-query --window main --selector i2788.__enumerate__` después de lanzar la GUI y guardar su error esperado missing_selector junto con la lista available en vivo. Confirmar allí el único project.header.<P-real> del fixture y usar ese sufijo real en los selectores del proyecto y de títulos; no asumir igualdad con el P predicho. Si available está truncada/no incluye el header o la identidad es ambigua, TEST_BLOCKED y reportar, sin inventar selector ni cambiar bootstrap. Esta consulta intencional solo se hace después de Node; nunca entre reset y Node.

Los comandos semánticos usan BIN y --window main; query/click/set requieren --selector con los testids anteriores, set añade --value. Para abrir: ui-context-click --selector "project.header.${P}", después ui-click --selector "project.action.newRoom.${P}.projectMenu"; confirmar newRoom.modal. UiContextClick y su verbo ui-context-click están confirmados en cli/mod.rs. Un selector esperado presente que falta/está duplicado o una acción rechazada es bloqueo explícito, no permiso para controlar otra ventana. Cuando el caso espera ausencia (modal cerrado o estado sin resultados desmontado), missing_selector es el resultado negativo esperado y se registra como tal.

1. Placement y título opcional: query newRoom.modal, teamSearch, team, taskTitle, taskTitle.hint, create y cancel; verificar metadata.detail de los dos inputs, texto exacto de ayuda y screenshot del orden buscador/lista/título/acciones. Parsear metadata.detail de newRoom.team: options:["dev-alpha","dev-beta","ops"], selected:"", size:6 y Create disabled:true. Cancel cierra y reabrir mantiene condiciones iniciales. La captura comprueba placeholder visible y layout; query solo no acredita píxeles.
2. Filtro: ui-set newRoom.teamSearch --value "  DEV "; query newRoom.team debe proyectar options:["dev-alpha","dev-beta"], selected:""; confirmar Create disabled. ui-set newRoom.team --value "dev-alpha" y comprobar selected:"dev-alpha"/Create enabled. Editar búsqueda nuevamente (aunque coincida) y comprobar selected:""/Create disabled. Buscar "missing": newRoom.team.empty tiene texto exacto, proyección options:[] y Create disabled. Borrar búsqueda con --value "": reaparecen las tres opciones y Create sigue disabled. Confirmar su orden por proyección y screenshot.
3. Vacío: abrir modal limpio, ui-set newRoom.team --value "dev-alpha", título untouched; Create enabled; ui-click newRoom.create. Esperar cierre del modal y nuevo TASK.md. Leer bytes exactos como se especifica abajo; query workgroup.taskTitle.${P}.workgroups.${W} exige text:"Clean", state:"clean". Esto prueba reconocimiento UI además del fichero.
4. Espacios: reabrir, elegir dev-alpha; ui-set newRoom.taskTitle --value "   "; click Create. Nueva sala, mismos bytes exactos y state:"clean".
5. Explícito: reabrir, elegir dev-alpha; ui-set newRoom.taskTitle --value "  Fixture title  "; click Create. Nueva sala, bytes "---\ntitle: 'USER: Fixture title'\n---\n"; query del título exige text:"USER: Fixture title", state:"task".

Antes/después de cada click Create, inventariar solo directorios room-* bajo PROJECT/.ac. Exigir exactamente una sala añadida del equipo elegido; no asumir numeración ni leer archivos de otra sala. Poll bounded 10 s para TASK.md y descubrimiento UI; registrar error si vence. Leer con Node fs.readFileSync(<ruta nueva>/TASK.md) como Buffer y comparar con Buffer.from("---\ntitle: 'Clean'\n---\nReady to start a new topic\n", "utf8") para casos 3/4, o Buffer.from("---\ntitle: 'USER: Fixture title'\n---\n", "utf8") para caso 5. Copiar esos bytes a evidencia case-3-TASK.md/case-4-TASK.md/case-5-TASK.md; guardar rutas/diff de inventario y sha256. No usar Get-Content, trim, normalización CRLF ni task-set-title/task-clean como sustitutos. PROJECT procede exclusivamente del JSON de creación del fixture; resolver la sala y verificar que sigue bajo PROJECT/.ac antes de leer. Preservar stdout/stderr/exit code de cada CLI, screenshots e informe por caso. Teclado nativo/IME/tema/ventana estrecha siguen fuera de estos cinco casos; no declarar esa cobertura por tests unitarios.

### Gates originales que los cinco casos no dispensan

El informe debe separar resultado de los cinco casos, cobertura automatizada y gates manuales. Cinco PASS no cierran la aceptación completa. Mantener estos estados/evidencias por separado en el SHA final; nada se dispensa por esta revisión:

| Gate original | Cobertura/resolución dentro de lo ya autorizado | Estado después de los cinco casos |
|---|---|---|
| Enter en lista no crea; Enter fuera crea una vez; Escape en dos pasos | B2 usa KeyboardEvent reales, bubbling/cancelable, equipo válido/Create enabled; suite focal en SHA final. Shift/IME/guardia contra duplicados ya tienen pruebas funcionales. Registrar PASS automatizado solo si la suite pasa. | Cubierto a nivel handlers/delegación; interacción nativa Windows sigue NOT-RUN hasta evidencia nativa autorizada. |
| Selección nativa por flechas, ratón y orden Tab | ui-set prueba value/input/change y selección lógica; screenshot acredita presentación de lista, sin acreditar comportamiento del teclado/ratón nativo ni foco por Tab. | NOT-RUN nativo: la instrucción vigente del coordinador excluye prueba manual GUI y el tester no tiene fallback OS-input autorizado. Requiere decisión del coordinador/usuario para autorizar esa interacción; no ejecutarla ahora. |
| Enter/Escape con WebView/select nativo Windows | B2 prueba consumo y orden de eventos reales en jsdom; no prueba defaults nativos del sistema operativo. | NOT-RUN nativo por el mismo límite de autorización. Decisión pendiente: autorizar el ensayo nativo o mantener explícitamente pendiente; no declarar equivalencia con B2. |
| IME real Windows | Suite prueba KeyboardEvent con isComposing:true (Enter/Escape) sin creación/cierre; no arranca un IME ni genera composición real. | NOT-RUN IME real por falta de autorización de interacción nativa; pedir decisión al coordinador/usuario, sin instalar/cambiar IME. |
| Tema claro/oscuro | El tester puede usar la acción semántica ya registrada actionBar.theme y capturas autorizadas de la ventana testeable. Extender solo evidencia visual: capturar New Room en ambos temas y restaurar el tema del fixture; no cambiar lógica/inventario ni tocar tema del usuario. | PENDIENTE hasta ambas capturas válidas. Si no están incluidas en la autorización efectiva del tester, NOT-RUN por ese motivo y decisión del coordinador; no asumir permiso adicional. |
| Ventana estrecha | Captura de New Room en tamaño estrecho mediante relanzamiento de la GUI testeable con colocación ya autorizada y --window-width <ancho-estrecho-autorizado> --window-height <alto-autorizado>, sin --window-maximized; window-info acredita HWND/rect/DPI, screenshot acredita texto/controles sin recorte. No decidir dimensiones nuevas en esta revisión. Conservar/resetear solo estado testeable. | PENDIENTE hasta captura válida si tamaño/colocación están autorizados; de otro modo NOT-RUN y pedir autorización para esa geometría. No mover app del usuario ni elegir monitor nuevo. |

Estas capturas complementarias no son un sexto caso funcional ni cambian los cinco casos; se reportan como gates visuales originales. Tamaño lógico/efectivo se registra con window-info y DPI, sin confundir ancho físico y lógico. Si la autorización vigente prohíbe variar geometría o tema, no hacerlo ni sustituirlo por una captura del tamaño/tema inicial. Decisiones nuevas pendientes que deben elevarse: interacción nativa/IME Windows; permiso de variar tema/geometría si no está ya concedido al tester. Esta revisión no las resuelve ni elimina sus gates. UI Clean/TASK.md y USER: sí quedan dentro de los cinco casos; errores, reintento, B1/B2 y contratos IPC siguen con sus checks automatizados originales.
