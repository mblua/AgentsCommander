# Decisión del usuario — 2026-09-11

Pregunta: «Cuando ponés el candado por tipo de agente, ¿querés que también lo hereden las réplicas que se creen en futuras rooms?»

Respuesta: «Sí, con excepciones por réplica».

Requisito confirmado: incluir futuras réplicas y permitir excepciones individuales. Actualizar propuesta y maquetas existentes para que dejen de presentar ese alcance como pendiente.

Esta respuesta define alcance de producto; no solicita implementar la funcionalidad ni aprobar un plan técnico.

## Iteración v2 del prototipo

El usuario pide una segunda fila de radios justo debajo de Apply to, con los mismos destinos y «+ lock», para asignar y bloquear al mismo tiempo. Entrega en index-v2.html; no sobrescribir index.html.

Al consultar si la operación masiva debe sobrescribir réplicas ya bloqueadas, el usuario respondió:

«Debe presentar cartel indicando con cuales conflictua y ahí ofrecer "Forzar de todas formas", "Forzar solo las no previamente lockeadas" o "Cancelar". Las opciones estas deben ser en ingles y yo dije los conceptos, pero expralo de la mejor manera posible. Y que el cancelar esté a izquierda/derecha según como generalmente esté el cancelar en la app.»

Requisito confirmado: diálogo de conflictos identificando réplicas afectadas, tres acciones que representen esos conceptos con buen texto en inglés, y Cancel en la posición habitual de la app. Esta decisión sustituye para las opciones masivas + lock la suposición provisional de omitir siempre las bloqueadas. Es alcance del prototipo, no autorización de implementación real.

## Alcance propio de Remove lock

El usuario señala la barra superior del estado del candado y pide: «El remove lock debería también presentar el concepto de This replica, All replicas of this kind y Entire room. Inclusive podría estar presentado ahí arriba como ya figura.»

Actualizar la v2 con alcance de desbloqueo en esa barra. Conservar v1. Sigue siendo una iteración del prototipo.

## Iteración v3: lista informativa solo de protegidas

El usuario pide ocultar las filas Not protected del listado por tipo mostrado en su captura: basta con el resumen «1 of 2» y la fila protegida. En el ejemplo debe desaparecer room-15, conservando el total. Entrega explícita en index-v3; preservar versiones anteriores. El cambio reduce líneas del listado, sin alterar el alcance real de las acciones.

## Completar resultado del desbloqueo

El usuario señala que falta mostrar cómo queda la pantalla después de aceptar el desbloqueo, por ejemplo Entire room. Incorporar ese estado posterior y el recorrido en la v3, manteniendo la semántica definida de conservar configuraciones y afectar solo el alcance elegido.

## Aprobación de la UX y archivo del plan — issue #1937

Después de revisar la v3 con el resultado de Entire room desbloqueada, el usuario confirmó: «dale, ahora me gustó. Agrega todo esto a un issue, y deja el plan dentro de una carpeta con nombre issue_(# de issue creado), TAMBIEN, en D:\0_repos\AgentsCommander_iac\.ac\plans».

Issue creado: https://github.com/mblua/AgentsCommander/issues/1937. Prototipo aprobado: index-v3.html con SHA-256 7901780e9d936c603ba0a704b9cf002197a8bdf5cf687b795b2c0fe37ae7f7b0. Alcance de esta solicitud: registrar issue, consolidar y guardar plan/artefactos en issue_1937, con copia compartida en la ubicación indicada. No se solicitó implementar ni fusionar cambios de producto.
