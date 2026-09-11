/* ============================================================================
   Prototipo de candados — lógica de la maqueta.
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
    scope: "replica",           // replica | kind | workgroup  (asignación de par)
    lockScope: "replica",       // replica | kind             (alcance del candado)
    lockDraft: false,
    restart: true,
    arm: false,
    applied: null,
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
    id: "replica-abierto",
    label: "2 · Candado abierto",
    title: "Réplica concreta — candado abierto",
    sub: "El modal real no cambia: se agrega una barra de candado entre el encabezado y los tres paneles.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      state.modal.lockDraft = replicaById("r12-ui").locked;
      state.modal.lockScope = "replica";
      state.modal.scope = "replica";
      state.modal.applied = null;
    },
    notes: [
      ["Par seleccionado", "El candado se define sobre el par elegido: <em>Coding Agent + Profile</em>. Aquí <strong>Codex · Profile A</strong>."],
      ["Estado abierto", "Sin candado, el cambio masivo puede escribir este par. El interruptor <strong>Keep across bulk changes</strong> lo activa."],
      ["Solo cambia la barra", "Los pasos 1 y 2, la comparación y la botonera de alcance siguen exactamente como hoy."],
    ],
    annots: [
      { find: ".selection-lock-bar", pad: 4, pin: "tl", n: 1 },
      { find: ".selection-lock-switch", pad: 4, pin: "tr", n: 2 },
      { find: ".selection-lock-pair", pad: 3, pin: "bl", n: 3 },
    ],
  },
  {
    id: "replica-cerrado",
    label: "3 · Candado cerrado",
    title: "Réplica concreta — candado cerrado",
    sub: "Activar materializa el par y lo protege de asignaciones masivas. Un cambio individual deliberado sigue permitido y conserva el candado.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      const r = replicaById("r12-ui");
      r.locked = true;
      state.modal.lockDraft = true;
      state.modal.lockScope = "replica";
      state.modal.scope = "replica";
      state.modal.applied = null;
    },
    notes: [
      ["Estado protegido", "El chip pasa a <strong>Protected</strong> y el par queda escrito junto al candado."],
      ["Cambio individual permitido", "Puede cambiar el par a mano; el candado se conserva. El candado no prohíbe la edición deliberada."],
      ["Quitar candado", "El botón <strong>Remove lock</strong> aparece solo con el candado activo y nunca está deshabilitado; al pulsarlo el par <em>no cambia</em>."],
    ],
    annots: [
      { find: ".selection-lock-state", pad: 4, pin: "tl", n: 1 },
      { find: ".selection-lock-hint", pad: 3, pin: "bl", n: 2 },
      { find: ".selection-lock-remove", pad: 4, pin: "tr", n: 3 },
    ],
  },
  {
    id: "tipo-preview",
    label: "4 · Por tipo: preview",
    title: "Por tipo de Matrix — quién entra y quién queda fuera",
    sub: "Alcance masivo: se enumeran las réplicas del mismo tipo (misma Matrix origen). Las protegidas se omiten, incluido su reinicio.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r12-ui").locked = true;
      state.modal.lockDraft = true;
      state.modal.lockScope = "kind";
      state.modal.scope = "kind";
      state.modal.arm = true;
      state.modal.restart = true;
      state.modal.applied = null;
    },
    notes: [
      ["Dos alcances, dos operaciones", "<strong>Set lock for</strong> canda: no cambia ninguna selección. <strong>Apply to</strong> cambia el par. El candado por tipo no copia el par de una réplica a otra."],
      ["Cada réplica conserva su par", "La lista muestra el par propio de cada réplica del tipo. Las píldoras <em>Protected / Not protected</em> son el estado actual, no una acción pendiente."],
      ["Una sola acción explícita", "El botón dice exactamente qué va a pasar: <strong>Lock 1 remaining replica</strong> y después <strong>Remove lock from 2 replicas</strong>. Por eso este alcance no muestra el <em>Remove lock</em> de una sola réplica."],
      ["Preview masivo con omitidas", "En <strong>Apply to → All replicas of this kind</strong> la protegida se marca <em>Protected · skipped</em>: no se escribe ni se reinicia, y el resumen separa elegibles de protegidas."],
      ["Apply se habilita con la confirmación", "El botón <strong>Apply</strong> queda deshabilitado hasta marcar la confirmación de sobrescritura; esa es la mecánica real del modal."],
    ],
    annots: [
      { find: ".selection-lock-scope", pad: 4, pin: "tl", n: 1 },
      { find: ".selection-lock-kind", pad: 3, pin: "bl", n: 2 },
      { find: "#lockKindAction", pad: 4, pin: "tr", n: 3 },
      { find: "#mpTargets", pad: 3, pin: "br", n: 4 },
      { find: ".agent-picker-apply", pad: 4, pin: "tr", n: 5 },
    ],
  },
  {
    id: "resultado",
    label: "5 · Resultado",
    title: "Resultado del cambio masivo",
    sub: "El conteo separa actualizadas, protegidas y errores. Las protegidas no se reinician.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      replicaById("r12-ui").locked = true;
      state.modal.lockDraft = true;
      state.modal.lockScope = "kind";
      state.modal.scope = "kind";
      state.modal.arm = true;
      state.modal.restart = true;
      applyAssignment();
      showToast("1 updated · 1 protected · 0 errors — protected replicas were not written or restarted.", "success");
    },
    notes: [
      ["Conteo explícito", "<strong>1 updated · 1 protected · 0 errors</strong>. Nunca se mezcla una protegida con una escritura fallida."],
      ["Sin reinicio de protegidas", "Solo se reinician sesiones de destinos elegibles; la protegida queda intacta."],
      ["Candado preservado", "La réplica protegida conserva su par y su candado; la réplica actualizada recibe el par nuevo sin candado (ver el sidebar detrás)."],
    ],
    annots: [
      { find: ".agent-scope-result", pad: 4, pin: "tr", n: 1 },
      { find: ".agent-scope-lock-summary", pad: 3, pin: "bl", n: 2 },
      { find: "#demoToast", pad: 4, pin: "tr", n: 3 },
    ],
  },
  {
    id: "futuro",
    label: "6 · Futuras réplicas",
    title: "Default para futuras réplicas",
    sub: "Alcance confirmado por el usuario: las réplicas que se creen en futuras rooms heredan el candado del tipo, con excepción por réplica y sin propagación retroactiva.",
    setup() {
      state.ctx.open = false;
      openModal("r12-ui");
      state.modal.lockDraft = replicaById("r12-ui").locked;
      state.modal.lockScope = "replica";
      state.modal.scope = "replica";
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
  state.modal.lockDraft = replica.locked;
  state.modal.scope = "replica";
  state.modal.lockScope = "replica";
  state.modal.arm = false;
  state.modal.applied = null;
  state.modal.futureOpen = false;
  $("#modalOverlay").hidden = false;
}

function closeModal() {
  state.modal.open = false;
  $("#modalOverlay").hidden = true;
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
  const kind = replica.kind;
  const kindTargets = kindReplicas(kind);
  const kindTotal = kindTargets.length;
  const kindLocked = kindTargets.filter((r) => r.locked).length;
  const kindRooms = new Set(kindTargets.map((r) => r.roomId)).size;
  const locked = state.modal.lockDraft;
  const kindScope = state.modal.lockScope === "kind";

  /* El chip de estado describe SIEMPRE el alcance elegido en "Set lock for". */
  const stateChip = kindScope
    ? `<span class="selection-lock-state" data-state="${kindLocked > 0 ? "locked" : "open"}">${kindLocked} of ${kindTotal} protected</span>`
    : `<span class="selection-lock-state" data-state="${locked ? "locked" : "open"}">${locked ? "Protected" : "Unlocked"}</span>`;

  const hint = kindScope
    ? "Locks only: each replica keeps its own pair. Apply to changes the selection."
    : locked
      ? "Skipped by bulk assignments and their restarts. Individual changes keep the lock."
      : "Bulk assignments may overwrite this pair. Turn the lock on to keep it.";

  let kindBlock = "";
  if (kindScope) {
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
        <div class="selection-lock-kind-note">Apply to changes the pairing; this section only sets locks. New replicas inherit this Matrix default.</div>
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

  /* Una sola acción explícita para el tipo: su rótulo coincide con el efecto. */
  const remaining = kindTotal - kindLocked;
  const kindActionLabel = kindLocked === kindTotal
    ? `Remove lock from ${kindTotal} ${kindTotal === 1 ? "replica" : "replicas"}`
    : kindLocked === 0
      ? `Lock ${kindTotal} ${kindTotal === 1 ? "replica" : "replicas"}`
      : `Lock ${remaining} remaining replica${remaining === 1 ? "" : "s"}`;
  const kindActionClass = kindLocked === kindTotal ? "remove" : "";

  const replicaControls = `
    <label class="selection-lock-switch">
      <input type="checkbox" id="lockToggle" ${locked ? "checked" : ""}> Keep across bulk changes
    </label>
    ${locked ? '<button type="button" class="selection-lock-remove" id="lockRemove">Remove lock</button>' : ""}`;

  const kindControls = `<button type="button" class="selection-lock-action ${kindActionClass}" id="lockKindAction">${kindActionLabel}</button>`;

  $("#mpLockBar").innerHTML = `
    <div class="selection-lock-bar" data-state="${(kindScope ? kindLocked > 0 : locked) ? "locked" : "open"}">
      <div class="selection-lock-icon">${lockIcon()}</div>
      <div class="selection-lock-main">
        <div class="selection-lock-title">Selection lock ${stateChip}</div>
        <div class="selection-lock-pair">${agent.label} · Profile ${state.modal.profile}</div>
        <div class="selection-lock-hint">${hint}</div>
      </div>
      <div class="selection-lock-right">
        <div class="selection-lock-scope" role="radiogroup" aria-label="Set lock for">
          <span class="selection-lock-scope-label">Set lock for</span>
          <label class="selection-lock-scope-opt ${!kindScope ? "active" : ""}">
            <input type="radio" name="lockScope" value="replica" ${!kindScope ? "checked" : ""}> This replica
          </label>
          <label class="selection-lock-scope-opt ${kindScope ? "active" : ""}">
            <input type="radio" name="lockScope" value="kind" ${kindScope ? "checked" : ""}> All replicas of this kind
            <span class="selection-lock-scope-count">${kindTotal} · ${kindRooms} room(s)</span>
          </label>
        </div>
        <div class="selection-lock-actionrow">${kindScope ? kindControls : replicaControls}</div>
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
  const targets = assignmentTargets(scope, replica);
  const eligible = targets.filter((t) => (scope === "replica" ? true : !t.locked));
  const skipped = targets.filter((t) => !eligible.includes(t));
  const rooms = new Set(targets.map((t) => t.roomId)).size;
  const liveEligible = eligible.reduce((n, t) => n + (t.live || 0), 0);

  const scopePicker = `
    <div class="agent-scope-picker" role="radiogroup" aria-label="Apply scope">
      <span class="agent-scope-label">Apply to</span>
      <label class="agent-scope-opt ${scope === "replica" ? "active" : ""}"><input type="radio" name="assignScope" value="replica" ${scope === "replica" ? "checked" : ""}> This replica <span class="agent-scope-count">1 replica</span></label>
      <label class="agent-scope-opt ${scope === "kind" ? "active dangerous" : ""}"><input type="radio" name="assignScope" value="kind" ${scope === "kind" ? "checked" : ""}> All replicas of this kind <span class="agent-scope-count">${kindReplicas(replica.kind).length} replicas</span></label>
      <label class="agent-scope-opt ${scope === "workgroup" ? "active dangerous" : ""}"><input type="radio" name="assignScope" value="workgroup" ${scope === "workgroup" ? "checked" : ""}> Entire room <span class="agent-scope-count">${roomOf(replica).replicas.length} replicas</span></label>
    </div>
    <div class="agent-scope-live-note"><span class="agent-scope-live-tag">live</span> Counts are read from the current room; the backend re-enumerates targets before applying.</div>`;

  let targetList = "";
  if (scope !== "replica") {
    targetList = `
      <div class="agent-scope-targets" data-ac-role="list" id="mpTargets">
        <div class="agent-scope-targets-head">${eligible.length} to update · ${skipped.length} protected (skipped) · ${targets.length} replica(s) across ${rooms} room(s) · ${liveEligible} live session(s)</div>
        <div class="agent-scope-lock-summary">
          <span class="eligible">${eligible.length} will update</span>
          <span class="protected">${skipped.length} protected · skipped, not restarted</span>
          <span class="errors">0 errors</span>
        </div>
        ${targets
          .map((t) => {
            const ineligible = skipped.includes(t);
            const rAgent = agentById(t.agent);
            return `
          <div class="agent-scope-target-row ${ineligible ? "protected" : ""}">
            <span class="agent-scope-target-wg">${t.roomName}</span>
            <span class="agent-scope-target-name">${t.name}</span>
            <span class="agent-scope-target-path">${rAgent.label} · Profile ${t.profile}</span>
            ${t.live ? `<span class="agent-scope-target-live">${t.live} live</span>` : ""}
            ${ineligible ? '<span class="agent-scope-target-state protected">Protected · skipped</span>' : '<span class="agent-scope-target-state eligible">Will update</span>'}
          </div>`;
          })
          .join("")}
      </div>`;
  }

  const result = state.modal.applied
    ? `<div class="agent-scope-result">
        <div><strong>${state.modal.applied.updated} updated · ${state.modal.applied.protected} protected · ${state.modal.applied.errors} errors</strong></div>
        <div class="protected-note">Protected replicas were not written and not restarted.</div>
        <div>${state.modal.applied.restarted} eligible session(s) restarted.</div>
      </div>`
    : "";

  const applyDisabled = scope === "replica" ? false : !state.modal.arm;
  const applyLabel = scope === "replica" ? "Assign to this replica" : scope === "kind" ? `Overwrite ${eligible.length} of this kind` : `Overwrite ${eligible.length} in this room`;
  const noun = (n) => (n === 1 ? "replica" : "replicas");
  const armLabel = scope === "kind"
    ? `I understand this overwrites ${eligible.length} ${noun(eligible.length)} of this kind (${skipped.length} protected is skipped)`
    : `I understand this overwrites ${eligible.length} ${noun(eligible.length)} (${skipped.length} protected is skipped)`;

  $("#mpBotonera").innerHTML = `
    ${scopePicker}
    ${targetList}
    ${result}
    <div class="agent-picker-bar">
      ${scope !== "replica" ? `<label class="agent-scope-switch"><input type="checkbox" id="restartToggle" ${state.modal.restart ? "checked" : ""}> Restart sessions after apply</label>` : ""}
      <div class="agent-picker-bar-spacer"></div>
      ${scope !== "replica" ? `<label class="agent-scope-arm"><input type="checkbox" id="armToggle" ${state.modal.arm ? "checked" : ""}> ${armLabel}</label>` : ""}
      <button type="button" class="modal-btn modal-btn-cancel" id="mpCancel">Cancel</button>
      <button type="button" class="modal-btn modal-btn-save agent-picker-apply ${scope !== "replica" ? "danger" : ""}" id="mpApply" ${applyDisabled ? "disabled" : ""}>${applyLabel}</button>
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
}

/* ---------- acciones -------------------------------------------------------- */
function setLock(next) {
  const replica = replicaById(state.modal.target);
  state.modal.lockDraft = next;
  replica.locked = next;
  showToast(next
    ? "Selection lock set — this pair is skipped by bulk changes."
    : "Lock removed — Coding Agent + Profile kept.", "info");
  render();
}

/* Acción explícita del candado por tipo: un solo botón, rótulo = efecto. */
function setKindLock() {
  const replica = replicaById(state.modal.target);
  const targets = kindReplicas(replica.kind);
  const allLocked = targets.every((t) => t.locked);
  const changed = allLocked ? targets.length : targets.filter((t) => !t.locked).length;
  targets.forEach((t) => { t.locked = !allLocked; });
  showToast(allLocked
    ? `${changed} ${changed === 1 ? "lock" : "locks"} removed — selections kept.`
    : `${changed} selection ${changed === 1 ? "lock" : "locks"} set — each replica kept its own pair.`, "info");
  render();
}

function applyAssignment() {
  const replica = replicaById(state.modal.target);
  const scope = state.modal.scope;
  const targets = assignmentTargets(scope, replica);
  const eligible = scope === "replica" ? targets : targets.filter((t) => !t.locked);
  const skipped = targets.filter((t) => !eligible.includes(t));
  eligible.forEach((t) => {
    t.agent = state.modal.agent;
    t.profile = state.modal.profile;
  });
  const restarted = state.modal.restart ? eligible.reduce((n, t) => n + (t.live || 0), 0) : 0;
  state.modal.applied = { updated: eligible.length, protected: skipped.length, errors: 0, restarted };
  state.modal.arm = false;
  render();
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
    render();
  });
  $("#mpProfiles").addEventListener("click", (e) => {
    const card = e.target.closest("[data-profile]");
    if (!card) return;
    state.modal.profile = card.dataset.profile;
    state.modal.applied = null;
    render();
  });
  $("#mpLockBar").addEventListener("change", (e) => {
    if (e.target.id === "lockToggle") setLock(e.target.checked);
    const scopeOpt = e.target.closest("[name=lockScope]");
    if (scopeOpt) { state.modal.lockScope = scopeOpt.value; render(); }
  });
  $("#mpLockBar").addEventListener("click", (e) => {
    if (e.target.id === "lockRemove") setLock(false);
    if (e.target.id === "lockKindAction") setKindLock();
  });
  $("#mpBotonera").addEventListener("change", (e) => {
    if (e.target.id === "restartToggle") { state.modal.restart = e.target.checked; render(); return; }
    if (e.target.id === "armToggle") { state.modal.arm = e.target.checked; render(); return; }
    const scope = e.target.closest("[name=assignScope]");
    if (scope) {
      state.modal.scope = scope.value;
      state.modal.arm = false;
      state.modal.applied = null;
      render();
    }
  });
  $("#mpBotonera").addEventListener("click", (e) => {
    if (e.target.id === "mpCancel") { closeModal(); render(); }
    if (e.target.id === "mpApply") applyAssignment();
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (state.ctx.open) { state.ctx.open = false; render(); return; }
    if (state.modal.open) { closeModal(); render(); }
  });
  window.addEventListener("resize", renderAnnotations);
}

wire();
/* Permite abrir una vista concreta para capturas: index.html?vista=tipo-preview
también acepta #vista=tipo-preview. */
const params = new URLSearchParams(location.search);
const hashParams = new URLSearchParams(location.hash.replace(/^#/, ""));
const initial = params.get("vista") || hashParams.get("vista") || "entrada";
setView(VIEWS.some((v) => v.id === initial) ? initial : "entrada");
