# Propuesta de candados para Coding Agent + Profile

Investigación Narrow design Q&A, 2026-09-10. Base comprobada: main, b4f7c9de7f15b75f8f12c034c8356f73babd0a8d, árbol limpio. No implementación, issue, branch, build ni certificación de implementación. Clase rutinaria; riesgo concreto: sobrescritura de selección y reinicio de sesiones.

Recomiendo un candado persistente por réplica que excluya conjuntamente su Coding Agent y Profile de asignaciones masivas. Reutilizar el selector, enumeración y persistencia actuales. “Tipo” puede seleccionar muchas réplicas para activar sus candados. El usuario confirmó incluir futuras réplicas con excepciones individuales: default al crear, par materializado y estado local posterior, sin propagación retroactiva.

## Evidencia y límites

MCP primero: list_projects, index_status, get_architecture, search_graph, get_code_snippet, trace_path y check_index_coverage, siempre con project igual a la ruta absoluta del repo de room-12. Índice ready: 23.036 nodos / 152.508 relaciones. La cobertura reporta metadata_changed en archivos productivos; algunos snippets tienen líneas desplazadas. Verifiqué directamente las funciones decisivas en disco. No afirmo frescura completa del grafo; sirve para descubrir. AgentPickerModal.test.tsx y ProjectPanel.profile-outdated.test.tsx están excluidos por fast-pattern y se leyeron directamente. Ninguna prueba fue ejecutada.

Referencias relativas al repo autorizado; líneas de disco cuando difieren del índice:

| Evidencia | Ubicación |
|---|---|
| Scopes, preview, aplicación, fingerprint | src-tauri/src/commands/config.rs:1032,1121,1189,1419,1642 |
| Selección persistida, lectura compatible, precedencia | src-tauri/src/config/coding_agent_profiles.rs:355,453,507,625 |
| Escritura serializada y publicación temporal | src-tauri/src/config/local_config_io.rs:11 |
| Creación de réplica y reemplazo completo del objeto | src-tauri/src/commands/entity_creation.rs:556,1223 |
| Repetir alta de miembro vuelve a crear/actualizar réplica | src-tauri/src/cli/team.rs:257 |
| Elección en reinicio | src-tauri/src/commands/session.rs:3721 |
| Elección wake y selección explícita | src-tauri/src/phone/mailbox.rs:590,12031 |
| Descubrimiento expone selección e identidad | src-tauri/src/commands/ac_discovery.rs:89,1230,2008 |
| GUI y contrato | src/sidebar/components/AgentPickerModal.tsx:481; src/shared/types.ts:1306,1484; src/shared/ipc.ts:397 |
| Paridad web y prueba de broadcast | src-tauri/src/web/commands.rs:616,1474 |
| CLI self-switch, send y catálogo | src-tauri/src/cli/self_switch.rs:49; src-tauri/src/cli/send.rs:85; src-tauri/src/cli/coding_agent.rs:64 |

Cobertura consultada para esos archivos, src-tauri/src/lib.rs, src-tauri/src/cli/mod.rs y ambos tests frontend; scope cli sin incidencias registradas, señal best-effort. Lectura de lib.rs pendiente si se concreta un nuevo comando Tauri; su registro fue localizado por MCP, no certificado.

## Qué hace hoy

- Replica modifica una instancia concreta. Workgroup enumera las réplicas de una room, no todas las rooms del Team lógico. Kind recorre raíces configuradas y compara la ruta canónica de la Matrix origen; no agrupa por proveedor, nombre parecido ni campo type de Role.md.
- Preview calcula destinos/sesiones y fingerprint. Apply reenumera y valida la confirmación masiva, escribe cada réplica y opcionalmente reinicia las sesiones cuya escritura tuvo éxito. Tiene resultados parciales y distingue destroyedButNotRecreated.
- La selección conjunta vive en config.json superior de la réplica: tooling.currentCodingAgent y tooling.profile; también escribe instanceProfileOverride y su source manual por compatibilidad. lastCodingAgent es historial separado.
- Perfil ordinario: override de réplica > solicitud explícita > defaultProfile de Matrix > default_profile_by_agent > A. Una solicitud wake marcada authoritative antepone su perfil explícito al override. El perfil efectivo puede bajar a una letra disponible.
- Reinicio elige Coding Agent explícito > selección persistida válida > agente de la sesión. Wake explícito también puede anteponerse a selección; la ruta auto consulta selección, historial y fallbacks.
- En las funciones de asignación inspeccionadas no hay exclusión por candado. Las referencias actuales a pin describen precedencia del perfil, no inmunidad a escritura masiva.
- Crear/actualizar réplica construye identity/repos/context y reemplaza el objeto completo. team add-member llama esa rutina incluso si el miembro ya estaba presente: riesgo verificable de perder tooling, incluida una futura protección.

## Semántica propuesta; alcance de futuras réplicas confirmado

| Operación | Resultado recomendado |
|---|---|
| Activar en réplica | Persistir protection junto a una selección conjunta explícita validada. Si solo hay herencia o historial ambiguo, mostrar y elegir el par antes de activar. |
| Desactivar | Quitar protección; conservar selección. La próxima asignación masiva podrá cambiarla. |
| Asignar Workgroup o Kind | Omitir protegidas tanto en escritura como en reinicios. Mostrar motivo y cantidad. |
| Asignar Replica directamente | Permitir modificación deliberada del par y conservar candado. |
| Reiniciar/reabrir normalmente | Conservar protección y selección persistida; utilizar resolución existente. |
| Self-switch explícito | Cambio individual permitido, preservando candado. No introducir prohibición global en el setter compartido. |
| Override individual de solo perfil | Permitir una letra explícita validada, conservar Coding Agent y candado. En protegidas, rechazar limpiar el perfil con null: primero desactivar o elegir otra letra. |
| Send con agente/perfil explícitos | Mantener override temporal existente; documentar que el candado protege asignaciones masivas, no toda ejecución. |
| Cambiar contenido del slot o eliminar proveedor | Mantener reglas actuales de resolución/fallback; mostrar drift o indisponibilidad. No promete congelar modelo, comando, entorno ni versión. |

El par protegido es ID del Coding Agent + letra solicitada del perfil, no una copia de su contenido ni necesariamente la letra efectiva. La activación debe mostrar ambos cuando haya fallback. Un candado sobre valores heredados sin materializarlos permitiría que el default cambiara lo “protegido”.

Para operar “por tipo”, activar/desactivar candados en las réplicas actualmente enumeradas por Kind. Cada instancia conserva su par propio; no copiar el par de la réplica inicial a todas. Las nuevas réplicas reciben el default vigente del ámbito elegido al crearlas, con excepciones individuales; configurar ese default no modifica las existentes.

La protección de futuras réplicas usa una política en Matrix o membresía Team separada del estado de instancia, copiada únicamente al crear. Las excepciones se guardan en el estado local; no se requiere triestado ni protección obligatoria. Team completo, una room y tipo de Matrix son ámbitos distintos. Precisar cuál corresponde a “tipo”; no inferirlo del rótulo “equipo”.

## Persistencia, interfaz y concurrencia

Nombre ilustrativo: tooling.selectionLocked, booleano ausente equivale a false. Mantener campos existentes, compatibilidad de profile/instanceProfileOverride y claves desconocidas. El alta repetida debe leer el objeto existente y preservar tooling, entradas custom de context y claves desconocidas de nivel superior; actualizar solo repos e identidad/contexto obligatorios mediante la normalización existente. No reconstruir context desde un arreglo vacío. Una réplica realmente nueva materializa el default conjunto vigente del ámbito elegido, salvo excepción explícita. Recrear una room eliminada aplica esa política persistente externa vigente; no recupera los candados locales eliminados.

Ubicar validación y lectura/escritura en coding_agent_profiles/local_config_io; coordinación, eventos y reinicios permanecen en commands/config. Añadir estado a discovery/AcAgentReplica, contratos Rust/TS y selector. Reutilizar evento coding_agent_profile_selection_updated con datos suficientes para refrescar todas las ventanas y navegador.

Intención interna tipada para el escritor del par: BulkAssignment frente a IndividualAssignment, más una operación explícita de toggle que activa/materializa en una sola mutación. La capa de comandos deriva la intención del scope y de la ruta autorizada; el cliente no recibe una bandera genérica para saltarse el candado. BulkAssignment relee y valida selectionLocked dentro del read-modify-write protegido y devuelve Updated o SkippedLocked. IndividualAssignment valida el par y puede cambiarlo conservando el flag; self-switch utiliza esa intención. Así la revalidación dentro del setter no bloquea toda edición individual.

El segundo escritor write_profile_to_launch_path también participa en la coordinación/exclusión. Cambiar solo la letra es una edición individual explícita del par, con Coding Agent intacto; no es inconsistencia por sí misma. En una réplica protegida debe existir un Coding Agent persistido válido, la letra nueva debe validarse y null debe rechazarse sin cambios porque quitaría la selección materializada. Si el par protegido ya está incompleto o corrupto, exigir reparación explícita mediante selección conjunta, no rellenarlo por historial silenciosamente.

Activar escribe currentCodingAgent, profile, instanceProfileOverride, instanceProfileOverrideSource="manual" y selectionLocked=true en el mismo read-modify-write/publicación. Cambiar una letra mantiene ambos campos de perfil iguales; no alterar el historial ni el hash de contenido cargado como efecto del toggle. Desactivar escribe false o elimina solamente el flag.

Alcance del flag en esta propuesta: únicamente réplicas __agent_* de room-* o wg-* validadas. Toggle de Root Agent o Matrix origen se rechaza; sus operaciones existentes de perfil siguen funcionando sin este candado. El default confirmado para futuras réplicas es una política de creación distinta, no el mismo flag de bloqueo sobre la Matrix.

Preview debe separar eligibleTargets y skippedTargets con motivo; candidateCount = eligibleCount + skippedCount, y liveSessionCount cuenta solo sesiones elegibles. targetCount, si se conserva por compatibilidad, significa elegibles y debe cambiar coordinadamente en Rust/TS/UI. El fingerprint de mutación utiliza las rutas canónicas elegibles, no una lista indiferenciada de candidatas. Además incorpora scope, identidad, selección pedida y restartSessions. Un componente de estado de confirmación separado puede representar protección/motivos de omitidas para detectar cambios de estado sin tratarlas como objetivos de escritura. Apply reenumera y compara bajo la coordinación común antes de escribir; un toggle cambia la pertenencia elegible y exige nuevo preview. Extender ProfileTargetEnumeration, DTOs de preview/apply, evento, contratos TS y paridad Tauri/web. skippedLocked es resultado esperado; lectura inválida es omisión con diagnóstico distinto, nunca configWriteFailed. Cero elegibles devuelve cero cambios y cero reinicios.

Decisión de serialización: ampliar la coordinación asíncrona actual para que cada lote conserve su turno desde la revalidación hasta finalizar todos sus reinicios. Toggle y cambios individuales de selección, incluido el persist de self-switch y el override de perfil, participan en ese orden. El toggle posterior espera y solo confirma al terminar el lote anterior; no promete deshacerlo. No se elige una relectura aislada antes de reiniciar porque conserva la carrera. Costo aceptado para esta propuesta: todos esos cambios esperan hasta N reinicios; UI muestra pendiente y no anuncia un candado aplicado anticipadamente. El mutex síncrono de archivos se libera antes de cualquier await.

La operación pertenece al backend: cerrar el modal o desconectar un cliente no libera prematuramente el turno. Ante cancelación o fallo, finalizar/cancelar los reinicios ya iniciados y reportar resultados parciales antes de liberar la coordinación; no dejar reinicios tardíos tras confirmar un toggle. Revisar orden con las coordinaciones existentes de ciclo de sesión y probar ausencia de deadlock en el plan detallado; una ruta interna ya coordinada no puede readquirir el mismo guard.

La CLI nueva de candado se encamina al mismo dueño de operación; sin backend disponible devuelve error explícito en la primera versión propuesta. Además, el alta CLI existente necesita protección entre procesos: usar exclusión compartida por archivo canónico de configuración de réplica, desde antes de leer hasta publicar, tanto en app como en CLI. Participan todos los read-modify-write de ese archivo, incluida el alta repetida corregida; no basta mergear ni publicar por rename atómico. Adquirir siempre turno de operación (si aplica) antes de exclusión de archivo. Un bloqueo de archivo con liberación del sistema al terminar el proceso y espera acotada evita propietarios abandonados; timeout devuelve error sin publicación. Esta exclusión protege bytes, no sustituye la coordinación escritura/reinicio. No usar un bloqueo sostenido durante awaits de reinicio. El mecanismo concreto y cobertura de escritores requieren verificación de plataforma antes del plan; el riesgo de concurrencia demostrado justifica este control.

GUI: candado visible junto al par, texto “Conservar ante cambios masivos”, resultado “N actualizadas, M protegidas, K errores”. Mantener accesibilidad y confirmación del alcance. El toggle independiente no debe quedar bloqueado por la detección de selección redundante.

CLI actual: coding-agent administra el catálogo global; self-switch cambia la réplica propia; send permite overrides de ejecución. No son un comando de candado. Para paridad futura, definir una operación de inspección/toggle con destino canónico y autoridad validada, encaminada al backend común. Nombre, modo offline y alcance de autorización quedan para diseño detallado; no inventar una escritura directa CLI que evada el filtro.

## Verificación requerida para una implementación posterior

- JSON viejo sin campo; activar/desactivar; conservación de claves, identity, repos, contexto y campos legacy.
- Lote mixto Workgroup/Kind, misma Matrix en varias rooms, matrices homónimas distintas, cero elegibles, réplicas sin sesión y múltiples sesiones.
- Protegida conserva ambos valores y no recibe reinicio; individual/self-switch conserva flag; override wake conserva semántica temporal.
- Toggle entre preview/apply y entre escritura/reinicio; selección concurrente; fallo de escritura, fallo de recreación y reporte parcial.
- Alta repetida preserva tooling, context custom y claves desconocidas; creación nueva y reapertura/restauración conservan la semántica acordada.
- App y CLI simultáneos conservan ambos cambios bajo exclusión compartida; timeout o muerte de un proceso no publica contenido parcial ni deja un bloqueo permanente.
- Bulk y self-switch se distinguen por intención interna; override individual cambia solo perfil y conserva el candado, pero null en protegida falla sin mutar.
- Un toggle en espera no confirma antes del último reinicio del lote, tampoco si el cliente se desconecta; fallo/cancelación no deja reinicios tardíos.
- Frontend: omitidas, conteos, confirmación obsoleta, candado con selección redundante, refresh multiventana/web.
- Perfil heredado/fallback, slot modificado, proveedor eliminado y ausencia de selección inicial.

Tests existentes útiles: config.rs prueba fingerprint y confirmación masiva; coding_agent_profiles.rs prueba precedencia y compatibilidad; AgentPickerModal.test.tsx:583,602,859 prueba confirmación y preview obsoleto; web/commands.rs:1474 prueba escritura real y broadcast. Son bases de extensión, no evidencia de comportamiento nuevo.

## Estado de entrega

Contraste dev completado y defectos D1-D6 incorporados como decisiones de propuesta para evaluación de producto; NO READY_FOR_IMPLEMENTATION. Confirmado por el usuario el alcance de futuras réplicas con excepciones locales (room-shared/candados-decision-usuario.md, 2026-09-11). Pendientes la precisión Matrix frente a membresía Team para “tipo” y la ratificación del contrato de protección ante lotes. El diseño detallado deberá validar coordinación, exclusión entre procesos, contratos y gates. No es un Full plan: particionado no aplicable todavía.

Antes de implementar, tech lead/implementer deben fijar issue y branch autorizados, scope exacto, base y drift relevante; derivar toolchain, checks locales y workflows reales; verificar recuperación limitada a cambios propios. CI deberá pasar en el SHA exacto del PR. Esos gates no fueron ejecutados ni certificados en esta investigación.

Se cargaron delivery-nonfunctional-invariants y verify-no-dependency-cycles. Preferencia estructural: reutilizar módulos/arcos existentes, sin trasladar AppHandle o transporte a persistencia. Inventario exacto de arcos, SCC pre/post y arc record quedan para el plan concreto y su reviewer; no se certifica ausencia de ciclos sin diff. No se requieren controles de procedencia de ejecutables ajenos al riesgo de esta tarea.


## Contraste con viabilidad dev completado

Leído room-shared/candados-viabilidad.md, SHA256 45d69cd983d202062245121f860a26e97ef3985f3b26b6feeb99ec097849de01. Coincidencia: formato persistido, scopes, override wake temporal, reemplazo en team add-member, necesidad de excluir escritura/reinicio, coherencia preview y concurrencia.

Diferencia de alcance: dev propone gatear también el escritor individual y presenta ausencia de procedencia manual como bloqueo. Para la semántica recomendada de protección ante lotes, no hace falta prohibir cambios individuales ni inferir procedencia histórica: flag explícito y operación/alcance tipado bastan. Si se exige inmovilidad absoluta, revisar todos los escritores y resolver wake explícito; sería otro contrato. No reutilizar instanceProfileOverrideSource como candado.

Verificación adicional contra disco: sync_workgroup_repos_inner en entity_creation.rs:3564 preserva tooling al editar equipo por GUI. El riesgo de reemplazo detectado corresponde a create_or_update_replica_on_disk invocado por alta CLI repetida; no atribuirlo a toda edición de Team.

Adopto el caso de lectura inválida señalado por dev: ausencia del campo en JSON válido equivale a false; JSON ilegible, tooling mal tipado o flag presente no booleano debe generar omisión/error visible sin escritura ni reinicio, nunca tratarse silenciosamente como desbloqueado. read_tooling_string en coding_agent_profiles.rs:49 colapsa errores en None, por lo que el lector del candado necesita distinguir ausencia y error. Añadir prueba de flag inválido.

Matiz de cobertura: “sin gaps” no prueba frescura. Nuestro check_index_coverage devuelve metadata_changed y frontend tests excluidos; mantener la evidencia directa y esa limitación, incluso cuando index_status sea ready. Los nuevos fragmentos verificados pertenecen a archivos ya cubiertos.

Contraste completado; quedan únicamente las decisiones de producto y gates del eventual plan, no la recepción del informe dev. Viable como cambio acotado tras fijar el contrato; sigue sin certificación de implementación.


## Variante confirmada: protección para réplicas de rooms futuras

El usuario eligió «Sí, con excepciones por réplica», registrado en room-shared/candados-decision-usuario.md. Se incorpora la variante presentada: default al crear la réplica, con selección conjunta materializada y excepciones locales. No hay herencia continua ni propagación retroactiva. Ajuste documental solicitado con repo de referencia repo-AgentsCommander, base f16edd976f9861648d04d137b3bb960189d4548e; no se revalidó código ni se sustituyó la base histórica de la evidencia anterior.

Ámbito: si “tipo” significa la Matrix ya usada por Kind, la política corresponde a esa Matrix. Si significa “ese miembro solamente dentro de este Team”, corresponde a la membresía Team+Matrix y exige ampliar su esquema; no guardar una preferencia de Team en la Matrix porque afectaría otros Teams. No ofrecer ambos ámbitos como equivalentes. Identificar Matrix mediante identidad validada; Kind puede buscar múltiples raíces configuradas.

Modelo ilustrativo de default: selección conjunta {codingAgentId, requestedProfile, protectFromBulk}. Para heredar solo el booleano haría falta una fuente inequívoca del par al crear; hoy no hay un default conjunto de selección con la misma semántica que el override. Exigir ese par al configurar protección futura evita depender de lastCodingAgent como política. Mantener el motor actual de perfiles y advertir fallback.

Creación: leer el default del ámbito elegido, validar proveedor/slot y copiar selección+flag a la nueva réplica. Si el usuario aporta una excepción explícita al crearla, prevalece esa selección/flag. Si no hay default, mantener comportamiento actual sin candado. En reintentos o actualización de una réplica existente preservar sus valores; no volver a copiar el default.

Vida posterior: el estado local materializado es la autoridad. Cambiar o desactivar la política futura no toca réplicas ya creadas. Un comando separado, con preview, puede aplicar protección a las existentes; no hacerlo como efecto lateral del cambio del default. Así no se necesita triestado durante cada asignación masiva. Debe rotularse “Default para nuevas réplicas”, no “Herencia continua”.

Las excepciones individuales permiten modificar el par o desactivar el candado de una réplica; los cambios posteriores del default no las sobrescriben. La propagación continua queda fuera del alcance confirmado.

Borrado/recreación: la room nueva recibe el default vigente, no los candados de la room eliminada. Archivar/restaurar la misma réplica mantiene su estado persistido. Configuración inválida de un default protector debe impedir crear una réplica supuestamente protegida con otra selección sin advertirlo.

Pruebas específicas futuras: nueva room, nuevo miembro, reintento sin reset, excepción desactivada, dos Teams compartiendo Matrix, default modificado sin alterar existentes, recreación tras borrado y proveedor/slot inválido. Validar esquema autoritativo de Team/Matrix y arcos exactos antes del plan; esta variante no está certificada.

## Cierre de defectos del contraste

Revisado candados-contraste.md, SHA256 ffade4b0708563ff212b1842681472d1212898bd387e416c4e36ddf5e0389e96. D1: conservación completa en re-alta. D2: intención interna bulk/individual, segundo escritor y null definidos. D3: turno asíncrono cubre escritura y reinicios, con espera visible y finalización propiedad del backend. D4: destinos elegibles separados de omitidas y contratos/conteos coordinados. D5: exclusión de archivo compartida entre procesos para todos sus escritores, además del merge. D6: Root/Matrix fuera del candado de instancia y dual-write atómico al materializar.

Son decisiones de diseño documentadas, no defectos corregidos en producto ni pruebas aprobadas. Investigación cerrada; inclusión de futuras réplicas con excepciones individuales confirmada por el usuario, bajo la semántica de creación descrita. La implementación posterior debe demostrar los mecanismos y gates detallados arriba.
