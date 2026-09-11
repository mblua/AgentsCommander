# Issue #1937 — Candados de Coding Agent + Profile

[Issue en GitHub](https://github.com/mblua/AgentsCommander/issues/1937)

Paquete del diseño aceptado por el usuario el 2026-09-11 UTC, con plan para retomar la implementación. La aprobación corresponde a la UX del prototipo v3; la funcionalidad todavía no está implementada en la aplicación.

## Abrir

- [Plan consolidado](PLAN.md).
- [Especificación del issue](ISSUE.md).
- [Prototipo aprobado v3](prototypes/index-v3.html): abrir localmente en el navegador, sin servidor ni build.
- [Guía del prototipo v3 y comprobaciones](prototypes/README-v3.md).
- [Decisiones literales del usuario](evidence/candados-decision-usuario.md).

En el prototipo: vista 2 para la fila `+ lock`, vista 5 para conflictos, vista 9 para el alcance independiente de `Remove lock`, y vista 10 para el resultado de desbloquear `Entire room`. Las operaciones son simuladas; no escriben configuración real.

## Capturas

- [Asignación + lock](prototypes/vistas-v3/apply-lock.png).
- [Diálogo con las tres acciones](prototypes/vistas-v3/conflicto.png).
- [Alcances de Remove lock](prototypes/vistas-v3/remove-scope-barra.png).
- [Lista v3 con solo protegidas](prototypes/vistas-v3/tipo-preview-lista.png).
- [Resultado de desbloquear Entire room](prototypes/vistas-v3/resultado-unlock-room.png).
- [Detalle del resultado](prototypes/vistas-v3/resultado-unlock-room-barra.png).

Las capturas enlazadas corresponden al HTML v3 aprobado, referencia visual final. Versiones anteriores, snapshots, fuentes de maquetas y herramientas de captura se conservan bajo `prototypes/` como historial. Los scripts de reconstrucción/captura reflejan el entorno original; el HTML autocontenido es la opción portable para revisar.

## Evidencia y precedencia

Las decisiones del usuario, la especificación del issue y el plan consolidado describen el alcance actual. Los informes en `evidence/` son evidencia histórica: algunas propuestas anteriores quedaron superadas por las iteraciones aprobadas, especialmente la resolución explícita de conflictos de `+ lock`. No usarlos para revertir esa decisión.

Se conserva el HTML final con SHA-256 `7901780e9d936c603ba0a704b9cf002197a8bdf5cf687b795b2c0fe37ae7f7b0`. `MANIFEST.sha256` permite verificar todos los archivos del paquete.

Ubicaciones de este mismo paquete:

- Repo: `plans/issue_1937/`, rama `feature/1937-selection-lock-plan`.
- Copia compartida solicitada: `D:\0_repos\AgentsCommander_iac\.ac\plans\issue_1937\`.

Base de código usada para el archivo del plan: `7aef1d14e6bb0255b1430614c078145335dd54a4`. Base del prototipo: `f16edd976f9861648d04d137b3bb960189d4548e`; la diferencia hasta la base de archivo es únicamente un Dockerfile.

Nota de archivo: las capturas `tipo-preview*.png` se actualizaron desde el HTML v3 final al preparar este paquete. La guía histórica README-v3 conserva su referencia a la iteración anterior; el HTML aprobado no cambió.
