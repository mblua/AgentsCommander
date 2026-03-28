# PLAN: Organigrama Dark Factory

## Objetivo

Crear una vista de **organigrama horizontal (izquierda a derecha)** accesible desde la Sidebar, que muestre la estructura jerarquica de equipos de agentes organizados en **Layers**. Cada nodo del organigrama es un Team con un Coordinator que actua como punto de entrada de solicitudes y canal de escalamiento.

---

## Conceptos Clave

### Layers
- Niveles jerarquicos del organigrama (Layer 1, Layer 2, Layer 3...)
- Configurables desde Settings > Dark Factory
- Cada Layer tiene un nombre (ej: "C-Suite", "Directors", "Teams")
- Los Teams se asignan a una Layer
- **El orden se determina por la posicion en el array `layers[]`** — no hay campo `order` explicito. El indice 0 es el mas a la izquierda (jerarquia mas alta).

### Teams (ya existente, se extiende)
- Cada Team tiene: nombre, miembros, coordinador
- **Nuevo**: cada Team se asigna a una Layer (`layerId`)
- El coordinador es la via de ingreso de solicitudes al equipo

### Vinculos Coordinator-to-Coordinator
- Un coordinador de Layer N puede tener **supervisores** en Layer N-1
- Un coordinador de Layer N puede **supervisar** coordinadores en Layer N+1
- Estos vinculos definen:
  - Quien puede dar indicaciones al coordinador (supervisor → coordinador)
  - A quien escala informacion el coordinador (coordinador → supervisor)

### Alcance de CoordinatorLink — FUNCIONAL

`CoordinatorLink` es **funcional** — no solo dibuja lineas en el organigrama, sino que **habilita comunicacion cross-team** entre coordinadores vinculados y se propaga al sistema de mensajeria.

#### Que habilita un CoordinatorLink

Dado un link `{ supervisorTeamId: "team-cto", subordinateTeamId: "team-alpha" }`:

1. **El coordinador de `team-cto`** puede enviar mensajes al **coordinador de `team-alpha`** (directivas top-down)
2. **El coordinador de `team-alpha`** puede enviar mensajes al **coordinador de `team-cto`** (escalamiento bottom-up)
3. **Miembros regulares** de ambos teams NO pueden comunicarse cross-team — solo los coordinadores vinculados

#### Impacto en el sistema de mensajeria

**`can_communicate()` en `phone/manager.rs`** — actualmente solo permite comunicacion intra-team. Se extiende con una segunda via:

```rust
// Regla actual (se mantiene):
// from y to estan en el mismo team → permitido (con coordinator gating)

// Regla nueva (se agrega):
// from es coordinador de team A, to es coordinador de team B,
// y existe un CoordinatorLink entre A y B → permitido
```

Logica concreta a agregar en `can_communicate`:
```rust
// After the existing shared_teams check, before returning false:
// Check cross-team coordinator links
for link in &config.coordinator_links {
    let sup_team = config.teams.iter().find(|t| t.id == link.supervisor_team_id);
    let sub_team = config.teams.iter().find(|t| t.id == link.subordinate_team_id);
    if let (Some(sup), Some(sub)) = (sup_team, sub_team) {
        // SECURITY: validate coordinator is actual member of the team
        let from_is_sup_coord = sup.coordinator_name.as_deref() == Some(from)
            && sup.members.iter().any(|m| m.name == from);
        let from_is_sub_coord = sub.coordinator_name.as_deref() == Some(from)
            && sub.members.iter().any(|m| m.name == from);
        let to_is_sup_coord = sup.coordinator_name.as_deref() == Some(to)
            && sup.members.iter().any(|m| m.name == to);
        let to_is_sub_coord = sub.coordinator_name.as_deref() == Some(to)
            && sub.members.iter().any(|m| m.name == to);
        // Bidireccional: supervisor coord ↔ subordinate coord
        if (from_is_sup_coord && to_is_sub_coord)
            || (from_is_sub_coord && to_is_sup_coord)
        {
            return true;
        }
    }
}
```

**Edge cases documentados:**
- **Link sin coordinator en un team**: si `coordinator_name` es `None` en cualquiera de los dos teams, el link es **inactivo** para comunicacion (los checks son `false`). Comportamiento correcto: no se puede enviar a un team sin coordinador designado.
- **Mismo agente coordinador de ambos teams**: caso degenerado, no causa problemas porque `from != to` en practica (no se envia a uno mismo).
- **Link referencia team eliminado/renombrado**: `find()` retorna `None`, el link es silenciosamente ignorado.
- **Security**: cada check valida que el `coordinator_name` es tambien miembro real del team (`members.iter().any()`). Sin esta validacion, un `config.json` manualmente editado podria asignar `coordinator_name: "victim"` a un team donde "victim" no es miembro, otorgando acceso cross-team no autorizado.

#### Propagacion a per-agent configs

**`AgentLocalConfig` en `dark_factory.rs`** — agregar `#[derive(Default)]` y dos campos nuevos:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocalConfig {
    pub teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub is_coordinator_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_coding_agent: Option<String>,
    // NUEVOS:
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supervises: Vec<String>,        // teams que este coordinador supervisa
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports_to: Vec<String>,        // teams cuyos coordinadores supervisan a este
}
```

> **CRITICO — `#[derive(Default)]`**: `AgentLocalConfig` se construye manualmente con struct literals en dos call-sites: `set_last_coding_agent` (dark_factory.rs:54-65, dos ocurrencias: fallback `unwrap_or` y branch `else`) y `sync_agent_configs` (dark_factory.rs:167). Al agregar campos nuevos, **todos esos call-sites fallan en compilacion**. La solucion es agregar `#[derive(Default)]` al struct y reemplazar los struct literals por `AgentLocalConfig { ..specific fields.., ..Default::default() }` o `unwrap_or_default()`.

**`sync_agent_configs()` en `dark_factory.rs`** — se extiende para propagar `supervises` y `reports_to`:

```rust
// ALGORITMO para poblar supervises/reports_to en sync_agent_configs:
//
// Despues de construir el agent_map existente (path → teams, coordinator_of),
// agregar un segundo pase sobre coordinator_links:

// Tipo extendido del mapa:
// agent_map: HashMap<String, AgentSyncData>
// struct AgentSyncData { teams, coordinator_of, supervises, reports_to }

for link in &config.coordinator_links {
    let sup_team = config.teams.iter().find(|t| t.id == link.supervisor_team_id);
    let sub_team = config.teams.iter().find(|t| t.id == link.subordinate_team_id);

    // Skip link si alguno de los teams no existe o no tiene coordinator
    let (sup, sub) = match (sup_team, sub_team) {
        (Some(s), Some(t)) => (s, t),
        _ => continue,  // team eliminado/renombrado → skip
    };
    let sup_coord = match &sup.coordinator_name {
        Some(name) if sup.members.iter().any(|m| &m.name == name) => name,
        _ => continue,  // sin coordinator o coordinator no es miembro → skip
    };
    let sub_coord = match &sub.coordinator_name {
        Some(name) if sub.members.iter().any(|m| &m.name == name) => name,
        _ => {
            // IMPORTANTE: si el subordinado no tiene coordinator,
            // NO escribir supervises al supervisor (seria asimetrico).
            // Log warning y skip.
            log::warn!("CoordinatorLink skip: team '{}' has no valid coordinator", sub.name);
            continue;
        }
    };

    // Encontrar paths de ambos coordinadores
    let sup_path = sup.members.iter().find(|m| &m.name == sup_coord).map(|m| &m.path);
    let sub_path = sub.members.iter().find(|m| &m.name == sub_coord).map(|m| &m.path);

    if let Some(path) = sup_path {
        agent_map.entry(path.clone()).or_default()
            .supervises.push(sub.name.clone());
    }
    if let Some(path) = sub_path {
        agent_map.entry(path.clone()).or_default()
            .reports_to.push(sup.name.clone());
    }
}
```

**Regla de simetria**: un link solo se propaga si **ambos** teams tienen un coordinator valido (que sea miembro real). Si uno no lo tiene, el link se ignora completamente para evitar escrituras asimetricas donde un agente ve `supervises: [X]` pero ningun agente en X tiene `reports_to`.

Esto permite que cada agente sepa, al leer su `config.json` local:
- `supervises: ["Team Alpha", "Team Beta"]` → "puedo dar directivas a los coordinadores de estos teams"
- `reports_to: ["CTO Team"]` → "escalo informacion al coordinador de este team"

#### `send_message` y el parametro `team` en mensajes cross-team

`send_message()` en `phone/manager.rs:121` requiere `team: &str` que se almacena en cada `PhoneMessage`. Para mensajes intra-team esto es claro. Para mensajes cross-team entre coordinadores vinculados, no hay un team unico.

**Convencion**: usar el formato `"<supervisor_team> → <subordinate_team>"` como valor de `team` para mensajes cross-team. Ejemplo: `"CTO Team → Team Alpha"`. Esto preserva la trazabilidad del canal por el que se envio el mensaje. Alternativamente, hacer `team` `Option<String>` con `None` para cross-team — pero esto requiere cambiar la firma de `send_message` y todos sus callers.

**Decision**: usar la convencion del formato string por ahora (menor impacto). El error message en `manager.rs:131` ("not in the same team or must go through coordinator") debe actualizarse a: `"Agent '{}' cannot communicate with '{}' — no shared team, no coordinator link, or must go through coordinator"`.

#### Archivos impactados

| Archivo | Cambio |
|---------|--------|
| `src-tauri/src/phone/manager.rs` | Extender `can_communicate()` con regla de coordinator links + actualizar error message |
| `src-tauri/src/config/dark_factory.rs` | `#[derive(Default)]` en `AgentLocalConfig`, campos `supervises`/`reports_to`, extender `sync_agent_configs()` con algoritmo concreto |
| `src/shared/types.ts` | Agregar `supervises`/`reportsTo` a tipos si se exponen al frontend |

---

## Modelo de Datos

### Cambios en `types.ts`

```typescript
// Nueva interfaz
export interface DarkFactoryLayer {
  id: string;
  name: string;       // ej: "C-Suite", "Directors", "Operations"
  // El orden se determina por la posicion en el array layers[]
}

// Nueva interfaz para vinculos entre coordinadores
export interface CoordinatorLink {
  supervisorTeamId: string;    // team del supervisor (layer superior)
  subordinateTeamId: string;   // team del supervisado (layer inferior)
}

// Extender Team existente
export interface Team {
  id: string;
  name: string;
  members: TeamMember[];
  coordinatorName?: string;
  layerId?: string;           // NUEVO: a que layer pertenece
}

// Extender DarkFactoryConfig existente
export interface DarkFactoryConfig {
  teams: Team[];
  layers?: DarkFactoryLayer[];         // NUEVO (optional para migracion)
  coordinatorLinks?: CoordinatorLink[]; // NUEVO (optional para migracion)
}
```

### Cambios en Rust (backend) — `src-tauri/src/config/dark_factory.rs`

```rust
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DarkFactoryLayer {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorLink {
    pub supervisor_team_id: String,
    pub subordinate_team_id: String,
}

// Extender Team existente — CRITICO: serde attributes para backward compat
pub struct Team {
    // ... campos existentes ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_id: Option<String>,
}

// Extender DarkFactoryConfig — CRITICO: #[serde(default)] en campos nuevos
pub struct DarkFactoryConfig {
    pub teams: Vec<Team>,
    #[serde(default)]
    pub layers: Vec<DarkFactoryLayer>,
    #[serde(default)]
    pub coordinator_links: Vec<CoordinatorLink>,
}
```

> **CRITICO**: Sin `#[serde(default)]` en `layers` y `coordinator_links`, y sin `#[serde(default, skip_serializing_if = "Option::is_none")]` en `layer_id`, los `teams.json` existentes fallan al deserializar y `load_dark_factory()` retorna `DarkFactoryConfig::default()`, **perdiendo silenciosamente todos los teams existentes**.

---

## Arquitectura de la Vista

### Acceso: Boton en Sidebar

**Ubicacion**: entre el boton de Bombilla (Hints) y el boton de Settings en `TeamFilter.tsx`.

```
[Eye] [Lightbulb] [🏭 NEW] [Settings]
```

- Icono: `&#x1F3ED;` (fabrica) o un SVG custom minimalista
- Tooltip: "Dark Factory"
- Al hacer click: abre una **nueva ventana Tauri** (como Guide), no un modal

### Nueva ventana: Dark Factory

Similar a la ventana Guide (`?window=darkfactory`):

1. **Registro en `main.tsx`**: agregar caso `windowType === "darkfactory"`
2. **Comando Rust**: `open_darkfactory_window` (similar a `open_guide_window`)
3. **Frontend**: `src/darkfactory/App.tsx`

**Singleton guard obligatorio**: el comando Rust debe verificar `app.get_webview_window("darkfactory")` antes de crear — si ya existe, solo hacer `set_focus()`. Esto previene multiples instancias con recalculo SVG costoso.

### Estructura de la ventana

```
DarkFactoryApp
├── Titlebar (icon + "dark factory" + minimize/close)
├── Toolbar (zoom controls, fullscreen toggle)
└── OrgChart (area principal scrolleable)
    ├── LayerColumn (por cada layer, de izq a der)
    │   ├── LayerHeader ("Layer 1: C-Suite")
    │   └── TeamNode[] (cards de cada team en esa layer)
    │       ├── TeamName
    │       ├── CoordinatorBadge
    │       └── MemberCount
    └── ConnectionLines (lineas SVG entre coordinadores vinculados)
```

---

## Plan de Implementacion

### Fase 1: Modelo de datos y Settings

**Archivos a modificar:**
- `src/shared/types.ts` — agregar `DarkFactoryLayer`, `CoordinatorLink`, extender `Team` y `DarkFactoryConfig`
- `src-tauri/src/config/dark_factory.rs` — structs Rust con atributos serde correctos (ver seccion Modelo de Datos)
- `src/sidebar/components/SettingsModal.tsx` — agregar UI para:
  - CRUD de Layers (nombre, reordenar arrastrando o con botones up/down)
  - Asignar Team a Layer (dropdown en cada team card)
  - Crear vinculos entre coordinadores (selector "Reports to")
  - **FIX save handler**: cambiar `DarkFactoryAPI.save({ teams: [...dfConfig.teams] })` a `DarkFactoryAPI.save({ ...dfConfig })` para no descartar los campos nuevos
  - **FIX createStore init**: cambiar `createStore<DarkFactoryConfig>({ teams: [] })` a `createStore<DarkFactoryConfig>({ teams: [], layers: [], coordinatorLinks: [] })` — sin esto, `dfConfig.layers` y `dfConfig.coordinatorLinks` son `undefined` antes del mount y el JSX falla

**Detalle de UI en Settings > Dark Factory:**

```
[Layers]
  ┌──────────────────────────────────┐
  │ Layer 1: C-Suite        [Edit][X]│
  │ Layer 2: Directors      [Edit][X]│
  │ Layer 3: Operations     [Edit][X]│
  │         [+ Add Layer]            │
  └──────────────────────────────────┘

[Teams]  (existente, se extiende)
  ┌──────────────────────────────────┐
  │ Team: Alpha Squad                │
  │ Layer: [dropdown: Layer 2 ▼]     │
  │ Coordinator: AgentX              │
  │ Reports to: [dropdown: CEO Team] │
  │ Members: 4                       │
  └──────────────────────────────────┘
```

El campo "Reports to" crea un `CoordinatorLink` donde `supervisorTeamId` es el team seleccionado y `subordinateTeamId` es el team actual.

**Paso 1b — Extender `AgentLocalConfig`, `sync_agent_configs`, `can_communicate`, y `save_dark_factory`**:
- `AgentLocalConfig` en `dark_factory.rs:29`: agregar `#[derive(Default)]`, campos `supervises` y `reports_to`. Actualizar los 3 call-sites que construyen struct literals (`set_last_coding_agent` x2, `sync_agent_configs` x1) para usar `..Default::default()`
- `sync_agent_configs()` en `dark_factory.rs:139`: agregar segundo pase sobre `coordinator_links` con regla de simetria (skip si un team no tiene coordinator valido). Ver code sketch arriba
- `can_communicate()` en `phone/manager.rs:14`: agregar regla de coordinator links con membership validation. Actualizar error message en linea 131
- `save_dark_factory()` en `dark_factory.rs:121`: agregar validacion de ciclos en `coordinator_links` y sanitizacion de `coordinator_name` (debe ser miembro real del team)

### Fase 2: Boton en Sidebar + Ventana Tauri + Zoom

**Archivos a crear:**
- `src/darkfactory/App.tsx` — componente principal
- `src/darkfactory/styles/darkfactory.css` — estilos (importar `../../terminal/styles/variables.css`)
- `src/darkfactory/components/OrgChart.tsx` — layout del organigrama
- `src/darkfactory/components/LayerColumn.tsx` — columna por layer
- `src/darkfactory/components/TeamNode.tsx` — card de cada team
- `src/darkfactory/components/ConnectionLines.tsx` — lineas SVG

**Archivos a modificar:**
- `src/main.tsx` — agregar caso `"darkfactory"`
- `src/sidebar/components/TeamFilter.tsx` — agregar boton Dark Factory
- `src/shared/ipc.ts` — agregar `DarkFactoryAPI.openWindow()`
- `src-tauri/src/commands/window.rs` — agregar `open_darkfactory_window` con singleton guard
- `src-tauri/src/lib.rs` — registrar el nuevo command (no se necesitan cambios en `commands/mod.rs` porque `window.rs` ya es modulo)
- `src-tauri/tauri.conf.json` — verificar capabilities (mismos permisos que Guide)

**Paso 2b — Zoom system** (archivos adicionales que el plan original omitia):

| Archivo | Cambio |
|---------|--------|
| `src/shared/zoom.ts` | Agregar `"darkfactory"` a `WindowType`, reemplazar ternarios por lookup |
| `src/shared/types.ts` (`AppSettings`) | Agregar `darkfactoryZoom: number`, `guideZoom: number` |
| `src-tauri/src/config/settings.rs` | Agregar `darkfactory_zoom: f64` y `guide_zoom: f64` con `#[serde(default = "default_zoom")]` |

> **CRITICO**: `zoom.ts` usa ternarios binarios (`windowType === "sidebar" ? sidebarZoom : terminalZoom`) en dos funciones: `debouncedSave` y `initZoom`. Solo agregar `"darkfactory"` al tipo NO cambia la logica — cae al branch `else` y escribe a `terminalZoom`. Hay que reemplazar ambos ternarios por un lookup:

```typescript
// zoom.ts — reemplazo de ternarios en debouncedSave e initZoom:
type WindowType = "sidebar" | "terminal" | "guide" | "darkfactory";

const zoomKeyMap: Record<WindowType, keyof AppSettings> = {
  sidebar: "sidebarZoom",
  terminal: "terminalZoom",
  guide: "guideZoom",
  darkfactory: "darkfactoryZoom",
};

// En debouncedSave:
const key = zoomKeyMap[windowType];

// En initZoom:
const saved = settings[zoomKeyMap[windowType]] as number;
```

> Esto tambien corrige el bug preexistente donde `"guide"` cae al branch `else` y escribe a `terminalZoom`.

### Fase 3: Render del Organigrama

**Layout engine** (CSS Grid + SVG):

```
┌─────────────────────────────────────────────────────────────┐
│                     DARK FACTORY                            │
│                                                             │
│  Layer 1          Layer 2           Layer 3                 │
│  ┌────────┐      ┌────────┐       ┌────────┐              │
│  │  CEO   │──────│  CTO   │───────│ Team A │              │
│  │  Team  │      │  Team  │       └────────┘              │
│  └────────┘      └────────┘       ┌────────┐              │
│                  │  │              │ Team B │              │
│                  │  └─────────────│        │              │
│                  │                 └────────┘              │
│                  ┌────────┐       ┌────────┐              │
│                  │  COO   │───────│ Team C │              │
│                  │  Team  │       └────────┘              │
│                  └────────┘                                │
└─────────────────────────────────────────────────────────────┘
```

**Estrategia de rendering:**

1. **Columnas con CSS Grid**: cada Layer es una columna. `grid-template-columns: repeat(N, 1fr)` donde N = cantidad de layers
2. **Nodos con Flexbox**: dentro de cada columna, los teams se apilan verticalmente con gap
3. **Conexiones con SVG overlay**: un `<svg>` posicionado `absolute` sobre todo el grid. Las lineas se calculan a partir de las posiciones DOM de los nodos (via `getBoundingClientRect`)
4. **Scroll horizontal**: si hay muchas layers, el contenedor tiene `overflow-x: auto`

**Calculo de posiciones para lineas:**
- Cada `TeamNode` reporta su posicion al padre via callback `onMount`
- `ConnectionLines` recibe array de `CoordinatorLink` + mapa de posiciones
- Dibuja paths SVG curvos (bezier) de punto medio derecho del nodo origen a punto medio izquierdo del nodo destino

### Fase 4: Interactividad

- **Hover en TeamNode**: resalta las conexiones de ese team
- **Click en TeamNode**: muestra panel lateral con detalle (miembros, coordinador, vinculos)
- **Zoom**: ctrl+scroll o botones +/- en toolbar (persistido via `darkfactoryZoom`)
- **Pan**: drag en area vacia para mover viewport
- **Responsive**: font-size y node-size se ajustan al zoom

---

## Detalle de Componentes

### `TeamNode.tsx`

```
┌─────────────────────┐
│ ★ Coordinator Name  │  ← badge si es coordinador
│ ─────────────────── │
│ Team Name           │  ← nombre del equipo
│ 4 members           │  ← conteo
│ Layer 2             │  ← indicador de layer
└─────────────────────┘
```

- Color de borde izquierdo: hereda del color del agente coordinador (si tiene sesion activa)
- Estado: opacidad reducida si ningun miembro tiene sesion activa
- Tamano fijo: ~180px ancho, ~80px alto (escalable con zoom)

### `LayerColumn.tsx`

- Header fijo arriba con nombre de layer
- Contenedor flex vertical con los TeamNodes
- Fondo sutilmente diferenciado por layer (alternating opacity)

### `ConnectionLines.tsx`

- SVG overlay `position: absolute; inset: 0; pointer-events: none`
- Paths con curva bezier horizontal
- Color: `var(--statusbar-fg)` por defecto, `var(--statusbar-accent)` en hover
- Grosor: 1.5px, 2.5px en hover
- Animacion: transicion de color/grosor 150ms ease-out

> **Nota**: las variables correctas son `--statusbar-fg` y `--statusbar-accent` (definidas en `src/terminal/styles/variables.css`). No existen `--text-dim` ni `--accent` en el codebase.

---

## Configuracion en Settings

### Seccion "Layers" (nueva en Dark Factory tab)

| Campo | Tipo | Descripcion |
|-------|------|-------------|
| Layer name | text input | Nombre descriptivo del layer |
| Posicion | array index | Determinada por orden en la lista (drag o botones ↑↓) |

### Campos nuevos en Team card (existente)

| Campo | Tipo | Descripcion |
|-------|------|-------------|
| Layer | select | Dropdown con layers disponibles |
| Reports to | select | Dropdown con teams de layers superiores (cuyos coordinadores supervisan a este) |

### Validaciones

**Frontend (UI en Settings):**
- Un team solo puede "reportar a" un team de una layer con **indice menor** en el array (superior jerarquicamente) — sin esta validacion se pueden crear ciclos que rompen el layout del arbol
- Un team sin layer asignada no aparece en el organigrama
- Un layer sin teams aparece como columna vacia (placeholder)

**Backend (`save_dark_factory` en Rust):**
- Validar que `coordinator_links` no contenga ciclos: recorrer el grafo de links y rechazar con error si se detecta un ciclo. Esto es necesario porque `config.json` puede ser editado a mano, bypaseando la validacion de UI. Sin esto, `supervises` y `reports_to` de un mismo agente pueden contener el mismo team, creando una contradiccion logica.
- Validar que `coordinator_name` de cada team (si existe) sea un miembro real de ese team (`members.iter().any(|m| m.name == coord)`). Logear warning y tratar como `None` si no lo es.

---

## Orden de Ejecucion

| Paso | Descripcion |
|------|-------------|
| 1 | Extender tipos TS + Rust (`DarkFactoryLayer`, `CoordinatorLink`, extender `Team`, `DarkFactoryConfig`) con atributos serde correctos |
| 1b | Extender `AgentLocalConfig` (Default + campos) + `sync_agent_configs` (algoritmo links) + `can_communicate` (membership validation) + `save_dark_factory` (cycle check + coordinator validation) |
| 2 | UI en Settings: CRUD de Layers + asignar layer a team + selector "Reports to" + **fix save handler** |
| 3 | Boton en TeamFilter + comando Rust `open_darkfactory_window` con singleton guard + inspeccionar `src-tauri/capabilities/*.json` |
| 3b | Zoom system: agregar `darkfactoryZoom` a `AppSettings` (TS + Rust) + extender `zoom.ts` WindowType |
| 4 | Scaffold ventana: `main.tsx` routing + `DarkFactoryApp` + titlebar basico |
| 5 | `OrgChart` con layout CSS Grid de layers |
| 6 | `TeamNode` cards con datos reales |
| 7 | `ConnectionLines` SVG con bezier curves |
| 8 | Interactividad: hover highlights, zoom, pan |
| 9 | Polish: animaciones, responsive, edge cases |

---

## Archivos Completos a Tocar

### Crear
| Archivo | Descripcion |
|---------|-------------|
| `src/darkfactory/App.tsx` | Componente principal de la ventana |
| `src/darkfactory/styles/darkfactory.css` | Estilos (importa `variables.css`) |
| `src/darkfactory/components/OrgChart.tsx` | Layout del organigrama |
| `src/darkfactory/components/LayerColumn.tsx` | Columna por layer |
| `src/darkfactory/components/TeamNode.tsx` | Card de cada team |
| `src/darkfactory/components/ConnectionLines.tsx` | Lineas SVG |

### Modificar
| Archivo | Cambio |
|---------|--------|
| `src/shared/types.ts` | `DarkFactoryLayer`, `CoordinatorLink`, extender `Team`, `DarkFactoryConfig`, `AppSettings` (+zoom) |
| `src/shared/ipc.ts` | `DarkFactoryAPI.openWindow()` |
| `src/shared/zoom.ts` | `"darkfactory"` en `WindowType` + mapping zoom key |
| `src/main.tsx` | Routing `"darkfactory"` → `DarkFactoryApp` |
| `src/sidebar/components/TeamFilter.tsx` | Boton Dark Factory |
| `src/sidebar/components/SettingsModal.tsx` | UI layers/links + **fix save `{ ...dfConfig }`** + **fix createStore init** |
| `src-tauri/src/config/dark_factory.rs` | `#[derive(Default)]` en `AgentLocalConfig`, campos `supervises`/`reports_to`, extender `sync_agent_configs()` con algoritmo concreto, validacion en `save_dark_factory` (cycles + coordinator membership) |
| `src-tauri/src/phone/manager.rs` | Extender `can_communicate()` con regla de coordinator links (con membership validation) + actualizar error message |
| `src-tauri/capabilities/*.json` | Inspeccionar y agregar `"darkfactory"` a filtros de window label si existen |
| `src-tauri/src/config/settings.rs` | `darkfactory_zoom` field |
| `src-tauri/src/commands/window.rs` | `open_darkfactory_window` con singleton guard |
| `src-tauri/src/lib.rs` | Registrar nuevo command |
| `src-tauri/tauri.conf.json` | Verificar capabilities |

---

## Riesgos y Mitigaciones

| # | Riesgo | Severidad | Mitigacion |
|---|--------|-----------|------------|
| 1 | `teams.json` existentes fallan al deserializar campos nuevos | **CRITICAL** | `#[serde(default)]` en todos los campos nuevos de Rust. Testeado con archivo existente antes de merge |
| 2 | `AgentLocalConfig` struct literals no compilan al agregar campos | **CRITICAL** | `#[derive(Default)]` en `AgentLocalConfig` + usar `..Default::default()` o `unwrap_or_default()` en `set_last_coding_agent` (2 ocurrencias) y `sync_agent_configs` (1 ocurrencia) |
| 3 | `coordinator_name` no validado contra `team.members` permite acceso cross-team no autorizado | **CRITICAL** | Validar membership en `can_communicate()` (cada check incluye `&& members.iter().any()`) y en `save_dark_factory` (sanitizar coordinator_name invalidos) |
| 4 | `sync_agent_configs` escribe `supervises` asimetricamente si un team no tiene coordinator | **HIGH** | Regla de simetria: skip completo del link si alguno de los dos teams no tiene coordinator valido + log warning |
| 5 | `createStore` init en SettingsModal no incluye campos nuevos | **HIGH** | Cambiar a `{ teams: [], layers: [], coordinatorLinks: [] }` |
| 6 | Save handler descarta `layers` y `coordinatorLinks` | **HIGH** | Cambiar spread explicito a `{ ...dfConfig }` en SettingsModal |
| 7 | `zoom.ts` ternario binario ignora nuevos window types | **HIGH** | Reemplazar ternarios por `zoomKeyMap` lookup en `debouncedSave` e `initZoom` |
| 8 | `send_message` `team` parameter indefinido para cross-team | **MEDIUM** | Convencion: formato `"TeamA → TeamB"` para mensajes cross-team + actualizar error message |
| 9 | Ciclos en CoordinatorLinks crean `supervises`/`reports_to` contradictorios | **MEDIUM** | Validacion en UI (layer index) + validacion backend en `save_dark_factory` (cycle check) |
| 10 | Variables CSS inexistentes causan lineas invisibles | **MEDIUM** | Usar `--statusbar-fg` y `--statusbar-accent` (verificados en `variables.css`) |
| 11 | Multiples instancias de la ventana causan performance hit | **LOW** | Singleton guard `get_webview_window("darkfactory")` en comando Rust |
| 12 | Rendimiento SVG con muchos nodos | **LOW** | Debounce 100ms en resize via `ResizeObserver` |
| 13 | Capabilities Tauri pueden excluir ventana nueva por label | **LOW** | Inspeccionar `src-tauri/capabilities/*.json` por filtros de window label y agregar `"darkfactory"` si existen |

---

## Decisiones Explicitas

1. **`CoordinatorLink` es funcional**: habilita comunicacion cross-team entre coordinadores vinculados. Se propaga a per-agent `config.json` (`supervises`/`reports_to`) y extiende `can_communicate()` en `phone/manager.rs`.
2. **Orden por posicion en array**: no hay campo `order` — el indice en `layers[]` determina la posicion visual. Reordenar = mover elementos en el array.
3. **Naming explicito**: `supervisorTeamId` / `subordinateTeamId` en vez de `from`/`to` para evitar ambiguedad.
4. **Ventana separada**: no es un modal ni un tab — es una ventana Tauri independiente como Guide.
5. **Simetria en propagacion**: un `CoordinatorLink` solo se propaga a per-agent configs si **ambos** teams tienen un coordinator valido (que sea miembro real). Links parciales se ignoran completamente.
6. **Validacion dual**: ciclos y membership se validan tanto en UI (preventivo) como en backend (defensivo, porque `config.json` puede ser editado a mano).
7. **`send_message` team convention**: mensajes cross-team usan formato string `"TeamA → TeamB"` como valor de `team` para preservar trazabilidad sin cambiar la firma de la funcion.
8. **`AgentLocalConfig` usa `Default` derive**: todos los call-sites usan `..Default::default()` en vez de enumerar campos explicitamente, previniendo errores de compilacion al agregar campos futuros.
