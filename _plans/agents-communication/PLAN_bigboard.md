# PLAN: Big-Board - Delegación de spawn a SummonGate

## Objetivo

Reemplazar el spawn directo de procesos Claude (`Command::new("claude.cmd")`) por llamadas HTTP a SummonGate. Big-board deja de ser el host de agentes y pasa a ser solo el **orquestador** (messaging, routing, lifecycle). SummonGate es quien ejecuta y muestra los agentes.

---

## Qué cambia y qué no

| Aspecto | Antes | Después |
|---------|-------|---------|
| Spawn de agentes | `agent::spawn_claude()` directo | HTTP POST a SummonGate |
| Output del agente | stdout/stderr descartados (null) | Visible en xterm.js via SummonGate |
| Monitoreo de proceso | `Child::try_wait()` + PID check | PID check o HTTP status endpoint |
| Kill de proceso | `taskkill /PID /F /T` directo | HTTP DELETE session + fallback taskkill |
| Payload delivery | stdin pipe | `stdinPayload` en el POST de creación |
| Token auth | Sin cambios | Sin cambios (sigue en el payload) |
| Task system | Sin cambios | Sin cambios |
| Messaging (JSONL) | Sin cambios | Sin cambios |
| Anti-thrashing | Sin cambios | Sin cambios |
| Workgroups | Sin cambios | Sin cambios |

**Principio:** el 90% de big-board queda igual. Solo cambia el módulo `agent.rs` y las partes de `daemon.rs` que interactúan con procesos.

---

## Decisiones abiertas

### D1: Modo de operación - exclusivo vs híbrido

- **Exclusivo:** Big-board SOLO usa SummonGate. Si SummonGate no está corriendo, no puede activar agentes.
- **Híbrido:** Si SummonGate está disponible, lo usa. Si no, fallback a spawn directo (comportamiento actual).

**Recomendación:** Híbrido. Permite usar big-board en servidores headless (VMs de QA/staging) donde SummonGate no tiene sentido.

### D2: Descubrimiento de SummonGate

Big-board necesita saber si SummonGate está corriendo y en qué puerto. Opciones:
- Leer `~/.summongate/api.token` (si existe, SummonGate está up). El puerto se puede incluir en el mismo archivo o en un `~/.summongate/api.json`.
- Config en `agents.json`: `"summongate_url": "http://127.0.0.1:19860"`
- Ambos: config define el URL, token file confirma que está vivo.

### D3: Mapeo sesión-agente

Cuando big-board crea una sesión via HTTP, recibe un `session_id` (UUID). Necesita guardarlo en `AgentState` para poder:
- Consultar status (`GET /api/sessions/:id/status`)
- Destruir la sesión (`DELETE /api/sessions/:id`)
- Escribir al PTY si necesita enviar más input (`POST /api/sessions/:id/write`)

Nuevo campo: `AgentState.summongate_session_id: Option<String>`

---

## Fases

### Fase 1 - Cliente HTTP y spawn delegado

**Reemplazar `agent::spawn_claude()` con llamada HTTP a SummonGate.**

1. Agregar dependencia `reqwest` (blocking) en `Cargo.toml`
2. Nuevo módulo `src/summongate.rs`:
   ```rust
   pub struct SummonGateClient {
       base_url: String,
       token: String,
   }

   impl SummonGateClient {
       pub fn from_discovery() -> Option<Self>  // Lee token file + config
       pub fn create_session(&self, req: CreateSessionRequest) -> Result<SessionInfo>
       pub fn get_session_status(&self, id: &str) -> Result<SessionStatus>
       pub fn destroy_session(&self, id: &str) -> Result<()>
       pub fn write_to_session(&self, id: &str, data: &str) -> Result<()>
   }
   ```

3. Modificar `agent.rs`:
   ```rust
   // Nuevo: spawn via SummonGate
   pub fn spawn_claude_via_summongate(
       client: &SummonGateClient,
       payload: &str,
       agent: &AgentState,
   ) -> Result<(String, u32), String>  // (session_id, pid)

   // Existente: spawn directo (se mantiene como fallback)
   pub fn spawn_claude(payload: &str, cwd: &str) -> Result<Child, String>
   ```

4. Modificar `daemon.rs` step 2 (activation):
   ```rust
   // Intentar SummonGate primero
   if let Some(sg) = &summongate_client {
       match agent::spawn_claude_via_summongate(sg, &payload, &agent) {
           Ok((session_id, pid)) => {
               // Guardar session_id en AgentState
               // No hay Child handle - monitorear via PID o API
           }
           Err(e) => {
               log::warn!("SummonGate spawn failed, falling back: {}", e);
               // Fallback a spawn directo
           }
       }
   } else {
       // Spawn directo (sin SummonGate)
   }
   ```

### Fase 2 - Monitoreo adaptado

**Adaptar step 3 del daemon para sesiones de SummonGate.**

1. Sin `Child` handle, el monitoreo cambia:
   - **Opción rápida:** Seguir usando `is_process_alive(pid)` - funciona igual
   - **Opción mejor:** Consultar `GET /api/sessions/:id/status` - más info (idle, exit code)

2. Modificar `daemon.rs` step 3:
   ```rust
   let process_dead = if let Some(session_id) = &agent.summongate_session_id {
       // Consultar SummonGate
       summongate_client.get_session_status(session_id)
           .map(|s| s.status == "exited")
           .unwrap_or_else(|_| !agent::is_process_alive(pid))  // Fallback PID
   } else if let Some(child) = children.get_mut(&agent_name) {
       // Monitoreo directo (Child handle)
       child.try_wait().map(|s| s.is_some()).unwrap_or(true)
   } else {
       !agent::is_process_alive(pid)
   };
   ```

3. Kill/timeout: `DELETE /api/sessions/:id` primero, fallback a `kill_process_tree(pid)`.

### Fase 3 - Modo interactivo (futuro)

**Cuando SummonGate soporte sesiones interactivas de larga duración.**

1. En vez de `claude -p` (que termina después de una respuesta), usar `claude` interactivo
2. Re-activación: en vez de crear nueva sesión, escribir nuevo prompt via `POST /api/sessions/:id/write`
3. Esto requiere que big-board sepa cuándo el agente terminó de responder (idle detection de SummonGate)
4. Cambia el modelo de "una sesión por activación" a "una sesión por agente, múltiples activaciones"

---

## Archivos a crear/modificar

| Archivo | Acción | Descripción |
|---------|--------|-------------|
| `Cargo.toml` | Modificar | Agregar reqwest |
| `src/summongate.rs` | Crear | HTTP client para SummonGate API |
| `src/agent.rs` | Modificar | Agregar `spawn_claude_via_summongate()`, mantener fallback |
| `src/daemon.rs` | Modificar | Step 2: spawn delegado. Step 3: monitoreo adaptado |
| `src/models.rs` | Modificar | Agregar `summongate_session_id` a AgentState |
| `src/main.rs` | Modificar | Inicializar SummonGateClient en el daemon loop |
| `agents.json` | Modificar | Agregar `summongate_url` opcional a config |

---

## Riesgos

- **SummonGate caído:** Si se cierra SummonGate mid-work, big-board pierde el handle. Mitigación: fallback a PID check, el agente sigue corriendo como proceso huérfano.
- **Latencia HTTP:** Spawn via HTTP agrega ~10-50ms vs spawn directo. Irrelevante para activaciones que duran minutos.
- **Token desincronizado:** Si SummonGate reinicia, genera nuevo token. Big-board necesita re-leerlo. Mitigación: re-discovery en cada activación o al fallar un request.
- **Sesiones huérfanas:** Si big-board muere, las sesiones en SummonGate quedan corriendo. Podría ser deseable (el usuario las ve) o no. Decisión de diseño.
