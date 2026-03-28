# Implementar ETAPA 2 — Ventana Dark Factory + Organigrama

Estoy en el branch `feature/organigrama-dark-factory`. La Etapa 1 ya esta completa y mergeada en este branch (tipos, backend, Settings UI). Necesito que implementes la **Etapa 2** completa.

El plan vive en `docs/PLAN_OrganigramaDF.md` — leelo completo para contexto, pero la spec detallada esta abajo.

---

## Que se hizo en Etapa 1 (ya en el codigo)

- `src/shared/types.ts`: interfaces `DarkFactoryLayer`, `CoordinatorLink`, `Team.layerId`, `DarkFactoryConfig.layers` y `.coordinatorLinks`
- `src-tauri/src/config/dark_factory.rs`: structs Rust equivalentes con serde backward compat, `AgentLocalConfig` con `supervises`/`reports_to`, `sync_agent_configs` propaga links, `save_dark_factory` valida ciclos y membership
- `src-tauri/src/phone/manager.rs`: `can_communicate()` soporta cross-team coordinator links
- `src/sidebar/components/SettingsModal.tsx`: CRUD de Layers, layer dropdown por team, selector "Reports to", fixes del save handler y createStore init

---

## Etapa 2 — Pasos

### Paso 3 — Boton en Sidebar + Comando Rust con singleton guard

**`src/sidebar/components/TeamFilter.tsx`**: Agregar boton Dark Factory entre el boton Hints (bombilla) y Settings (engranaje). El orden actual es:

```
[Eye] [Lightbulb/Hints] [Settings/Gear]
```

Debe quedar:

```
[Eye] [Lightbulb] [Factory NEW] [Settings]
```

- Icono: `&#x1F3ED;` (emoji fabrica) — consistente con los otros botones que usan HTML entities
- Tooltip: "Dark Factory"
- Usa `DarkFactoryWindowAPI.open()` (ver paso IPC abajo)
- Patron identico al boton Hints que llama `GuideAPI.open()`

**`src/shared/ipc.ts`**: Agregar:

```typescript
export const DarkFactoryWindowAPI = {
  open: () => invoke<void>("open_darkfactory_window"),
};
```

**`src-tauri/src/commands/window.rs`**: Agregar comando `open_darkfactory_window` con **singleton guard** (patron identico a `open_guide_window` que ya existe en el archivo):

```rust
#[tauri::command]
pub async fn open_darkfactory_window(app: AppHandle) -> Result<(), String> {
    // Si ya existe, solo focus
    if let Some(existing) = app.get_webview_window("darkfactory") {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    // Crear ventana nueva — misma estructura que open_guide_window
    // URL: "index.html?window=darkfactory"
    // Label: "darkfactory"
    // Title: "Dark Factory — Agents Commander"
    // inner_size: 960.0, 640.0
    // min_inner_size: 640.0, 400.0
    // decorations: false
    // zoom_hotkeys_enabled: true
}
```

**`src-tauri/src/lib.rs`**: Registrar `commands::window::open_darkfactory_window` en el `.invoke_handler()` junto a los demas window commands.

**`src-tauri/capabilities/default.json`**: Agregar `"darkfactory"` al array `"windows"`. Actualmente es:

```json
"windows": ["sidebar", "terminal", "terminal-*", "guide"]
```

Debe quedar:

```json
"windows": ["sidebar", "terminal", "terminal-*", "guide", "darkfactory"]
```

### Paso 3b — Zoom system

**CRITICO**: `src/shared/zoom.ts` tiene un bug preexistente. El type `WindowType` ya incluye `"guide"` pero los ternarios en `debouncedSave` e `initZoom` son binarios — `"guide"` (y el nuevo `"darkfactory"`) caen al branch `else` y escriben/leen `terminalZoom`. Hay que:

1. Agregar `"darkfactory"` al type `WindowType`
2. Reemplazar AMBOS ternarios por un lookup map:

```typescript
type WindowType = "sidebar" | "terminal" | "guide" | "darkfactory";

const zoomKeyMap: Record<WindowType, keyof AppSettings> = {
  sidebar: "sidebarZoom",
  terminal: "terminalZoom",
  guide: "guideZoom",
  darkfactory: "darkfactoryZoom",
};
```

En `debouncedSave` (linea 28-29):
```typescript
// ANTES: const key: keyof AppSettings = windowType === "sidebar" ? "sidebarZoom" : "terminalZoom";
const key = zoomKeyMap[windowType];
```

En `initZoom` (linea 73-74):
```typescript
// ANTES: const saved = windowType === "sidebar" ? settings.sidebarZoom : settings.terminalZoom;
const saved = settings[zoomKeyMap[windowType]] as number;
```

**`src/shared/types.ts` (`AppSettings`)**: Agregar:

```typescript
guideZoom: number;
darkfactoryZoom: number;
```

**`src-tauri/src/config/settings.rs`**: Agregar al struct `AppSettings`:

```rust
#[serde(default = "default_zoom")]
pub guide_zoom: f64,
#[serde(default = "default_zoom")]
pub darkfactory_zoom: f64,
```

Y en el `impl Default for AppSettings`, agregar:
```rust
guide_zoom: default_zoom(),
darkfactory_zoom: default_zoom(),
```

### Paso 4 — Scaffold ventana

**`src/main.tsx`**: Agregar routing para `"darkfactory"`:

```tsx
} else if (windowType === "darkfactory") {
  render(() => <DarkFactoryApp />, root);
}
```

Con el import correspondiente.

**Crear `src/darkfactory/App.tsx`**: Componente principal. Usar `src/guide/App.tsx` como referencia para la estructura (titlebar custom con `data-tauri-drag-region`, initZoom, minimize/close). La ventana Guide es el patron exacto a seguir.

```
DarkFactoryApp
├── Titlebar (icon + "dark factory" + minimize/close) — data-tauri-drag-region
├── Toolbar (zoom display opcional)
└── OrgChart (area principal con scroll)
```

- Cargar datos via `DarkFactoryAPI.get()` en `onMount`
- Inicializar zoom con `initZoom("darkfactory")`
- Si no hay layers configuradas, mostrar un CTA: "No layers configured. Go to Settings > Dark Factory to set up your organization."

**Crear `src/darkfactory/styles/darkfactory.css`**: Importar variables desde `../../terminal/styles/variables.css`. Usar las mismas CSS variables del proyecto:

Variables disponibles:
- `--terminal-bg: #0a0a0f`, `--terminal-fg: #e8e8e8`
- `--titlebar-bg: #08080d`, `--titlebar-fg: #888898`
- `--statusbar-bg: #08080d`, `--statusbar-fg: #666678`, `--statusbar-accent: #00d4ff`
- `--font-ui: "Geist", "Outfit", ...`, `--font-terminal: "Cascadia Code", ...`
- `--font-size-sm: 11px`, `--font-size-md: 13px`
- `--transition-fast: 150ms ease-out`
- `--spacing-xs: 4px`, `--spacing-sm: 8px`, `--spacing-md: 12px`

Estetica: industrial-dark, spacecraft dashboard. Separacion por opacity/color, no borders gruesos. Animaciones 150-200ms ease-out.

### Paso 5 — OrgChart layout

**Crear `src/darkfactory/components/OrgChart.tsx`**: Layout CSS Grid horizontal (izquierda a derecha).

- Cada Layer es una columna: `grid-template-columns: repeat(N, 1fr)`
- Scroll horizontal si hay muchas layers: `overflow-x: auto`
- Recibe `DarkFactoryConfig` como prop

**Crear `src/darkfactory/components/LayerColumn.tsx`**: Una columna por layer.

- Header fijo arriba con nombre del layer
- Contenedor flex vertical con TeamNodes
- Fondo alternado sutil por layer (opacity diferenciada)
- Teams sin `layerId` NO aparecen en el organigrama

### Paso 6 — TeamNode cards

**Crear `src/darkfactory/components/TeamNode.tsx`**: Card de cada team.

```
┌─────────────────────┐
│ ★ Coordinator Name  │  ← badge si tiene coordinador
│ ─────────────────── │
│ Team Name           │
│ 4 members           │
└─────────────────────┘
```

- Tamano fijo: ~180px ancho, ~80px alto (escalable con zoom)
- Borde izquierdo coloreado (usar `--statusbar-accent` como default)
- Opacidad reducida (0.5) si el team no tiene miembros
- Cada nodo necesita reportar su posicion DOM para las lineas SVG (via ref + `getBoundingClientRect`)

### Paso 7 — ConnectionLines SVG

**Crear `src/darkfactory/components/ConnectionLines.tsx`**: SVG overlay para lineas entre coordinadores vinculados.

- `<svg>` con `position: absolute; inset: 0; pointer-events: none; overflow: visible`
- Para cada `CoordinatorLink`, dibujar un path bezier horizontal desde el borde derecho del nodo supervisor al borde izquierdo del nodo subordinado
- Calcular posiciones a partir de refs de los TeamNodes (necesita un sistema de registro de posiciones — el padre OrgChart mantiene un Map<teamId, DOMRect>)
- Recalcular en resize via `ResizeObserver` con debounce 100ms
- Estilos: color `var(--statusbar-fg)`, grosor 1.5px, transicion 150ms

### Paso 8 — Interactividad

- **Hover en TeamNode**: resaltar las conexiones de ese team (cambiar color a `var(--statusbar-accent)`, grosor a 2.5px)
- **Hover propagation**: OrgChart mantiene un signal `hoveredTeamId`, TeamNode y ConnectionLines lo leen
- **Zoom**: ya funciona via Ctrl+scroll/+/- gracias a initZoom del Paso 3b
- Teams sin layer asignada NO aparecen (filtrar en OrgChart)

### Paso 9 — Polish

- Animaciones de entrada para TeamNodes (fade-in 150ms staggered)
- Empty state si no hay layers o no hay teams con layer asignada
- Responsive: si la ventana es muy chica, reducir node size

---

## Archivos a crear

| Archivo | Descripcion |
|---------|-------------|
| `src/darkfactory/App.tsx` | Componente principal con titlebar, zoom, data loading |
| `src/darkfactory/styles/darkfactory.css` | Estilos (importa variables.css) |
| `src/darkfactory/components/OrgChart.tsx` | Layout CSS Grid de layers con hover state |
| `src/darkfactory/components/LayerColumn.tsx` | Columna por layer con header |
| `src/darkfactory/components/TeamNode.tsx` | Card de team con ref reporting |
| `src/darkfactory/components/ConnectionLines.tsx` | SVG overlay con bezier curves |

## Archivos a modificar

| Archivo | Cambio |
|---------|--------|
| `src/shared/types.ts` (`AppSettings`) | Agregar `guideZoom: number`, `darkfactoryZoom: number` |
| `src/shared/zoom.ts` | `"darkfactory"` en WindowType + `zoomKeyMap` lookup (reemplaza ternarios) |
| `src/shared/ipc.ts` | `DarkFactoryWindowAPI.open()` |
| `src/main.tsx` | Routing `"darkfactory"` → `DarkFactoryApp` |
| `src/sidebar/components/TeamFilter.tsx` | Boton Dark Factory entre Hints y Settings |
| `src-tauri/src/config/settings.rs` | `guide_zoom` y `darkfactory_zoom` fields + Default impl |
| `src-tauri/src/commands/window.rs` | `open_darkfactory_window` con singleton guard |
| `src-tauri/src/lib.rs` | Registrar nuevo command |
| `src-tauri/capabilities/default.json` | Agregar `"darkfactory"` a windows array |

---

## Criterios de aceptacion

- [ ] Boton Dark Factory visible en sidebar entre Hints y Settings
- [ ] Click abre ventana separada (singleton — segundo click hace focus)
- [ ] Organigrama muestra layers como columnas izq→der con teams asignados
- [ ] Lineas SVG conectan coordinadores vinculados via CoordinatorLink
- [ ] Hover resalta conexiones del team
- [ ] Zoom persiste entre reinicios (`darkfactoryZoom` en settings)
- [ ] Bug fix: `guideZoom` ya no escribe a `terminalZoom`
- [ ] Teams sin layer no aparecen en el organigrama
- [ ] Ventana vacia muestra CTA para configurar layers/teams
- [ ] `cargo check` y `npx tsc --noEmit` pasan sin errores

---

## Reglas de proyecto

- **SolidJS idioms**: createSignal, createEffect, onMount, onCleanup. NO React patterns
- **CSS**: vanilla con CSS variables, zero frameworks. Font UI: Geist/Outfit. Font terminal: Cascadia Code
- **IPC**: componentes nunca llaman invoke() directo — siempre via wrappers en ipc.ts
- **Tipos**: todos en `src/shared/types.ts`, nunca locales
- **Rust serde**: `#[serde(rename_all = "camelCase")]` en todos los structs. `#[serde(default)]` en campos nuevos para backward compat
- **Titlebar custom**: `data-tauri-drag-region` en el drag area, botones internos deben funcionar sin conflicto
- Despues de implementar, verifica que `cargo check` y `npx tsc --noEmit` pasen
