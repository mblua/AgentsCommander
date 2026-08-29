# Plan #1625 — Seed de `.ac/Context.platform.*.md` en open de proyecto conocido

- **Issue**: #1625 `fix(#1605): Context.platform.*.md never seeded on open of an existing project — owner requirement unmet`
- **Repo**: `D:\0_repos\AgentsCommander_iac\.ac\wg-19-ac-dev-team-v3\repo-AgentsCommander`
- **Branch**: `fix/1625-seed-platform-files-on-open` (creada desde `main` = `origin/main` = `809120fa`; árbol limpio al planear)
- **Base congelada**: `809120fa` (immutable para este plan; drift posterior se clasifica por paths según gate 3 de delivery-nonfunctional-invariants)
- **Plan canónico**: este archivo (`plans/1625-seed-platform-files-on-open.md`), único archivo de plan
- **Ronda**: 3. **Estado**: `READY_FOR_IMPLEMENTATION`
- **Clase de tarea / modelo de amenaza (registro obligatorio)**: cambio de código de producto rutinario (comportamiento de lectura/materialización del seeder de contextos; sin frontera de seguridad, sin empaquetado, sin firma, sin migración destructiva). Se aplica el modelo de amenaza baseline del repo (toolchain pinned por CI dtolnay stable + `Cargo.lock` commiteado, gates estándar). Ningún control reforzado es aplicable (razones en §9).

## 1. Causa raíz (evidencia compacta, verificada sobre `809120fa`)

El requisito del owner de #1605 (editar un archivo para cambiar `{{HOST_PLATFORM_RULES}}` sin rebuild) NO se cumple en open de un proyecto YA conocido con el binario nuevo: el seeder corre (global/coordinator → v5) pero no existe ningún `.ac/Context.platform.{windows,linux,macos}.md` ni entradas `platform.*` de estado; app.log registra 22× `[WARN] ... platform rules file Context.platform.windows.md is missing; using the embedded default`. El render funciona pero desde el default embebido.

1. `platform_specs()` (`src-tauri/src/config/seeded_context_templates.rs:545`) alimenta `project_specs()` (:510) — los 3 specs de plataforma están en todos los loops (ensure/scan/read-sync).
2. El camino que crea un archivo faltante es `sync_one_template(..., allow_create_missing=true)` (:1117), alcanzable desde `ensure_project_context_templates_with_clock` (:1408) — que corre en creación/registración de proyecto (`create_default_context_templates*` en `ac_discovery.rs:1631/4079/4218` y `projects.rs:202/287/2200/2358`) — y también desde `sync_project_context_template_for_read_with_clock` (:1512-1527), que corre en cada materialización del read-sync; pero este segundo camino está acotado en el caller: `read_or_create_context_template_with_sync` (`session_context.rs:1265-1266`, `is_managed_project_template`) solo sincroniza los filenames global/coordinator; un filename de plataforma nunca llega a `sync_project_context_template_for_read_with_clock`. Para filenames de plataforma, el único camino creador sigue siendo el ensure de creación/registración.
3. Los dos caminos que corren para un proyecto ya conocido NUNCA crean:
   - **Scan** de arranque/open (`scan_project_context_template_updates_with_clock`, :1465): `allow_create_missing=false` → `Skipped(CreationDisabled)` (scan deliberadamente no-creador).
   - **Read** de materialización: global/coordinator se sincronizan vía `read_or_create_context_template_with_sync` (`session_context.rs:1233`; sincroniza SOLO los filenames `GLOBAL_CONTEXT_TEMPLATE_FILENAME` y `COORDINATOR_CONTEXT_TEMPLATE_FILENAME`); los de plataforma se leen DIRECTO por `render_host_platform_rules_block` (`session_context.rs:3550` → `read_context_template` :1169, sin caché, deliberadamente fuera del sync). `read_context_template` para el archivo de plataforma tiene UN único call site (:3559, dentro de `render_host_platform_rules_block`).
4. Hipótesis `suppress_unknown_without_state` huevo-gallina DESCARTADA: la rama (:1228) solo dispara cuando el archivo EXISTE sin entrada de estado (preservación silenciosa); nunca bloquea crear un faltante.
5. La doc `docs/agents/host-platform-rules.md` (líneas 36-43) promete seed en open/create/scan; hoy solo "create" es cierto.

**Conclusión**: el open de un proyecto conocido no alcanza la rama creadora para los specs de plataforma; la lectura directa por `render_host_platform_rules_block` los deja fuera del ciclo de vida del seeder.

## 2. Dirección de fix (del owner; adoptada)

Enrutar la lectura del archivo de plataforma por el ciclo de vida del seeder: si falta al render (que ocurre en el open, en la materialización), sembrarlo absent-only por la maquinaria de sync (crear + `mark_seeded` + entrada de estado), y después leer.

Restricciones obligatorias (en orden de prioridad):
1. Scan no-creador intacto (no convertir el scan en creador).
2. Contenedor (`repo_mounts=Some`) sin lectura ni seed (sin sección, como hoy).
3. Preservación de ediciones y self-heal sin cambio: archivo pre-existente nunca se pisa; edición preservada (observed + WARN solo si tiene entrada de estado válida; preservación silenciosa, sin entrada de estado y sin WARN, si es unowned); borrado → re-seed absent-only (hoy: + WARN; tras el fix: sin WARN porque el archivo se siembra antes de leer).
4. Cero arcos nuevos de módulo (`module-arcs.txt` byte-idéntico, SCC idéntico).
5. Constantes de presupuesto intactas (8313/9070/757/6810/7567) y presupuesto medido dentro de techo.
6. Comportamiento sin caché: editar → respawn → texto nuevo (como hoy).

## 3. In-scope / Out-of-scope

**In-scope**:
- Nueva maquinaria de seed absent-only para los 3 specs de plataforma, reutilizando `sync_one_template` (el mismo ciclo de vida de global/coordinator).
- Hook de seed en `render_host_platform_rules_block` (choke point único de lectura de plataforma; cubre agente y root prologue).
- Ajuste de la doc `docs/agents/host-platform-rules.md` para reflejar la verdad post-fix.
- Tests T-1..T-6 (abajo) y actualización del test T-8 de #1605 (caso missing).

**Out-of-scope**:
- Scan (`scan_project_context_template_updates_with_clock` y derivados): sin cambios de comportamiento (sigue no-creador).
- `ensure_project_context_templates*` (global/coordinator): sin cambios.
- Contenido de los defaults `DEFAULT_HOST_PLATFORM_RULES_*` y del template global: sin cambios (presupuestos intactos).
- `read_or_create_context_template*`, `seed_manifest` gate, CLI: sin cambios.
- Ningún cambio de frontend/TypeScript, CI workflows, lockfiles.

## 4. Solución decidida (decision-complete; sin TBD)

### D1 — Nueva función `ensure_platform_context_templates` en `seeded_context_templates.rs`

Tres funciones, mirror exacto de la familia `ensure_project_context_templates*` (:1395-1443) pero iterando `platform_specs()` (:545) en lugar de `project_specs()`, ubicadas inmediatamente después de `ensure_project_context_templates_with_clock` (:1408-1424):

```rust
/// #1625: absent-only seed of the three per-execution-platform rule files at
/// render time for an already-known project. Mirrors
/// `ensure_project_context_templates_with_clock` but ONLY for `platform_specs()`
/// (global/coordinator are never touched here; their read-sync stays the
/// managed-filename path in session_context).
pub fn ensure_platform_context_templates(context_dir: &Path) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    ensure_platform_context_templates_with_publications(context_dir, &mut on_publication)
}

pub(crate) fn ensure_platform_context_templates_with_publications(
    context_dir: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    ensure_platform_context_templates_with_clock(context_dir, &mut clock, on_publication)
}

fn ensure_platform_context_templates_with_clock(
    context_dir: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    validate_existing_dir(context_dir, "Context template directory")?;
    let mut loaded = load_state(context_dir, false)?;
    for spec in platform_specs() {
        let execution = sync_one_template(None, context_dir, spec, &mut loaded, true, false, clock);
        let _ = consume_template_execution(spec, execution, on_publication)?;
    }
    persist_state_best_effort(context_dir, &loaded);
    Ok(())
}
```

Decisiones explícitas:
- **`allow_create_missing=true`, `return_pending=false`**: idéntico al ensure de creación y al read-sync de global/coordinator. Absent-only: `create_missing_template` usa `write_template_if_missing_with_clock` (create-only, nunca pisa); un archivo existente pasa por las ramas de sync (seeded si byte-igual al default; si editado: observed + WARN solo con entrada de estado válida (`has_valid_entry`, rama :1264-1273); preservación silenciosa `Skipped(AmbiguousWithoutState)`, sin entrada de estado y sin WARN, si es unowned — el caso del issue, nunca seedado — vía `suppress_unknown_without_state=true` de los 3 specs).
- **`validate_existing_dir` sí, `create_dir_all` no**: `context_dir` llega resuelto por `find_ac_root` (existe por construcción); no crear directorios que el caller no pidió. Igual posture que `sync_project_context_template_for_read_with_clock` (:1498).
- **`load_state(context_dir, false)`**: degradación best-effort idéntica al resto del seeder (estado corrupto/inaccesible → se crean los archivos igual, persistencia saltada).
- **`platform_specs()` es privada y del mismo módulo**: sin cambios de visibilidad.
- Visibilidad `pub` para la función plana (familia `ensure_project_context_templates` es `pub`); `_with_publications` `pub(crate)` (mismo patrón que la familia sync-for-read) para el seam de test y futuros callers gated; `_with_clock` privada (seam de reloj para tests).

### D2 — Hook de seed en `render_host_platform_rules_block` (`session_context.rs:3550`)

Insertar, DESPUÉS del early-return de `repo_mounts.is_some()` (restricción 2) y ANTES del `match read_context_template(...)`:

```rust
// #1625: a known project opened with a binary that never reached the
// creation path (create/register) has no platform rule files; the read
// must seed them absent-only through the seeder lifecycle before reading.
// Guard: one resolve (walk + canonicalize) + one symlink_metadata per
// render in steady state; read_context_template repeats the same resolve
// + stat immediately after, so the marginal cost is one extra resolve +
// one extra stat per render — negligible per materialization.
if let Some(context_dir) = resolve_ac_root_context_dir(Path::new(agent_root)) {
    let path = context_dir.join(filename);
    let missing = match std::fs::symlink_metadata(&path) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    if missing {
        if let Err(error) = crate::config::seeded_context_templates::
            ensure_platform_context_templates(&context_dir)
        {
            log::warn!(
                "[session_context] failed to seed platform rules files in {}: {}",
                context_dir.display(),
                error
            );
        }
    }
}
```

Decisiones explícitas:
- **Trigger = ausencia del archivo del HOST** (`host_platform_rules_filename()`): es la única dependencia real del render; el guard corre `resolve_ac_root_context_dir` (walk + canonicalize) + 1 `symlink_metadata` en CADA render (no solo cuando falta), y `read_context_template` repite esa misma resolución + stat inmediatamente después: costo en estado estable = 1 resolve + 1 stat extra por render, ambos ya pagados por el read inmediato — negligible por materialización. Cuando dispara, `ensure_platform_context_templates` siembra los 3 specs absent-only en un solo pass (una carga de estado, un persist si dirty).
- **Detección por `symlink_metadata` NotFound** (no `path.exists()`): un symlink roto (`exists()` = false) NO dispara seed; un symlink/no-regular existente tampoco; ambos siguen el camino actual (WARN "not readable"/"not a regular file" + fallback). Semántica idéntica a `read_context_template` (:1169).
- **Error de seed → WARN + continuar al read**: el render nunca bloquea la sesión por el archivo de plataforma; si el seed falla, el read encuentra el archivo ausente y cae al fallback (el WARN "is missing" actual queda como camino de fallo, no de operación normal).
- **Sin cambios en el resto de la función** (fallback, WARN empty, WARN not-readable, retorno de contenido).
- El choke point es único: `render_agent_context_template_inner` (:2600) y `render_root_runtime_prologue_inner` (:3287) llaman a `render_host_platform_rules_block`; no hay otro lector de archivos de plataforma (verificado: `read_context_template` para platform solo en :3559).

### D3 — Doc `docs/agents/host-platform-rules.md`

Reemplazar la bala "Seeding" (líneas 36-43) por la verdad post-fix:

- **Seeding**: on project creation/registration the full project template set is seeded; for an already-known project, the platform files are seeded absent-only by the render path on the first materialization after open when a platform file is missing (same `sync_one_template` lifecycle, state entries `platform.*` v1 with `lastSeededSha256`). The startup scan never creates templates (unchanged). A pre-existing file is never overwritten; an unowned pre-existing file is preserved silently.

### D4 — Por qué NO se toca el scan ni el open/discover

- El scan es no-creador por diseño (restricción 1): seguir con `allow_create_missing=false`.
- No se engancha el seed en `discover_ac_agents`/`discover_project_inner`: ese camino no conoce `repo_mounts` (resolución por sesión) y sembraría archivos de plataforma en proyectos de contenedor, violando la restricción 2 ("sin seed" para contenedores). El render SÍ conoce `repo_mounts` y es el punto donde la lectura ocurre — el único lugar donde el seed tiene sentido semántico.
- El open de la app materializa contextos (así es como hoy el seeder lleva global/coordinator a v5 y como aparecen los 22 WARN); por lo tanto el primer render del open dispara el seed. AC-1 del owner (abrir este proyecto → 3 archivos + estado + sin WARN) queda satisfecho en el open, sin cambios en discover.

## 5. Comportamiento, edge cases y failure behavior

| Caso | Antes (#1605) | Después (#1625) |
|---|---|---|
| Open proyecto conocido, archivos ausentes | WARN "is missing" ×N + fallback; sin archivos, sin estado | 1er render: seed absent-only de los 3 + estado `platform.*` v1 `lastSeededSha256`; read devuelve el archivo; SIN WARN "is missing" |
| Archivo editado (con o sin estado) | existe → read directo, texto editado, sin WARN | idéntico (existe → no seed → read directo; el guard no toca el archivo, solo 1 resolve + 1 stat extra, ver D2) |
| Editar → respawn (sin rebuild) | texto nuevo en cada materialización | idéntico (sin caché; restricción 6) |
| Archivo borrado | WARN "is missing" + fallback | re-seed absent-only en el próximo render; sin WARN (restricción 3) |
| Borrar solo un archivo no-host (p.ej. linux.md en Windows) | nada (nunca se lee) | no se re-siembra hasta el próximo trigger de seed (host ausente o create/register); sin WARN, sin impacto de render. Edge aceptado y documentado en D2 |
| Archivo vacío | WARN "is empty" + fallback | idéntico (existe → no seed → WARN empty + fallback) |
| Symlink / no-regular / symlink roto | WARN "not readable"/"not a regular file" + fallback | idéntico (NotFound no dispara para symlink roto porque usamos `symlink_metadata`; ver D2) |
| Contenedor (`repo_mounts=Some`) | sin sección, sin lectura | idéntico (early-return antes del seed; restricción 2) |
| Scan de arranque | `CreationDisabled` para faltantes | idéntico (restricción 1) |
| Creación/registración de proyecto | `ensure_project_context_templates*` siembra los 5 specs | idéntico (sin cambios) |
| `ensure_platform_context_templates` falla (dir no escribible, estado corrupto) | — | WARN "[session_context] failed to seed platform rules files" + read → fallback. La sesión nunca se bloquea |
| Estado JSON corrupto | (no aplica: nunca se creaba) | `load_state(false)` degrada a estado mínimo con `can_persist=true, dirty=true` (JSON inválido, :795-803) → `persist_state_best_effort` REESCRIBE el estado corrupto con el estado mínimo válido (misma posture que global/coordinator); `can_persist=false` solo para state path no-regular/inaccesible. En ambos casos los archivos se crean igual y la sesión nunca se bloquea |
| Dos renders concurrentes (mismo `.ac`) | (no aplica) | `write_template_if_missing_with_clock` es create-only: segundo writer → `AlreadyPresent`; `mark_seeded` idempotente; persist best-effort (misma posture que global/coordinator hoy) |
| Archivos linux/macos editados mientras falta el host | (no se tocaban) | `ensure` los preserva en silencio (unowned, `suppress_unknown_without_state` → `Skipped(AmbiguousWithoutState)` con `log::debug`, sin entrada de estado, sin WARN; restricción 3). Solo si tienen entrada de estado válida (p. ej. seed previo y edición posterior) `ensure` los marca `observed` y emite el WARN pre-existente "preserving customized context template ...; a newer default is available" (texto genérico del seeder) |
| Presupuesto de bytes | — | sin cambios: no se altera ningún default ni el template; el fixture del budget test (`FAKE_REPLICA_ROOT = "C:/fake/..."`) no tiene ancestro `.ac` → `resolve_ac_root_context_dir` = None → sin seed; medición idéntica |

## 6. Archivos y símbolos exactos (diff esperado)

1. `src-tauri/src/config/seeded_context_templates.rs`
   - Añadir: `pub fn ensure_platform_context_templates(context_dir: &Path) -> Result<(), String>`, `pub(crate) fn ensure_platform_context_templates_with_publications(...)`, `fn ensure_platform_context_templates_with_clock(...)` (tras `ensure_project_context_templates_with_clock`, ~:1424).
   - Tests: `ensure_platform_context_templates_seeds_only_missing_platform_files` (T-3) en el módulo `#[cfg(test)]`.
2. `src-tauri/src/config/session_context.rs`
   - Modificar: `render_host_platform_rules_block` (:3550) — bloque D2 insertado.
   - Tests: reemplazar `host_platform_rules_missing_or_empty_file_falls_back_to_embedded_default` (:5568) por T-1 + T-2; añadir T-4, T-5, T-6.
3. `docs/agents/host-platform-rules.md` — bala "Seeding" (D3).

Sin cambios: `module-arcs.txt`, `Cargo.lock`, workflows, frontend, `seed_manifest.rs`, `projects.rs`, `ac_discovery.rs`, `root_agent.rs`.

## 7. Tests T-* (criterio objetivo)

- **T-1** (session_context.rs, reemplaza el caso "missing" de T-8 de #1605): `render_seeds_missing_platform_file_absent_only` — temp `.ac` + replica, sin archivo: `render_agent_context_template_inner` → (a) `ac.join(host_platform_rules_filename())` existe y es byte-igual a `host_platform_rules_default()`; (b) los 3 archivos de plataforma existen byte-iguales a sus defaults; (c) estado `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME` registra `platform.<os>` v1 con `lastSeededSha256 == hash_text(default)`; (d) el render contiene `## Host Platform Rules` y el default; (e) segundo render idempotente (mismo estado, contenido igual, archivos sin cambio).
- **T-2** (session_context.rs, reemplaza el caso "empty" de T-8): `host_platform_rules_empty_file_falls_back_to_embedded_default` — archivo vacío: render contiene el default, 1 sola ocurrencia de `## Host Platform Rules`, el archivo vacío NO se pisa (sigue vacío tras render) y NO aparece entrada de estado para `platform.<os>` (preservación silenciosa del unowned-empty vía `suppress_unknown_without_state`).
- **T-3** (seeded_context_templates.rs): `ensure_platform_context_templates_seeds_only_missing_platform_files` — fresh `.ac`: los 3 archivos creados byte-iguales a defaults; un `Context.platform.windows.md` pre-existente custom se preserva y queda sin entrada de estado (unowned, suppress); `GLOBAL_CONTEXT_TEMPLATE_FILENAME` y `COORDINATOR_CONTEXT_TEMPLATE_FILENAME` NO se crean (scope solo platform); estado con 3 entradas `platform.*` v1 `lastSeededSha256`.
- **T-4** (session_context.rs): `render_with_container_mounts_never_seeds_platform_files` — temp `.ac` + replica, sin archivo, `repo_mounts=Some(...)`: render sin sección `## Host Platform Rules`; ningún `Context.platform.*` creado; sin archivo de estado.
- **T-5** (session_context.rs): `deleted_platform_file_is_reseeded_absent_only` — seed vía render (T-1), borrar el archivo del host, render de nuevo → archivo re-creado byte-igual al default, estado sigue `platform.<os>` v1 seeded (sin WARN posible de verificar en unit test; el estado y el archivo lo prueban).
- **T-6** (session_context.rs): `render_never_overwrites_edited_platform_file` — escribir contenido custom, render → el contenido custom se renderiza y el archivo permanece intacto (no hay sync sobre archivos existentes).

**Guardas que deben seguir verdes SIN cambios**: `host_platform_rules_reads_project_file_each_materialization` (escribe el archivo antes de render → existe → sin seed), `platform_file_edit_is_preserved_and_observed`, `scan_existing_ac_does_not_create_missing_templates`, `ensure_project_context_templates_seeds_platform_files_absent_only`, `container_session_omits_host_platform_rules_block`, `container_root_prologue_omits_host_platform_rules_block`, `root_prologue_embeds_host_platform_rules_block` (raíz fake sin `.ac` → fallback), `summarized_default_context_meets_size_budget`, `token_accounting_report`, todos los `render_global_template_for_test`/`default_context*` (raíces `C:/fake/...` sin ancestro `.ac`).

## 8. Criterios de aceptación (owner, verdes, manuales, reportados en el cierre; CI no los corre)

1. Abrir ESTE proyecto (`D:\0_repos\AgentsCommander_iac`) con el binario nuevo crea los 3 archivos en `.ac/` byte-iguales al default (absent-only), el estado `.ac/.agentscommander-context-templates.json` registra `platform.windows|linux|macos` v1 con `lastSeededSha256`, y desaparece de app.log el WARN "platform rules file ... is missing".
2. Editar `.ac/Context.platform.windows.md` y respawnear una réplica cambia el texto en el CLAUDE.md/AGENTS.md sin rebuild; un archivo pre-existente nunca se pisa.
3. Gates manuales: SCC igual pre/post y `module-arcs.txt` sin arcos nuevos; guards de layering 4/4 (`loops_layering`, `instance_gitignore_layering`, `project_settings_layering`, `claude_watcher_layering`); `check:frontend-dependencies` en 0; presupuesto medido con constantes intactas (8313/9070/757/6810/7567).

### Step-N — Gate de dependencias (criterio ejecutable para el reviewer de implementación)

```
# base SHA 809120fa vs head final del branch, árbol limpio en ambos
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```
Verde iff (pre-medido por architect sobre `809120fa`):
1. `cyclicSccs` igual pre/post: PRE = 1 SCC cíclico (89 módulos, incluye `config::session_context` y `config::seeded_context_templates` — dependencia mutua pre-existente); POST debe tener el MISMO conjunto de miembros, módulo a módulo;
2. cero pares `from -> to` nuevos: el único call añadido es `config::session_context -> config::seeded_context_templates` (arc YA presente en `module-arcs.txt` línea 630 → interno al SCC pre-existente; no crea ni cruza fronteras limpias);
3. `module-arcs.txt` regenerado byte-idéntico (`git status` vacío sobre él);
4. guards estructurales verdes (4 layering de arriba).
Exit code del detector: 1 = hay ciclos (normal; el grafo igual se escribe); 3 = sin grafo. Nunca confundir.

## 9. Gates de delivery-nonfunctional-invariants (evidencia y dueños)

Clase de tarea: rutinaria (ver registro al inicio). Gates aplicables baseline:

1. **CI-to-plan parity**: PR `fix/1625-seed-platform-files-on-open` → `main` dispara `pr-regression-gates.yml` (jobs `test-debt`+480-guards, `rust-regression` [windows: `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests`], `rust-regression-linux` [check+clippy+test 1577], `rust-regression-macos` [check+clippy], `rust-fmt`, `terminal-snapshot-portable` 4 OS, `windows-release-cli-smoke`), `validate-branch-name.yml`, `lockfile-check.yml`, `version-sync-check.yml`. Locales (dueño: implementador, en el repo): los tres comandos del job windows + `cargo fmt --all -- --check`. Remotos (dueño: CI, exact-head del PR): legs linux/macos, terminal-snapshot, smoke, node jobs (sin cambios de frontend → resultado esperado idéntico a base). Criterio: todos los checks configurados-requeridos verdes en el SHA exacto del head del PR. Tests nuevos T-1..T-6 corren en el leg windows (suite completa); en linux/macos corren con su `host_platform_rules_filename()`/`default` por-OS (misma lógica, sin cfg-gate).
2. **Toolchain determinista**: CI fija stable vía `dtolnay/rust-toolchain` (no hay `rust-toolchain*` en el repo); `Cargo.lock` commiteado; sin dependencias nuevas. Local: registrar `rustc -V` del implementador; usar el toolchain del workspace.
3. **Git autorizado y trazable**: issue #1625 abierto; branch `fix/1625-seed-platform-files-on-open` desde `main=origin/main=809120fa`; cambios SOLO dentro de `repo-AgentsCommander`; entrega por PR a `main`, nunca push directo; precondición: árbol limpio al iniciar.
4. **Cwd/config/estado**: todos los comandos con cwd explícito en el repo; sin overrides de cargo en el repo (verificar `src-tauri/.cargo` inexistente al implementar); scratch y gráficos en directorio temporal ignorado, fuera del árbol.
5. **Validación y scope**: base congelada (809120fa); path set esperado = 3 archivos (§6); postcondición: `git status`/`git diff --stat` muestran solo esos paths; diff shape: 2 fns nuevas + bloque D2 + tests + doc.
6. **Mutación y no-clobber**: antes de escribir, rechequear branch/índice; recovery = restaurar SOLO paths que el run tocó y solo si su estado actual es demostrablemente el output del run (nunca `git reset` amplio); el cambio es aditivo y local a 3 archivos.
7. **Ejecución acotada**: `cargo test --lib` (y filtros por test T-*) con timeout de runner; stdout/stderr + exit conservados; ningún comando interactivo.
8. **Disciplina de evidencia**: cero y ausencia son estados válidos (0 arcos nuevos, 0 findings de frontend-deps, estado ausente en T-2); comandos exactos y resultado esperado en §7/§8; lo no reproducible localmente (legs remotos) queda asignado a CI con regla exact-head.

**Controles reforzados: NO aplicables** — sin release/firma/empaquetado en el diff; sin host no confiable; sin migración destructiva; sin frontera de seguridad; mutación de archivos `.ac` del usuario por maquinaria pre-existente create-only (mismo riesgo que global/coordinator hoy, aceptado por el repo). Ningún control reforzado se añade.

## 10. Secuencia de implementación (dueño: implementador; revisión: dev/grinch; cierre: tech-lead)

1. Añadir D1 + T-3 (red-green en `seeded_context_templates.rs`).
2. Añadir D2 + T-1/T-2/T-4/T-5/T-6 (red-green en `session_context.rs`); borrar el test T-8 viejo.
3. D3 (doc).
4. Gates locales: `cargo test --lib`, `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, budget test, Step-N (grafo pre/post + arc record byte-idéntico), 4 guards de layering, `npm run check:frontend-dependencies` (0).
5. Commit único con mensaje `fix(context-templates): seed platform rules files absent-only at render (#1625)`; push; PR a `main` con la evidencia.
6. Manual del owner (AC-1..AC-3) con el binario release nuevo sobre ESTE proyecto; reportar en el cierre.

## 11. Bloqueos

Ninguno.
