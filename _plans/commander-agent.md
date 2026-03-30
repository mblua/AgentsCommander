# Plan: Agents Commander — El Agente Central

**Branch**: `feature/commander-agent` (en `agentscommander_3`)
**Fecha**: 2026-03-29
**Estado**: En planificacion

---

## 1. Vision

Hoy "Agents Commander" es solo el nombre de la app. Este feature lo convierte en **el agente central**: un agente real de Claude Code que es la puerta de entrada entre el humano y todos los demas agentes.

El humano escribe prompts informales, vagos, en cualquier idioma al Commander. Commander los refina, determina que agente(s) deben ejecutar el trabajo, despacha via mensajeria inter-agentes, y presenta los resultados. Nuclea toda la interfaz humano-agentes en un unico punto.

En el futuro, Commander sera el agente conectado a Telegram para acceso movil — el unico punto de contacto necesario.

### Principios de diseno

1. **Commander es una sesion real de Claude Code** — no es una capa de UI sintetica. Tiene todas las capacidades de cualquier otro agente (shell, filesystem, send CLI)
2. **Master token** — Commander puede comunicarse con CUALQUIER agente sin restriccion de team
3. **Siempre presente** — no se puede cerrar, renombrar, ni reordenar. Es la primera sesion que aparece
4. **Acceso directo preservado** — los otros agentes siguen accesibles en el sidebar para debugging e interaccion directa

---

## 2. Arquitectura

### 2.1 Modelo de datos

```
Session (Rust struct)
  + is_commander: bool    // nuevo flag, serde(default)

SessionInfo (IPC struct)
  + is_commander: bool    // propagado desde Session

PersistedSession (persistence)
  + is_commander: bool    // para sobrevivir restarts

Session (TypeScript interface)
  + isCommander: boolean  // frontend
```

### 2.2 Working directory del Commander

```
~/.agentscommander/commander/          (PROD)
~/.agentscommander-dev/commander/      (DEV)
  |-- CLAUDE.md                        // instrucciones del Commander (regenerado al startup)
  |-- .agentscommander/
  |     |-- config.json                // config local del agente
  |     |-- inbox/                     // mensajes entrantes
  |     |-- outbox/                    // mensajes salientes
```

El directorio se crea automaticamente al primer startup. El CLAUDE.md se regenera en cada startup para reflejar el estado actual de teams/agentes.

### 2.3 Flujo de mensajeria

```
HUMANO escribe prompt al Commander (via xterm.js)
  |
  v
COMMANDER (Claude Code en PTY)
  |-- Refina el prompt
  |-- Determina agente(s) target
  |-- Ejecuta: agentscommander.exe send --token <MASTER_TOKEN> --to <agent> --message "..." --mode wake
  |
  v
MAILBOX POLLER (instancia PROD/DEV)
  |-- Valida master token (bypass team check)
  |-- Entrega al inbox del target o inyecta en PTY si idle
  |
  v
AGENTE TARGET procesa el trabajo
  |-- Ejecuta: agentscommander.exe send --to "Commander" --message "resultado..." --mode wake
  |
  v
MAILBOX POLLER
  |-- Regla especial: Commander siempre alcanzable (bypass team check para mensajes TO Commander)
  |-- Inyecta respuesta en PTY del Commander
  |
  v
COMMANDER recibe respuesta, la presenta al humano
```

### 2.4 Nombre del agente Commander en el sistema de routing

El nombre del Commander en el sistema de mensajeria se deriva del CWD path (ultimos 2 componentes):
- PROD: `.agentscommander/commander`
- DEV: `.agentscommander-dev/commander`

Para simplificar el reply de otros agentes, el Commander incluye instrucciones de reply explicitas en cada mensaje despachado con el nombre correcto pre-llenado.

Ademas, `can_communicate()` tiene una regla especial: **cualquier agente puede enviar mensajes TO Commander** sin necesitar estar en el mismo team.

---

## 3. Implementacion — Fase 1: MVP (Commander existe, pinneado, protegido)

### 3.1 Rust types: flag `is_commander`

**Archivo: `src-tauri/src/session/session.rs`**

```rust
// En Session struct:
#[serde(default)]
pub is_commander: bool,

// En SessionInfo struct:
#[serde(default)]
pub is_commander: bool,

// En From<&Session> for SessionInfo:
is_commander: s.is_commander,
```

**Archivo: `src-tauri/src/config/sessions_persistence.rs`**

```rust
// En PersistedSession struct:
#[serde(default)]
pub is_commander: bool,

// En snapshot_sessions(), dentro del map closure agregar:
is_commander: s.is_commander,  // s es SessionInfo que ya tiene el flag
```

**Archivo: `src/shared/types.ts`**

```typescript
export interface Session {
  // ... campos existentes ...
  isCommander: boolean;
}
```

**Archivo: `src/sidebar/stores/sessions.ts`** — `makeInactiveEntry()` (~linea 27)

```typescript
// Agregar al literal de Session:
isCommander: false,
```

### 3.2 SessionManager: crear, proteger, ordenar

**Archivo: `src-tauri/src/session/manager.rs`**

Nuevos metodos:

```rust
/// Retorna el ID de la sesion Commander, si existe.
pub async fn get_commander_id(&self) -> Option<Uuid> {
    let sessions = self.sessions.read().await;
    sessions.values().find(|s| s.is_commander).map(|s| s.id)
}

/// Marca una sesion como Commander.
pub async fn set_commander(&self, id: Uuid) -> Result<(), AppError> {
    let mut sessions = self.sessions.write().await;
    if let Some(session) = sessions.get_mut(&id) {
        session.is_commander = true;
        Ok(())
    } else {
        Err(AppError::SessionNotFound(id.to_string()))
    }
}
```

Modificaciones:

```rust
// En destroy_session():
if session.is_commander {
    return Err(AppError::Other("Cannot destroy the Commander session".into()));
}

// En rename_session():
if session.is_commander {
    return Err(AppError::Other("Cannot rename the Commander session".into()));
}

// En list_sessions():
// Actualmente el return es un .collect() directo. Cambiar a:
let mut result: Vec<SessionInfo> = order
    .iter()
    .filter_map(|id| sessions.get(id).map(SessionInfo::from))
    .collect();
result.sort_by(|a, b| b.is_commander.cmp(&a.is_commander));
result
// Esto pone is_commander=true primero (true > false)
```

### 3.3 Auto-creacion al startup

**Archivo: `src-tauri/src/lib.rs`** (en el bloque async de restore, despues de linea ~320 — DESPUES del restore loop, NO dentro)

```rust
// Despues del restore loop y persist:

// Ensure Commander session exists
let has_commander = {
    let mgr = session_mgr_clone.read().await;
    mgr.get_commander_id().await.is_some()
};

if !has_commander {
    // 1. Ensure commander directory exists
    let commander_dir = config::commander::ensure_commander_dir();

    // 2. Write/update CLAUDE.md
    let dark_factory = config::dark_factory::load_dark_factory();
    let settings = app_handle.state::<SettingsState>();
    let cfg = settings.read().await;
    config::commander::write_commander_claude_md(&dark_factory, &cfg, &commander_dir);
    drop(cfg);

    // 3. Determine Claude CLI path
    let shell = /* resolve from settings.agents or fallback to "claude" */;
    let shell_args = vec!["--dangerously-skip-permissions".to_string()];

    // 4. Create the session
    match commands::session::create_session_inner(
        &app_handle,
        &session_mgr_clone,
        &pty_mgr_clone,
        shell,
        shell_args,
        commander_dir.to_string_lossy().to_string(),
        Some("Commander".to_string()),
        None,
    ).await {
        Ok(info) => {
            // 5. Mark as commander
            let mgr = session_mgr_clone.read().await;
            if let Ok(uuid) = Uuid::parse_str(&info.id) {
                let _ = mgr.set_commander(uuid).await;
            }
            // 6. Switch to Commander
            // (only if no other session was wasActive)
        }
        Err(e) => log::error!("Failed to create Commander session: {}", e),
    }

    // 7. Re-persist with commander flag (usar persist_merging_failed para no perder entries fallidas)
    let mgr = session_mgr_clone.read().await;
    sessions_persistence::persist_merging_failed(&mgr, &failed_recoverable).await;
}
```

### 3.4 Proteger en commands

**Archivo: `src-tauri/src/commands/session.rs`**

```rust
// En destroy_session (Tauri command):
#[tauri::command]
pub async fn destroy_session(...) -> Result<(), String> {
    let mgr = session_mgr.read().await;
    if let Some(commander_id) = mgr.get_commander_id().await {
        if uuid == commander_id {
            return Err("Cannot close the Commander session".to_string());
        }
    }
    // ... resto del destroy ...
}

// Idem para rename_session
```

### 3.5 Frontend: Commander siempre primero

**Archivo: `src/sidebar/stores/sessions.ts`**

```typescript
// En filteredSessionsMemo:
const filteredSessionsMemo = createMemo(() => {
  // ... logica de filtrado existente ...

  // Commander siempre visible, siempre primero
  const commander = result.find(s => s.isCommander);
  const rest = result.filter(s => !s.isCommander);

  return commander ? [commander, ...rest] : rest;
});
```

### 3.6 Frontend: Visual treatment del Commander

**Archivo: `src/sidebar/components/SessionItem.tsx`**

```tsx
// Clase condicional en el root div:
<div class={`session-item ${isActive() ? 'active' : ''} ${props.session.isCommander ? 'commander' : ''}`}>

// Badge en lugar de status dot:
<Show when={props.session.isCommander} fallback={<div class={`session-item-status ${statusClass}`} />}>
  <div class="commander-badge" title="Commander">&#x2318;</div>
</Show>

// Ocultar botones de close, detach, rename:
<Show when={!props.session.isCommander}>
  {/* close button, detach button, etc. */}
</Show>

// Deshabilitar doble-click (abre OpenAgentModal, no rename — guard para Commander):
onDblClick={(e) => { if (!props.session.isCommander) handleDoubleClick(); }}
```

### 3.7 CSS del Commander

**Archivo: `src/sidebar/styles/sidebar.css`**

```css
/* Commander session - visually distinct */
.session-item.commander {
  border-left: 3px solid var(--commander-accent, #f59e0b);
  background: rgba(245, 158, 11, 0.04);
  margin-bottom: 6px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

.session-item.commander .session-item-name {
  font-weight: 700;
  letter-spacing: 0.5px;
  text-transform: uppercase;
  font-size: 11px;
  color: var(--commander-accent, #f59e0b);
}

.commander-badge {
  width: 14px;
  height: 14px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 12px;
  color: var(--commander-accent, #f59e0b);
  flex-shrink: 0;
}
```

---

## 4. Implementacion — Fase 2: Commander operativo (CLAUDE.md + master token)

### 4.1 Nuevo modulo: `commander.rs`

**Nuevo archivo: `src-tauri/src/config/commander.rs`**

```rust
use std::path::PathBuf;
use crate::config::dark_factory::DarkFactoryConfig;
use crate::config::settings::AppSettings;

/// Retorna el path del working directory del Commander.
pub fn commander_cwd() -> PathBuf {
    let base = super::config_dir().expect("Cannot determine config dir");
    base.join("commander")
}

/// Crea el directorio del Commander y su estructura .agentscommander/.
pub fn ensure_commander_dir() -> PathBuf {
    let dir = commander_cwd();
    let ac_dir = dir.join(".agentscommander");
    std::fs::create_dir_all(ac_dir.join("inbox")).ok();
    std::fs::create_dir_all(ac_dir.join("outbox")).ok();
    dir
}

/// Genera y escribe el CLAUDE.md del Commander.
/// Se regenera en cada startup para reflejar el estado actual.
pub fn write_commander_claude_md(
    config: &DarkFactoryConfig,
    settings: &AppSettings,
    commander_dir: &PathBuf,
) {
    let mut md = String::new();

    // Header
    md.push_str("# Commander — Agents Commander\n\n");
    md.push_str("## Rol\n\n");
    md.push_str("Sos el Commander: el agente central entre el humano y todos los demas agentes.\n");
    md.push_str("Tu funcion principal es recibir pedidos del humano, refinarlos, y despacharlos al agente correcto.\n\n");

    // Responsabilidades
    md.push_str("## Responsabilidades\n\n");
    md.push_str("1. **Recibir prompts** del humano (pueden ser informales, vagos, en espanol o ingles)\n");
    md.push_str("2. **Refinar** el prompt: clarificar, estructurar, agregar contexto relevante del proyecto\n");
    md.push_str("3. **Determinar** que agente(s) deben ejecutar el trabajo\n");
    md.push_str("4. **Despachar** via mensajeria inter-agentes usando el comando `send`\n");
    md.push_str("5. **Recibir respuestas** de los agentes y presentarlas al humano\n");
    md.push_str("6. **Nunca ejecutar trabajo tecnico directamente** — siempre delegar a agentes especializados\n\n");

    // Agentes disponibles (dinamico)
    md.push_str("## Agentes disponibles\n\n");
    for team in &config.teams {
        md.push_str(&format!("### Team: {}\n\n", team.name));
        for member in &team.members {
            md.push_str(&format!("- **{}** — `{}`\n", member.name, member.path));
        }
        md.push_str("\n");
    }

    // Instrucciones de messaging (el token se inyecta via init prompt, no en CLAUDE.md)
    md.push_str("## Como despachar trabajo\n\n");
    md.push_str("Usa el comando `send` con tu token (inyectado al inicio de la sesion):\n\n");
    md.push_str("```bash\n");
    md.push_str("agentscommander.exe send --token <TU_TOKEN> --root \"<TU_CWD>\" --to \"<nombre_agente>\" --message \"prompt refinado\" --mode wake\n");
    md.push_str("```\n\n");
    md.push_str("- `--mode wake`: inyecta directo si el agente esta idle, o encola si esta busy\n");
    md.push_str("- El agente respondera a tu terminal cuando termine\n\n");

    // Instrucciones de refinamiento
    md.push_str("## Como refinar prompts\n\n");
    md.push_str("Cuando el humano escribe algo vago como \"arreglame el bug del login\":\n");
    md.push_str("1. Preguntale al humano que detalle mas si es realmente ambiguo\n");
    md.push_str("2. Si podes inferir el contexto (por los proyectos activos), estructura el prompt\n");
    md.push_str("3. Incluye el contexto relevante: branch actual, ultimo commit, archivos involucrados\n");
    md.push_str("4. Envia un prompt claro y accionable al agente target\n\n");

    // Como recibir respuestas
    md.push_str("## Como recibir respuestas\n\n");
    md.push_str("Los agentes responden via `send --to \"Commander\"`. El mensaje aparece en tu terminal.\n");
    md.push_str("Cuando recibas una respuesta:\n");
    md.push_str("1. Resume el resultado para el humano\n");
    md.push_str("2. Si hubo errores, sugiere como proceder\n");
    md.push_str("3. Si el trabajo requiere follow-up, coordina el siguiente paso\n\n");

    // Reglas
    md.push_str("## Reglas\n\n");
    md.push_str("- SIEMPRE verificar nombres de agente con `list-peers` antes de enviar\n");
    md.push_str("- NUNCA ejecutar codigo o builds directamente — delegar a agentes\n");
    md.push_str("- SIEMPRE informar al humano que se despacho y a quien\n");
    md.push_str("- Si no hay agente adecuado para una tarea, decirlo al humano\n");
    md.push_str("- Responder en el mismo idioma que el humano uso\n");

    let claude_md_path = commander_dir.join("CLAUDE.md");
    if let Err(e) = std::fs::write(&claude_md_path, &md) {
        log::error!("Failed to write Commander CLAUDE.md: {}", e);
    }
}
```

**Archivo: `src-tauri/src/config/mod.rs`**
```rust
pub mod commander;  // agregar
```

### 4.2 Init prompt con master token

**Archivo: `src-tauri/src/commands/session.rs`** (bloque async de init prompt, ~linea 74)

```rust
// Dentro del bloque async que inyecta el init prompt:

// Determinar si es Commander para usar master token
let token_to_inject = if is_commander_session {
    let master = app_clone.state::<MasterToken>();
    master.value()  // retorna &str con el token
} else {
    token.to_string()
};

// El init prompt usa token_to_inject en lugar de token directamente
```

### 4.3 Routing: Commander siempre alcanzable

**Archivo: `src-tauri/src/phone/manager.rs`** — `can_communicate()`

```rust
pub fn can_communicate(from: &str, to: &str, config: &DarkFactoryConfig) -> bool {
    // Commander es siempre alcanzable por cualquier agente
    if to.ends_with("/commander") || to == "Commander" {
        return true;
    }

    // ... resto de la logica existente (team check, coordinator links) ...
}
```

---

## 5. Implementacion — Fase 3: Contenido del CLAUDE.md del Commander

El CLAUDE.md generado en `write_commander_claude_md()` (Fase 2) es funcional pero basico. En esta fase se enriquece con:

### 5.1 Contexto del humano

Secciones adicionales que se agregan al CLAUDE.md:

```markdown
## Contexto del humano

### Proyectos activos
[Lista de todos los repo_paths de settings.json con descripcion inferida]

### Sesiones activas
[Lista de sesiones actuales con nombre, CWD, y estado]

### Preferencias
- Idioma principal: espanol
- Estilo de comunicacion: directo, conciso
- [Se enriquece con el tiempo via memory del Commander]
```

### 5.2 Estrategias de routing

```markdown
## Estrategias de routing

Mapeo de tipos de trabajo a agentes:

| Tipo de trabajo | Agente recomendado | Razon |
|---|---|---|
| Codigo en agentscommander | 0_repos/agentscommander_N | N = instancia disponible |
| Ship/Release | Agents/Shipper | Agente especializado en build+release |
| Investigacion tecnica | claude-code-expert | Conoce Claude Code en profundidad |
| Community/Marketing | ac-community-manager | Maneja presencia publica |

Si no hay match claro, preguntar al humano.
```

### 5.3 Templates de despacho

```markdown
## Templates de mensaje

Cuando despachas trabajo, estructura el mensaje asi:

## [Tipo: Feature/Bug/Ship/Research]

**Contexto**: [que necesita saber el agente]
**Tarea**: [que debe hacer, paso a paso]
**Criterio de exito**: [como saber que termino]
**Responder a**: Commander

---
```

---

## 6. Implementacion — Fase 4: Polish y Telegram (futuro)

### 6.1 Telegram bridge auto-attach

Al crear el Commander, si hay un Telegram bot configurado en settings, auto-attach al Commander:

```rust
// En la creacion del Commander (lib.rs):
if let Some(bot) = settings.telegram_bots.first() {
    telegram_bridge.attach(commander_session_id, bot.clone());
}
```

Esto hace que mensajes de Telegram lleguen al Commander, quien los refina y despacha.

### 6.2 Dashboard de despachos

Nuevo componente en el sidebar o en la terminal window que muestra:
- Mensajes enviados por Commander (timestamp, destino, estado)
- Respuestas recibidas
- Mensajes pendientes

### 6.3 Commander mode en sidebar

Cuando Commander esta procesando (status=Running), highlight visual en toda la app:
- Borde amber pulsante en el sidebar
- Indicador "Commander working..." en el status bar

---

## 7. Secuencia de implementacion

| Paso | Componente | Descripcion | Dependencias |
|------|-----------|-------------|-------------|
| 1 | Rust types | `is_commander` en Session/SessionInfo/PersistedSession | Ninguna |
| 2 | TS types | `isCommander` en Session interface + `makeInactiveEntry()` | Paso 1 |
| 3 | SessionManager | get_commander_id, set_commander, protect destroy/rename, sort | Paso 1 |
| 4 | config/commander.rs | Nuevo modulo: ensure_dir, write_claude_md | Paso 1 |
| 5 | config/mod.rs | Registrar modulo commander | Paso 4 |
| 6 | lib.rs | Auto-create Commander al startup | Pasos 3, 4 |
| 7 | sessions_persistence | Persist/restore is_commander flag | Paso 1 |
| 8 | commands/session.rs | Protect destroy/rename del Commander | Paso 3 |
| 9 | Frontend store | Commander-first sorting, filter exemption | Paso 2 |
| 10 | SessionItem.tsx | Visual treatment, button guards | Paso 2 |
| 11 | sidebar.css | Commander styling (amber accent, uppercase) | Paso 10 |
| 12 | commands/session.rs | Master token en init prompt del Commander | Paso 6 |
| 13 | phone/manager.rs | Commander always-reachable rule | Paso 6 |
| 14 | commander.rs | CLAUDE.md content enriquecido (Fase 3) | Paso 12 |

**MVP funcional**: Pasos 1-11 (~Commander existe y se ve)
**Operativo**: Pasos 12-14 (~Commander puede despachar y recibir)

---

## 8. Verificacion

### 8.1 Build checks

```bash
cd src-tauri && cargo check        # Rust compila
npx tsc --noEmit                   # TypeScript compila
```

### 8.2 Funcional MVP

- [ ] App inicia con Commander como primera sesion
- [ ] Commander no se puede cerrar (boton oculto, command rechazado)
- [ ] Commander no se puede renombrar (doble-click no activa rename)
- [ ] Commander tiene visual distinto (amber accent, uppercase, badge)
- [ ] Commander sobrevive restart de la app (persistence)
- [ ] Commander no se filtra al cambiar team filter
- [ ] Otros agentes siguen funcionando normalmente debajo del Commander

### 8.3 Funcional operativo

- [ ] Commander tiene master token en su init prompt
- [ ] Commander puede enviar mensajes a cualquier agente via `send`
- [ ] Cualquier agente puede responder al Commander (bypass team check)
- [ ] CLAUDE.md del Commander lista todos los agentes disponibles
- [ ] Commander refina prompts antes de despachar

---

## 9. Riesgos y mitigaciones

| Riesgo | Impacto | Mitigacion |
|--------|---------|-----------|
| Claude CLI no instalado | Commander no arranca | Fallback a powershell + warning visible |
| Nombre de agente del Commander es raro (`.agentscommander-dev/commander`) | Otros agentes no saben como responder | Incluir instrucciones de reply explicitas en cada despacho |
| CLAUDE.md muy largo con muchos agentes | Context window del Commander saturado | Mantener conciso, solo nombres y paths |
| Session duplicada del Commander en sessions.json | Dos Commanders (caos) | Dedup existente + get_commander_id verifica unicidad |
| Master token expuesto en PTY | Seguridad | El master token ya se inyecta via init prompt, mismo riesgo que session tokens |

---

## 10. Impacto en la arquitectura existente

### Cambios minimos
- Flag `is_commander` es un bool con `serde(default)` — retrocompatible con sessions.json existentes
- `can_communicate()` agrega una linea de early return — no afecta routing existente
- Frontend agrega condicionales CSS — no cambia estructura de componentes

### Cambios significativos
- `lib.rs` startup: nuevo bloque de auto-creacion post-restore
- `commands/session.rs`: init prompt bifurca entre master token y session token
- Nuevo modulo `config/commander.rs`

### Sin cambios
- MailboxPoller: ya soporta master token, no necesita modificacion
- PtyManager: Commander es una sesion normal de PTY
- Terminal window: Commander se renderiza igual que cualquier otra sesion
- xterm.js: sin cambios

---

## 11. Correcciones post-verificacion (2026-03-29)

Verificado con feature-dev:code-architect. 7 discrepancias corregidas:

| # | Correccion |
|---|-----------|
| 1 | Eliminada nota confusa sobre acceso en `snapshot_sessions` — `list_sessions()` ya devuelve `SessionInfo` con el flag |
| 2 | Agregado paso faltante: actualizar `makeInactiveEntry()` en `sessions.ts:27` con `isCommander: false` |
| 3 | `list_sessions()` no tiene variable `result` — corregido a `let mut result = ...collect()` + sort |
| 4 | Punto de insercion en `lib.rs` corregido: despues de linea ~320, NO ~296 (que esta dentro del loop) |
| 5 | Re-persist post-Commander usa `persist_merging_failed` en vez de `persist_current_state` para no perder entries fallidas |
| 6 | `master.token_string()` no existe — corregido a `master.value()` |
| 7 | Doble-click no activa rename sino `OpenAgentModal` — corregido guard a `handleDoubleClick()` |
