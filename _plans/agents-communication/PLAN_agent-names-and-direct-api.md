# Plan: Agent Extended Names + Direct Communication API

**Branch:** `feature/agent-direct-communication`
**Date:** 2026-03-24

---

## Contexto

Hoy los agentes se identifican solo por el nombre del directorio final donde corren (ej: `agentscommander_2`). Esto genera colisiones cuando hay repos con el mismo nombre en distintas ubicaciones, y no da suficiente contexto para identificar inequivocamente a un agente. Ademas, no existe un mecanismo para que los agentes (los procesos CLI corriendo en las PTY sessions) puedan enviar mensajes directamente a otros agentes instanciados.

---

## Parte 1: Nombres extendidos de agentes

### Problema actual

En `src-tauri/src/commands/repos.rs:40`, el nombre del repo se toma con `path.file_name()` — solo el ultimo componente:

```
C:\Users\maria\0_repos\agentscommander_2  →  "agentscommander_2"
```

### Cambio deseado

Incluir el directorio padre:

```
C:\Users\maria\0_repos\agentscommander_2  →  "0_repos/agentscommander_2"
```

### Archivos a modificar

#### 1. `src-tauri/src/commands/repos.rs` — `try_add_repo()`

Cambiar la derivacion del `name` (linea 40):

```rust
// ANTES:
let name = match path.file_name().and_then(|n| n.to_str()) {
    Some(n) => n.to_string(),
    None => return,
};

// DESPUES:
let name = {
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return,
    };
    match path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
        Some(parent) => format!("{}/{}", parent, file_name),
        None => file_name.to_string(),
    }
};
```

**Impacto cascada** — el `name` extendido fluye automaticamente a:
- `RepoMatch.name` → se envia al frontend
- `OpenAgentModal.tsx:108` → `sessionName: repo.name` → nombre de la session
- Session sidebar → muestra el nombre extendido
- Phone system → si se usa el session name como agent identifier

#### 2. `src-tauri/src/commands/repos.rs` — filtro de busqueda (linea 50)

El filtro `name.to_lowercase().contains(query_lower)` ya funciona — buscar "agents" matchea `0_repos/agentscommander_2`. No requiere cambio.

#### 3. `src-tauri/src/commands/repos.rs` — filtro DEPRECATED (linea 46)

Hoy filtra `name.starts_with("DEPRECATED")`. Con el nombre extendido, el parent dir podria ser "DEPRECATED_stuff/repo". Ajustar para que el check se haga sobre el file_name solamente, no sobre el nombre extendido completo. Guardar el `file_name` por separado antes de construir el nombre extendido.

#### 4. Frontend — Sidebar session list display

El nombre extendido puede ser largo para el sidebar. Dos opciones:
- **Opcion A**: Mostrar completo con CSS truncation (`text-overflow: ellipsis`)
- **Opcion B**: Mostrar solo el file_name en la lista, nombre completo en tooltip

**Recomendacion**: Opcion A — mostrar completo. El sidebar ya tiene text truncation. El nombre extendido da contexto visual util.

#### 5. `teams.json` — DarkFactory agent names

Los `TeamMember.name` en `teams.json` son definidos manualmente. Si queremos consistencia, el nombre en teams.json deberia matchear el formato extendido. Esto es un cambio de convencion, no de codigo. Documentar que el formato es `parent/repo`.

**Nota**: Esto NO es un cambio breaking porque `teams.json` ya permite cualquier string como name. Solo se documenta la convencion.

---

## Parte 2: API de comunicacion directa para agentes

### Problema

Los agentes (Claude Code, Codex, etc.) corren como procesos CLI dentro de PTY sessions. Hoy no tienen forma de descubrir ni enviar mensajes a otros agentes instanciados. El Phone system existe pero solo es accesible via Tauri IPC (desde el frontend).

### Solucion propuesta: File-based Agent Mailbox API

Exponer la comunicacion via filesystem — el mecanismo mas universal para procesos CLI. Cada agente tiene un directorio de buzón en su workspace.

#### Arquitectura

```
<repo>/.agentscommander/
├── config.json                  # Ya existe — team memberships
├── inbox/                       # NUEVO — mensajes entrantes
│   ├── 0001-from_sender.json    # Mensaje pendiente
│   └── 0002-from_other.json
├── outbox/                      # NUEVO — mensajes salientes
│   └── 0003-to_recipient.json   # Mensaje a enviar
└── agents.json                  # NUEVO — directorio de agentes activos
```

#### Flujo de comunicacion

```
Agente A quiere enviar mensaje a Agente B:
1. Agente A escribe archivo en su propio outbox/:
   <repo_A>/.agentscommander/outbox/msg-to-<agent_B_name>.json

2. agentscommander detecta el archivo nuevo (filesystem watcher)

3. agentscommander valida routing (can_communicate)

4. agentscommander mueve el mensaje al inbox/ de Agente B:
   <repo_B>/.agentscommander/inbox/msg-from-<agent_A_name>.json

5. Agente B puede leer su inbox/ (polling o watcher propio)

6. Agente B escribe en su outbox/ para responder → ciclo se repite
```

#### Formato del mensaje (outbox)

```json
{
  "to": "0_repos/agentscommander_2",
  "body": "Necesito que revises el endpoint /api/health",
  "priority": "normal",
  "timestamp": "2026-03-24T15:30:00Z"
}
```

#### Formato del mensaje (inbox)

```json
{
  "id": "uuid",
  "from": "0_repos/project_x",
  "body": "Necesito que revises el endpoint /api/health",
  "priority": "normal",
  "timestamp": "2026-03-24T15:30:00Z",
  "status": "unread"
}
```

#### `agents.json` — Directorio de agentes activos

agentscommander mantiene un archivo `agents.json` en el config dir global Y lo sincroniza a cada `<repo>/.agentscommander/agents.json` para que los agentes puedan descubrir peers:

```json
{
  "updatedAt": "2026-03-24T15:30:00Z",
  "agents": [
    {
      "name": "0_repos/agentscommander_2",
      "path": "C:\\Users\\maria\\0_repos\\agentscommander_2",
      "status": "active",
      "sessionId": "uuid",
      "teams": ["dev-core"]
    },
    {
      "name": "0_repos/project_x",
      "path": "C:\\Users\\maria\\0_repos\\project_x",
      "status": "active",
      "sessionId": "uuid",
      "teams": ["dev-core"]
    }
  ]
}
```

### Implementacion — Archivos a crear/modificar

#### Backend (Rust)

| Archivo | Accion | Que |
|---|---|---|
| `src-tauri/src/phone/mailbox.rs` | **CREAR** | FileSystem watcher para outbox/ dirs. Procesa mensajes salientes, rutea a inbox/ de destinatario |
| `src-tauri/src/phone/agent_registry.rs` | **CREAR** | Mantiene `agents.json` sincronizado. Registra/desregistra agentes cuando sessions se crean/destruyen |
| `src-tauri/src/phone/mod.rs` | MODIFICAR | Agregar submodulos mailbox y agent_registry |
| `src-tauri/src/session/manager.rs` | MODIFICAR | Al crear/destruir session → registrar/desregistrar en agent_registry |
| `src-tauri/src/commands/phone.rs` | MODIFICAR | Agregar comando `list_active_agents` para el frontend |

#### Frontend (TypeScript)

| Archivo | Accion | Que |
|---|---|---|
| `src/shared/types.ts` | MODIFICAR | Agregar `ActiveAgent` interface |
| `src/shared/ipc.ts` | MODIFICAR | Agregar `PhoneAPI.listActiveAgents()` |

#### Config sync

| Archivo | Accion | Que |
|---|---|---|
| `src-tauri/src/config/dark_factory.rs` — `sync_agent_configs()` | MODIFICAR | Ademas del `config.json`, sincronizar `agents.json` a cada agent dir |

---

## Orden de ejecucion

### Step 1: Nombre extendido (Parte 1)
1. Modificar `try_add_repo()` en `repos.rs`
2. Ajustar filtro DEPRECATED
3. Verificar que el frontend muestra correctamente
4. Test manual: abrir modal, verificar nombres

### Step 2: Agent Registry (Parte 2a)
1. Crear `agent_registry.rs` — struct que trackea sessions activas
2. Integrar con `SessionManager` — register on create, unregister on destroy
3. Escribir `agents.json` global y per-agent
4. Test manual: crear sessions, verificar que `agents.json` se actualiza

### Step 3: Mailbox watcher (Parte 2b)
1. Crear `mailbox.rs` — filesystem watcher en outbox/ dirs
2. Implementar ruteo: outbox → validacion → inbox
3. Integrar con phone/manager.rs can_communicate
4. Test manual: escribir archivo en outbox/, verificar que llega a inbox/

### Step 4: Frontend integration
1. Mostrar agentes activos en algun panel
2. Notificar cuando llega mensaje a inbox

---

## Consideraciones

- **Performance**: El filesystem watcher (notify crate) es eficiente. Un watcher por session activa.
- **Limpieza**: Mensajes procesados del outbox se eliminan o mueven a un `outbox/processed/`.
- **Colisiones de nombre**: Con el formato `parent/repo`, las colisiones son practicamente eliminadas. Si aun asi hay colision (dos repos con mismo parent + name), el `path` absoluto sirve como desambiguador.
- **Seguridad**: Solo se rutean mensajes entre agentes que pasan `can_communicate()`. Mensajes a destinatarios desconocidos se rechazan y se loguean.
- **Formato forward-slash**: Siempre usar `/` como separador en el nombre extendido, independientemente del OS. Normalizar `\` a `/` en Windows.
