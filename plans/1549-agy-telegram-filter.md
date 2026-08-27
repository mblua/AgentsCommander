# Plan — #1549 Telegram bridge leaks Antigravity (agy) TUI chrome into messages

Status: READY_FOR_IMPLEMENTATION
Plan-SHA256: computed at delivery (see reply)
Path chosen: Lite
Round 2 revision (2026-08-25): Blocker A (status line con contador, diag-sent.log L208) resuelto con regla contains-based sobre ` · high|medium|low`; Blocker B (thought lines `▸`, glifos sueltos `●`/`○`, encuesta, step labels) resuelto con reglas nuevas + residual documentado y acceptance realineada; editoriales (tg.attach en telegram.rs:167, detector externo en <VAULT>) aplicadas.
Round 3 revision (2026-08-25, último round): Blocker C (logo VERTICAL del frame 03:15:27 — filas de texto sin glifo de bloque) resuelto: formas paréntesis `(High)/(Medium)/(Low)` en `AGY_STATUS_PATTERNS`, patrón de cuenta `Google AI Pro`, fila de path como residual documentado, tests que pinean las 4 filas exactas; nits aplicados (ruta del detector sin segmento `rust/` duplicado; falso positivo ` · higher/highly` documentado).

## 1. Frozen authority

- Repo: `repo-AgentsCommander` (workgroup replica), branch `fix/1549-agy-telegram-tui-filter`.
- Base: branch tip `2a1bb0e3a7d74502342e2c226e44f6de0cfe3229` (== `main` == `origin/main`), worktree clean at plan time.
- Issue: #1549 (OPEN, label bug). Evidence artifacts: `D:\0_repos\AgentsCommander_iac\.agentscommander_ac2\diag-sent.log` (session 19de56d2-5625-4bfc-ad8c-f524a25f960a, 03:14 UTC 2026-08-25), `telegram-bridge.log:10752-10770` (STABLE/SEND_TG pairs). Full diagnosis: `messaging/20260825-033800-wg21-ac-dev-rust-v3-to-wg21-ac-tech-lead-v3-1545-telegram-leak-diagnosis.md`.
- Plan file is gitignored; implementer force-adds (`git add -f plans/1549-agy-telegram-filter.md`).

## 2. Issue and objective

agy (Antigravity CLI 1.1.19) sessions bridged to Telegram send the full TUI chrome in every message: logo, hints (`? for shortcuts`, `esc to cancel`), input echo (`> Hola`), and the status line (`Gemini 3.7 Flash · high`, model · effort), mixed with real conversation content. Objective: filter that chrome for agy sessions exactly as Claude Code sessions already are, so Telegram messages carry only real content, with zero behavior change for other agents and zero IPC contract change.

## 3. Evidence (compact)

- `src-tauri/src/telegram/bridge.rs:574` hardcodes `let filter: Box<dyn AgentFilter> = Box::new(ClaudeCodeFilter);` inside `output_task` (bridge.rs:552-667). `AgentFilter` is explicitly designed for per-agent rules (trait at bridge.rs:268-271; section comment bridge.rs:261-266). `ClaudeCodeFilter` (bridge.rs:307-391) is the only implementation in the codebase.
- `src-tauri/src/commands/telegram.rs:36-89` `derive_reader` returns `Ok(None)` for `CodingAgentKind::Antigravity` (and `Pi`) → `spawn_bridge` (bridge.rs:456-534) takes the `None` branch → `output_task` (PTY path). The stabilized VT screen IS the sent content, unfiltered for agy chrome.
- Chain: `output_task` → vt100 parse (bridge.rs:631) → 800 ms stabilization (`RowTracker::harvest_stable`, bridge.rs:199-232) → filter (bridge.rs:574) → buffer → `flush_buffer` (`src-tauri/src/telegram/output.rs:192-268`, consecutive-line dedup + 4000-char chunks) → `api::send_message` (output.rs:230).
- Log evidence (diag-sent.log, session 19de56d2-…, frames 03:14-03:23 UTC): logo rows (**horizontal** 03:14:29: `▄▀▀▄ … Antigravity CLI 1.1.19`, `▀▀▀▀▀▀ … mariano.blua@gmail.com`, `▀▀▀▀▀▀▀▀ … Gemini 3.7 Flash (High)`, `▄▀▀ … ▀▀▄ … D:/0_repos/…`; **vertical** frame de respuesta 03:15:27 L19-33, verificado con `cat -A`: `  Antigravity CLI 1.1.19`, `  mariano.blua@gmail.com (Google AI Pro)`, `  Gemini 3.7 Flash (High)`, `  D:/0_repos/…/__agent_ac-cli-tester-v3` — SOLO 2 espacios de indent, SIN glifo de bloque); hints (`? for shortcuts …`, `esc to cancel …`) y status line sola (`Gemini 3.7 Flash · high`); **status line con contador** `esc to cancel … Gemini 3.7 Flash · high · 1 task(s) · /tasks` (L208); eco `> Hola`; **thought lines** `▸ Thought for 2s, 236 tokens` / `… 14s, 2.8k tokens` (L96/122/140) y fila fusionada `▸ ThougUse /feedback to share your experience with the team.` (L~201); **glifos sueltos** `●` y `○` (frames 03:22:47, 03:23:37); **step labels** `  Analyzing Shell Arguments`, `  Clarifying Authorization Boundaries`, `  Investigating System Behavior`, `  Observing System Activity` (L97/108/186/194); **encuesta** `How's the CLI experience so far? Help us improve:` + `[1] Good  [2] Fine  [3] Bad  [0] Skip` (L240/241, SEND_TG 03:23:50.301); filas de actividad de tool-calls (`● Read(…)`, `○ Search(…)`, `  Read(…)`); y el frame de respuesta con contenido real (`  ¡Hola! Soy ac-cli-tester-v3…`).
- Input direction (Telegram→PTY) already works (echo `> Hola` proves `poll_task` writes to the PTY); the leak is output-only.

## 4. Scope

### In scope
- New per-agent filter `AgyFilter` for the PTY output path, mirroring `ClaudeCodeFilter`.
- Per-agent filter selection in `output_task` keyed on `CodingAgentKind`, threaded from the existing attach entry point.
- Unit tests for the filter and the selection; module-arcs record regeneration.

### Out of scope (explicit)
- Pi: keeps today's behavior (ClaudeCodeFilter as-is); separate evaluation, same generic PTY path (see §10 residual).
- Reader-mode agents (Claude/Codex watchers) and the Telegram→PTY input path: untouched.
- `flush_buffer`/`output.rs`/`api.rs`/`src/shared/ipc.ts`: untouched (no contract change).
- VT100 parsing, stabilization timing, chunking: untouched.
- Validating the full pattern surface of agy 1.1.19 TUI with dev-rust: NOT a blocking dependency — the filter is data-driven and pinned by unit tests; the observed formats are sufficient for the decision (§10).

## 5. Decided solution

1. **`AgyFilter`** in `src-tauri/src/telegram/bridge.rs` — `struct AgyFilter;` implementing `AgentFilter` (`keep_line` + `name() -> "antigravity"`). Keep a line iff it is NOT any of:
   - empty after trim;
   - contains any of `AGY_CHROME_PATTERNS` = `["? for shortcuts", "esc to cancel", "Antigravity CLI", "How's the CLI experience so far?", "[0] Skip", "Use /feedback", "Google AI Pro"]` (el último cubre la fila de cuenta del logo vertical `mariano.blua@gmail.com (Google AI Pro)` — nombre de plan estable, colisión baja);
   - trimmed contains any of `AGY_STATUS_PATTERNS` = `[" · high", " · medium", " · low", " (High)", " (Medium)", " (Low)"]` (status line model · effort — **contains-based** en ambas grafías: middot ` · high` y paréntesis ` (High)` del logo vertical; cubre la variante con contador `Gemini 3.7 Flash · high · 1 task(s) · /tasks`, las filas fusionadas con hints y la fila `  Gemini 3.7 Flash (High)` del frame 03:15:27; tradeoff: contenido real con ` · high`/` (High)` en cualquier posición también cae, aceptado y documentado en §7/§10 — incl. falso positivo ` · higher`/` · highly`);
   - trimmed starts with `▸ ` (U+25B8 + space): thought/status marker (`▸ Thought for 2s, 236 tokens`, incl. fila fusionada `▸ ThougUse /feedback…`);
   - trimmed is exactly ONE non-alphanumeric glyph (bare `●`, `○` — decoración suelta);
   - trimmed starts with a half-block/block glyph U+2580..U+259F (logo rows: `▄▀▀▄`, `▀▀▀▀`, `▐`…);
   - trimmed is `>` or starts with `> ` (input echo — extensión DELIBERADA sobre la regla de ClaudeCodeFilter, que solo tira `❯`, `>` y `❯ `-prefixed, no `> `-prefixed);
   - fails the shared decoration checks (below).
2. **Extract shared predicates** from `ClaudeCodeFilter::keep_line` (bridge.rs:310-387) into free functions in the same module, used by BOTH filters (behavior-neutral refactor; identical predicates, identical order):
   - `is_box_drawing_line(non_space: &str) -> bool` (all non-space chars in U+2500..U+256C set, len > 5);
   - `starts_with_braille(s: &str) -> bool` (first char in U+2800..=U+28FF);
   - `is_low_alnum_line(trimmed: &str) -> bool` (len > 5 and alnum+space ratio < 0.30);
   - keep `is_thinking_line` (bridge.rs:397-429) as-is.
3. **Selection**: `fn filter_for_agent(agent_kind: Option<CodingAgentKind>) -> Box<dyn AgentFilter>` in bridge.rs:
   - `Some(CodingAgentKind::Antigravity)` → `Box::new(AgyFilter)`;
   - everything else (`Claude`, `Codex`, `Pi`, `None`) → `Box::new(ClaudeCodeFilter)` — preserves current behavior exactly for all non-agy sessions.
4. **Thread `agent_kind`** (type `Option<CodingAgentKind>`, Copy — proven by the existing move out of the RwLock read guard at telegram.rs:107) through the PTY-bridge chain:
   - `output_task` (bridge.rs:552) gains parameter `agent_kind: Option<CodingAgentKind>`; line 574 becomes `let filter = filter_for_agent(agent_kind);`. INIT log line (`output_task started: filter=…`) then shows `filter=antigravity` for agy sessions — objective live evidence hook.
   - `spawn_bridge` (bridge.rs:456) gains the same parameter; passes it in the `None` reader branch.
   - `TelegramBridgeManager::attach` (`src-tauri/src/telegram/manager.rs:90-148`) gains the same parameter; passes it to `spawn_bridge`.
   - `attach_telegram_bot_by_id` (`src-tauri/src/commands/telegram.rs:91-206`) passes the `agent_kind` it already reads from the session (line ~107) to `tg.attach(…)` (**telegram.rs:167** — verified). All production attach paths (`attach_local_config_telegram_if_any`, restart transactions) funnel through this one entry point; Rust's compiler catches any other call site (tests included).

## 6. Affected files and exact symbols

| File | Change |
|---|---|
| `src-tauri/src/telegram/bridge.rs` | `use crate::session::profile::CodingAgentKind;` (new arc); extract `is_box_drawing_line`, `starts_with_braille`, `is_low_alnum_line`; `AgyFilter` + `AGY_CHROME_PATTERNS` (incl. `Google AI Pro`) + `AGY_STATUS_PATTERNS` (contains-based, grafías middot Y paréntesis) + `impl AgentFilter` (incl. reglas `▸ ` y glifo único) + `filter_for_agent`; `output_task` signature + line 574; `spawn_bridge` signature + `None` branch. |
| `src-tauri/src/telegram/manager.rs` | `use crate::session::profile::CodingAgentKind;` (new arc); `TelegramBridgeManager::attach` signature + `spawn_bridge` call. |
| `src-tauri/src/commands/telegram.rs` | `tg.attach(…, reader, agent_kind)` call site only. |
| `src-tauri/module-arcs.txt` | Regenerate (adds exactly the 2 new arcs, see §11); commit with the change. |
| `src-tauri/src/telegram/bridge.rs` `#[cfg(test)]` | New unit tests (§9). |

No other files. `src/shared/ipc.ts`, `src-tauri/src/telegram/output.rs`, `api.rs`, watchers: untouched.

## 7. Required behavior, edge cases, failures

- Per frame (diag-sent.log fixtures): logo rows dropped — horizontal (block-art start y/o `Antigravity CLI`) y **vertical 03:15:27** (`  Antigravity CLI 1.1.19` por patrón; `  mariano.blua@gmail.com (Google AI Pro)` por patrón `Google AI Pro`; `  Gemini 3.7 Flash (High)` por paréntesis ` (High)`); hints (`? for shortcuts` / `esc to cancel`) dropped (patrón, también cuando comparten fila con la status line — el contains de ` · high` los cubre por igual); `> Hola` echo dropped (`> ` prefix); status line sola Y con contador (`Gemini 3.7 Flash · high` y `… · high · 1 task(s) · /tasks`) dropped (contains); `▸ Thought for Ns, N tokens` (incl. `14s, 2.8k`) y la fusión `▸ ThougUse /feedback…` dropped (prefijo `▸ `); glifos sueltos `●` / `○` dropped (glifo único no alfanumérico); encuesta (`How's the CLI experience so far? Help us improve:` y `[1] Good  [2] Fine  [3] Bad  [0] Skip`) dropped (patrones); fila de respuesta conserva solo contenido real (`  ¡Hola! Soy ac-cli-tester-v3…` y las líneas siguientes).
- No new cross-flush state: `RowTracker::emitted_content` (bridge.rs:224) ya deduplica filas byte-idénticas; las reglas contains/patrón eliminan las variantes cambiantes de status (contador, hints, grafías) ANTES de entrar al buffer, por lo que la status line no puede re-entrar tras un flush — incluidas la variante con contador (Blocker A) y las de paréntesis (Blocker C). El dedup de líneas consecutivas en `flush_buffer` queda intacto.
- Edge cases:
  - Status line variant `Gemini 3.7 Flash (High)` — en el logo HORIZONTAL cae por block-art start; en el logo VERTICAL (sin glifo) cae por el patrón paréntesis ` (High)` (Blocker C). Ninguna forma depende de la otra.
  - Contenido que contenga ` · high` / ` · medium` / ` · low` o ` (High)` / ` (Medium)` / ` (Low)` en CUALQUIER posición → dropped; tradeoff contains-based aceptado (Blockers A y C); incluye el falso positivo adicional ` · higher` / ` · highly` (substring) — misma clase, documentado.
  - Contenido que empiece con `> ` (texto citado) → dropped; tradeoff documentado (extensión deliberada sobre la regla de Claude, que solo cubre `❯`/`>`/`❯ `).
  - Fila de path del logo vertical (`  D:/0_repos/…`) → **NO filtrada — residual documentado** (§10.4): el path es indistinguible de contenido real (aparece igual en filas de tool-calls y en mensajes); solo se ve en re-displays del alt-screen.
  - Step labels de agy (`  Analyzing Shell Arguments`, `  Clarifying Authorization Boundaries`, `  Observing System Activity`, etc.) → **NO filtrados — residual documentado** (§10.4): vocabulario dependiente de la tarea, sin marcador estructural generalizable; una denylist de labels observados no generaliza (rechazada por YAGNI).
  - Filas de actividad de tool-calls (`● Read(…)`, `○ Search(…)`, `  Read(…)`, `(ctrl+o to expand)`) → pasan intencionalmente: son actividad informativa del agente, no chrome del issue; quedan fuera de la garantía de acceptance.
  - Effort label desconocido para `AGY_STATUS_PATTERNS` → la fila se filtra una vez; extender la lista (data-driven; unit tests la fijan).
  - `Pi` / no-agent sessions → filter name stays `claude-code` en el INIT log (comportamiento bit-idéntico al actual).
- Failure modes: `filter_for_agent` is total (no error path); `keep_line` is pure; a future agy TUI change degrades to "chrome leaks" only — triage remains via `diag-sent.log` and the STABLE/SEND_TG log pairs. No crash, no send-path change.

## 8. Compatibility and security

- No IPC/contract change: `src/shared/ipc.ts` and all Tauri command signatures untouched; bridge attach/detach persistence flow unchanged.
- No secrets involved; no config changes; no new dependencies.
- agy's own output is not modified — filtering happens only on the Telegram bridge path (the terminal TUI is unaffected).
- Enables the #1545 agy "Telegram input/output" matrix row to be verified live with clean content.

## 9. Tests and acceptance criteria

Unit tests (bridge.rs `#[cfg(test)] mod tests`, create if absent):
1. `agy_filter_keep_line_drops_logo_rows` — las 4 filas de logo HORIZONTAL del diag-sent.log (`▄▀▀▄ … Antigravity CLI 1.1.19`, `▀▀▀▀▀▀ … mariano.blua@gmail.com`, `▀▀▀▀▀▀▀▀ … Gemini 3.7 Flash (High)`, `▄▀▀ … ▀▀▄ … D:/0_repos/…`) + las filas de texto del logo VERTICAL (Blocker C): `  Antigravity CLI 1.1.19`, `  mariano.blua@gmail.com (Google AI Pro)`, `  Gemini 3.7 Flash (High)` → todas DROPPED.
2. `agy_filter_keep_line_pins_frame_031527_rows` — las 4 filas EXACTAS del frame 03:15:27 (L19-33) con desenlace explícito: `  mariano.blua@gmail.com (Google AI Pro)` → DROPPED; `  Gemini 3.7 Flash (High)` → DROPPED; `  D:/0_repos/AgentsCommander_iac/.ac/wg-21-ac-dev-team-v3/__agent_ac-cli-tester-v3` → KEPT (residual de path documentado §10.4); `  ¡Hola! Soy ac-cli-tester-v3. Estoy listo para ayudarte con la validación` → KEPT (contenido).
3. `agy_filter_keep_line_drops_hint_rows` — `? for shortcuts … Gemini 3.7 Flash · high`, `esc to cancel … Gemini 3.7 Flash · high`.
4. `agy_filter_keep_line_drops_status_line` — bare `Gemini 3.7 Flash · high`; `… · medium`, `… · low`; **y la variante con contador (Blocker A) en forma desnuda** `Gemini 3.7 Flash · high · 1 task(s) · /tasks` y **fusionada** `esc to cancel               Gemini 3.7 Flash · high · 1 task(s) · /tasks` (contenido EXACTO del frame 03:23:42.495, L208).
5. `agy_filter_keep_line_drops_thought_lines` — `▸ Thought for 2s, 236 tokens`, `▸ Thought for 14s, 2.8k tokens`, y la fila fusionada `▸ ThougUse /feedback to share your experience with the team.` (L96/122/140/~201).
6. `agy_filter_keep_line_drops_single_glyph` — `●` sola (frame 03:22:47), `○` sola (frame 03:23:37).
7. `agy_filter_keep_line_drops_survey` — `How's the CLI experience so far? Help us improve:` y `[1] Good  [2] Fine  [3] Bad  [0] Skip` (L240/241); `Use /feedback to share your experience with the team.` standalone.
8. `agy_filter_keep_line_drops_input_echo` — `> Hola`, `>`.
9. `agy_filter_keep_line_keeps_content` — `¡Hola! Soy ac-cli-tester-v3. Estoy listo…`, `He ejecutado la validación en vivo de las 5 filas PENDING asignadas`, una línea de código/herramienta normal, y una fila de actividad `● Read(D:/…sesion.rs) (ctrl+o to expand)` (pasa intencionalmente).
10. `agy_filter_keep_line_passes_step_labels` — `  Analyzing Shell Arguments`, `  Clarifying Authorization Boundaries`, `  Investigating System Behavior`, `  Observing System Activity` → KEPT (fija el residual documentado §10.4 como deliberado, no accidental).
11. `agy_filter_keep_line_tradeoff_drops_midline_effort` — `El modelo rinde · high en esta prueba` → DROPPED; `El modo (High) es el más potente` → DROPPED; `Rendimiento · higher que antes` → DROPPED (fija el tradeoff contains-based de Blockers A/C, incluido el falso positivo substring ` · higher`/` · highly`).
12. `filter_for_agent_maps_antigravity` — name() == "antigravity"; `filter_for_agent_maps_everything_else_to_claude` — Claude/Codex/Pi/None → name() == "claude-code" (regression pin).
13. Shared predicate spot tests (box-drawing separator, braille first char, low-alnum line) — same inputs behave identically through both filters.

Acceptance (objective, run on branch head, clean tree):
- `cargo build` and `cargo test` (src-tauri) green, including the new tests; existing telegram/commands tests untouched and green.
- Live check (cli-tester or dev-rust): agy session via Telegram, exchanges que cubran las variantes (`? for shortcuts`, `esc to cancel`, status con contador — tarea corriendo, **frame de respuesta con logo vertical (03:15:27-style)**, encuesta si reaparece) — los mensajes NO contienen status line (ninguna variante: middot, paréntesis, contador), hints, logo (horizontal Y vertical: email + plan + modelo·esfuerzo), eco `> `, thought lines `▸ …`, glifos sueltos ni encuesta; el contenido real (respuesta y actividad de tool-calls) sí llega. Step labels y fila de path del logo fuera de la garantía (residuales §10.4-10.5). `telegram-bridge.log` INIT line muestra `filter=antigravity` para la sesión agy y `filter=claude-code` para Pi/plana.
- No regression: Pi session sends byte-identical message content to pre-change (same filter name, same pipeline).
- Cycle gate (§11): instrument pre/post on clean trees; `cyclicSccs` unchanged (1), SCC member sets identical, new arcs exactly the two listed, `module-arcs.txt` regenerated and committed so a re-run yields empty `git status` on it; structural layering guards (`loops_layering`, `instance_gitignore_layering`, `project_settings_layering`) stay green.

## 10. Decisions and residuals (no TBDs)

1. Filter selection lives in bridge.rs (`filter_for_agent`), not in commands: keeps the trait, the filters, and the selection co-located and unit-testable; commands only forwards the session's already-known `agent_kind`.
2. Shared-predicate extraction (not copy-paste) for the generic decoration checks: behavior-neutral, avoids drift between the two filters; ClaudeCodeFilter semantics unchanged.
3. `Pi` intentionally keeps `ClaudeCodeFilter` — changing it is a separate decision (same generic PTY path; follow-up issue if desired). Documented, not deferred silently.
4. Residual documentado (Blocker B): los **step labels** de agy (`  Analyzing Shell Arguments`, `  Clarifying Authorization Boundaries`, `  Observing System Activity`, …) NO se filtran — son transitorios durante la ejecución, dependen de la tarea y no tienen marcador estructural generalizable; enumerar los observados sería una denylist que no generaliza (YAGNI). Fijado por el test 9 como deliberado. Las **filas de actividad de tool-calls** pasan intencionalmente (información, no chrome). Ambos fuera de la garantía de acceptance.
5. Residual documentado (Blocker C): la **fila de path del logo vertical** (`  D:/0_repos/…/__agent_ac-cli-tester-v3`) NO se filtra — el path es indistinguible de contenido real (misma cadena en filas de tool-calls y mensajes); aparece solo en re-displays del alt-screen (inicio de respuesta). Fijado por el test 2 como KEPT deliberado.
6. Residual: formato de status agy más allá de lo observado (` · low/medium/high` con/sin contador, paréntesis `(High)/(Medium)/(Low)`, logo 1.1.19 horizontal y vertical) no validado exhaustivamente; listas data-driven + unit tests como enforcement. La auditoría de patrones 1.1.19 de dev-rust es bienvenida pero NO bloqueante.
7. Tradeoffs aceptados: (a) contains-based tira contenido real con ` · high|medium|low` / ` (High)|(Medium)|(Low)` en cualquier posición — incl. substring ` · higher`/` · highly` (Blockers A y C); (b) `> `-prefixed (cita) cae — extensión deliberada sobre la regla de Claude (`❯`/`>`/`❯ `-prefixed only); (c) líneas de glifo único no alfanumérico caen; (d) `Google AI Pro` (nombre de plan de cuenta) como patrón — colisión con contenido real improbable. Todos fijados por tests (10, 7/9, 5, 1/2).
8. `ClaudeCodeFilter` queda intocado en comportamiento (solo code motion de los predicates compartidos): no recibe las reglas nuevas (contains, `▸ `, glifo único, paréntesis) — cambio acotado a agy.

## 11. Dependency-cycle statement

New module arcs (enumerated, file-level):
- `telegram::bridge → session::profile` (new `use` in bridge.rs for `CodingAgentKind`).
- `telegram::manager → session::profile` (new `use` in manager.rs for `CodingAgentKind`).
- Removed arcs: none. `commands::telegram → session::profile` already exists (derive_reader).

Per-arc verdict (measured on base `2a1bb0e3`, clean tree, con el detector EXTERNO en `<VAULT>/01-rust_module-dependency-cycles.mjs`, `<VAULT>` = `repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust` — la ruta canónica está en el header de `02-module-arc-record.mjs`; el detector NO vive en el repo, no existe `…/IA-Programming/rust/rust/`): the repository has exactly ONE cyclic SCC — a pre-existing 85-module SCC containing `telegram::bridge`, `telegram::manager`, `commands::telegram`. `session::profile` is NOT in it; it is an acyclic leaf with ZERO outgoing edges (verified by reachability from the graph). Both new arcs go from members of the pre-existing SCC INTO that acyclic leaf; a return path is impossible (leaf has no outgoing edges) — zero cross-boundary cycle risk. Baseline measured: modulesResolved 190, moduleEdges 3694, moduleCycles 1.

Role/layering: the new references are downward (telegram transport layer → session domain enum); no UI-transport/`AppHandle`/tauri type flows into a lower layer; `session::profile` gains nothing. No role inversion.

Reviewer rerun (Step N acceptance): on the final branch head with a clean tree, regenerate `pre.json`/`post.json` and the arc record; green iff `cyclicSccs` equal (1), every cyclic SCC member set identical set-to-set, zero new `from → to` pairs other than the two arcs above (both verified into the acyclic leaf), regenerated `src-tauri/module-arcs.txt` committed and byte-identical on re-run, layering guards green. Exit code 1 of the detector is the normal gating outcome (graph still written); exit 3 would mean no graph.

## 12. Implementation order

1. bridge.rs: extract shared predicates; add `AgyFilter` + `AGY_CHROME_PATTERNS` (incl. `Google AI Pro`) + `AGY_STATUS_PATTERNS` (contains-based, middot y paréntesis) + reglas `▸ `/glifo único; add `filter_for_agent`; thread `agent_kind` into `output_task` (replace hardcode at 574) and `spawn_bridge`.
2. manager.rs: `TelegramBridgeManager::attach` parameter; forward to `spawn_bridge`.
3. commands/telegram.rs: pass `agent_kind` at the single `tg.attach` call site.
4. Unit tests (§9.1-9.13); `cargo build` + `cargo test`.
5. Regenerate and commit `src-tauri/module-arcs.txt` (exactly +2 arcs); run the cycle-gate rerun (§11) and confirm the criteria.
6. Live verification (§9) with a cli-tester/agy session; update the #1545 matrix row verdict if owned by the WG.
