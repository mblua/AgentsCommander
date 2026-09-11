# Contraste de la propuesta de candados (dev-rust, sin implementar)

**Objeto:** `room-shared/candados-propuesta.md` (arquitecto), contra evidencia de `room-shared/candados-viabilidad.md`.
**Base:** main `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, árbol limpio. Sin pruebas ejecutadas.

## Validaciones (la propuesta coincide con el código)

1. No hay candado hoy; el único pin es el override de profile, que el bulk sobrescribe. Confirmado.
2. Bulk `Replica/Workgroup/Kind` escribe sin exclusiones (`src-tauri/src/commands/config.rs:1187-1319`); `Kind` = todas las réplicas de todos los AC roots configurados con la misma Matrix canónica (`:1439-1444`, `:1568`). Confirmado.
3. El par vive en `tooling.currentCodingAgent` + `tooling.profile` (+ alias legacy y source manual), y `lastCodingAgent` es historial separado. Confirmado (`src-tauri/src/config/coding_agent_profiles.rs:453-506`).
4. Precedencias: no autoritativo `override > pedido > defaultProfile Matrix > default_profile_by_agent > A`; wake autoritativo antepone el pedido (`:678-687`, `src-tauri/src/phone/mailbox.rs:11979`). Confirmado.
5. Reinicio: `pedido > currentCodingAgent > agente de la sesión` (`src-tauri/src/commands/session.rs:3721-3740`). Confirmado.
6. **La carrera toggle/escritura/reinicio es real**: `drop(apply_lock)` en `config.rs:1249` ocurre antes del loop de reinicios (`:1254-1291`). Confirmado.
7. **La pérdida por repetir alta es real**: `cli/team.rs:258-287` llama `create_or_update_replica_on_disk` sin condicionar a `added`, y `write_local_config_value` (`entity_creation.rs:556-565`) reemplaza el objeto completo. Confirmado.
8. **Persistencia viable**: `tooling.selectionLocked` (ausente=false) sobrevive porque todos los writers de config de réplica trabajan sobre JSON crudo con `update_config_json_object` (preserva claves desconocidas); `AgentLocalConfig`/`AgentTooling` solo se deserializan, nunca se serializan a disco (verificado por grep). `is_empty()`/`skip_serializing_if` no aplica porque no hay round-trip estructurado.
9. Contrato de discovery ya expone el par (`AcAgentReplica.current_coding_agent_id`/`current_profile`, `src-tauri/src/commands/ac_discovery.rs:89-103`; emisión `:1320`/`:2091`; TS `src/shared/types.ts:1313-1316`), así que agregar el flag es extensión natural.
10. Preview y apply re-enumeran y validan fingerprint (`config.rs:1419`, `:1642`); filtrar en la enumeración compartida mantiene consistencia y hace que un toggle entre preview/apply invalide la confirmación, como propone el arquitecto.

## Defectos y decisiones abiertas (concretas)

### D1 — El daño de repetir alta es mayor que `tooling`

`create_or_update_replica_on_disk` calcula `context` con `normalize_wg_replica_context_entries(&[], ...)` (`entity_creation.rs:1243-1249`): no lee el contexto existente. Por lo tanto el reemplazo total borra además las entradas de `context` personalizadas (las que administra `set_replica_context_files`) y cualquier clave desconocida de nivel superior. La propuesta dice "preservar tooling"; el fix debe preservar `context` custom y claves desconocidas, o decidir explícitamente que se recalculan. No hay test que cubra re-alta con preservación.

### D2 — Setter compartido: autoridad sin definir y segundo writer de profile ignorado

- `set_replica_coding_agent_selection` es el único writer de producción del par y lo usan el bulk y el self-switch (`mailbox.rs:11033`). La propuesta pide a la vez "revalidar protección dentro de la mutación del JSON" y "no introducir prohibición global en el setter compartido": ambas cosas no conviven sin un parámetro de autoridad/intención (o una ruta de escritura masiva separada). Si la revalidación vive en el setter, self-switch queda bloqueado; si no vive ahí, el bulk no tiene cierre en la mutación.
- `write_profile_to_launch_path` (`coding_agent_profiles.rs:507-539`, único caller `set_instance_profile_override` en `:449`) es un segundo writer que cambia **media pareja** (solo profile) sin tocar el flag ni `currentCodingAgent`. Con un candado activo, deja el par inconsistente. Decisión necesaria: bloquearlo en protegidas, materializar y conservar el candado, o permitir explícitamente el cambio parcial. Está expuesto por Tauri (`lib.rs:3654`) y web (`web/commands.rs:597`) aunque hoy no tenga caller de UI.

### D3 — Orden serial toggle/lote: el alcance debe incluir el reinicio, con costo explícito

- El reinicio usa el `coding_agent_id`/`profile` del request (`config.rs:1268-1280`) y **no relee la réplica**. Un candado escrito entre la escritura y el reinicio es ignorado por ese reinicio. Por eso el orden serial debe cubrir `write → restart`, no solo la escritura (la propuesta lo insinúa pero no fija el mecanismo).
- Cubrirlo con el lock existente exige mantener el `tokio::sync::Mutex` (así es `broad_profile_apply_lock`, `config.rs:1327-1331`) a través de N awaits de `restart_session_inner_with_intent`: bloquea todo apply durante el lote. La alternativa (releer el flag antes de cada reinicio) deja una ventana TOCTOU. El trade-off debe decidirse, no quedar implícito.

### D4 — Fingerprint y contratos: las omitidas deben salir del input de confirmación

Hoy `canonical_target_paths` del fingerprint (`config.rs:1642`) incluye todas las réplicas enumeradas y el preview cuenta todas (`target_count`, `live_session_count`, `:1121-1160`). Si las bloqueadas se marcan pero siguen en el fingerprint, un toggle entre preview y apply no invalida la confirmación (silencioso); si se excluyen, la invalidación funciona. La propuesta elige lo segundo ("cambio desde preview exige nueva confirmación") pero eso obliga a cambiar el contrato de `ProfileTargetEnumeration`/`ProfileAssignmentTarget`, el TS (`types.ts:1500-1533`) y el dispatcher web (`web/commands.rs:1474-1539`), además de partir los conteos. También debe definirse que la omisión esperada NO se reporte como `configWriteFailed`.

### D5 — Persistencia cruzada entre procesos: el merge no elimina el lost-update

Corregir `write_local_config_value` para mergear evita el borrado, pero app y CLI son procesos distintos: el mutex de `local_config_io` es por proceso y la publicación es atómica por archivo, no por transacción. Un read-modify-write del CLI `team add-member` concurrente con una escritura de selección/candado de la app puede perder cualquiera de las dos (gana la última publicación). La propuesta exige "mismo dueño de mutaciones" para la CLI nueva de candado; la ruta add-member necesita el mismo tratamiento o un ordenamiento documentado, o el fix queda incompleto.

### D6 — Menores

- Sin definir si el candado aplica a Root Agent (soporta `profileContentHash`, `coding_agent_profiles.rs:388-419`, y queda fuera de la enumeración masiva) y a matrices origen (las acepta `set_instance_profile_override`).
- Si el toggle materializa el par reutilizando `set_replica_coding_agent_selection`, hereda la forma dual completa (`profile` + `instanceProfileOverride` + `instanceProfileOverrideSource="manual"`); si escribe solo `profile`, se dispara el warning de divergencia en `read_replica_profile_result` (`:355-373`). Conviene fijar la forma de escritura en una sola mutación.
- La observación de GUI es correcta y verificable: `applyEnabled` bloquea por selección redundante (`AgentPickerModal.tsx:65`, `:471-483`, `:1079-1085`); el toggle no debe reutilizar ese gate.

## Veredicto

**Viable.** La propuesta no contradice la evidencia y su núcleo (booleano persistente por réplica, filtro en escritura y reinicio, revalidación en la mutación, materialización del par al activar, fix de conservación en el alta) es implementable con los módulos existentes. Antes de aprobar quedan: D1/D5 (completitud y seguridad del fix de alta repetida), D2 (autoridad en el setter compartido + segundo writer de profile), D3 (mecanismo y costo del orden toggle/lote con reinicios) y D4 (contrato de enumeración/fingerprint).
