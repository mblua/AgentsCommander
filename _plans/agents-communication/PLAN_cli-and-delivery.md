# Plan: CLI de agentscommander + Sistema de Delivery de Mensajes

**Branch:** `feature/agent-direct-communication`
**Date:** 2026-03-25
**Status:** Plan revisado con decisiones de diseño confirmadas

---

## Contexto

Los agentes CLI (Claude Code, Codex, etc.) necesitan comunicarse entre sí. El mecanismo es un CLI integrado en el propio binario de agentscommander que los agentes invocan desde su PTY. El CLI escribe mensajes a archivo, y la instancia de agentscommander que está corriendo se encarga de entregarlos al destinatario.

---

## Arquitectura General

### Dos modos del binario

```
agentscommander.exe               → Modo app (Tauri, sidebar, terminal)
agentscommander.exe send ...      → Modo CLI (escribe mensaje, termina)
agentscommander.exe list-peers .. → Modo CLI (lista peers, termina)
```

El binario detecta si tiene subcomandos y actúa en consecuencia. Sin subcomandos → Tauri app. Con subcomandos → CLI puro, sin ventanas.

### Arg parsing: `clap` con derive

Patrón tomado de `amp-big-board/big-board`. En `main.rs`:

```rust
#[derive(Parser)]
#[command(name = "agentscommander")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Send { ... },
    ListPeers { ... },
}

fn main() {
    // Si no hay args (o arg es un path como Tauri pasa), ir a modo app
    // Si hay subcomando reconocido → modo CLI
    let args = Cli::try_parse();
    match args {
        Ok(cli) => match cli.command {
            Some(cmd) => handle_cli(cmd),  // CLI mode, exit after
            None => agentscommander_lib::run(),  // App mode
        },
        Err(_) => agentscommander_lib::run(),  // No args / unknown → App mode
    }
}
```

### Windows console: `AttachConsole`

`main.rs` tiene `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` que suprime la consola en release builds. Para CLI mode, se usa `AttachConsole(ATTACH_PARENT_PROCESS)` antes de cualquier stdout para reconectar a la consola del proceso padre (la PTY del agente).

---

## Autenticación: Session Token

Cada session de agente recibe un **token UUID único** al ser creada. Este token:

- Se genera en `create_session_inner()` al crear la session
- Se pasa al agente **dentro del prompt de inicialización** (NO como env var, para evitar exposición)
- El agente lo incluye en cada invocación del CLI
- agentscommander valida el token contra la session activa antes de procesar el mensaje
- Si el token no matchea → rechazo, el agente no puede spoofear otro agente

### Validación de token

Linear scan en `SessionManager`: `sessions.values().find(|s| s.token == token)`. Con max 10-20 sessions activas, es instantáneo. Sin índice secundario.

### Prompt de inicialización

Al spawnar un agente, agentscommander espera un **delay de 3-5 segundos** post-spawn, luego inyecta en el PTY stdin:

```
Tu token de sesión para comunicarte con otros agentes es: <UUID>

Para enviar mensajes a otros agentes:
  agentscommander send --token <UUID> --to "<agent_name>" --message "..." [--mode wake|active-only|wake-and-sleep|queue] [--get-output] [--agent <agent_cli>]

Para ver agentes disponibles para mensajear:
  agentscommander list-peers --token <UUID>

Cuando recibas un mensaje con get-output, encerrá tu respuesta entre markers:
  %%AC_RESPONSE::<requestId>::START%%
  <tu respuesta>
  %%AC_RESPONSE::<requestId>::END%%
```

El delay fijo cubre el boot del agente CLI (1-2s típico). Si no alcanza para algún agente, se puede hacer configurable.

---

## Modos de Delivery

### 1. `queue` (default)
Deja el mensaje en el inbox del destinatario. No toca la PTY. El agente lo lee cuando quiera.

### 2. `active-only`
Entrega solo si el agente tiene una session activa y despierta (no idle). Si está idle o no tiene session → encola como `queue`.

### 3. `wake`
Si el agente está idle (esperando input en la PTY), inyecta el mensaje en el stdin de la PTY como si el usuario lo tipeara. Si no hay session activa → encola como `queue`.

### 4. `wake-and-sleep`
- Si no hay session activa: levanta una session temporal no-interactiva con el agente CLI apropiado
- Inyecta el mensaje en el stdin de la PTY
- Monitorea el idle detector
- Cuando el agente vuelve a idle (terminó de responder) → captura el output si aplica, cierra la session
- Es un "one-shot": spawn → deliver → wait for idle → kill

### Concurrencia de mensajes wake

Procesamiento secuencial. Si llegan 2 mensajes wake al mismo agente en el mismo ciclo de 3s del poller, el primero se inyecta y el segundo queda en outbox hasta el próximo ciclo. Para wake-and-sleep, si ya hay una session temporal activa para ese destinatario, el segundo mensaje va como `queue` (inbox).

---

## Selección de Agente CLI (para wake-and-sleep)

Cuando hay que levantar una session nueva, se necesita saber qué agente CLI usar:

### Flag `--agent`
- `auto` (default): usa el último agente CLI levantado en ese repo. Si no hay historial, usa el mismo agente CLI del sender
- Nombre específico (ej: `claude`, `codex`): usa ese agente puntual

### Historial: `lastCodingAgent` en config.json

Cada vez que se instancia un coding agent en un repo, se guarda en `<repo>/.agentscommander/config.json`:

```json
{
  "teams": ["dev-core"],
  "lastCodingAgent": "claude"
}
```

Se guarda el `id` del `AgentConfig` (no el command completo). Al hacer wake-and-sleep, se busca en `settings.agents` por ese id para obtener el `command` completo. Si el usuario cambia el command en settings, wake-and-sleep usa la versión actualizada.

### Información del sender
El mensaje incluye qué agente CLI usa el sender (campo `senderAgent`) para el fallback de `auto` cuando no hay historial en el destinatario.

---

## Flag `--get-output` — Response Markers

Cuando se pasa `--get-output`, el agente destinatario debe encerrar su respuesta en markers detectables:

```
%%AC_RESPONSE::<requestId>::START%%
La respuesta del agente...
%%AC_RESPONSE::<requestId>::END%%
```

### Flujo completo

1. El CLI genera un `requestId` UUID y escribe el mensaje con `getOutput: true`
2. El CLI entra en polling loop: lee `.agentscommander/responses/<requestId>.json` cada 2s
3. El MailboxPoller entrega el mensaje al agente (wake/wake-and-sleep), inyectando en la PTY
4. El agente procesa el mensaje y emite su respuesta entre los markers
5. agentscommander monitorea el output stream de la PTY, detecta los markers via regex
6. Extrae el contenido entre START y END, lo escribe en `.agentscommander/responses/<requestId>.json`
7. El CLI detecta el response file, lo lee, imprime a stdout, exit 0

### Ventajas sobre captura raw
- No requiere vt100 parser temporal
- No depende del idle detector para delimitar la respuesta
- Output limpio y delimitado por el propio agente
- El requestId en los markers previene colisiones con otros mensajes concurrentes

### Timeout
El CLI tiene un timeout configurable (default 5 minutos). Si no llega respuesta → exit con error.

Sin `--get-output`: fire-and-forget. El CLI escribe, sale con exit 0.

---

## Discovery: `list-peers`

```bash
agentscommander list-peers --token <UUID>
```

### Reglas de visibilidad

1. Si el agente tiene team(s) en `teams.json` → devuelve la **unión de miembros** de todos sus teams
2. Si el agente NO tiene team → devuelve todos los agentes que comparten su **parent directory**
3. Un agente puede estar en múltiples teams

### Información por peer

Para cada peer devuelve:
- `name`: nombre extendido (parent/repo)
- `status`: si tiene session activa o no
- `role`: descripción de qué hace — leído de la sección `## Role Prompt` de `CLAUDE.md`. Fallback: primeras 5 líneas. Fallback final: "No role description available."
- `teams`: teams compartidos
- `lastCodingAgent`: último agente CLI usado en ese repo

---

## Outbox Files: Lifecycle

### Formato del mensaje

```json
{
  "id": "uuid-del-mensaje",
  "token": "uuid-session-token-del-sender",
  "from": "0_repos/agentscommander_2",
  "to": "0_repos/project_x",
  "body": "Revisá el endpoint /api/health",
  "mode": "wake",
  "getOutput": false,
  "requestId": null,
  "senderAgent": "claude",
  "preferredAgent": "auto",
  "priority": "normal",
  "timestamp": "2026-03-25T01:00:00Z"
}
```

Todos los campos nuevos son `Option` con `#[serde(default)]` para backwards compatibility con el formato anterior.

### Lifecycle del archivo

1. **CLI escribe** → `<repo>/.agentscommander/outbox/<uuid>.json`
2. **MailboxPoller procesa**:
   - Éxito → mueve a `outbox/delivered/<uuid>.json` **con token stripeado** (historial de mensajería)
   - Fallo de validación → mueve a `outbox/rejected/<uuid>.json` con token stripeado + `.reason.txt`
3. Nunca se borran — queda historial completo

### Seguridad del token en archivos
- El outbox file contiene el token momentáneamente (entre escritura del CLI y procesamiento del poller, ~3s max)
- En `delivered/` y `rejected/` el token se stripea antes de mover
- El directorio outbox es local al repo del agente — acceso al filesystem implica acceso a todo
- El token solo es útil mientras la session existe — session destruida = token inválido

---

## Wiring: MailboxPoller acceso a PtyManager y SessionManager

El `MailboxPoller` necesita `PtyManager` (para inyectar en PTY) y `SessionManager` (para validar tokens y buscar sessions). Pero `PtyManager` se crea dentro de `setup()` después del poller.

**Solución**: El poller obtiene los handles del `AppHandle` on-demand dentro de `poll()`:

```rust
async fn poll(&self, app: &AppHandle) {
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
    let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
    // ... usar según necesidad del delivery mode
}
```

Cero cambio en el orden de construcción. El `AppHandle` ya se pasa al poller en `start()`.

---

## Flujo Completo

### Envío normal (fire-and-forget)

```
Agente A ejecuta:
  agentscommander send --token ABC --to "0_repos/project_x" --message "Hola" --mode queue

CLI:
  1. AttachConsole (Windows release)
  2. Valida que --token, --to, --message están presentes
  3. Genera message UUID
  4. Escribe .agentscommander/outbox/<uuid>.json
  5. Imprime "Message queued: <uuid>"
  6. Exit 0

MailboxPoller (cada 3s):
  1. Detecta el archivo en outbox/
  2. Lee el mensaje
  3. Valida el token contra SessionManager (linear scan)
  4. Valida visibilidad: peers por team o parent directory
  5. Según mode:
     - queue: escribe en <to>/.agentscommander/inbox/
     - active-only: si hay session despierta → inyecta en PTY via PtyManager, sino → inbox
     - wake: si hay session idle → inyecta en PTY, sino → inbox
     - wake-and-sleep: levanta session temporal → inyecta → monitorea idle → cierra
  6. Mueve archivo a outbox/delivered/ (token stripeado)
  7. Emite evento al frontend
```

### Envío con get-output

```
Agente A ejecuta:
  agentscommander send --token ABC --to "0_repos/project_x" --message "Revisá esto" --mode wake --get-output

CLI:
  1. Genera message UUID y requestId
  2. Escribe outbox/<uuid>.json con getOutput: true, requestId: <rid>
  3. Entra en loop de polling: lee .agentscommander/responses/<rid>.json cada 2s
  4. (bloqueante — timeout 5 minutos)

MailboxPoller:
  1. Detecta el mensaje, valida token y peers
  2. Inyecta en PTY del destinatario (wake)
  3. Monitorea output stream del PTY buscando:
     %%AC_RESPONSE::<rid>::START%%
     ...contenido...
     %%AC_RESPONSE::<rid>::END%%
  4. Extrae el contenido entre markers
  5. Escribe en <sender_repo>/.agentscommander/responses/<rid>.json
  6. El CLI del sender detecta el archivo, lo lee, imprime a stdout, exit 0
```

---

## Implementación — Archivos a crear/modificar

### CLI (Rust)

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/main.rs` | MODIFICAR | clap parsing antes de run(). AttachConsole en Windows. Modo CLI vs app |
| `src-tauri/src/cli/mod.rs` | **CREAR** | Módulo CLI: structs clap, dispatch de subcomandos |
| `src-tauri/src/cli/send.rs` | **CREAR** | Subcomando `send`: valida args, genera mensaje, escribe a outbox, polling para get-output |
| `src-tauri/src/cli/list_peers.rs` | **CREAR** | Subcomando `list-peers`: lee agents.json, filtra peers, lee CLAUDE.md roles |
| `src-tauri/Cargo.toml` | MODIFICAR | Agregar `clap = { version = "4", features = ["derive"] }` |

### Session Token (Rust)

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/session/session.rs` | MODIFICAR | Agregar campo `token: Uuid` a `Session` y `SessionInfo` |
| `src-tauri/src/session/manager.rs` | MODIFICAR | Generar token en `create_session`, agregar `find_by_token()` (linear scan) |

### Delivery (Rust)

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/phone/mailbox.rs` | MODIFICAR | Delivery por modo, validación de token (via AppHandle → SessionManager), inyección PTY (via AppHandle → PtyManager), monitoreo de response markers, move to delivered/ |
| `src-tauri/src/phone/types.rs` | MODIFICAR | Campos nuevos en OutboxMessage (todos Option + serde default), ResponseMarker struct |

### Prompt de inicialización

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/commands/session.rs` | MODIFICAR | Delay 3-5s post-spawn, inyectar prompt con token, instrucciones CLI, y formato de response markers |

### Agent History

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/config/dark_factory.rs` | MODIFICAR | Agregar `lastCodingAgent` a `AgentLocalConfig`, actualizar en `sync_agent_configs` |
| `src-tauri/src/commands/session.rs` | MODIFICAR | Al crear session con agent, guardar el agent id en config.json del repo |

### Frontend

| Archivo | Acción | Qué |
|---|---|---|
| `src/shared/types.ts` | MODIFICAR | Agregar campos de delivery mode, response marker types |
| `src/shared/ipc.ts` | MODIFICAR | Nuevos eventos si los hay |

---

## Orden de ejecución

### Step 1: CLI skeleton + clap
1. Agregar `clap` a Cargo.toml
2. Modificar `main.rs`: clap parsing, AttachConsole, dispatch
3. Crear módulo `cli/` con structs
4. Implementar `send` básico (escribe outbox, fire-and-forget, sin validación de token)
5. Test: ejecutar `agentscommander send --to X --message Y` desde una terminal

### Step 2: Session Token
1. Agregar `token: Uuid` a `Session` struct
2. Generar en `create_session`
3. Implementar `find_by_token()` en SessionManager (linear scan)
4. Agregar validación de token en MailboxPoller (via AppHandle)
5. Test: verificar que el token se genera y se valida correctamente

### Step 3: Prompt de inicialización
1. Después de spawn + delay 3-5s, inyectar prompt con token e instrucciones
2. El prompt incluye: token, sintaxis de `send`, sintaxis de `list-peers`, formato de response markers
3. Test: crear session, verificar que el prompt aparece en la PTY

### Step 4: Delivery modes
1. MailboxPoller obtiene PtyManager y SessionManager del AppHandle
2. Implementar `queue` (ya existe — escribir a inbox)
3. Implementar `active-only` (verificar session status)
4. Implementar `wake` (inyectar en PTY stdin via PtyManager::write si idle)
5. Implementar `wake-and-sleep` (spawn temporal, inyectar, wait idle, kill)
6. Mover procesados a `outbox/delivered/` con token stripeado
7. Test manual de cada modo

### Step 5: get-output con response markers
1. Agregar requestId al mensaje
2. CLI entra en polling loop esperando response file
3. MailboxPoller monitorea output stream buscando `%%AC_RESPONSE::<rid>::START/END%%`
4. Extrae contenido, escribe response file
5. Test: enviar con --get-output, verificar que llega la respuesta

### Step 6: list-peers
1. Implementar subcomando `list-peers`
2. Leer agents.json, filtrar por teams o parent directory
3. Leer `## Role Prompt` de CLAUDE.md de cada peer
4. Devolver JSON formateado
5. Test: ejecutar list-peers, verificar output

### Step 7: Agent History (lastCodingAgent)
1. Al crear session con un agent, guardar su id en `<repo>/.agentscommander/config.json` como `lastCodingAgent`
2. Usar para el fallback de `--agent auto` en wake-and-sleep
3. Test: crear sessions con distintos agents, verificar que se actualiza

---

## Consideraciones

- **Seguridad del token**: Solo existe en memoria (SessionManager) y en el prompt del agente. Outbox files lo contienen brevemente (~3s). En delivered/rejected se stripea.
- **Timeout para get-output**: Default 5 minutos, configurable via flag `--timeout`.
- **Response markers**: `%%AC_RESPONSE::<requestId>::START%%` / `%%AC_RESPONSE::<requestId>::END%%`. El agente aprende el formato del prompt de inicialización.
- **wake-and-sleep cleanup**: Si la session temporal no vuelve a idle, timeout forzoso para matar la session (configurable, default 10 minutos).
- **Concurrencia**: Secuencial por destinatario. Segundo mensaje wake queda en outbox 3s más. Wake-and-sleep con session temporal activa → fallback a queue.
- **CLI binary path**: Se pasa en el prompt de inicialización junto con el token.
- **Backwards compatibility**: Campos nuevos en OutboxMessage son `Option` con `serde(default)`. Mensajes sin token se procesan sin autenticación (legacy mode).
