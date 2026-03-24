# PLAN: Phone Call - Inter-Agent Messaging System

## Objetivo

Permitir que agentes (Claude Code instances corriendo en PTY sessions) puedan descubrir que otros agentes existen y comunicarse entre si via un sistema de inbox asincronico gestionado por Agents Commander.

Un agente puede:
1. Preguntar "que agentes hay cargados?" (directorio)
2. Dejar un mensaje para otro agente especifico (inbox)
3. Leer sus propios mensajes pendientes (pickup)

Agents Commander actua como **operador telefonico** - no conecta agentes directamente, sino que recibe mensajes y los deposita en el inbox del destinatario.

---

## Modelo de comunicacion

```
Agente A                    Agents Commander                   Agente B
   |                              |                               |
   |-- "who's online?" ---------->|                               |
   |<- [agent-b, agent-c] -------|                               |
   |                              |                               |
   |-- "tell agent-b: ..." ----->|                               |
   |<- "message queued" ---------|                               |
   |                              |-- (inbox notification) ------>|
   |                              |                               |
   |                              |<- "check my inbox" ----------|
   |                              |-- [msg from agent-a] ------->|
```

**Asincrono**: el emisor no espera respuesta. Si agent-b quiere responder, deja otro mensaje en el inbox de agent-a.

---

## Como interactuan los agentes con el sistema

Los agentes son Claude Code instances en PTY sessions. Pueden ejecutar comandos de shell. El mecanismo de interfaz es un **CLI tool** que se comunica con Agents Commander.

### Opcion elegida: CLI tool (`ac-phone`)

Un ejecutable liviano (o script) que los agentes pueden invocar:

```bash
# Listar agentes registrados
ac-phone list

# Dejar mensaje para un agente
ac-phone send --to "backend-agent" --message "Termine de migrar la tabla users"

# Leer inbox
ac-phone inbox

# Leer y marcar como leido
ac-phone inbox --ack
```

`ac-phone` se comunica con el backend via un mecanismo local (ver Decisiones abiertas).

### Alternativa descartada: archivos directos

Que los agentes lean/escriban archivos en `~/.agentscommander/inbox/`. Problemas: race conditions, no hay notificacion, el agente necesita saber la ruta exacta, no hay validacion del destinatario.

---

## Decisiones abiertas

### D1: Transporte CLI <-> Backend

`ac-phone` necesita hablar con el backend de Agents Commander. Opciones:

| Opcion | Pros | Contras |
|--------|------|---------|
| **HTTP API local** (depende de PLAN_summongate) | Clean, estandar, ya planeado | Requiere que el HTTP server exista primero |
| **Named pipe / Unix socket** | No depende del HTTP server | Mas complejo en Windows, otro canal de IPC |
| **Archivo + polling** | Zero deps | Lento, race conditions |
| **Stdin injection** | Ya existe `pty_write` | Invasivo, no es un canal limpio |

**Recomendacion:** HTTP API local. Esto se puede implementar como extension de los endpoints de PLAN_summongate, o como un server HTTP minimo independiente dedicado solo a phone-call. Si el HTTP server de SummonGate no esta listo, se puede arrancar con archivos como bridge temporal.

### D2: Identificacion de agentes

Los agentes se identifican por... que?

- **Session name** (ej: "backend-agent"): human-readable, pero puede haber duplicados o renombres
- **Session UUID**: unico, pero los agentes no lo conocen facilmente
- **Agent label** (de AgentConfig): estable, definido en settings.json

**Recomendacion:** Agent label como identificador primario (es el nombre estable que el usuario les dio en settings). Session name como fallback. UUID internamente.

### D3: Persistencia de mensajes

- **Solo en memoria**: rapido, se pierden al reiniciar. Suficiente si los agentes procesan rapido.
- **Archivo JSON**: persiste entre reinicios. Util si un agente no esta activo y se activa despues.
- **Hibrido**: en memoria + flush a disco periodico.

**Recomendacion:** En memoria para MVP, con la estructura preparada para agregar persistencia despues.

### D4: Notificacion de mensaje nuevo

Cuando llega un mensaje al inbox de agent-b, como se entera?

- **Polling**: agent-b ejecuta `ac-phone inbox` periodicamente. Simple pero ineficiente.
- **PTY injection**: Agents Commander escribe un aviso en el PTY stdin del agente. Intrusivo pero inmediato.
- **File signal**: Agents Commander crea un archivo marker que el agente puede watchear.
- **Evento Tauri**: Emitir evento al frontend, que muestre indicador visual (no llega al agente CLI, pero el usuario lo ve).

**Recomendacion:** Evento Tauri para UI (el usuario ve que hay mensajes pendientes) + el agente usa `ac-phone inbox` cuando necesite. No inyectar en PTY stdin - es demasiado invasivo y puede romper el flujo del agente.

---

## Estructura de datos

### Message

```rust
pub struct PhoneMessage {
    pub id: Uuid,
    pub from: String,           // agent label o session name del emisor
    pub to: String,             // agent label o session name del destinatario
    pub body: String,           // contenido del mensaje
    pub created_at: DateTime<Utc>,
    pub read: bool,             // si fue leido por el destinatario
}
```

### Inbox (por agente)

```rust
pub struct AgentInbox {
    pub agent_id: String,       // agent label
    pub messages: Vec<PhoneMessage>,
}
```

### PhoneBook (directorio de agentes)

```rust
pub struct PhoneBookEntry {
    pub label: String,          // agent label (de AgentConfig)
    pub session_id: Option<Uuid>, // None si no tiene sesion activa
    pub status: AgentStatus,    // Online (sesion activa), Offline, Busy
}
```

---

## Endpoints / Commands

### Via Tauri Commands (frontend)

```rust
#[tauri::command]
fn phone_list_agents() -> Vec<PhoneBookEntry>

#[tauri::command]
fn phone_send_message(from: String, to: String, body: String) -> Result<Uuid, String>

#[tauri::command]
fn phone_get_inbox(agent_id: String) -> Vec<PhoneMessage>

#[tauri::command]
fn phone_ack_messages(agent_id: String, message_ids: Vec<Uuid>) -> Result<(), String>
```

### Via HTTP API (para ac-phone CLI)

```
GET    /api/phone/agents             - Listar agentes y su status
POST   /api/phone/messages           - Enviar mensaje { from, to, body }
GET    /api/phone/inbox/:agent_id    - Leer inbox de un agente
POST   /api/phone/inbox/:agent_id/ack - Marcar mensajes como leidos { messageIds }
```

### Via ac-phone CLI

```bash
ac-phone list                          # GET /api/phone/agents
ac-phone send --to <agent> --msg "..."  # POST /api/phone/messages
ac-phone inbox                         # GET /api/phone/inbox/:self
ac-phone inbox --ack                   # POST /api/phone/inbox/:self/ack
```

`ac-phone` necesita saber quien es "self" - se resuelve via variable de entorno `AC_AGENT_ID` que Agents Commander setea al spawnear la sesion.

---

## Fases

### Fase 1 - Backend: PhoneBook + Inbox en memoria

1. Nuevo modulo `src-tauri/src/phone/`
   - `mod.rs` - re-exports
   - `models.rs` - PhoneMessage, AgentInbox, PhoneBookEntry
   - `manager.rs` - PhoneManager con inbox HashMap y metodos CRUD
2. Integrar PhoneManager en el app state (`lib.rs`)
3. Tauri Commands para phone_list_agents, phone_send_message, phone_get_inbox, phone_ack_messages
4. Evento Tauri `phone_new_message { to, from, messageId }` emitido al enviar mensaje
5. Tests unitarios del PhoneManager

### Fase 2 - CLI tool: ac-phone

1. Nuevo crate o script en `tools/ac-phone/`
2. Comunicacion con backend (requiere HTTP server o mecanismo alternativo)
3. Auto-discovery: lee puerto y token de `~/.agentscommander/api.token` (misma convencion que SummonGate)
4. Variable de entorno `AC_AGENT_ID` para identificar al agente caller
5. Setear `AC_AGENT_ID` al crear sesiones en PtyManager

### Fase 3 - UI: indicadores en Sidebar

1. Icono/badge en cada session item mostrando mensajes pendientes
2. Panel de mensajes (click en el badge abre lista de mensajes)
3. Integracion con eventos `phone_new_message` para actualizar en real-time

### Fase 4 - Mejoras

1. Persistencia a disco (JSON en `~/.agentscommander/phone/`)
2. Mensajes con TTL (expiran si no se leen en X tiempo)
3. Broadcast: enviar mensaje a todos los agentes
4. Reply threading: responder a un mensaje especifico
5. Adjuntos: enviar snippets de codigo o paths de archivos

---

## Archivos a crear/modificar

| Archivo | Accion | Descripcion |
|---------|--------|-------------|
| `src-tauri/src/phone/mod.rs` | Crear | Module re-exports |
| `src-tauri/src/phone/models.rs` | Crear | PhoneMessage, AgentInbox, PhoneBookEntry |
| `src-tauri/src/phone/manager.rs` | Crear | PhoneManager: inbox CRUD, phonebook |
| `src-tauri/src/commands/phone.rs` | Crear | Tauri Commands para phone |
| `src-tauri/src/lib.rs` | Modificar | Registrar PhoneManager en state, registrar commands |
| `src-tauri/src/pty/manager.rs` | Modificar | Setear AC_AGENT_ID env var al spawnear sesion |
| `src/shared/types.ts` | Modificar | Tipos TS para PhoneMessage, PhoneBookEntry |
| `src/shared/ipc.ts` | Modificar | Wrappers IPC para phone commands |
| `src/sidebar/components/SessionItem.tsx` | Modificar | Badge de mensajes pendientes |
| `tools/ac-phone/` | Crear | CLI tool para agentes |

---

## Relacion con otros planes

- **PLAN_summongate**: El HTTP server de SummonGate es el transporte ideal para `ac-phone`. Si se implementa primero, Fase 2 de este plan se simplifica. Si no, se puede usar un HTTP server minimo o archivos como bridge temporal.
- **PLAN_bigboard**: Big-board podria usar phone-call para comunicar instrucciones entre agentes de un workgroup en vez de solo activarlos con payloads unidireccionales.

---

## Riesgos

- **Spam entre agentes**: Un agente buggy podria floodear inboxes. Mitigacion: rate limit por agente (ej: max 10 msgs/min).
- **Mensajes perdidos**: Si el backend reinicia, inbox en memoria se pierde. Mitigacion: Fase 4 agrega persistencia.
- **Identidad falsa**: Un agente podria mentir sobre su `from`. Mitigacion: el backend resuelve el `from` desde `AC_AGENT_ID` (seteado por Agents Commander, no por el agente).
- **Dead letter**: Mensajes a agentes que no existen o nunca se activan. Mitigacion: validar destinatario contra PhoneBook, TTL en Fase 4.
