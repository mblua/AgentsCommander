# Plan: Notificación de sesión idle (dot verde)

**Branch:** `feature/add-notification`
**Issue:** Cuando una sesión termina de ejecutar un comando y queda esperando input del usuario, el puntito azul del sidebar debe cambiar a **verde**. Cuando el usuario escribe algo nuevo, vuelve a azul/cyan.

---

## Detección

Basada en silencio de PTY. Si pasan **700ms** sin output del PTY, se considera "idle/waiting for input". Cuando llega output nuevo, vuelve a "busy".

**Trade-offs aceptados:**
- Un comando que no produce output (ej: `sleep 60`) se detectaría como idle tras 700ms. Aceptable - desde la perspectiva del usuario, la shell IS waiting.
- Sesiones pasivas (en background) también reciben el dot verde cuando su comando termina. Esto es el valor de la feature.

---

## Archivos a crear

| Archivo | Qué hace |
|---------|----------|
| `src-tauri/src/pty/idle_detector.rs` | Tracker de actividad por sesión. Un thread watcher cada 200ms detecta silencio > 700ms y dispara callback |

---

## Archivos a modificar

### Rust backend

| Archivo | Cambio |
|---------|--------|
| `src-tauri/src/pty/mod.rs` | Agregar `pub mod idle_detector` |
| `src-tauri/src/session/session.rs` | Agregar `waiting_for_input: bool` a `Session` y `SessionInfo` |
| `src-tauri/src/session/manager.rs` | Agregar `mark_idle(id)` y `mark_busy(id)` |
| `src-tauri/src/pty/manager.rs` | Integrar `IdleDetector`, llamar `record_activity()` en el read loop, emitir eventos `session_idle`/`session_busy` |
| `src-tauri/src/lib.rs` | Pasar `session_mgr` al `PtyManager` |

### Frontend

| Archivo | Cambio |
|---------|--------|
| `src/shared/types.ts` | Agregar `waitingForInput: boolean` a `Session` |
| `src/shared/ipc.ts` | Agregar listeners `onSessionIdle` y `onSessionBusy` |
| `src/sidebar/stores/sessions.ts` | Agregar `setSessionWaiting(id, bool)` |
| `src/sidebar/App.tsx` | Suscribirse a los eventos en `onMount` |
| `src/sidebar/components/SessionItem.tsx` | Agregar clase `waiting` al dot |
| `src/sidebar/styles/variables.css` | Agregar `--status-waiting: #22c55e` |
| `src/sidebar/styles/sidebar.css` | Agregar regla `.session-item-status.waiting` con color verde + glow |

---

## Data flow

```
PTY output → record_activity() → reset timer del silencio
700ms sin output → on_idle → emit "session_idle" → dot verde
Nuevo output → mark_busy → emit "session_busy" → dot azul/cyan
```

---

## Diseño del IdleDetector

```rust
pub struct IdleDetector {
    activity: Arc<Mutex<HashMap<Uuid, Instant>>>,
    idle_set: Arc<Mutex<HashSet<Uuid>>>,
}

impl IdleDetector {
    pub fn new() -> Self
    pub fn record_activity(&self, session_id: Uuid)  // actualiza timestamp, remueve de idle_set
    pub fn start(self, on_idle: impl Fn(Uuid), on_busy: impl Fn(Uuid))  // spawns watcher thread
}

const IDLE_THRESHOLD_MS: u64 = 700;
```

El watcher thread:
- Corre cada 200ms
- Itera todas las sesiones en el HashMap
- Si `now - last_seen > IDLE_THRESHOLD` y la sesión NO está en `idle_set`, llama `on_idle` y la agrega a `idle_set`
- `record_activity` remueve la sesión de `idle_set` para que el siguiente check pueda detectar idle de nuevo

---

## Concurrencia

- `SessionManager` usa `Arc<tokio::sync::RwLock<>>`. El idle detector watcher corre en `tokio::task::spawn_blocking` para poder usar `.await` al adquirir el lock.
- Race condition en destroy: si una sesión se destruye mientras el detector la tiene trackeada, `mark_idle` debe manejar `SessionNotFound` sin panic (log + skip).

---

## CSS

```css
/* variables.css */
--status-waiting: #22c55e;

/* sidebar.css */
.session-item-status.waiting {
  background: var(--status-waiting);
  box-shadow: 0 0 6px var(--status-waiting);
}
```

La clase `.waiting` se agrega al dot cuando `session.waitingForInput === true`. Tiene suficiente especificidad para overridear `.active`, `.running`, `.idle`.

---

## Build sequence

1. **Rust backend** - idle_detector + session fields + manager methods + wiring en pty read loop
2. **IPC wiring** - types.ts + ipc.ts
3. **Frontend store** - sessions.ts + App.tsx subscriptions
4. **CSS + rendering** - variables.css + sidebar.css + SessionItem.tsx
5. **Verificación visual** - seguir el checklist de CLAUDE.md

---

## Decisión UX

Cuando la sesión activa (la que el usuario está mirando) queda idle, el dot se pone verde también. Esto es consistente y no confunde - el dot indica estado I/O, no estado de foco.

---

## Estado

- [ ] Phase 1: Rust backend
- [ ] Phase 2: IPC wiring
- [ ] Phase 3: Frontend store + subscriptions
- [ ] Phase 4: CSS + dot rendering
- [ ] Phase 5: Verificación visual
