/* ============================================================================
   Prototipo v2 de candados — lógica de la maqueta.
   V2 (iteración visual pedida por el usuario): los radios de alcance se repiten
   debajo de "Apply to" con "+ lock", para asignar y candar en un solo paso.
   Si un lote + lock encuentra réplicas ya candadas, aparece un cartel que las
   identifica y ofrece: Cancel | Apply only to unlocked | Force all (con candado).
   La barra superior no compite con la asignación: muestra estado y quita el candado
   con su propio alcance (This replica / All replicas of this kind / Entire room),
   independiente del alcance de Apply to / + lock de la botonera.
   Nada de esto es producto: es una demo autocontenida que reproduce las
   pantallas reales (mismas clases CSS que la app) y agrega la UI propuesta del
   candado (.selection-lock-*).
   Lenguaje: el texto DENTRO del producto va en inglés (lenguaje de la app);
   la guía, las anotaciones y el marco van en español.
   ========================================================================== */

/* ---------- catálogo real de coding agents (labels/colores del producto) --- */
const AGENTS = [
  {
    id: "claude", label: "Claude Code", command: "claude", color: "#d97706",
    profiles: {
      A: { enabled: true, command: "--model sonnet", env: [["ANTHROPIC_MODEL", "claude-sonnet-4-5"], ["CLAUDE_CONFIG_DIR", "<replica>/.claude"]] },
      B: { enabled: true, command: "--model opus", env: [["ANTHROPIC_MODEL", "claude-opus-4-1"]] },
      C: { enabled: false, command: "", env: [] },
    },
  },
  {
    id: "codex", label: "Codex", command: "codex", color: "#10b981",
    profiles: {
      A: { enabled: true, command: "--profile default", env: [["OPENAI_BASE_URL", "https://api.openai.com/v1"]] },
      B: { enabled: true, command: "--profile fast", env: [] },
      C: { enabled: false, command: "", env: [] },
    },
  },
  {
    id: "opencode", label: "OpenCode", command: "opencode", color: "#64748b",
    profiles: {
      A: { enabled: true, command: "", env: [] },
      B: { enabled: true, command: "", env: [] },
      C: { enabled: true, command: "--experimental", env: [] },
    },
  },
];

/* ---------- datos ficticios de demostración -------------------------------- */
/* Cada réplica lleva roomId/roomName para que los accesores devuelvan el
   MISMO objeto (las mutaciones del demo — candados, pares — deben persistir). */
function initialWorld() {
  const replicas = (room, list) => list.map((r) => ({ ...r, roomId: room, roomName: room }));
  return {
    project: "AgentsCommander_iac",
    rooms: [
      {
        id: "room-12-ac-dev-team-v4",
        name: "room-12-ac-dev-team-v4",
        task: "Candados de selección — prototipos",
        replicas: replicas("room-12-ac-dev-team-v4", [
          { id: "r12-lead", name: "ac-tech-lead-v4", kind: "ac-tech-lead-v4", agent: "claude", profile: "A", status: "active", coord: true, locked: true, live: 1 },
          { id: "r12-ui", name: "ac-dev-webpage-ui-v4", kind: "ac-dev-webpage-ui-v4", agent: "codex", profile: "A", status: "idle", coord: false, locked: false, live: 0 },
          { id: "r12-rust", name: "ac-dev-rust-core-v4", kind: "ac-dev-rust-core-v4", agent: "opencode", profile: "B", status: "running", coord: false, locked: false, live: 1 },
          { id: "r12-qa", name: "ac-dev-qa-v4", kind: "ac-dev-qa-v4", agent: "claude", profile: "B", status: "idle", coord: false, locked: false, live: 0 },
        ]),
      },
      {
        id: "room-15-dev-team",
        name: "room-15-dev-team",
        task: "Otro room del mismo Team lógico",
        replicas: replicas("room-15-dev-team", [
          { id: "r15-ui", name: "ac-dev-webpage-ui-v4", kind: "ac-dev-webpage-ui-v4", agent: "opencode", profile: "C", status: "idle", coord: false, locked: false, live: 1 },
        ]),
      },
    ],
  };
}

const state = {
  world: initialWorld(),
  modal: {
    open: false,
    target: null,
    agent: "codex",
    profile: "A",
    scope: "replica",           // replica | kind | workgroup  (alcance de "Apply to")
    withLock: false,            // la elección es "+ lock": escribe el par y canda en un paso
    removeScope: "replica",     // replica | kind | workgroup  (alcance propio de "Remove lock", independiente)
    lastRemoval: null,          // { scope, count } aviso persistente del último quitar candado
    restart: true,
    arm: false,
    applied: null,
    conflict: null,             // { targets, conflicts, requested } mientras el cartel está abierto
    highlightIndex: 1,
  },
  ctx: { open: false, replica: null, x: 340, y: 180 },
  view: "entrada",
  annotations: true,
};

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

const agentById = (id) => AGENTS.find((a) => a.id === id) || AGENTS[0];
const allReplicas = () => state.world.rooms.flatMap((room) => room.replicas);
const replicaById = (id) => allReplicas().find((r) => r.id === id) || null;
const kindReplicas = (kind) => allReplicas().filter((r) => r.kind === kind);
const roomOf = (replica) => state.world.rooms.find((room) => room.id === replica.roomId);
const lockIcon = (cls) =>
  `<svg class="${cls || ""}" viewBox="0 0 16 16" fill="none" aria-hidden="true"><rect x="3" y="7" width="10" height="7" rx="1.6" stroke="currentColor" stroke-width="1.5"/><path d="M5.5 7V5.4A2.5 2.5 0 0 1 8 3a2.5 2.5 0 0 1 2.5 2.4V7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>`;

/* ---------- vistas / storyboard -------------------------------------------- */
const VIEWS = [
  {
    id: "entrada",
    label: "1 · Entrada",
    title: "Dónde se llega al candado",
    sub: "Sidebar → clic derecho en la réplica → Coding Agent. El candado aparece junto al par (Coding Agent + Profile).",
    setup() {
      closeModal();
      state.ctx = { open: true, replica: "r12-ui", x: 372, y: 214 };
    },
    notes: [
      ["El candado se ve sin abrir nada", "En la fila de <strong>ac-tech-lead-v4</strong> el candado ámbar aparece pegado al par: agente vivo + profile. Una réplica protegida se reconoce de un vistazo."],
      ["El camino real, sin menú nuevo", "Clic derecho sobre una réplica → <strong>Coding Agent</strong>. Es la entrada que ya existe en el producto; el candado se pone dentro de ese modal."],
    ],
    annots: [
      { find: '.replica-item[data-id="r12-lead"] .selection-lock-chip', pad: 3, pin: "tr", n: 1 },
      { find: ".session-context-menu", pad: 3, pin: "tr", n: 2 },
    ],
  },
  {
    id: "apply-lock",
    label: "2 · Fila + lock",
    title: "Asignar y candar en un paso",
    sub: "Debajo de Apply to están los mismos destinos con + lock: la misma elección escribe el par y pone el candado.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      state.modal.scope = "replica";
      state.modal.withLock = false;
      state.modal.applied = null;
    },
    notes: [
      ["Dos filas, una sola elección", "La segunda fila repite los radios con <strong>+ lock</strong>: escribir el par y candar es un solo paso. Es una única elección: marcar una fila desmarca la otra."],
      ["Sin conflicto, camino directo", "Si ninguna réplica del alcance tiene candado, Apply escribe y canda sin preguntar nada. El cartel de conflictos solo aparece cuando hay bloqueadas."],
      ["La barra quita por alcance propio", "Arriba, <strong>Remove lock from</strong> ofrece <em>This replica / All replicas of this kind / Entire room</em> con su recuento; el candado se pone abajo, junto a la asignación."],
    ],
    annots: [
      { find: ".agent-scope-picker--lock", pad: 4, pin: "bl", n: 1 },
      { find: ".agent-scope-lock-assumption", pad: 3, pin: "br", n: 2 },
      { find: ".selection-lock-bar", pad: 4, pin: "tl", n: 3 },
    ],
  },
  {
    id: "replica-cerrado",
    label: "3 · Candado cerrado",
    title: "Réplica concreta — candado cerrado",
    sub: "Un cambio individual deliberado sigue permitido y conserva el candado; la barra ofrece quitarlo sin tocar el par.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      const r = replicaById("r12-ui");
      r.locked = true;
      state.modal.scope = "replica";
      state.modal.withLock = false;
      state.modal.applied = null;
    },
    notes: [
      ["Estado protegido", "El chip dice <strong>Protected</strong> y el par queda escrito junto al candado."],
      ["Cambio individual permitido", "Elegir <strong>This replica + lock</strong> y aplicar cambia el par sin cartel: es un cambio deliberado y el candado se conserva."],
      ["Quitar candado con alcance propio", "La barra ofrece <strong>This replica / All replicas of this kind / Entire room</strong> con recuento de protegidas; es una elección separada de <em>Apply to</em>."],
      ["Quitar no reescribe", "Quitar el candado conserva el par <em>Coding Agent + Profile</em>, no reinicia la sesión y no toca el default de futuras réplicas."],
    ],
    annots: [
      { find: ".selection-lock-state", pad: 4, pin: "tl", n: 1 },
      { find: ".selection-lock-remove-scopes", pad: 3, pin: "bl", n: 2 },
      { find: "#lockRemoveBtn", pad: 4, pin: "tr", n: 3 },
    ],
  },
  {
    id: "tipo-preview",
    label: "4 · Por tipo + lock",
    title: "Por tipo — preview antes de aplicar",
    sub: "Alcance masivo con + lock: se enumeran las réplicas del tipo, cada una con su par, y la protegida queda marcada antes de aplicar.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r12-ui").locked = true;
      state.modal.agent = "claude";
      state.modal.profile = "B";
      state.modal.scope = "kind";
      state.modal.withLock = true;
      state.modal.removeScope = "kind";
      state.modal.arm = true;
      state.modal.restart = true;
      state.modal.applied = null;
    },
    notes: [
      ["Elegir + lock en un destino masivo", "La fila <strong>All replicas of this kind + lock</strong> es una sola elección: escribe Claude Code · Profile B y canda las elegibles."],
      ["Preview de conflictos", "La lista muestra cada par actual y la píldora <em>Protected / Not protected</em>. Con Apply aparece el cartel de conflictos si hay bloqueadas."],
      ["Quitar candado disponible", "Con <strong>All replicas of this kind</strong> elegido en la barra, <strong>Remove lock from 1 replica</strong> deshace la protección sin abrir otro control."],
      ["Apply se habilita con la confirmación", "El botón <strong>Apply</strong> queda deshabilitado hasta marcar la confirmación de sobrescritura; esa es la mecánica real del modal."],
    ],
    annots: [
      { find: ".agent-scope-picker--lock", pad: 4, pin: "tl", n: 1 },
      { find: ".selection-lock-kind", pad: 3, pin: "bl", n: 2 },
      { find: "#lockRemoveBtn", pad: 4, pin: "tr", n: 3 },
      { find: "#mpTargets", pad: 3, pin: "br", n: 4 },
      { find: ".agent-picker-apply", pad: 4, pin: "tr", n: 5 },
    ],
  },
  {
    id: "conflicto",
    label: "5 · Cartel de conflictos",
    title: "Réplicas ya bloqueadas: el cartel decide",
    sub: "Al aplicar + lock con réplicas bloqueadas, el cartel las lista con par actual y solicitado, y ofrece las tres salidas.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r12-ui").locked = true;
      state.modal.agent = "claude";
      state.modal.profile = "B";
      state.modal.scope = "kind";
      state.modal.withLock = true;
      state.modal.arm = true;
      state.modal.restart = true;
      state.modal.applied = null;
      requestApply();
    },
    notes: [
      ["El cartel nombra a las bloqueadas", "Cada réplica en conflicto aparece con <em>Now</em> (su par actual + Protected) y <em>Requested</em> (el par pedido). No hay duda de a quién afecta."],
      ["Tres salidas inequívocas", "<strong>Cancel</strong> no cambia nada. <strong>Apply only to unlocked</strong> escribe y canda solo las libres. <strong>Force all, including locked</strong> también sobrescribe las bloqueadas, que conservan su candado."],
      ["Cancel, a la izquierda", "El botón <strong>Cancel</strong> queda a la izquierda de las acciones, como en la botonera real del modal."],
      ["Decisión del usuario", "Esta salida reemplaza la suposición de omitir siempre las bloqueadas en lotes + lock: ver <em>room-shared/candados-decision-usuario.md</em>."],
    ],
    annots: [
      { find: ".lock-conflict-card", pad: 4, pin: "tl", n: 1 },
      { find: ".lock-conflict-list", pad: 3, pin: "bl", n: 2 },
      { find: ".lock-conflict-actions", pad: 4, pin: "br", n: 3 },
    ],
  },
  {
    id: "resultado-force",
    label: "6 · Resultado: forzar todo",
    title: "Force all — las bloqueadas se sobrescriben y conservan su candado",
    sub: "Salida más agresiva del cartel: todas las réplicas del alcance reciben el par y quedan candadas.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r12-ui").locked = true;
      state.modal.agent = "claude";
      state.modal.profile = "B";
      state.modal.scope = "kind";
      state.modal.withLock = true;
      state.modal.arm = true;
      state.modal.restart = true;
      applyAssignment("forceAll");
      showToast("2 updated + locked · 0 protected · 0 errors — the previously locked replica kept its lock.", "success");
    },
    notes: [
      ["Efecto completo", "<strong>2 updated + locked · 0 protected · 0 errors</strong>: las dos réplicas del tipo quedan con el par pedido y con candado."],
      ["La bloqueada mantiene protección", "La réplica que ya estaba bloqueada fue sobrescrita y <em>conserva su candado</em>; el resultado lo dice en una línea aparte."],
      ["Nada se omite", "No hay filas <em>Protected · skipped</em>: el usuario eligió forzar y el resumen no mezcla omitidas con errores."],
    ],
    annots: [
      { find: ".agent-scope-result", pad: 4, pin: "tr", n: 1 },
      { find: ".agent-scope-lock-summary", pad: 3, pin: "bl", n: 2 },
      { find: '.replica-item[data-id="r15-ui"] .selection-lock-chip', pad: 3, pin: "tr", n: 3 },
    ],
  },
  {
    id: "resultado-unlocked",
    label: "7 · Resultado: solo libres",
    title: "Apply only to unlocked — la bloqueada queda intacta",
    sub: "Salida conservadora: se escribe y canda solo la réplica libre; la bloqueada conserva par, candado y sesión.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r12-ui").locked = true;
      state.modal.agent = "claude";
      state.modal.profile = "B";
      state.modal.scope = "kind";
      state.modal.withLock = true;
      state.modal.arm = true;
      state.modal.restart = true;
      applyAssignment("unlockedOnly");
      showToast("1 updated + locked · 1 protected · 0 errors — protected replicas were not written or restarted.", "success");
    },
    notes: [
      ["Efecto conservador", "<strong>1 updated + locked · 1 protected · 0 errors</strong>: solo la libre recibe par y candado."],
      ["La bloqueada no se toca", "Conserva su par, su candado y su sesión: no se escribe ni se reinicia."],
      ["Cancel no llega hasta acá", "Si el usuario cancela en el cartel, no hay resultado: pares, candados y reinicios quedan como estaban."],
    ],
    annots: [
      { find: ".agent-scope-result", pad: 4, pin: "tr", n: 1 },
      { find: ".agent-scope-target-row.protected", pad: 3, pin: "br", n: 2 },
      { find: "#demoToast", pad: 4, pin: "tr", n: 3 },
    ],
  },
  {
    id: "futuro",
    label: "8 · Futuras réplicas",
    title: "Default para futuras réplicas",
    sub: "Alcance confirmado por el usuario: las réplicas que se creen en futuras rooms heredan el candado del tipo, con excepción por réplica y sin propagación retroactiva.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      state.modal.scope = "replica";
      state.modal.withLock = false;
      state.modal.applied = null;
      state.modal.futureOpen = true;
    },
    notes: [
      ["Alcance confirmado", "El usuario ratificó la herencia a futuras rooms, con excepciones por réplica: ver <em>room-shared/candados-decision-usuario.md</em>."],
      ["Excepción por réplica", "Cada réplica nueva puede quitarse su propio candado; el default solo decide cómo nace una réplica nueva. Los pares de cada réplica no se copian entre sí."],
      ["Sin propagación retroactiva", "Cambiar el default no toca réplicas existentes: su estado local sigue siendo la autoridad."],
    ],
    annots: [
      { find: ".selection-lock-future", pad: 4, pin: "tl", n: 1 },
      { find: ".selection-lock-future-note", pad: 3, pin: "bl", n: 2 },
    ],
  },
  {
    id: "remove-scope",
    label: "9 · Quitar por alcance",
    title: "Quitar candado — alcance propio en la barra",
    sub: "La barra elige de qué réplicas quitar el candado (This replica / All replicas of this kind / Entire room) sin depender de Apply to: cada opción muestra cuántas protegidas tiene.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r15-ui").locked = true;   /* misma especie, otra room */
      state.modal.removeScope = "kind";      /* el alcance masivo sigue disponible aunque la enfocada no tenga candado */
      state.modal.scope = "replica";
      state.modal.withLock = false;
      state.modal.applied = null;
    },
    notes: [
      ["Alcance propio, no el de Apply to", "Los tres alcances de la barra no dependen de <em>Apply to</em>: cambiar uno no cambia el otro. La réplica enfocada está <strong>Unlocked</strong> y aun así el alcance masivo conserva su recuento y su acción."],
      ["Conteos claros", "Cada opción dice cuántas protegidas tiene: <em>This replica 0 protected</em>, <em>All replicas of this kind 1 of 2 protected</em>, <em>Entire room 1 of 4 protected</em>."],
      ["Quitar no reescribe", "Quitar el candado conserva <strong>Coding Agent + Profile</strong>, no reinicia la sesión y deja intacto el default de futuras réplicas."],
      ["Cero candidatas", "Si el alcance elegido no tiene protegidas, la acción queda deshabilitada y avisa: <em>No protected replicas in this scope</em>."],
    ],
    annots: [
      { find: ".selection-lock-remove-scopes", pad: 4, pin: "bl", n: 1 },
      { find: ".selection-lock-state", pad: 4, pin: "tl", n: 2 },
      { find: "#lockRemoveBtn", pad: 4, pin: "tr", n: 3 },
      { find: ".selection-lock-remove-note", pad: 3, pin: "br", n: 4 },
    ],
  },
];

/* ---------- render: sidebar ------------------------------------------------- */
function sidebarStatusClass(replica) {
  if (replica.status === "running") return "running";
  if (replica.status === "active") return "active";
  if (replica.status === "idle") return "idle";
  return "offline";
}

function renderSidebar() {
  const rows = state.world.rooms
    .map((room) => {
      const replicas = room.replicas
        .map((r) => {
          const agent = agentById(r.agent);
          const lockChip = r.locked
            ? `<span class="selection-lock-chip" title="Protected from bulk changes · ${agent.label} · Profile ${r.profile}">${lockIcon()}KEEP</span>`
            : "";
          const coordBadge = r.coord ? '<span class="ac-discovery-badge coord">orchestrator</span>' : "";
          return `
          <div class="replica-item" data-id="${r.id}" title="D:\\0_repos\\${state.world.project}\\.ac\\${room.name}\\__agent_${r.name}">
            <div class="session-item-status ${sidebarStatusClass(r)}"></div>
            <div class="replica-item-info">
              <div class="ac-discovery-badges">
                <span class="agent-name-chip">${r.name}</span>
                ${coordBadge}
                <span class="ac-discovery-badge agent">${agent.label}</span>
                <span class="profile-badge" title="Profile letter">${r.profile}</span>
                ${lockChip}
              </div>
            </div>
          </div>`;
        })
        .join("");
      return `
      <div class="ac-wg-subgroup">
        <div class="ac-wg-header ac-wg-header--collapsible">
          <span class="ac-discovery-chevron">&#x25BE;</span>
          <div class="ac-wg-header-text">
            <span class="ac-wg-name">${room.name}</span>
            <span class="ac-wg-task">${room.task}</span>
          </div>
        </div>
        ${replicas}
      </div>`;
    })
    .join("");

  $("#sidebarRoot").innerHTML = `
    <div class="project-panel">
      <div class="project-header">
        <button type="button" class="project-header-main">
          <span class="ac-discovery-chevron">&#x25BE;</span>
          <span class="project-title">Project: ${state.world.project}</span>
        </button>
      </div>
      <div class="project-content">${rows}</div>
    </div>`;
}

/* ---------- render: menú contextual ---------------------------------------- */
function renderContextMenu() {
  const menu = $("#ctxMenu");
  if (!state.ctx.open || !state.ctx.replica) {
    menu.hidden = true;
    return;
  }
  const replica = replicaById(state.ctx.replica);
  menu.hidden = false;
  menu.style.left = `${state.ctx.x}px`;
  menu.style.top = `${state.ctx.y}px`;
  menu.innerHTML = `
    <button type="button" class="session-context-option context-option-danger"><span class="session-context-option-icon">&#x21BA;</span> Restart Session</button>
    <button type="button" class="session-context-option" data-action="coding-agent"><span class="session-context-option-icon">&#x1F916;</span> Coding Agent</button>
    <button type="button" class="session-context-option"><span class="session-context-option-icon">&#x1F4C2;</span> Open Replica's Folder</button>
    <button type="button" class="session-context-option"><span class="session-context-option-icon">&#x1F5C2;</span> Open Matrix folder</button>
    <div class="context-separator"></div>
    <button type="button" class="session-context-option"><span class="session-context-option-icon">&#x270E;</span> Edit TASK title</button>
    <button type="button" class="session-context-option"><span class="session-context-option-icon">&#x1F9F9;</span> Clear task title</button>
    <div class="demo-ctx-target">${replica.roomName} · ${replica.name}</div>`;
  menu.querySelector('[data-action="coding-agent"]').onclick = () => {
    state.ctx.open = false;
    openModal(state.ctx.replica);
    render();
    showToast("Modal real: el candado vive junto al par.", "info");
  };
}

/* ---------- modal ----------------------------------------------------------- */
function openModal(replicaId) {
  const replica = replicaById(replicaId);
  if (!replica) return;
  state.modal.open = true;
  state.modal.target = replicaId;
  state.modal.agent = replica.agent;
  state.modal.profile = replica.profile;
  state.modal.scope = "replica";
  state.modal.withLock = false;
  state.modal.removeScope = "replica";
  state.modal.lastRemoval = null;
  state.modal.arm = false;
  state.modal.applied = null;
  state.modal.conflict = null;
  state.modal.futureOpen = false;
  $("#modalOverlay").hidden = false;
}

function closeModal() {
  state.modal.open = false;
  state.modal.conflict = null;
  $("#modalOverlay").hidden = true;
  const conflict = $("#lockConflict");
  conflict.hidden = true;
  conflict.innerHTML = "";
}

function effectiveProfile(agentId, letter) {
  const agent = agentById(agentId);
  if (agent.profiles[letter]?.enabled) return letter;
  const fallback = Object.keys(agent.profiles).find((l) => agent.profiles[l].enabled) || "A";
  return fallback;
}

function renderLockBar() {
  const replica = replicaById(state.modal.target);
  const agent = agentById(state.modal.agent);
  const removeScope = state.modal.removeScope;
  const kindTargets = kindReplicas(replica.kind);
  const kindTotal = kindTargets.length;
  const kindRooms = new Set(kindTargets.map((r) => r.roomId)).size;
  const scopeTargets = assignmentTargets(removeScope, replica);
  const scopeLocked = scopeTargets.filter((t) => t.locked);

  /* El chip describe el estado del alcance elegido en "Remove lock" (barra),
     que es independiente del alcance de "Apply to" (botonera de abajo). */
  const stateChip = removeScope === "replica"
    ? `<span class="selection-lock-state" data-state="${replica.locked ? "locked" : "open"}">${replica.locked ? "Protected" : "Unlocked"}</span>`
    : `<span class="selection-lock-state" data-state="${scopeLocked.length > 0 ? "locked" : "open"}">${scopeLocked.length} of ${scopeTargets.length} protected</span>`;

  const hint = removeScope === "replica"
    ? replica.locked
      ? "Bulk assignments and restarts skip it. Removing the lock keeps the pair and does not restart."
      : 'Bulk assignments may overwrite this pair. Use "+ lock" below to write and protect in one step.'
    : "Only protection changes here: pairs, sessions and the future default stay untouched.";

  /* Alcance propio de "Remove lock": cada opción muestra cuántas protegidas tiene. */
  const removeOption = (targetScope, label) => {
    const targets = assignmentTargets(targetScope, replica);
    const locked = targets.filter((t) => t.locked).length;
    const active = removeScope === targetScope;
    const count = targetScope === "replica" ? `${locked} protected` : `${locked} of ${targets.length} protected`;
    return `<label class="selection-lock-scope-opt${active ? " active" : ""}${locked === 0 ? " empty" : ""}"><input type="radio" name="removeScope" value="${targetScope}" ${active ? "checked" : ""}>${label} <span class="selection-lock-scope-count">${count}</span></label>`;
  };

  let kindBlock = "";
  if (state.modal.scope === "kind") {
    const rows = kindTargets
      .map((r) => {
        const rAgent = agentById(r.agent);
        return `<div class="selection-lock-kind-row">
          <span class="wg">${r.roomName}</span>
          <span class="name">${r.name}</span>
          <span class="pair">${rAgent.label} · Profile ${r.profile}</span>
          <span class="selection-lock-pill ${r.locked ? "locked" : "will-lock"}">${r.locked ? "Protected" : "Not protected"}</span>
        </div>`;
      })
      .join("");
    kindBlock = `
      <div class="selection-lock-kind">
        <div class="selection-lock-kind-head">${kindTotal} replica(s) of this kind · ${kindRooms} room(s) · every row shows its own pair</div>
        ${rows}
        <div class="selection-lock-kind-note">Apply to and + lock act on every eligible row; protected rows are resolved by the conflict dialog. New replicas inherit this Matrix default.</div>
      </div>`;
  }

  const futureBlock = state.modal.futureOpen
    ? `
    <div class="selection-lock-future">
      <div class="selection-lock-future-head">
        Default for new replicas of this Matrix
        <span class="decision-tag">confirmed · user decision</span>
        <label class="selection-lock-scope-opt active" style="margin-left:auto"><input type="checkbox" checked> Start locked</label>
      </div>
      <div class="selection-lock-kind-row">
        <span class="pair">Pair used at creation: ${agent.label} · Profile ${state.modal.profile}</span>
        <span class="selection-lock-pill locked">inherited at creation</span>
      </div>
      <div class="selection-lock-future-note">Inherited by replicas created in future rooms of this Team. Existing replicas keep their current lock state; changing this default never propagates. Each new replica can remove its own lock.</div>
    </div>`
    : "";

  /* V2 iteración: la barra quita el candado con alcance propio (no elige el de Apply to). */
  const removeLabel = scopeLocked.length === 0
    ? "Nothing to remove"
    : removeScope === "replica"
      ? "Remove lock"
      : `Remove lock from ${scopeLocked.length} ${scopeLocked.length === 1 ? "replica" : "replicas"}`;
  const removeNote = scopeLocked.length > 0
    ? "Keeps Coding Agent + Profile. No restart."
    : "No protected replicas in this scope — nothing to remove.";
  const lastRemoval = state.modal.lastRemoval
    ? `<div class="selection-lock-remove-done">Lock removed from ${state.modal.lastRemoval.count} ${state.modal.lastRemoval.count === 1 ? "replica" : "replicas"} · Coding Agent + Profile kept · no restart</div>`
    : "";

  $("#mpLockBar").innerHTML = `
    <div class="selection-lock-bar" data-state="${scopeLocked.length > 0 ? "locked" : "open"}">
      <div class="selection-lock-icon">${lockIcon()}</div>
      <div class="selection-lock-main">
        <div class="selection-lock-title">Selection lock ${stateChip}</div>
        <div class="selection-lock-pair">${agent.label} · Profile ${state.modal.profile}</div>
        <div class="selection-lock-hint">${hint}</div>
      </div>
      <div class="selection-lock-right">
        <div class="selection-lock-remove-head">Remove lock from</div>
        <div class="selection-lock-remove-scopes" role="radiogroup" aria-label="Remove lock scope">
          ${removeOption("replica", "This replica")}
          ${removeOption("kind", "All replicas of this kind")}
          ${removeOption("workgroup", "Entire room")}
        </div>
        <div class="selection-lock-remove-row">
          <span class="selection-lock-remove-note">${removeNote}</span>
          <button type="button" class="selection-lock-remove" id="lockRemoveBtn" ${scopeLocked.length === 0 ? "disabled" : ""}>${removeLabel}</button>
        </div>
        ${lastRemoval}
      </div>
    </div>
    ${kindBlock}
    ${futureBlock}`;
}

function renderProviders() {
  const rows = AGENTS.map((agent) => {
    const active = agent.id === state.modal.agent;
    return `
      <button type="button" class="agent-profile-provider-card ${active ? "active" : ""}" data-agent="${agent.id}" style="--agent-color:${agent.color}" aria-pressed="${active}">
        <span>
          <span class="agent-profile-provider-name">${agent.label}</span>
          <span class="agent-profile-provider-command">${agent.command}</span>
        </span>
        <span class="agent-profile-provider-chip">Profile ${state.modal.profile}</span>
      </button>`;
  }).join("");
  $("#mpProviders").innerHTML = rows;
}

function renderProfiles() {
  const agent = agentById(state.modal.agent);
  const letters = ["A", "B", "C"];
  const rows = letters
    .map((letter) => {
      const cell = agent.profiles[letter];
      const configured = Boolean(cell?.enabled);
      const selected = state.modal.profile === letter;
      const eff = effectiveProfile(agent.id, letter);
      const pill = letter === "A" ? "match" : configured ? "configured" : "fallback";
      const pillLabel = { match: "MATCH", configured: "CONFIGURED", fallback: "FALLBACK", missing: "MISSING" }[pill];
      const envRows = configured && selected && cell.env.length
        ? `<span class="agent-profile-declared-env"><span class="agent-profile-declared-env-head">Declared env</span><span class="agent-profile-declared-env-grid">${cell.env
            .map(([k, v]) => `<span class="agent-profile-declared-env-row"><span class="agent-profile-declared-env-key">${k}</span><span class="agent-profile-declared-env-value">${v}</span><span class="agent-profile-declared-env-origin">cell</span></span>`)
            .join("")}</span></span>`
        : "";
      return `
      <button type="button" class="agent-profile-card ${selected ? "active" : ""} ${configured ? "" : "missing"}" data-profile="${letter}" aria-pressed="${selected}">
        <span class="agent-profile-card-head">
          <span>
            <span class="agent-profile-card-title">Profile ${letter}</span>
            <span class="agent-profile-card-subtitle">${configured ? `configured for ${agent.label}` : `missing; launches Profile ${eff}`}</span>
          </span>
          <span class="agent-profile-card-tags">
            <span class="agent-profile-card-pill ${pill}">${pillLabel}</span>
            ${letter === "A" ? '<span class="agent-profile-default-marker">Default</span>' : ""}
          </span>
        </span>
        <span class="agent-profile-param-list">
          <span class="agent-profile-param"><span>Command </span><span>${agent.command}${cell?.command ? " " + cell.command : ""}</span></span>
          ${configured ? "" : `<span class="agent-profile-token warn">Fallback ${letter} -&gt; ${eff}</span>`}
        </span>
        ${envRows}
      </button>`;
    })
    .join("");
  $("#mpProfiles").innerHTML = rows;
}

function renderComparison() {
  const rows = AGENTS.map((agent) => {
    const direct = Boolean(agent.profiles[state.modal.profile]?.enabled);
    const eff = effectiveProfile(agent.id, state.modal.profile);
    const status = direct ? "direct" : "fallback";
    return `
      <button type="button" class="agent-comparison-row ${agent.id === state.modal.agent ? "active" : ""}" data-agent="${agent.id}">
        <span class="agent-comparison-agent-cell">
          <span class="agent-comparison-agent-name">${agent.label}</span>
          <span class="agent-comparison-agent-sub">${agent.id === state.modal.agent ? "selected coding agent" : "configured peer"}</span>
        </span>
        <span class="agent-comparison-resolution-cell">
          <span class="agent-comparison-status ${status}">${status}</span>
          <span class="agent-comparison-resolution">${direct ? `Profile ${state.modal.profile} direct` : `Profile ${state.modal.profile} &rarr; ${eff} (fallback)`}</span>
        </span>
      </button>`;
  }).join("");
  const summary = {
    direct: AGENTS.filter((a) => a.profiles[state.modal.profile]?.enabled).length,
    fallback: AGENTS.filter((a) => !a.profiles[state.modal.profile]?.enabled).length,
  };
  $("#mpComparison").innerHTML = `
    <div class="agent-comparison-summary">
      <div class="agent-comparison-summary-tile"><span class="agent-comparison-summary-value direct">${summary.direct}</span><span class="agent-comparison-summary-label">Direct</span></div>
      <div class="agent-comparison-summary-tile"><span class="agent-comparison-summary-value fallback">${summary.fallback}</span><span class="agent-comparison-summary-label">Fallback</span></div>
      <div class="agent-comparison-summary-tile"><span class="agent-comparison-summary-value missing">0</span><span class="agent-comparison-summary-label">Missing</span></div>
    </div>
    <div class="agent-comparison-table" role="table" aria-label="Same profile comparison">
      <div class="agent-comparison-table-head" role="row"><span>Coding Agent</span><span>Resolution</span></div>
      <div class="agent-comparison-table-body" role="rowgroup">${rows}</div>
    </div>`;
}

function assignmentTargets(scope, replica) {
  if (scope === "workgroup") return roomOf(replica).replicas;
  if (scope === "kind") return kindReplicas(replica.kind);
  return [replica];
}

function renderScopeAndBar() {
  const replica = replicaById(state.modal.target);
  const scope = state.modal.scope;
  const withLock = state.modal.withLock;
  const targets = assignmentTargets(scope, replica);
  const eligible = targets.filter((t) => (scope === "replica" ? true : !t.locked));
  const skipped = targets.filter((t) => !eligible.includes(t));
  const rooms = new Set(targets.map((t) => t.roomId)).size;
  const liveEligible = eligible.reduce((n, t) => n + (t.live || 0), 0);

  /* V2: la fila pedida. Repite los mismos destinos con "+ lock". */
  const destination = (targetScope, targetWithLock) => {
    const name = targetScope === "replica" ? "This replica" : targetScope === "kind" ? "All replicas of this kind" : "Entire room";
    const count = targetScope === "replica"
      ? "1 replica"
      : targetScope === "kind"
        ? `${kindReplicas(replica.kind).length} replicas`
        : `${roomOf(replica).replicas.length} replicas`;
    const active = scope === targetScope && withLock === targetWithLock;
    const classes = ["agent-scope-opt"];
    if (active) classes.push("active");
    if (targetScope !== "replica") classes.push("dangerous");
    if (targetWithLock) classes.push("locked-choice");
    return `<label class="${classes.join(" ")}"><input type="radio" name="assignChoice" value="${targetScope}${targetWithLock ? "+lock" : ""}" ${active ? "checked" : ""}>${targetWithLock ? lockIcon("agent-scope-opt-lock") : ""}${name}${targetWithLock ? " + lock" : ""} <span class="agent-scope-count">${count}</span></label>`;
  };

  const scopePicker = `
    <div class="agent-scope-stack" role="radiogroup" aria-label="Apply to, optionally with lock">
      <div class="agent-scope-picker">
        <span class="agent-scope-label">Apply to</span>
        ${destination("replica", false)}
        ${destination("kind", false)}
        ${destination("workgroup", false)}
      </div>
      <div class="agent-scope-picker agent-scope-picker--lock">
        <span class="agent-scope-label">Apply to <span class="agent-scope-lock-badge">${lockIcon()} + lock</span></span>
        ${destination("replica", true)}
        ${destination("kind", true)}
        ${destination("workgroup", true)}
      </div>
    </div>
    <div class="agent-scope-live-note"><span class="agent-scope-live-tag">live</span> Counts are read from the current room; the backend re-enumerates targets before applying.</div>
    <div class="agent-scope-lock-assumption">One step: <strong>+ lock</strong> writes the pair and sets the lock on the same eligible replicas. Replicas already locked are listed by the conflict dialog before applying; <strong>Cancel</strong> changes nothing.</div>`;

  /* Con resultado aplicado, las filas muestran el desenlace real, no el preview. */
  const applied = state.modal.applied;
  const eligibleLabel = `${eligible.length} will update${withLock ? " + lock" : ""}`;
  let targetList = "";
  if (scope !== "replica") {
    const rows = applied && applied.rows
      ? applied.rows
      : targets.map((t) => {
          const ineligible = skipped.includes(t);
          const rAgent = agentById(t.agent);
          return {
            roomName: t.roomName,
            name: t.name,
            path: `${rAgent.label} · Profile ${t.profile}`,
            live: t.live || 0,
            stateLabel: ineligible ? "Protected · skipped" : (withLock ? "Will update + lock" : "Will update"),
            protected: ineligible,
          };
        });
    const head = applied
      ? ""
      : `<div class="agent-scope-targets-head">${eligible.length} to update${withLock ? " + lock" : ""} · ${skipped.length} protected (skipped) · ${targets.length} replica(s) across ${rooms} room(s) · ${liveEligible} live session(s)</div>
        <div class="agent-scope-lock-summary">
          <span class="eligible">${eligibleLabel}</span>
          <span class="protected">${skipped.length} protected · skipped, not restarted</span>
          <span class="errors">0 errors</span>
        </div>`;
    targetList = `
      <div class="agent-scope-targets" data-ac-role="list" id="mpTargets">
        ${head}
        ${rows
          .map(
            (r) => `
          <div class="agent-scope-target-row ${r.protected ? "protected" : ""}">
            <span class="agent-scope-target-wg">${r.roomName}</span>
            <span class="agent-scope-target-name">${r.name}</span>
            <span class="agent-scope-target-path">${r.path}</span>
            ${r.live ? `<span class="agent-scope-target-live">${r.live} live</span>` : ""}
            <span class="agent-scope-target-state ${r.protected ? "protected" : "eligible"}">${r.stateLabel}</span>
          </div>`
          )
          .join("")}
      </div>`;
  }

  const result = applied
    ? `<div class="agent-scope-result">
        <div><strong>${applied.updated} updated${applied.withLock ? " + locked" : ""} · ${applied.protected} protected · ${applied.errors} errors</strong></div>
        ${applied.protected ? '<div class="protected-note">Protected replicas were not written and not restarted.</div>' : ""}
        ${applied.forced ? `<div class="forced-note">${applied.forced} previously locked ${applied.forced === 1 ? "replica was" : "replicas were"} overwritten and kept the lock.</div>` : ""}
        <div>${applied.restarted} eligible session(s) restarted.${applied.locked ? ` ${applied.locked} selection lock(s) in place.` : ""}</div>
      </div>`
    : "";

  const applyDisabled = scope === "replica" ? false : !state.modal.arm;
  const noun = (n) => (n === 1 ? "replica" : "replicas");
  const applyLabel = scope === "replica"
    ? (withLock ? "Assign + lock this replica" : "Assign to this replica")
    : scope === "kind"
      ? (withLock ? `Overwrite ${eligible.length} + lock of this kind` : `Overwrite ${eligible.length} of this kind`)
      : (withLock ? `Overwrite ${eligible.length} + lock in this room` : `Overwrite ${eligible.length} in this room`);
  const armLabel = scope === "kind"
    ? (withLock
        ? `I understand this overwrites and locks ${eligible.length} ${noun(eligible.length)} of this kind`
        : `I understand this overwrites ${eligible.length} ${noun(eligible.length)} of this kind (${skipped.length} protected is skipped)`)
    : (withLock
        ? `I understand this overwrites and locks ${eligible.length} ${noun(eligible.length)}`
        : `I understand this overwrites ${eligible.length} ${noun(eligible.length)} (${skipped.length} protected is skipped)`);

  $("#mpBotonera").innerHTML = `
    ${scopePicker}
    ${targetList}
    ${result}
    <div class="agent-picker-bar">
      ${scope !== "replica" && !applied ? `<label class="agent-scope-switch"><input type="checkbox" id="restartToggle" ${state.modal.restart ? "checked" : ""}> Restart sessions after apply</label>` : ""}
      <div class="agent-picker-bar-spacer"></div>
      ${scope !== "replica" && !applied ? `<label class="agent-scope-arm"><input type="checkbox" id="armToggle" ${state.modal.arm ? "checked" : ""}> ${armLabel}</label>` : ""}
      <button type="button" class="modal-btn modal-btn-cancel" id="mpCancel">Cancel</button>
      ${applied ? "" : `<button type="button" class="modal-btn modal-btn-save agent-picker-apply ${scope !== "replica" ? "danger" : ""}" id="mpApply" ${applyDisabled ? "disabled" : ""}>${withLock ? lockIcon("agent-picker-apply-lock") : ""}${applyLabel}</button>`}
    </div>`;
}

function renderModal() {
  if (!state.modal.open) return;
  const replica = replicaById(state.modal.target);
  const agent = agentById(state.modal.agent);
  $("#mpFqn").textContent = `${replica.roomName}/${replica.name}`;
  $("#mpName").textContent = replica.name;
  $("#mpRoom").textContent = replica.roomName;
  renderLockBar();
  renderProviders();
  renderProfiles();
  renderComparison();
  renderScopeAndBar();
  renderConflict();
}

/* ---------- acciones -------------------------------------------------------- */
/* V2: el candado se pone con la asignación ("Apply to + lock"); la barra lo quita
   con su propio alcance (removeScope), independiente del alcance de "Apply to".
   Quitar candado solo cambia la protección: el par se conserva y no hay reinicio. */
function removeLocks() {
  const replica = replicaById(state.modal.target);
  const scope = state.modal.removeScope;
  const locked = assignmentTargets(scope, replica).filter((t) => t.locked);
  if (!locked.length) return; /* cero candidatas: no cambia nada */
  locked.forEach((t) => { t.locked = false; });
  state.modal.lastRemoval = { scope, count: locked.length };
  showToast(locked.length === 1
    ? "Lock removed — Coding Agent + Profile kept, no restart."
    : `${locked.length} locks removed — pairs kept, no restart.`, "info");
  render();
}

/* Apply: camino directo, o cartel de conflictos cuando un lote "+ lock" choca. */
function requestApply() {
  const replica = replicaById(state.modal.target);
  const scope = state.modal.scope;
  const targets = assignmentTargets(scope, replica);
  const conflicts = targets.filter((t) => t.locked);
  const bulkWithLock = scope !== "replica" && state.modal.withLock;
  if (bulkWithLock && conflicts.length > 0) {
    state.modal.conflict = { targets, conflicts, requested: { agent: state.modal.agent, profile: state.modal.profile } };
    render();
    return;
  }
  applyAssignment("direct");
}

function applyAssignment(mode = "direct") {
  const replica = replicaById(state.modal.target);
  const scope = state.modal.scope;
  const withLock = state.modal.withLock;
  const targets = assignmentTargets(scope, replica);
  let chosen;
  if (scope === "replica") chosen = targets;
  else if (!withLock) chosen = targets.filter((t) => !t.locked);
  else if (mode === "unlockedOnly") chosen = targets.filter((t) => !t.locked);
  else chosen = targets; /* direct (sin conflictos) o forceAll */
  const skipped = targets.filter((t) => !chosen.includes(t));
  const forced = chosen.filter((t) => t.locked);
  const rows = targets.map((t) => {
    const willWrite = chosen.includes(t);
    const wasLocked = t.locked;
    const rAgent = agentById(willWrite ? state.modal.agent : t.agent);
    const rProfile = willWrite ? state.modal.profile : t.profile;
    return {
      roomName: t.roomName,
      name: t.name,
      path: `${rAgent.label} · Profile ${rProfile}`,
      live: t.live || 0,
      stateLabel: !willWrite
        ? "Protected · skipped"
        : withLock
          ? (wasLocked ? "Overwritten + kept lock" : "Updated + locked")
          : "Updated",
      protected: !willWrite,
    };
  });
  chosen.forEach((t) => {
    t.agent = state.modal.agent;
    t.profile = state.modal.profile;
    if (withLock) t.locked = true;
  });
  const restarted = state.modal.restart ? chosen.reduce((n, t) => n + (t.live || 0), 0) : 0;
  state.modal.lastRemoval = null;
  state.modal.applied = {
    updated: chosen.length,
    protected: skipped.length,
    errors: 0,
    restarted,
    locked: withLock ? chosen.length : 0,
    forced: forced.length,
    withLock,
    mode,
    rows,
  };
  state.modal.arm = false;
  state.modal.conflict = null;
  render();
}

/* Cartel de conflictos: lista las bloqueadas (Now vs Requested) y las tres salidas. */
function renderConflict() {
  const host = $("#lockConflict");
  const data = state.modal.conflict;
  if (!data || !state.modal.open) {
    host.hidden = true;
    host.innerHTML = "";
    return;
  }
  const requested = agentById(data.requested.agent);
  const total = data.targets.length;
  const conflicts = data.conflicts.length;
  const rows = data.conflicts
    .map((t) => {
      const now = agentById(t.agent);
      return `<div class="lock-conflict-row">
        <span class="lock-conflict-who"><span class="wg">${t.roomName}</span> · <span class="name">${t.name}</span></span>
        <span class="lock-conflict-now">Now: ${now.label} · Profile ${t.profile} <span class="selection-lock-pill locked">Protected</span></span>
        <span class="lock-conflict-arrow">&rarr;</span>
        <span class="lock-conflict-next">Requested: ${requested.label} · Profile ${data.requested.profile}</span>
      </div>`;
    })
    .join("");
  host.hidden = false;
  host.innerHTML = `
    <div class="lock-conflict-scrim"></div>
    <div class="lock-conflict-card" role="alertdialog" aria-modal="true" aria-labelledby="lockConflictTitle">
      <div class="lock-conflict-head">
        <div class="lock-conflict-title" id="lockConflictTitle">${conflicts} ${conflicts === 1 ? "replica is" : "replicas are"} already locked</div>
        <div class="lock-conflict-sub">${total} replica(s) in scope for <strong>+ lock</strong> with ${requested.label} · Profile ${data.requested.profile}. Choose how to treat ${conflicts === 1 ? "it" : "them"}.</div>
      </div>
      <div class="lock-conflict-list">${rows}</div>
      <div class="lock-conflict-effects">
        <div class="lock-conflict-effect"><strong>Cancel</strong><span>Nothing changes: no pair, no lock and no restart.</span></div>
        <div class="lock-conflict-effect"><strong>Apply only to unlocked</strong><span>${total - conflicts} updated + locked; ${conflicts} protected ${conflicts === 1 ? "stays" : "stay"} untouched.</span></div>
        <div class="lock-conflict-effect danger"><strong>Force all, including locked</strong><span>${total} updated + locked; the ${conflicts} protected ${conflicts === 1 ? "keeps its" : "keep their"} lock.</span></div>
      </div>
      <div class="lock-conflict-actions">
        <button type="button" class="modal-btn modal-btn-cancel" id="conflictCancel">Cancel</button>
        <button type="button" class="modal-btn modal-btn-save" id="conflictUnlockedOnly">Apply only to unlocked</button>
        <button type="button" class="modal-btn modal-btn-save danger" id="conflictForceAll">Force all, including locked</button>
      </div>
    </div>`;
}

/* ---------- anotaciones ------------------------------------------------------ */
function renderAnnotations() {
  const layer = $("#annLayer");
  layer.innerHTML = "";
  if (!state.annotations) return;
  const view = VIEWS.find((v) => v.id === state.view);
  if (!view) return;
  const winRect = $("#acWindow").getBoundingClientRect();
  (view.annots || []).forEach((a) => {
    const el = document.querySelector(a.find);
    if (!el) return;
    const r = el.getBoundingClientRect();
    const pad = a.pad ?? 3;
    const left = r.left - winRect.left - pad;
    const top = r.top - winRect.top - pad;
    const width = r.width + pad * 2;
    const height = r.height + pad * 2;
    const outline = document.createElement("div");
    outline.className = "demo-annot-outline";
    outline.style.left = `${left}px`;
    outline.style.top = `${top}px`;
    outline.style.width = `${width}px`;
    outline.style.height = `${height}px`;
    layer.appendChild(outline);
    const pin = document.createElement("div");
    pin.className = "demo-annot-pin";
    pin.textContent = String(a.n);
    const right = a.pin === "tr" || a.pin === "br";
    const bottom = a.pin === "bl" || a.pin === "br";
    pin.style.left = `${right ? left + width : left - 20}px`;
    pin.style.top = `${bottom ? top + height : top - 20}px`;
    layer.appendChild(pin);
  });
}

/* ---------- marco de demo ---------------------------------------------------- */
function renderTopbar() {
  $("#demoViews").innerHTML = VIEWS.map(
    (v) => `<button type="button" class="demo-viewbtn ${v.id === state.view ? "active" : ""}" data-view="${v.id}">${v.label}</button>`
  ).join("");
  $("#btnAnnotations").classList.toggle("off", !state.annotations);
  $("#btnAnnotations").textContent = `Anotaciones: ${state.annotations ? "sí" : "no"}`;
}

function renderSidePanel() {
  const view = VIEWS.find((v) => v.id === state.view);
  const idx = VIEWS.findIndex((v) => v.id === state.view) + 1;
  $("#explainTitle").textContent = view.title;
  $("#explainSub").textContent = view.sub;
  $("#explainList").innerHTML = view.notes
    .map(([t, body], i) => `<li><span class="num">${i + 1}</span><span><strong>${t}</strong>. ${body}</span></li>`)
    .join("");
  $("#demoCounter").textContent = `Vista ${idx} de ${VIEWS.length}`;
  $("#demoPrev").disabled = idx === 1;
  $("#demoNext").disabled = idx === VIEWS.length;
}

function showToast(message, kind) {
  const toast = $("#demoToast");
  toast.className = `toast-item toast-item--${kind || "info"}`;
  toast.querySelector(".toast-item__message").textContent = message;
  toast.hidden = false;
  clearTimeout(showToast.timer);
  showToast.timer = setTimeout(() => { toast.hidden = true; }, 3200);
}

function render() {
  renderTopbar();
  renderSidebar();
  renderContextMenu();
  renderModal();
  renderSidePanel();
  requestAnimationFrame(renderAnnotations);
}

function hideToast() {
  const toast = $("#demoToast");
  toast.hidden = true;
  clearTimeout(showToast.timer);
}

function setView(id) {
  const view = VIEWS.find((v) => v.id === id);
  if (!view) return;
  hideToast();
  state.world = initialWorld();
  state.modal.applied = null;
  state.modal.conflict = null;
  state.view = id;
  view.setup();
  render();
}

/* ---------- eventos ---------------------------------------------------------- */
function wire() {
  $("#demoViews").addEventListener("click", (e) => {
    const btn = e.target.closest("[data-view]");
    if (btn) setView(btn.dataset.view);
  });
  $("#btnAnnotations").onclick = () => {
    state.annotations = !state.annotations;
    render();
  };
  $("#btnReset").onclick = () => {
    state.world = initialWorld();
    setView("entrada");
  };
  $("#demoPrev").onclick = () => {
    const i = VIEWS.findIndex((v) => v.id === state.view);
    if (i > 0) setView(VIEWS[i - 1].id);
  };
  $("#demoNext").onclick = () => {
    const i = VIEWS.findIndex((v) => v.id === state.view);
    if (i < VIEWS.length - 1) setView(VIEWS[i + 1].id);
  };

  $("#sidebarRoot").addEventListener("contextmenu", (e) => {
    const row = e.target.closest(".replica-item");
    e.preventDefault();
    if (!row) {
      state.ctx.open = false;
      render();
      return;
    }
    const win = $("#acWindow").getBoundingClientRect();
    state.ctx = { open: true, replica: row.dataset.id, x: e.clientX - win.left, y: e.clientY - win.top };
    render();
  });
  $("#sidebarRoot").addEventListener("click", (e) => {
    if (state.ctx.open) { state.ctx.open = false; render(); }
    if (e.target.closest(".replica-item")) setView("entrada");
  });

  $("#modalOverlay").addEventListener("click", (e) => {
    if (e.target === $("#modalOverlay")) { closeModal(); render(); }
  });
  $("#mpProviders").addEventListener("click", (e) => {
    const card = e.target.closest("[data-agent]");
    if (!card) return;
    state.modal.agent = card.dataset.agent;
    state.modal.applied = null;
    state.modal.lastRemoval = null;
    render();
  });
  $("#mpProfiles").addEventListener("click", (e) => {
    const card = e.target.closest("[data-profile]");
    if (!card) return;
    state.modal.profile = card.dataset.profile;
    state.modal.applied = null;
    state.modal.lastRemoval = null;
    render();
  });
  $("#mpLockBar").addEventListener("click", (e) => {
    if (e.target.id === "lockRemoveBtn") removeLocks();
  });
  $("#mpLockBar").addEventListener("change", (e) => {
    const choice = e.target.closest("[name=removeScope]");
    if (!choice) return;
    state.modal.removeScope = choice.value;
    state.modal.lastRemoval = null;
    render();
  });
  $("#mpBotonera").addEventListener("change", (e) => {
    if (e.target.id === "restartToggle") { state.modal.restart = e.target.checked; render(); return; }
    if (e.target.id === "armToggle") { state.modal.arm = e.target.checked; render(); return; }
    const choice = e.target.closest("[name=assignChoice]");
    if (choice) {
      const [nextScope, mode] = choice.value.split("+");
      state.modal.scope = nextScope;
      state.modal.withLock = mode === "lock";
      state.modal.arm = false;
      state.modal.applied = null;
      state.modal.lastRemoval = null;
      render();
    }
  });
  $("#mpBotonera").addEventListener("click", (e) => {
    if (e.target.id === "mpCancel") { closeModal(); render(); }
    if (e.target.id === "mpApply") requestApply();
  });
  $("#lockConflict").addEventListener("click", (e) => {
    if (e.target.id === "conflictCancel") { state.modal.conflict = null; render(); return; }
    if (e.target.id === "conflictUnlockedOnly") { applyAssignment("unlockedOnly"); return; }
    if (e.target.id === "conflictForceAll") { applyAssignment("forceAll"); }
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (state.modal.conflict) { state.modal.conflict = null; render(); return; }
    if (state.ctx.open) { state.ctx.open = false; render(); return; }
    if (state.modal.open) { closeModal(); render(); }
  });
  window.addEventListener("resize", renderAnnotations);
}

wire();
/* Permite abrir una vista concreta para capturas: index-v2.html?vista=conflicto
también acepta #vista=tipo-preview. */
const params = new URLSearchParams(location.search);
const hashParams = new URLSearchParams(location.hash.replace(/^#/, ""));
const initial = params.get("vista") || hashParams.get("vista") || "entrada";
setView(VIEWS.some((v) => v.id === initial) ? initial : "entrada");
