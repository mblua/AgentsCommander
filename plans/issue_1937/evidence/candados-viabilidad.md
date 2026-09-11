# Candado activable/desactivable para conservar coding agent + profile en réplicas

**Tipo:** informe de viabilidad (investigación). **Sin implementación, sin issue, sin branch, sin cambios de producto, sin pruebas interactivas.**
**Repo:** `D:/0_repos/AgentsCommander_iac/.ac/room-12-ac-dev-team-v4/repo-AgentsCommander`
**Commit:** `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d` (main, working tree limpio).
**Fecha del relevamiento:** 2026-09-10.

## Método y cobertura

- Descubrimiento principal por MCP graph (`cbm_search_graph`, `cbm_trace_path`, `cbm_search_code`); verificación contra archivos reales por lectura directa y `rg` sobre `src-tauri/` y `crates/`.
- Índice del graph: 23.036 nodos / 152.508 edges, estado `ready`. Cobertura (`cbm_index_status`): `skipped` = 0; `parse_partial` = solo `crates/session-bridge/Dockerfile` y `docker/ac-claude/Dockerfile` (no tocan este dominio). El código Rust relevante está indexado sin gaps reportados.
- No se ejecutaron pruebas.

## 1. Modelo de datos actual (persistencia)

Selección por réplica (archivo raíz `<replica>/config.json`, compartido entre instancias):

| Campo `tooling.*` | Significado | Escrito por |
|---|---|---|
| `currentCodingAgent` | Coding agent elegido por la UI de selección | `set_replica_coding_agent_selection` (`src-tauri/src/config/coding_agent_profiles.rs:453-506`) y self-switch |
| `profile` | Letra de profile (v2) | ídem + `write_profile_to_launch_path` (`:507-539`) |
| `instanceProfileOverride` | Alias legacy de `profile` (se escribe en paralelo) | ídem |
| `instanceProfileOverrideSource` | Marca `"manual"` | ídem — **nadie la lee** (verificado: únicas apariciones son las escrituras) |
| `profileContentHash` | Hash del comando/env cargado, para drift | `set_replica_profile_content_hash` (`:392-419`) en spawn |
| `lastCodingAgent`, `codingAgents` | Historial de lanzamiento | `set_last_coding_agent` (`src-tauri/src/config/agent_config.rs:135-171`) |
| `lastAgentMessageAt` | Stamp de fin de turno | `set_last_agent_message_at` (`agent_config.rs:176-217`) |

- Config **por instancia** `<replica>/<.local>/config.json` (`.agentscommander` por defecto, `src-tauri/src/config/mod.rs:969`): `lastCodingAgent`, `codingAgents`, `lastAgentMessageAt`. El comentario de `src-tauri/src/phone/mailbox.rs:12047-12060` confirma el reparto: `currentCodingAgent` va al config raíz, `lastCodingAgent` al per-instance.
- Matriz origen `_agent_*/config.json`: `tooling.defaultProfile` (`set_agent_default_profile`, `coding_agent_profiles.rs:421-434`).
- `settings.json` → `codingAgentProfiles`: `profilesByAgent` (celdas comando+env), `profileSlots`, `defaultProfileByAgent` (`src-tauri/src/config/settings.rs:153-175`).
- Deserialización tolera legado: `profile` tiene `alias = "instanceProfileOverride"` (`agent_config.rs:57-66`) y la lectura advierte si divergen, prefiriendo v2 (`coding_agent_profiles.rs:355-373`).
- Root Agent (`ac-root-agent`) también puede llevar `profileContentHash` (gate explícito, `coding_agent_profiles.rs:388-419`).

**No existe hoy ningún campo, archivo o concepto de candado/pin de selección.** El único mecanismo de conservación existente es el `instanceProfileOverride` por réplica: gana en la resolución, pero el bulk lo sobrescribe (sección 2).

## 2. Rutas que ESCRIBEN la selección (coding agent + profile)

### 2.1 Bulk equipo/room/tipo (el caso del pedido)

- `preview_coding_agent_profile_selection` / `apply_coding_agent_profile_selection` (`src-tauri/src/commands/config.rs:1107-1180`; inner `:1187-1319`; web dispatcher equivalente en `src-tauri/src/web/commands.rs:1474-1539`).
  - Scopes: `Replica | Kind | Workgroup` (`config.rs:1032-1037`). UI: "This replica" / "All replicas of this kind" / "Entire room" (`src/sidebar/components/AgentPickerModal.tsx:901-950`).
  - Enumeración: `enumerate_profile_assignment_targets` (`config.rs:1419-1515`). `Workgroup` = todos los `__agent_*` del room (`collect_replica_dirs_in_workgroup`, `:1550`); `Kind` = todas las réplicas de **todos los AC roots configurados** cuyo `identity.matrix_dir` coincide (`collect_kind_replica_dirs`, `:1568`, y filtro en `:1466-1471`).
  - Escritura: loop sobre targets → `set_replica_coding_agent_selection` sin ninguna condición de exclusión (`config.rs:1231-1247`). Escribe `currentCodingAgent`, `profile`, `instanceProfileOverride`, `instanceProfileOverrideSource="manual"` (`coding_agent_profiles.rs:486-503`). **No toca `lastCodingAgent`** (test `selection_write_dual_writes_profile_and_preserves_last_coding_agent`, `:921-945`).
  - `restart_sessions=true` relanza las sesiones vivas de las réplicas escritas con `Some(agent)`/`Some(profile)` explícitos (`config.rs:1254-1291`).
  - Serialización in-process: `broad_profile_apply_lock` (`config.rs:1327-1331`). Confirmación obligatoria y fingerprint para todo scope ≠ Replica (`:1355-1371`, `:1642`).
- Creación de room: `create_workgroup_on_disk` (`src-tauri/src/commands/entity_creation.rs:1140`) y el comando legacy `create_workgroup` (`:2818`, escritura en `:2923`) crean `__agent_*` con `write_local_config_value` (reemplazo total del JSON, `:556-565`). Solo aplica a rooms nuevos (exige que el dir no exista), no hay selection previa que conservar.
- Edición de equipo GUI: `update_team` (`entity_creation.rs:3361-3436`) → `sync_workgroup_repos_inner` (`:3477-3670`). Actualiza `repos`/`context`/`identity` con `update_config_json_object` **preservando `tooling`** (`:3560-3600`). No crea ni borra réplicas por sí mismo.
- **CLI `ac team add-member`** (`src-tauri/src/cli/team.rs:258-287`): llama `create_or_update_replica_on_disk` (`entity_creation.rs:1223-1261`) **sin condicionar a si el miembro ya existía** (`added` no se usa para saltar). Esa función escribe `{identity, repos, context}` con `write_local_config_value`, que **reemplaza el objeto completo** (`:560-564`). Consecuencia verificada por código: re-agregar un miembro existente (o correr el comando dos veces) **borra todo `tooling` de la réplica**, incluidos pin, `currentCodingAgent`, `profileContentHash` y cualquier candado que se guarde en ese archivo. No hay test que cubra preservación en ese camino.

### 2.2 Individual

- Mismo apply con scope `Replica`.
- `set_instance_profile_override` (`commands/config.rs:950-978`; `coding_agent_profiles.rs:436-451`): escribe `profile` + legacy + source, **no** escribe `currentCodingAgent`. Acepta réplica **y matriz origen** (`validate_profile_selection_agent_path`, `:261-304`). Declarado en `src/shared/ipc.ts:397-400`; sin callers de producción actuales en el frontend (solo la superficie Tauri/web).
- `set_agent_default_profile` (`commands/config.rs:920-948`): escribe el `defaultProfile` de la matriz.
- Self-switch (agente iniciado por sí mismo): `handle_self_handoff_switch` (`phone/mailbox.rs:10400`) → persist closure (`mailbox.rs:11025-11037`, llamada en `:11033`) → `set_replica_coding_agent_selection`. Es la única escritura de selección fuera del bulk. Solo permitido desde réplica de room (`:1525-1550`).
- Catálogo de coding agents (add/update/remove) es otro dominio: `cli/coding_agent.rs` + `config/coding_agent_mutations.rs` (`CodingAgentOp`, `:47-56`); no escribe selección de réplica.

### 2.3 Defaults / origen

- Ranking de `resolve_profile` (`coding_agent_profiles.rs:625-720`):
  - no autoritativo (picker, restart, drift): **instance override > pedido explícito > `defaultProfile` de la matriz > `defaultProfileByAgent` de settings > "A"** (`:678-687`).
  - autoritativo (wake con `--profile`): pedido explícito > instance override > … (mismo bloque).
- Wake dispatch pasa `requested_profile_authoritative = msg.requested_profile.is_some()` (`phone/mailbox.rs:11979`): un wake con profile explícito **gana al pin de la réplica para ese spawn**, sin escribir en disco. Además, un wake con flags (`--agent`/`--profile`) suprime el guardado de tooling (`mailbox.rs:7224-7229`).
- Selección de agente en wake: `preferredAgent` > `currentCodingAgent` > `lastCodingAgent` > `senderAgent` > primer agente (`resolve_wake_agent_command_from_sources`, `mailbox.rs:590-650`).
- Settings: `default_profile_by_agent` se valida A-Z (`settings.rs:2306-2314`), se poda/crea "A" en repair (`settings.rs:1863-1891`) y se elimina al borrar el agente (`entity_creation.rs:1938-1963` y `:2473-2487`).
- Si la celda pedida no está habilitada, `resolve_profile` cae a la letra inferior disponible; "A" sintetiza celda vacía (`coding_agent_profiles.rs:690-720`).

### 2.4 Arranque y CLI

- Spawn/creación de sesión: escribe `lastCodingAgent` + `codingAgents` + `profileContentHash` (`src-tauri/src/commands/session.rs:2580-2626`); **no** escribe `currentCodingAgent`/`profile`. En restore de arranque se re-resuelve el spawn desde settings + `requested_profile` persistido de la fila de sesión (`lib.rs:1714-1860` y bloque análogo en `:2050-2140`).
- Restart: resuelve agente con `requested > currentCodingAgent > stored` (`session.rs:3721-3740`) y profile con `requested.or(stored)` (`:3703-3708`); no persiste selección. El bulk sí pasa agent/profile explícitos al restart (`config.rs:1268-1280`).
- Drift: `compute_profile_outdated` compara el hash configurado vs `profileContentHash` (memoria o disco) (`session.rs:2755-2800`).
- CLI `self-switch`: escribe un mensaje en outbox (`cli/self_switch.rs:237-266`); la persistencia la hace el mailbox (2.2). CLI `self-restart`: no toca la selección (invariante documentada en `mailbox.rs:11129-11136`). CLI `send --profile` (`cli/send.rs:87-96`, `:1060-1095`): wake autoritativo, sin escritura persistente. `cli/team.rs` es el único verbo CLI que sí escribe config de réplica (2.1, con el hallazgo del wipe).

## 3. Omisiones y puntos ciegos detectados

1. **No existe candado ni marcador equivalente**; el bulk pisa el único pin existente (`instanceProfileOverride`) sin consultar nada.
2. **Dos choke points de escritura de profile**: `set_replica_coding_agent_selection` (bulk + self-switch) y `write_profile_to_launch_path` (override individual, matrices incluidas). Un gate en uno solo deja fuga.
3. **Reemplazo total de `config.json` en `write_local_config_value`** (`entity_creation.rs:556-565`): el CLI `team add-member` lo invoca en réplicas existentes y destruye pin/candado (2.1). Un candado guardado en la réplica no sobrevive ese camino.
4. **`instanceProfileOverrideSource` es write-only**: no hay lectura de procedencia ("manual" vs masivo) que un candado pudiera reutilizar.
5. **`Kind` es multi-proyecto**: enumera todos los AC roots configurados, no solo el room/proyecto actual (`config.rs:1439-1444`).
6. **Preview vs apply**: ambos re-enumeran y el apply valida fingerprint (`config.rs:1355-1371`). Si el candado excluye targets en apply pero no en preview, o cambia entre ambos, el apply se rechaza por fingerprint; si se filtra en la enumeración compartida, preview y apply quedan consistentes.
7. **Concurrencia**: `broad_profile_apply_lock` es in-process; self-switch persist, CLI y rutas de daemon no lo toman. `update_config_json_object` serializa solo dentro del proceso (`local_config_io.rs:7-13`) con publicaciones atómicas con retry (#537). Ventanas de carrera entre apply y self-switch/CLI existen.
8. **"Conservar agente+letra" no congela la celda**: editar el comando/env de la celda en Settings o deshabilitarla cambia el lanzamiento efectivo (y dispara drift) aunque la selección quede intacta.
9. **Wake autoritativo** con `--profile` ignora el pin (por diseño, un solo spawn). Un candado no bloquea ese forzado salvo que se decida lo contrario.
10. **Restart del bulk**: con `restart_sessions=true` relanza sesiones vivas de cada target; una semántica de candado debe decidir si la réplica bloqueada también se excluye del restart.
11. **Root Agent y matrices origen fuera de la enumeración** (`__agent_*` solamente), pero `set_instance_profile_override` acepta matrices: alcance del candado a definir.
12. **Legado divergente**: `profile` vs `instanceProfileOverride` distintos producen warning y gana v2 (`coding_agent_profiles.rs:355-373`); convivencia a definir para el candado.
13. **Errores de lectura**: la enumeración omite réplicas ilegibles/inválidas con warning (`config.rs:1459-1471`); hay que definir si un candado ilegible falla abierto o cerrado.
14. **La eliminación de agente/equipo** borra defaults y réplicas (`entity_creation.rs:1938-1963`, `:2998+`, `cli/team.rs remove-member`); no hay conservación posible una vez borrado el target.

## 4. Riesgos (para el contraste con la propuesta)

- **R1 — Candado evitable o destruible**: guardarlo en el `config.json` de la réplica lo expone al borrado total del camino `team add-member`; el candado "desaparece" y una corrida bulk posterior pisa la selección. Cualquier diseño debe fijar un almacenamiento que ese camino preserve o corregir el camino.
- **R2 — Conservación parcial**: gatear solo `currentCodingAgent` o solo `profile` deja el otro pise. El write real es dual y el restart del bulk pasa ambos explícitos.
- **R3 — Falsa sensación de conservación**: candado de selección ≠ congelamiento del comando efectivo (celda, env base, fallback A-Z, cambio de catálogo). El badge de drift seguirá reflejando cambios de la celda.
- **R4 — Divergencia UI/backend**: preview, fingerprint, contadores (`target_count`, `updated_count`) y eventos `coding_agent_profile_selection_updated` deben reflejar las réplicas excluidas, o la UI mostrará targets que el apply omite.
- **R5 — Carreras**: apply concurrente con self-switch/CLI; el lock in-process no cubre procesos CLI separados.
- **R6 — Persistencia multi-instancia**: el config raíz de la réplica es compartido; el per-instance no. Elegir mal la ubicación cambia el alcance del candado (por réplica lógica vs por instancia local).
- **R7 — Semántica de desactivación**: no hay precedente de "activar/desactivar" por réplica; `set_instance_profile_override(None)` borra pin (individual), pero el bulk no tiene forma de "des-bloquear y re-aplicar" en una sola operación.
- **R8 — Migración**: réplicas ya escritas por bulk no tienen forma de distinguir una selección histórica de una reciente; sin procedencia (`instanceProfileOverrideSource` no se lee), un candado nuevo no puede inferir intención previa.

## 5. Evidencia de referencia rápida

- Bulk apply: `src-tauri/src/commands/config.rs:1107-1319`, `:1231-1247`, `:1254-1291`, `:1327-1331`, `:1355-1371`, `:1419-1515`, `:1550`, `:1568`, `:1642`.
- Escritura de selección: `src-tauri/src/config/coding_agent_profiles.rs:421-434`, `:436-451`, `:453-506`, `:507-539`.
- Resolución/ranking: `coding_agent_profiles.rs:625-720`; defaults: `settings.rs:153-175`, `:2306-2314`, `:1863-1891`.
- Wipe CLI: `src-tauri/src/cli/team.rs:258-287`; `src-tauri/src/commands/entity_creation.rs:556-565`, `:1223-1261`; preservación en edición GUI: `:3477-3670`.
- Arranque/CLI: `src-tauri/src/commands/session.rs:2580-2626`, `:3703-3740`, `:2755-2800`; `src-tauri/src/phone/mailbox.rs:590-650`, `:10400`, `:11025-11037`, `:11979`, `:7224-7229`; `src-tauri/src/cli/self_switch.rs:237-266`; `src-tauri/src/cli/send.rs:87-96`.
- UI: `src/sidebar/components/AgentPickerModal.tsx:901-950`; `src/shared/ipc.ts:397-424`.

**Conclusión de viabilidad:** el pedido es viable sobre el código actual porque existe un único formato de persistencia por réplica y un choke point dominante de escritura masiva (`set_replica_coding_agent_selection`), más un segundo escritor individual a gatear (`write_profile_to_launch_path`). Los bloqueantes concretos son el reemplazo total de `config.json` en el camino CLI `team add-member`, la ausencia de precedencia legible de "manual", la consistencia preview/fingerprint y las carreras entre procesos/self-switch. No se diseñó solución; queda para la propuesta del arquitecto.
