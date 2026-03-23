# Problema: Bridge bidireccional Terminal <-> Telegram

## Contexto

Tengo una app de escritorio (Tauri 2 + Rust + SolidJS) que gestiona sesiones de terminal. Cada sesion tiene un PTY (pseudo-terminal) que ejecuta un shell. Una de las funcionalidades es un "bridge" bidireccional que conecta una sesion de terminal con un bot de Telegram:

- **Terminal -> Telegram**: la salida de la terminal se envia a Telegram para monitoreo remoto
- **Telegram -> Terminal**: los mensajes enviados al bot se inyectan como input al PTY

El caso de uso principal es monitorear y controlar remotamente sesiones de Claude Code (un agente de IA que trabaja en la terminal) via Telegram.

## Problema 1: Terminal -> Telegram (filtrado de output)

La salida cruda del PTY contiene ANSI escape codes que codifican no solo colores sino **movimiento de cursor, borrado de lineas, reescritura de pantalla**. Claude Code usa un TUI (Text User Interface) que redibuja la pantalla constantemente para mostrar spinners, barras de progreso, status bars y animaciones.

Cuando intento enviar esa salida a Telegram, llega basura:

1. **Texto sin espacios**: "HolaMariano,teescucho" en vez de "Hola Mariano, te escucho" (los espacios que el terminal renderiza via posicionamiento de cursor se pierden)
2. **Spinners mezclados con contenido**: `✢Bre·Bwriewng✢i…ng…*…✻Brewing…` intercalado con respuestas reales
3. **Chrome del TUI filtrandose**: status bars, tips, barras de progreso, tool headers (`●─Bash(...)`)
4. **Duplicacion masiva**: la misma linea enviada decenas de veces porque cada caracter que Claude escribe genera un cambio

## Problema 2: Telegram -> Terminal (inyeccion de input)

Cuando el usuario envia un mensaje desde Telegram, ese texto debe llegar al PTY como si el usuario lo hubiera tipeado en la terminal. Los problemas:

1. **Echo duplicado**: el texto enviado desde Telegram se escribe en el PTY, la terminal lo muestra (echo), el bridge lo detecta como output nuevo y lo reenvia a Telegram - loop de eco
2. **Timing de input**: mientras el usuario tipea en la terminal fisica, los caracteres individuales generan output que el bridge intenta enviar caracter por caracter
3. **Visibilidad**: el usuario que envia desde Telegram no tiene confirmacion de que su mensaje llego a la terminal, y el usuario en la terminal no sabe que llego input remoto

## Que deberia llegar a Telegram

- Respuestas del modelo (lo que Claude Code le dice al usuario)
- Output real de comandos ejecutados (resultado de git, npm, etc.)
- Input del usuario en terminal (lo que escribio, una vez, al presionar Enter)
- Confirmacion de input recibido desde Telegram

## Que NO deberia llegar

- Spinner animations (`✻ Brewing…`, `✶ Gallivanting…` - Claude Code randomiza el verbo)
- Tool headers (`●─Bash(rtk git pull)`, `●─Read(file.ts)`)
- Progress indicators (`⎿ Running…`, `⎿ Running… (3s)`)
- Status bar (modelo, contexto, uso de tokens)
- TUI chrome (tips, shortcuts, separadores visuales con box-drawing chars)
- Caracteres de reescritura/cursor movement residuales
- Lineas garbled por concatenacion incorrecta de posiciones de pantalla
- Echo de lo que el usuario envio desde Telegram (ya lo sabe, lo acaba de escribir)
- Streaming caracter por caracter mientras el usuario tipea en la terminal

## Datos tecnicos relevantes

- El PTY emite bytes crudos con ANSI escape sequences
- Claude Code usa un TUI que reescribe la pantalla con cursor movement (no es output lineal)
- Los spinners cambian cada ~450ms, rotando entre los caracteres: ✻ ✶ * ✢ · ● ✽
- Claude Code randomiza el verbo del spinner en cada sesion (Brewing, Gallivanting, Pontificating, Topsy-turvying, etc.), asi que no se puede mantener una lista fija
- El caracter `●` (U+25CF) es tanto un spinner como el indicador de respuesta de Claude Code (`● Respuesta del modelo`)
- La terminal tiene ~220 columnas de ancho, y los spinners aparecen en el extremo derecho concatenados con contenido real en la misma fila
- El backend esta en Rust con tokio (async)
- El bridge usa Telegram Bot API: `getUpdates` para polling de mensajes entrantes, `sendMessage` para enviar output
- El input de Telegram se inyecta al PTY con `write(text + "\r")` (simula Enter)

## Que se intento y fallo

- **strip_ansi_escapes**: solo quita los escape codes pero no simula el terminal, asi que se pierden los espacios y el posicionamiento. Resultado: texto concatenado sin espacios + spinners mezclados
- **vt100 crate + HashSet diff**: simula un terminal virtual y compara filas entre frames. Resuelve los espacios, pero emite cada cambio de caracter (streaming char-by-char genera ~60 mensajes por oracion)
