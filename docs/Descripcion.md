# Agents Commander — Descripción del Producto

## Qué es

Agents Commander es una aplicación de escritorio para Windows que permite a un humano operar una **Dark Factory**: una organización de agentes AI autónomos que trabajan en paralelo, se comunican entre sí, y producen software sin intervención humana constante.

La aplicación tiene dos capas:

1. **SummonGate** — La interfaz visual. Dos ventanas sincronizadas: un sidebar con la lista de sesiones de terminal, y una ventana terminal con xterm.js renderizando el PTY activo. El humano ve aquí qué está haciendo cada agente.

2. **La Dark Factory** — La capa organizacional. Una estructura de carpetas donde cada agente AI es un directorio con un `CLAUDE.md` que define su identidad. Los agentes se organizan en neighborhoods (por proximidad de carpeta) y teams (por función), se comunican via archivos en sus inboxes, y escalan al humano cuando no pueden resolver algo solos.

---

## Conceptos Fundamentales

### Agente

Un agente es una instancia de un LLM (Claude, Codex, o cualquier otro) que opera dentro de un directorio propio. Su identidad está definida por el archivo `CLAUDE.md` en la raíz de su carpeta. Ese archivo contiene el role prompt que le dice al LLM quién es, qué sabe hacer, y cuáles son sus responsabilidades.

Regla inviolable: un directorio de agente no puede contener otro `CLAUDE.md` en ningún subdirectorio. Un folder, un agente, una identidad.

Para instanciar un agente basta con abrir un proceso LLM apuntando a su directorio:

```
claude --cwd .factory/platform/rust-core
codex --cwd .factory/platform/rust-core
```

El LLM lee el `CLAUDE.md`, adopta ese rol, y comienza a operar. No hay configuración adicional. El folder es el agente.

Cada agente tiene dentro de su folder:
- `CLAUDE.md` — Su identidad y role prompt.
- `inbox/` — Donde recibe mensajes de otros agentes.
- `work/` — Donde deja sus artifacts de trabajo.

### Neighborhood

Un neighborhood es el directorio padre donde viven varios agentes. Representa proximidad física en la estructura de carpetas. Los agentes dentro de un mismo neighborhood pueden ver quién más está ahí (por filesystem), pero no pueden comunicarse a menos que pertenezcan al mismo team.

Ejemplo: el neighborhood `platform/` contiene los agentes `rust-core`, `api-gateway`, y `db-ops`. Están cerca en la estructura pero solo se hablan si un team los agrupa.

### Team

Un team es una agrupación lógica de agentes que cruza neighborhoods. Se define en `TEAMS.toml`, no en la estructura de carpetas. Un agente puede pertenecer a múltiples teams.

Un team tiene:
- Un nombre.
- Una lista de miembros (paths a los folders de los agentes).
- Un coordinator designado.

Los miembros de un team pueden comunicarse libremente entre sí. Un miembro no puede enviar mensajes a agentes que no estén en ninguno de sus teams.

### Coordinator

Uno de los miembros de cada team es el coordinator. El coordinator tiene un privilegio adicional: puede comunicarse con coordinators de otros teams. Esto crea una jerarquía de comunicación que evita el caos:

- Intra-team: cualquier miembro habla con cualquier miembro.
- Inter-team: solo coordinator habla con coordinator.

Si un agente necesita algo de otro team, el flujo es: miembro → su coordinator → coordinator del otro team → miembro destino.

### Factory

La factory es el directorio raíz (`.factory/`) que contiene toda la organización. Dentro tiene:
- Los neighborhoods (subdirectorios con agentes).
- `FACTORY.md` — Reglas y políticas globales que todos los agentes deben respetar.
- `TEAMS.toml` — Definición de todos los teams y sus coordinadores.
- `PHONEBOOK.toml` — Registry en tiempo real de qué agentes están online, idle, o offline.
- `_broadcast/` — Mensajes dirigidos a todos los agentes.
- `_escalations/` — Pedidos de intervención humana.

### Comunicación

Los agentes se comunican escribiendo archivos markdown en el `inbox/` del destinatario. Cada mensaje tiene un frontmatter YAML con metadata (quién envía, a quién, desde qué team, timestamp, prioridad, status de lectura).

El filesystem es la fuente de verdad. La comunicación HTTP que ofrece SummonGate es un acelerador (notificaciones en tiempo real, polling reducido), pero no reemplaza los archivos. Si SummonGate se cae, los agentes siguen pudiendo leer y escribir archivos en sus inboxes.

### Escalation

Cuando un agente no puede resolver un problema por sí solo y ya agotó la comunicación con su team, escribe un archivo en `_escalations/`. SummonGate detecta esto y notifica al humano via la UI o via el bridge de Telegram.

---

## La Aplicación: SummonGate

### Arquitectura

SummonGate está construida con Tauri 2.x (Rust backend) + SolidJS (frontend) + xterm.js (terminal emulation). Tiene dos ventanas independientes:

- **Sidebar Window**: Lista de sesiones de terminal. Cada sesión puede ser un shell normal, un agente Claude, o un agente Codex. Muestra estado (idle/busy), nombre, grupo, y color. Permite crear, renombrar, cerrar, y cambiar entre sesiones.

- **Terminal Window**: Renderiza el PTY de la sesión activa con xterm.js y WebGL. Es donde el humano ve lo que el agente está haciendo en tiempo real.

### PTY Flow

Cada sesión de terminal tiene un pseudo-terminal (PTY) manejado por Rust con la crate `portable-pty` (ConPTY en Windows). El flujo es:

1. El humano (o el orchestrator) envía bytes al PTY stdin via un Tauri Command.
2. El PTY produce output que Rust lee en un loop async con tokio.
3. Rust emite un evento Tauri con los bytes de output.
4. xterm.js en el frontend recibe el evento y renderiza.

### HTTP API

SummonGate expone un servidor HTTP local (puerto 19860) para que procesos externos puedan crear, monitorear, y destruir sesiones programáticamente. Esto permite que un orchestrator externo (big-board) lance agentes sin interacción humana.

### Telegram Bridge

Cada sesión puede tener un bridge bidireccional con un bot de Telegram. El output del PTY se limpia (ANSI stripping, spinner filtering via emulador vt100), se formatea, y se envía al chat de Telegram. Los mensajes entrantes desde Telegram se inyectan como input en el PTY del agente.

---

## Stack Técnico

| Capa | Tecnología |
|---|---|
| App framework | Tauri 2.x |
| Backend | Rust + tokio |
| Frontend | SolidJS + TypeScript |
| Terminal | xterm.js (WebGL addon) |
| PTY | portable-pty (ConPTY) |
| Estilos | CSS vanilla + CSS variables |
| Persistencia | TOML y JSON en `~/.summongate/` |
| IPC | Tauri Commands (frontend→backend) + Events (backend→frontend) |

---

## Principios de Diseño

1. **Files over databases**: Toda la persistencia es archivos planos (TOML, JSON, markdown). No hay base de datos. Esto hace todo legible, versionable, y debuggeable.

2. **No MCP**: No se usa Model Context Protocol. La comunicación entre agentes es via filesystem. La comunicación con SummonGate es via Tauri IPC o HTTP.

3. **Folder = Identity**: Un agente existe porque su folder con `CLAUDE.md` existe. No hay otro registro de identidad.

4. **Tool-agnostic**: La misma estructura de factory funciona con Claude Code, Codex, o cualquier LLM futuro que lea un directorio de trabajo. El `CLAUDE.md` es el contrato universal.

5. **Human-observable**: El estado completo de la factory es visible haciendo `ls` y `cat`. No se necesita UI especial para entender qué está pasando.

6. **Git-native**: La estructura, la comunicación, y la organización se versionan con git.

---

## Resumen en una línea

Agents Commander es el panel de control de una fábrica de agentes AI: les da identidad via carpetas, organización via teams, comunicación via archivos, y visibilidad al humano via terminales y Telegram.
