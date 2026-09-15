# Plan #2010: KEEP con la misma selección bloqueada

Status: READY_FOR_IMPLEMENTATION

- Plan canónico (única autoridad): `repo-AgentsCommander/plans/2010-keep-locked-selection.md`, por asignación explícita del tech lead. Copia informativa: `.ac/plans/issue-2010/2010-keep-locked-selection.md`.

- Issue: https://github.com/mblua/AgentsCommander/issues/2010 (OPEN)
- Rama: `fix/2010-keep-locked-selection`
- Base congelada: `main` 5edf0cfeff0efb41988647f26aa2402c69d135d3 (HEAD de la rama = base, verificado)
- Clase: Lite, una sola fase, sin partición. Cambio de aplicación rutinario.
- Modelo de amenaza: rutinario. Sin controles reforzados (no hay release, firma, migración ni host no confiable).
- Autorización: el usuario aprobó la propuesta KEEP (issue, sección "Approved behavior"; mensajes 20260914-141205 arquitecto y 20260914-141135 dev-rust).
- Diferencias materiales respecto a la propuesta: ninguna. Añadidos solo de prueba: test de stale con mismo par y test de apply con par distinto.

## 1. Problema

En `write_replica_selection` (`src-tauri/src/config/coding_agent_profiles.rs:1014-1022`) un replica bloqueado con intent `Individual` o `IndividualAssignLock` devuelve error "is locked" antes de mirar el par pedido. El picker (`AgentPickerModal.tsx:986-996`) aborta con errores y nunca llama a `onSelect`, así que KEEP con el mismo par no puede lanzar.

## 2. Regla nueva (única, en el writer)

Replica bloqueado + intent `Individual` o `IndividualAssignLock`:
- par pedido normalizado == par actual (tras el CAS) => `Ok(SelectionWriteOutcome::UNCHANGED)`, sin publicar, sin temp, candado intacto.
- par distinto (agente o perfil) => el error actual, sin cambios.

Igualdad = la misma del CAS (`current_pair != expected.pair`, :967): `ReplicaSelectionPair` con id de agente exacto (sin trim ni mayúsculas) y letra de perfil normalizada. El par actual ya viene normalizado por `selection_state_from_value`; el pedido se normaliza en :999. No se reconcilia `profile` vs `instanceProfileOverride` legado (no se escribe nada). `profileContentHash` no participa.

Orden dentro del callback, sin cambios: primero `validate_selection_write_state` (identidad, par, flag, inválido => error stale/invalid), después la regla. Así un estado stale o inválido sigue fallando aunque el par coincida.

Alcanza a ambos llamadores sin tocarlos: apply (`commands/config.rs:1564`) y `set_replica_coding_agent_selection` (:1137, usado por el self-switch de `phone/mailbox.rs:11033, 19484, 19727, 19932, 20075`).

## 3. Cambios de código (solo 2 archivos)

### 3.1 `src-tauri/src/config/coding_agent_profiles.rs` (modificado)

a) `write_replica_selection` (:992-1083):
- Antes del closure, construir `let requested_pair = ReplicaSelectionPair { coding_agent_id: pair.coding_agent_id.clone(), requested_profile: profile.clone() };`
- Cambiar `let (_current_pair, current_locked)` a `let (current_pair, current_locked)`.
- En la rama `Individual | IndividualAssignLock` (:1016-1022): si `current_pair.as_ref() == Some(&requested_pair)`, hacer `marker.set(Some(SelectionGuardMarker::SkippedLocked)); return Err(NO_PUBLISH_SENTINEL.to_string());`. Si no, el `return Err(format!(... "is locked; unlock it or use a reviewed force" ...))` actual, texto idéntico.
- Reusar la variante `SkippedLocked` y el mapeo existente de :1079 a `UNCHANGED`. No se añaden variantes, tipos ni funciones.

b) Comentarios:
- Enum `SelectionWriteIntent` (:560-565): "A locked replica is rejected unless the request equals its saved pair, which returns UNCHANGED without publishing."
- Doc de `set_replica_coding_agent_selection` (:1134-1136): igual idea; el mismo par bloqueado es no-op sin escritura.
- Comentario de `SelectionWriteOutcome::UNCHANGED` (:594): añadir "same locked pair".

c) Tests nuevos en el módulo de tests del mismo archivo (helpers existentes `selection_fixture`, `seed_selection_config`, `write_config`, `config_bytes`, `pair`, `read_replica_selection_state`):

- T1 `issue_2010_locked_same_pair_individual_intents_are_unchanged`: para `Individual` e `IndividualAssignLock`, semilla `{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}`; pedir `pair("codex","b")` (prueba normalización). Esperado: `Ok(SelectionWriteOutcome::UNCHANGED)`, bytes idénticos, `selectionLocked` sigue `true`. Luego crear directorio `.config.json.{pid}.tmp` (patrón :2402-2413) y repetir: sigue `UNCHANGED`, el directorio sigue siendo directorio, bytes idénticos.
- T2 `issue_2010_locked_different_agent_rejected`: ambos intents, semilla codex/B bloqueada, pedir `pair("claude","B")`. Error contiene "locked", bytes idénticos.
- T3 `issue_2010_locked_different_profile_rejected`: ambos intents, semilla codex/B bloqueada, pedir `pair("codex","C")`. Error contiene "locked", bytes idénticos.
- T4 `issue_2010_locked_same_pair_stale_expectation_rejected`: semilla codex/B bloqueada, tomar expectativa, reescribir config a codex/A bloqueada; pedir `pair("codex","A")` con la expectativa vieja, ambos intents. Error contiene "stale", bytes iguales a los de la reescritura.
- T5 `issue_2010_wrapper_locked_same_pair_is_no_write`: semilla codex/A bloqueada; `set_replica_coding_agent_selection(..., "codex", "a")` => `Ok(())`, bytes idénticos; con el directorio temp obstruido sigue `Ok(())`.

Tests existentes que NO se tocan (usan par distinto y siguen verdes): :2154-2181 (claude/A vs codex/B), :2659-2688 (codex/A vs codex/B), bulk :2182-2207, force :2209-2233, malformed :2237+, unlock no-publish :2367+.

### 3.2 `src-tauri/src/commands/config.rs` (modificado, solo tests)

Sin cambio de producción: la rama `Ok(_unchanged)` (:1588-1591) ya añade solo un warning, no toca `updated_replica_paths`, `write_succeeded_keys` ni `newly_protected_paths`; el restart (:1603) requiere `write_succeeded_keys` no vacío. Replica scope pasa locked a writer (`assignment_write_paths` :2647-2650 usa `valid_paths`).

Tests nuevos (`#[tokio::test]`, helpers `selection_api_fixture`, `selection_api_replica`, `locked_tooling`, `state_for`, `selection_api_settings`, `api_preview`, `api_apply`, `config_bytes`; el request usa agent-0/B):

- T6 `issue_2010_selection_api_replica_locked_same_pair_keeps_lock`: para `AssignmentMode::Ordinary` y `AssignmentMode::AssignAndLock`, replica con `locked_tooling("B","agent-0")`, scope `Replica`; preview y apply con `preview.target_fingerprint`. Esperado: `Ok`, `errors` vacío, `updated_count == 0`, `updated_replica_paths` vacío, `newly_protected_paths` vacío, `skipped_locked_paths` vacío, `restarted_count == 0`, `!force_applied`, bytes idénticos, `selectionLocked` true.
- T7 `issue_2010_selection_api_replica_locked_different_pair_still_errors`: `locked_tooling("A","agent-0")`, mismo flujo en ambos modos. Esperado: `errors.len() == 1`, mensaje contiene "locked", `updated_count == 0`, bytes idénticos.

Si preview o apply rechazan el replica bloqueado antes del writer en algún modo (no esperado por el código leído), parar y reportar al tech lead: sería diferencia material.

Tests existentes preservados: bulk skip :7871, stale :8164, restart skip :8326, carrera :8421, inválido :8482, removal :8599-8784, CAS :8830, barreras :9040-9215.

## 4. Fuera de alcance

Frontend (`AgentPickerModal.tsx`) sin cambios; sin cambio visual; `web/commands.rs`, `entity_creation`, `agent_config`, bulk, removal y default sin cambios. Sin nuevas dependencias.

## 5. Inventario

| Tipo | Archivo |
|---|---|
| Added | `plans/2010-keep-locked-selection.md` (este plan) |
| Modified | `src-tauri/src/config/coding_agent_profiles.rs` |
| Modified | `src-tauri/src/commands/config.rs` |
| Removed | ninguno |

## 6. Ciclos de dependencias

No aplica: no se añade ni quita ninguna referencia entre módulos (el writer usa tipos de su propio módulo; config.rs ya referencia los mismos símbolos). Sin arcos nuevos.

## 7. Entrega y verificación (owner: dev-rust implementa; Grinch revisa código y prueba; tech lead abre PR)

Todo desde `D:/0_repos/AgentsCommander_iac/.ac/room-18-ac-dev-team-v4/repo-AgentsCommander`, Git Bash.

### 7.1 Precondiciones (antes de editar)
```
git fetch origin main
git rev-parse --abbrev-ref HEAD            # fix/2010-keep-locked-selection
git merge-base --is-ancestor 5edf0cfeff0efb41988647f26aa2402c69d135d3 HEAD && echo ok
git status --porcelain                     # vacío (plans/ está en .gitignore:11, el plan no aparece)
test -f plans/2010-keep-locked-selection.md && echo plan-ok
git diff --name-only 5edf0cfeff0efb41988647f26aa2402c69d135d3 origin/main -- src-tauri/src/config/coding_agent_profiles.rs src-tauri/src/commands/config.rs src-tauri/src/phone/mailbox.rs src-tauri/Cargo.toml src-tauri/Cargo.lock .github/workflows
```
Si el último comando lista algo: refrescar solo la evidencia de esos archivos y avisar. Deriva ajena se registra y no reabre el plan.

### 7.2 Prueba de regresión (tests primero)
1. Añadir T1-T7 sin tocar el writer.
2. Control rojo, conservando el exit code (el log va a `target/`, ignorado; se crea si falta):
   ```
   mkdir -p target && set -o pipefail
   cd src-tauri && cargo test --lib issue_2010 2>&1 | tee ../target/i2010-red.log; echo "red_exit=$?"
   ```
   Esperado: `red_exit` distinto de 0; T1, T5, T6 FALLAN (error "locked"); T2, T3, T4, T7 pasan. Si `red_exit=0` o T1/T5/T6 pasan, el test no prueba el bug: parar.
3. Aplicar 3.1 a) y b).
4. Control verde: `cargo test --lib issue_2010 2>&1 | tee ../target/i2010-green.log; echo "green_exit=$?"` con `pipefail` activo. Esperado `green_exit=0` y 7 pasan.

### 7.3 Puertas locales (mismas que `pr-regression-gates.yml`)
```
cd src-tauri
cargo fmt --all -- --check
cargo test --lib issue_1937_selection
cargo test --lib issue_2010
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib --bins --tests
```
Esperado: todo verde. Un fallo previo en `main` solo se acepta si se reproduce igual en la base sin el cambio y se reporta con salida. No cancelar compilación ni tests por un tiempo fijo: si un comando puede pasar de 600 s, correrlo en background y esperar a que termine; un comando no terminado nunca cuenta como verde. Logs en `target/` (ignorado), con `pipefail` para no ocultar el exit code.

### 7.4 Alcance final
```
git diff --name-only 5edf0cfeff0efb41988647f26aa2402c69d135d3   # exactamente los 2 archivos Rust de §5
git add -f plans/2010-keep-locked-selection.md                    # plans/ es ignorado; se trackea forzado
git diff --cached --name-only 5edf0cfeff0efb41988647f26aa2402c69d135d3   # tras stage: los 2 Rust + el plan, nada más
git status --porcelain --ignored=no                               # sin otros untracked
```
Recuperación: si algo falla, revertir solo esos 2 archivos con `git restore -- <archivo>` si su contenido es aún del propio trabajo; nunca `git reset` ni limpieza global.

### 7.5 PR y CI
PR a `main` desde la rama, cuerpo con `Closes #2010`. Verificar base `main`, head rama, head SHA. Aceptación: todos los checks disparados y requeridos en verde sobre ese SHA exacto. Sin push directo a `main`. Sin release, instalación ni GUI.

Runner gate (owner: shipper): como máximo 3 pares repo+rama activos a la vez. El shipper hace una captura fresca antes de cada push, PR y merge, y repite cada 10 min mientras esté bloqueado. No esperar a que todo esté idle ni disparar directo.

### 7.6 Build final (owner: shipper; coordina tech lead tras CI y merge)
El shipper construye `main` ya mergeado, con artefacto con timestamp de room-18 dentro del repo, y entrega ruta, SHA256, tamaño y SHA de origen. No instalar.

## 8. Criterios de aceptación (mapa issue => test)
- Mismo par bloqueado OK, bytes idénticos, sin temp, candado puesto: T1, T5, T6.
- Agente distinto / perfil distinto rechazados: T2, T3, T7.
- Stale rechazado: T4 + :8164, :8830.
- Apply: 0 actualizados, sin restart ni transición, sin errores: T6.
- Self-switch equivalente sin escritura: T5.
- Protecciones previas verdes: §7.3.
