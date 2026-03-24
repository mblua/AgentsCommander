# PLAN: SummonGate - Agent Communication API

## Objetivo

Exponer una HTTP API local para que procesos externos (big-board) puedan crear sesiones, escribir en PTYs, consultar estado, y destruir sesiones. SummonGate pasa de ser una app cerrada a ser un **host programable de agentes**.

---

## Decisiones abiertas

### D1: Modo del agente - `claude -p` vs `claude` interactivo

| | `claude -p` (pipe) | `claude` interactivo |
|---|---|---|
| Payload | stdin (como hoy en big-board) | Hay que escribirlo via `pty_write` |
| Output visible | Si, en xterm.js | Si, con toda la UI de Claude Code |
| Intervención del usuario | No (proceso termina solo) | Si, podés tipear en cualquier momento |
| Re-activación | Nueva sesión o nuevo proceso | Podés escribir en la sesión existente |
| Complejidad | Baja (casi no cambia big-board) | Media (cambia cómo se entrega el payload) |

**Recomendación:** Arrancar con `claude -p` visible (mínimo cambio), migrar a interactivo cuando el flujo esté validado.

### D2: Puerto HTTP

Necesita un puerto fijo o configurable. Propuesta: `19860` (fuera de rangos comunes).
Se agrega a `settings.json` como `"apiPort": 19860`.

### D3: Autenticación

Token bearer simple. Se genera al iniciar SummonGate y se guarda en `~/.summongate/api.token`.
Big-board lo lee de ahí. Mismo patrón que big-board usa con `master.token`.

---

## Fases

### Fase 1 - HTTP Server básico

**Agregar un servidor HTTP embebido en el backend Rust de SummonGate.**

1. Agregar dependencia `axum` + `tower-http` en `Cargo.toml`
2. Nuevo módulo `src-tauri/src/http/`
   - `mod.rs` - Router setup, server spawn
   - `auth.rs` - Middleware de bearer token
   - `handlers.rs` - Request handlers que delegan a SessionManager/PtyManager
3. Arrancar el server en un tokio task dentro de `lib.rs` (antes del Tauri app builder)
4. Generar token al inicio, escribir en `~/.summongate/api.token`
5. Agregar `apiPort` a `AppSettings`

**Endpoints Fase 1:**

```
POST   /api/sessions           - Crear sesión (shell, args, cwd, name)
GET    /api/sessions           - Listar sesiones activas
GET    /api/sessions/:id       - Detalle de una sesión (status, idle, pid)
POST   /api/sessions/:id/write - Escribir bytes al PTY stdin
DELETE /api/sessions/:id       - Destruir sesión
```

**Request/Response ejemplo:**

```
POST /api/sessions
Authorization: Bearer <token>
Content-Type: application/json

{
  "shell": "claude.cmd",
  "shellArgs": ["-p", "--output-format", "text", "--dangerously-skip-permissions"],
  "cwd": "C:\\Users\\maria\\0_repos\\amp-backend",
  "sessionName": "backend@wg1",
  "stdinPayload": "...activation payload..."
}

Response 201:
{
  "id": "a1b2c3d4-...",
  "name": "backend@wg1",
  "status": "running",
  "pid": 12345
}
```

Nota: `stdinPayload` es opcional. Si se provee, se escribe al PTY inmediatamente después del spawn. Esto permite que big-board envíe el activation payload sin cambiar su modelo actual.

### Fase 2 - Eventos y monitoreo

**Permitir que big-board sepa qué pasa en las sesiones sin pollear constantemente.**

1. Endpoint de status con idle detection:
   ```
   GET /api/sessions/:id/status
   Response: { "status": "running", "idle": true, "pid": 12345, "runningSince": "..." }
   ```

2. Opcional: WebSocket en `/api/sessions/:id/output` para stream de PTY output en real-time (big-board probablemente no lo necesite, pero es útil para debugging)

### Fase 3 - Session lifecycle hooks

**Notificar a big-board cuando una sesión termina.**

1. Agregar campo opcional `callbackUrl` en `POST /api/sessions`
2. Cuando la sesión termina (proceso sale), SummonGate hace POST al callbackUrl con exit code
3. Alternativa: big-board pollea `/api/sessions/:id/status` cada 500ms (ya lo hace con PIDs hoy)

---

## Archivos a crear/modificar

| Archivo | Acción | Descripción |
|---------|--------|-------------|
| `src-tauri/Cargo.toml` | Modificar | Agregar axum, tower-http |
| `src-tauri/src/http/mod.rs` | Crear | Router, server setup |
| `src-tauri/src/http/auth.rs` | Crear | Bearer token middleware |
| `src-tauri/src/http/handlers.rs` | Crear | HTTP handlers |
| `src-tauri/src/lib.rs` | Modificar | Spawn HTTP server task |
| `src-tauri/src/config/app_config.rs` | Modificar | Agregar apiPort |
| `src-tauri/src/session/manager.rs` | Modificar | Método para write-after-spawn (stdinPayload) |

---

## Riesgos

- **Puerto ocupado:** Si el puerto ya está en uso, el server falla silenciosamente. Necesita retry o error visible.
- **Seguridad:** Solo escuchar en `127.0.0.1`, nunca `0.0.0.0`. El token previene acceso de otros procesos locales.
- **Concurrencia:** SessionManager ya usa `Arc<RwLock<>>`, debería funcionar. Hay que asegurar que los HTTP handlers no holdeen el lock mucho tiempo.
- **stdinPayload timing:** El write al PTY debe ocurrir después de que el proceso esté listo para leer stdin. Puede necesitar un pequeño delay o un mecanismo de readiness.
