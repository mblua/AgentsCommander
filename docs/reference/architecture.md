# Agents Commander: Architecture Map

> Generated from codebase analysis, kept current against the `main` branch (0.20.x). For developers exploring the codebase or contributing changes. Maps the backend, frontend, IPC, PTY, and container layers.

---

## 1. High-Level Architecture

```mermaid
graph TB
    subgraph "Frontend (SolidJS + TypeScript, one main.tsx router)"
        MAIN["main.tsx<br/>Routes on ?window= param"]
        MAIN -->|"default"| SB["Main window<br/>Sidebar + Terminal panes"]
        MAIN -->|"detached"| DT["Detached Terminal<br/>(locked to one session)"]
        MAIN -->|"?window=guide"| GU["Guide window"]
        MAIN -->|"?window=resource-monitor"| RM["Resource Monitor window"]
        MAIN -->|"?window=watchers"| WA["Watchers window"]
        MAIN -->|"?window=screenshot-overlay"| SO["Screenshot overlay"]
        MAIN -->|"?window=spec-board"| SP["Spec Board window"]
        MAIN -->|"not Tauri"| BR["Browser build"]
    end

    subgraph "Shared Layer"
        IPC["shared/ipc.ts<br/>API wrappers + event listeners"]
        TRANSPORT["shared/transport.ts<br/>Tauri vs WebSocket transport"]
        TYPES["shared/types.ts<br/>Interfaces + SessionSelection union"]
        SELDEC["shared/session-selection.ts<br/>Selection decoder"]
        VOICE["shared/voice-recorder.ts<br/>Mic + Gemini"]
        SETTINGS_STORE["stores/settings.ts<br/>Global settings"]
        SHORTCUTS["shortcuts.ts<br/>Keyboard bindings"]
    end

    subgraph "Rust Backend (Tauri 2.x + tokio)"
        LIB["lib.rs<br/>App bootstrap + run()"]
        CMD["commands/<br/>IPC handlers"]
        SESS["session/<br/>SessionManager + selection"]
        PTY["pty/<br/>PtyManager, local + container backends"]
        TG["telegram/<br/>Bridge + Claude/Codex/Gemini watchers"]
        PH["phone/<br/>Mailbox, inter-agent messaging"]
        CFG["config/<br/>Settings, teams, projects, seeds"]
        LOOPS["loops/<br/>Project Loops scheduler"]
        API["api/<br/>Control-plane API server"]
        RSC["resource_monitor/<br/>Process watchdog"]
        SCR["screenshot/<br/>Capture"]
        VCE["voice/<br/>Transcription"]
        WEB["web/<br/>Embedded web/WS server"]
    end

    subgraph "External"
        SHELL["Shell / Agent Process<br/>(PowerShell, Claude, Codex, Pi...)"]
        DOCKER["Docker / Podman<br/>container transport"]
        TGAPI["Telegram Bot API"]
        GEMINI["Google Gemini API"]
        APICLIENT["Control-plane API clients<br/>(containers, scripts)"]
        FS["Filesystem<br/>per-instance config + project .ac/"]
    end

    SB <-->|"invoke() / events"| CMD
    DT <-->|"invoke() / events"| CMD
    GU <-->|"invoke() / events"| CMD

    CMD --> SESS
    CMD --> PTY
    CMD --> TG
    CMD --> PH
    CMD --> CFG

    PTY <-->|"ConPTY / pipes"| SHELL
    PTY <-->|"Docker API"| DOCKER
    TG <-->|"HTTP"| TGAPI
    VOICE -->|"HTTP"| GEMINI
    API <-->|"HTTP"| APICLIENT
    CFG <-->|"JSON/TOML"| FS
    PH <-->|"JSON files"| FS

    style SB fill:#16213e,stroke:#0f3460,color:#fff
    style DT fill:#16213e,stroke:#53a8b6,color:#fff
    style CMD fill:#0f3460,stroke:#53a8b6,color:#fff
    style SHELL fill:#333,stroke:#888,color:#ccc
    style DOCKER fill:#333,stroke:#888,color:#ccc
    style TGAPI fill:#333,stroke:#0088cc,color:#ccc
    style GEMINI fill:#333,stroke:#d97706,color:#ccc
    style FS fill:#333,stroke:#888,color:#ccc
```

---

## 2. Rust Backend Modules

```mermaid
graph LR
    subgraph "lib.rs: App Bootstrap"
        BOOTSTRAP["State init<br/>Window creation<br/>Command registration<br/>Session restore"]
    end

    subgraph "commands/ (IPC handlers)"
        C_SESSION["session.rs<br/>create, destroy, switch, rename,<br/>list, selection, restore"]
        C_PTY["pty.rs<br/>pty_write, pty_resize"]
        C_CONFIG["config.rs<br/>get/update_settings, save_debug_logs"]
        C_TELEGRAM["telegram.rs<br/>attach, detach, list_bridges, send_test"]
        C_WINDOW["window.rs<br/>detach_terminal, close_detached"]
        C_REPOS["repos.rs<br/>search_repos"]
        C_VOICE["voice.rs<br/>voice_transcribe"]
        C_ACD["ac_discovery.rs<br/>branch/repo discovery"]
        C_TASK["task.rs<br/>TASK.md ops"]
        C_LOOPS["loops.rs<br/>Project Loop ops"]
        C_RESMON["resource_monitor.rs<br/>snapshot + thresholds"]
        C_SCR["screenshot.rs<br/>window capture"]
        C_SPEC["spec_board.rs<br/>Spec Board ops"]
        C_ENT["entity_creation.rs<br/>agents, teams, workgroups"]
        C_AGENT["agent_creator.rs<br/>New Agent flows"]
        C_TEMPL["role_templates.rs<br/>role-template picker"]
        C_PROJSET["project_settings.rs<br/>per-project settings"]
        C_NONSTOP["non_stop.rs<br/>non-stop mode"]
        C_WGDEL["wg_delete_diagnostic.rs<br/>workgroup delete diagnostics"]
        C_TEST["testability.rs<br/>test-only bridges"]
    end

    subgraph "session/"
        S_MGR["manager.rs<br/>SessionManager + selection state"]
        S_SESS["session.rs<br/>Session struct, SessionInfo"]
        S_SEL["selection.rs<br/>Selection contract, epoch, revision"]
        S_AC["auto_close.rs<br/>idle auto-close"]
        S_PROF["profile.rs<br/>profile resolution"]
    end

    subgraph "pty/"
        P_MGR["manager.rs<br/>PtyManager, spawn/write/resize/kill"]
        P_BE["backend.rs<br/>local vs container transport"]
        P_LOCAL["local_backend.rs<br/>ConPTY/pipes"]
        P_CONT["container_backend.rs<br/>Docker transport"]
        P_IDLE["idle_detector.rs<br/>2.5s silence → idle"]
        P_GIT["git_watcher.rs<br/>5s branch poll"]
        P_INJ["inject.rs<br/>logical clear + exact PTY input"]
        P_WATCH["watchers/<br/>context-scrape watchers"]
        P_SNAP["terminal_snapshot.rs<br/>viewport snapshots"]
    end

    subgraph "telegram/"
        T_MGR["manager.rs<br/>TelegramBridgeManager"]
        T_BRIDGE["bridge.rs<br/>output_task (vt100 pipeline)<br/>poll_task (getUpdates)"]
        T_API["api.rs<br/>send_message, get_updates"]
        T_WATCH["claude_watcher.rs, codex_watcher.rs,<br/>gemini_watcher.rs"]
        T_REDACT["redact.rs<br/>secret redaction"]
    end

    subgraph "phone/"
        PH_TYPES["types.rs<br/>OutboxMessage, PTY-input protocol"]
        PH_MAIL["mailbox.rs<br/>per-session authorization"]
        PH_MSG["messaging.rs<br/>message pump"]
    end

    subgraph "config/"
        CFG_SET["settings.rs<br/>AppSettings, load/save JSON"]
        CFG_TEAMS["teams.rs<br/>team discovery, FQNs, routing"]
        CFG_PROJ["projects.rs<br/>dual-path project registry"]
        CFG_SEED["seed_manifest.rs, config_seed.rs<br/>seeding"]
        CFG_PERSIST["sessions_persistence.rs<br/>snapshot/restore"]
        CFG_ROOT["root_agent.rs<br/>Root Agent layout"]
    end

    subgraph "Other daemon subsystems"
        O_LOOPS["loops/<br/>cron scheduler + delivery"]
        O_API["api/<br/>control-plane server, auth, audit"]
        O_RES["resource_monitor/<br/>watchdog"]
        O_SCR["screenshot/<br/>capture"]
        O_VCE["voice/<br/>tracker"]
        O_WEB["web/<br/>embedded server + WS broadcast"]
    end

    BOOTSTRAP --> C_SESSION
    BOOTSTRAP --> C_PTY
    BOOTSTRAP --> C_CONFIG
    BOOTSTRAP --> C_TELEGRAM
    BOOTSTRAP --> C_WINDOW

    C_SESSION --> S_MGR
    C_SESSION --> P_MGR
    C_SESSION --> T_MGR
    C_SESSION --> CFG_PERSIST
    C_PTY --> P_MGR
    C_CONFIG --> CFG_SET
    C_TELEGRAM --> T_MGR
    C_WINDOW --> S_MGR
    C_REPOS --> CFG_SET

    T_MGR --> T_BRIDGE
    T_BRIDGE --> T_API
    P_MGR --> P_IDLE
    P_MGR --> P_BE
    P_BE --> P_LOCAL
    P_BE --> P_CONT
    P_MGR --> T_MGR

    style BOOTSTRAP fill:#e94560,stroke:#fff,color:#fff
    style C_SESSION fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_PTY fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_CONFIG fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_TELEGRAM fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_WINDOW fill:#0f3460,stroke:#53a8b6,color:#fff
    style O_LOOPS fill:#533483,stroke:#fff,color:#fff
    style O_API fill:#533483,stroke:#fff,color:#fff
```

---

## 3. Frontend Components

### 3.1 Main Window (sidebar + terminal)

```mermaid
graph TD
    MA["App.tsx<br/>Root: events, shortcuts,<br/>settings, bridge subs"]

    MA --> PP["ProjectPanel.tsx<br/>Projects, workgroups, replicas → SessionItem"]
    MA --> WR["WorkgroupGroupRail.tsx<br/>Favorites, group rail, raise-hand"]
    MA --> AB["ActionBar.tsx<br/>Project creation + Settings gear"]

    PP --> SI["SessionItem.tsx<br/>Status dot, name (inline rename)<br/>git branch, shell path, mic,<br/>detach, telegram, close"]

    AB --> SM["SettingsModal.tsx<br/>tabs: General, Agents, Integrations,<br/>Watchers, API clients, ..."]
    SI --> OA["OpenAgentModal.tsx<br/>Repo search → Agent picker → launch"]

    subgraph "Sidebar Stores"
        SS["stores/sessions.ts<br/>session rows + selection state"]
        BS["stores/bridges.ts<br/>bridges[]"]
        PS["stores/project.ts<br/>project/workgroup trees"]
    end

    MA --> SS
    MA --> BS
    MA --> PS
    SI --> SS
    SI --> BS

    style MA fill:#16213e,stroke:#0f3460,color:#fff
    style SM fill:#533483,stroke:#fff,color:#fff
    style SI fill:#0f3460,stroke:#53a8b6,color:#fff
```

### 3.2 Terminal Pane

```mermaid
graph TD
    TA["App.tsx<br/>Selection reconciliation,<br/>detached mode support"]

    TA --> TTB["Titlebar.tsx<br/>Session name, shell,<br/>DETACHED badge, zoom"]
    TA --> TV["TerminalView.tsx<br/>xterm.js, WebGL addon, FitAddon<br/>multi-session container"]
    TA --> SB["StatusBar.tsx<br/>Launch command, mic button,<br/>clear input, watchers"]
    TA --> LP["LastPrompt.tsx<br/>Last command per session"]
    TA --> WT["WorkgroupTask.tsx<br/>TASK.md title/status"]

    subgraph "Store"
        TS["stores/terminal.ts<br/>selection order, connection<br/>generation, live activeSessionId"]
    end

    TA --> TS
    TV --> TS

    style TA fill:#16213e,stroke:#0f3460,color:#fff
    style TV fill:#e94560,stroke:#fff,color:#fff
    style SB fill:#0f3460,stroke:#53a8b6,color:#fff
```

### 3.3 Shared Layer

```mermaid
graph LR
    subgraph "shared/"
        TYPES["types.ts<br/>Session, AppSettings, AgentConfig,<br/>Team, BridgeInfo, SessionSelection..."]

        IPC["ipc.ts<br/>SessionAPI, PtyAPI, SettingsAPI,<br/>TelegramAPI, VoiceAPI, WindowAPI,<br/>+ event listeners"]

        TRANSPORT["transport.ts<br/>transport-tauri.ts / transport-ws.ts<br/>invoke + event multiplexing"]

        SELDEC["session-selection.ts<br/>Runtime decoder for untrusted<br/>selection hydration and events"]

        VOICE["voice-recorder.ts<br/>MediaRecorder → Gemini<br/>→ PTY write"]

        CONSOLE["console-capture.ts<br/>Monkey-patch console<br/>500 entries buffer"]

        SSTORE["stores/settings.ts<br/>Global settings signal"]

        SHORTS["shortcuts.ts<br/>Ctrl+Shift+W/R"]
    end

    IPC --> TYPES
    IPC --> TRANSPORT
    SELDEC --> TYPES
    VOICE --> IPC
    SSTORE --> IPC

    style TYPES fill:#533483,stroke:#fff,color:#fff
    style IPC fill:#533483,stroke:#fff,color:#fff
    style TRANSPORT fill:#533483,stroke:#fff,color:#fff
    style VOICE fill:#0f3460,stroke:#53a8b6,color:#fff
```

---

## 4. IPC Contract: All Commands

Rust handlers live in `src-tauri/src/commands/`; the frontend invokes them through `shared/ipc.ts`, which routes over a Tauri or WebSocket transport.

| Frontend API area | Rust handler | Examples |
|---|---|---|
| SessionAPI | `commands/session.rs` | `create_session`, `destroy_session`, `switch_session`, `rename_session`, `list_sessions`, `get_active_session` (selection hydration) |
| PtyAPI | `commands/pty.rs` | `pty_write`, `pty_resize` |
| SettingsAPI | `commands/config.rs` | `get_settings`, `update_settings`, `save_debug_logs` |
| ReposAPI | `commands/repos.rs` | `search_repos` |
| TelegramAPI | `commands/telegram.rs` | `attach_telegram`, `detach_telegram`, `list_bridges`, `send_test` |
| WindowAPI | `commands/window.rs` | `detach_terminal`, `close_detached_terminal` |
| VoiceAPI | `commands/voice.rs` | `voice_transcribe` |
| DiscoveryAPI | `commands/ac_discovery.rs` | branch/repo discovery events |
| TaskAPI | `commands/task.rs` | `TASK.md` read/update |
| LoopAPI | `commands/loops.rs` | Project Loop CRUD + state |
| ResourceMonitorAPI | `commands/resource_monitor.rs` | process snapshot, thresholds |
| ScreenshotAPI | `commands/screenshot.rs` | window capture |
| SpecBoardAPI | `commands/spec_board.rs` | spec board CRUD |
| EntityAPI | `commands/entity_creation.rs`, `commands/agent_creator.rs` | agents, teams, workgroups |
| RoleTemplatesAPI | `commands/role_templates.rs` | role-template picker |
| ProjectSettingsAPI | `commands/project_settings.rs` | per-project settings |
| NonStopAPI | `commands/non_stop.rs` | non-stop mode |
| TestabilityAPI | `commands/testability.rs` | test-only bridges and resets |

---

## 5. Events: Backend to Frontend

```mermaid
graph LR
    subgraph "Rust emits"
        E1["pty_output<br/>{sessionId, data}"]
        E2["session_created<br/>{SessionInfo}"]
        E3["session_destroyed<br/>{id}"]
        E4["session_switched<br/>{SessionSelection}<br/>authoritative"]
        E5["session_renamed<br/>{id, name}"]
        E6["session_idle / session_busy<br/>{id}"]
        E7["last_prompt<br/>{sessionId, text}"]
        E8["pty_input_status<br/>{sessionId, status}"]
        E9["telegram_bridge_attached / detached / error / warning"]
        E10["telegram_incoming<br/>{sessionId, text, from}"]
        E11["session_context<br/>{context usage}"]
        E12["coding_agent_profiles_updated<br/>profile_selection_updated"]
        E13["workgroup_task_updated"]
        E14["spec_board_changed / conflict / file_missing"]
        E15["ac_discovery_branch_updated"]
        E16["log_level_changed, npm_update_available"]
    end

    subgraph "Frontend listeners"
        L1["TerminalView<br/>→ xterm.js write"]
        L2["App<br/>→ sessionsStore"]
        L3["App<br/>→ selection reconciliation"]
        L4["LastPrompt<br/>→ command display"]
        L5["SessionItem<br/>→ bridge indicator"]
    end

    E1 --> L1
    E2 --> L2
    E3 --> L2
    E4 --> L2
    E4 --> L3
    E5 --> L2
    E6 --> L2
    E7 --> L4
    E9 --> L5
    E10 --> L5
```

### 5.1 Authoritative session selection contract

`session_switched` is the only authoritative selection event for the central pane. Native Tauri windows and WebSocket clients receive the same stored payload. The `get_active_session` command returns that same payload for hydration, despite retaining its older command name.

The complete Rust and TypeScript wire contract is:

```ts
export type SessionSelectionMode = "none" | "live" | "dormant";

export type SessionSelectionCause =
  | { source: "initialHydration"; userInitiated: false; mode: "none" }
  | { source: "sessionCreated"; userInitiated: boolean; mode: "live" }
  | { source: "userSwitch"; userInitiated: true; mode: "live" | "dormant" }
  | { source: "manualClose"; userInitiated: true; mode: "live" | "none" }
  | { source: "autoClose"; userInitiated: false; mode: "none" }
  | { source: "restart"; userInitiated: boolean; mode: "live" | "none" }
  | { source: "restore"; userInitiated: false; mode: "live" | "dormant" | "none" }
  | { source: "detach"; userInitiated: true; mode: "live" | "none" }
  | { source: "attach"; userInitiated: true; mode: "live" | "dormant" }
  | { source: "spawnRollback"; userInitiated: false; mode: "none" }
  | { source: "resourceMonitor"; userInitiated: boolean; mode: "none" }
  | { source: "backgroundCleanup"; userInitiated: false; mode: "none" }
  | { source: "livenessReconcile"; userInitiated: false; mode: "dormant" | "none" };

export type SessionSelectionSource = SessionSelectionCause["source"];

interface SessionSelectionOrder {
  epoch: string;
  revision: number;
}

type SessionSelectionBase = SessionSelectionOrder & SessionSelectionCause;

export type SessionSelection =
  | (SessionSelectionBase & {
      mode: "none";
      id: null;
      status: null;
      hasPty: false;
      detached: false;
      displayable: false;
    })
  | (SessionSelectionBase & {
      mode: "live";
      id: string;
      status: "active";
      hasPty: true;
      detached: false;
      displayable: true;
    })
  | (SessionSelectionBase & {
      mode: "dormant";
      id: string;
      status: { exited: number };
      hasPty: boolean;
      detached: false;
      displayable: false;
    });
```

The source contract is exact:

| `source` | Allowed modes | Allowed `userInitiated` |
|---|---|---|
| `initialHydration` | `none` at revision 0 only | `false` |
| `sessionCreated` | `live` | Trusted create intent |
| `userSwitch` | `live`, `dormant` | `true` |
| `manualClose` | `live`, `none` | `true` |
| `autoClose` | `none` | `false` |
| `restart` | `live`, `none` | Trusted restart intent |
| `restore` | `live`, `dormant`, `none` | `false` |
| `detach` | `live`, `none` | `true` |
| `attach` | `live`, `dormant` | `true` |
| `spawnRollback` | `none` | `false` |
| `resourceMonitor` | `none` | `true` for a user kill; `false` for a watchdog kill; app shutdown does not emit |
| `backgroundCleanup` | `none` | `false` |
| `livenessReconcile` | `dormant`, `none` | `false` |

All ten fields are required. `epoch` and every non-null `id` are canonical UUID strings, `revision` is a nonnegative safe integer, and an exited status contains a signed 32-bit code. `live` is the only displayable mode and the only mode that may carry status `"active"`. `dormant` preserves the selected record's exit code and reports its actual PTY snapshot, normally `false`, for sidebar continuity and wake guidance. It cannot route terminal input, resize, snapshots, voice, or task actions. `none` clears the central selection. Source, mode, and `userInitiated` must match the table and union above.

The manager creates one nonempty UUID `epoch` with its initial state and owns it for the backend process lifetime. It also owns the process-local `revision`. Each material selection commit increments the revision exactly once; a no-op does not increment or emit. A restarted backend creates a new epoch and a new revision domain. Commands, Tauri event delivery, and WebSocket delivery serialize this stored state but never create or advance an epoch or revision.

### 5.2 Hydration, reconnect, and stale data

The frontend calls `SessionAPI.getSelection()`, which invokes `get_active_session` as `unknown` and passes the result through `decodeSessionSelection`. `onSessionSwitched` uses the same decoder before invoking a consumer. The decoder constructs a normalized object and rejects malformed keys, values, and source-mode combinations before they can mutate a store.

Terminal and sidebar consumers register the selection listener before hydration. WebSocket consumers also track a local connection generation. They apply these ordering rules:

1. Check the captured connection generation before the epoch or revision. A disconnect immediately suspends live routing and display state.
2. Within one epoch, accept only a strictly newer revision during normal reconciliation.
3. Accept any revision from a new, nonretired epoch, and retire the previous epoch. Never accept a retired epoch again.
4. Allow an equal epoch and revision only once for the exact current-generation reconnect hydration that is still awaited. A newer event cancels that permission.
5. Reserve an accepted generation, epoch, revision, mode, and ID before awaiting session metadata. A late hydration or list result cannot overwrite a newer selection.

An exact `selectionCoordinatorBusy` hydration failure schedules one current-generation capped-backoff retry. A selection event, disconnect, replacement generation, or component disposal cancels that retry. Other malformed or failed hydration leaves the consumer in its safe neutral state.

### 5.3 `session_destroyed` does not select

`session_destroyed` reports lifecycle disposal only. Consumers use it to dispose the matching xterm cache and close a matching locked detached window. If its ID matches the writable central binding, the terminal also safety-suspends that binding immediately because the destroy event arrives before the final selection event.

The destroy handler never calls selection hydration, derives a fallback, or chooses a replacement. A later authoritative `session_switched` payload or reconnect hydration alone decides whether the central pane becomes `none`, `live`, or `dormant`.

---

## 6. Data Flows

### 6.1 Session Lifecycle

```mermaid
sequenceDiagram
    participant U as User
    participant SB as Sidebar
    participant CMD as commands/session.rs
    participant SM as SessionManager
    participant PM as PtyManager
    participant TM as Terminal

    U->>SB: Click "+ New Session"
    SB->>CMD: invoke("create_session")
    CMD->>SM: create_session() → UUID
    CMD->>PM: spawn(id, shell, cwd)
    PM->>PM: Open PTY (ConPTY or pipes)
    PM->>PM: Start read loop (std::thread)
    CMD-->>SB: emit("session_created")
    CMD-->>TM: emit("session_created")
    SB->>SB: sessionsStore.addSession()
    opt Creation changes canonical selection
        CMD-->>SB: emit("session_switched", SessionSelection)
        CMD-->>TM: emit("session_switched", SessionSelection)
        SB->>SB: Apply authoritative selection
        TM->>TM: Reconcile and bind the live selection
    end
```

### 6.2 Terminal I/O

```mermaid
sequenceDiagram
    participant XT as xterm.js
    participant IPC as PtyAPI
    participant PM as PtyManager
    participant SHELL as Shell Process

    Note over XT,SHELL: User types
    XT->>IPC: onData → PtyAPI.write(sessionId, bytes)
    IPC->>PM: pty_write command
    PM->>SHELL: writer.write_all(bytes)

    Note over XT,SHELL: Shell produces output
    SHELL->>PM: PTY read loop: reader.read()
    PM->>PM: idle_detector.record_activity()
    PM-->>XT: emit("pty_output", {sessionId, data})
    XT->>XT: terminal.write(data)
```

### 6.3 Telegram Bridge Pipeline

```mermaid
sequenceDiagram
    participant PTY as PTY Read Loop
    participant CH as mpsc channel
    participant VT as vt100 Parser
    participant RT as RowTracker
    participant CF as AgentFilter (Claude/Codex/Gemini)
    participant TG as Telegram API

    PTY->>CH: try_send(data)
    CH->>VT: process(bytes)
    VT->>RT: update_from_screen()
    Note over RT: Per-row stability tracking
    RT->>RT: Row stable 800ms+?
    RT->>CF: harvest_stable(filter)
    CF->>CF: Reject spinners, chrome,<br/>box-drawing, low-alpha
    CF-->>TG: send_message(clean_text)
    Note over TG: Chunk at 4000 chars<br/>rate-limited
```

Claude, Codex, and Gemini each have a dedicated watcher (`telegram/claude_watcher.rs`, `codex_watcher.rs`, `gemini_watcher.rs`) that filters screen rows for that agent's terminal chrome.

### 6.4 Voice-to-Text

```mermaid
sequenceDiagram
    participant U as User
    participant MIC as MediaRecorder
    participant VR as voice-recorder.ts
    participant GM as Gemini API
    participant PTY as PtyAPI

    U->>MIC: Press mic button
    MIC->>MIC: getUserMedia → start()
    Note over MIC: Audio level monitoring
    U->>MIC: Release button
    MIC->>VR: onstop → Blob → ArrayBuffer
    VR->>GM: VoiceAPI.transcribe(bytes, mime)
    GM-->>VR: Transcribed text
    VR->>PTY: PtyAPI.write(sessionId, text)
```

---

## 7. State Management

### 7.1 Rust Managed State

```mermaid
graph TD
    subgraph "Tauri .manage() / OnceLock"
        SM["SessionManager<br/>Arc + async lock"]
        PM["PtyManager<br/>Arc + mutex"]
        TBM["TelegramBridgeManager<br/>Arc + async mutex"]
        SETT["SettingsState<br/>Arc + async RwLock"]
        DET["DetachedSessions<br/>Arc + mutex set"]
        API["API dispatcher + registry<br/>control-plane"]
        LOOPS["Loops scheduler state"]
        RES["ResourceMonitor registry"]
    end

    subgraph "Shared (not managed)"
        OSM["OutputSenderMap<br/>PTY read → Telegram bridge"]
        AHL["AppHandle via OnceLock<br/>For native thread callbacks"]
        IDLE["IdleDetector<br/>Arc, inner mutex"]
        GW["GitWatcher<br/>Arc, polls every 5s"]
    end

    PM -.->|"shares"| OSM
    TBM -.->|"shares"| OSM
    PM -.->|"uses"| IDLE
    PM -.->|"uses"| GW

    style SM fill:#0f3460,stroke:#53a8b6,color:#fff
    style PM fill:#0f3460,stroke:#53a8b6,color:#fff
    style TBM fill:#0f3460,stroke:#53a8b6,color:#fff
    style SETT fill:#0f3460,stroke:#53a8b6,color:#fff
    style OSM fill:#e94560,stroke:#fff,color:#fff
```

### 7.2 Frontend State

```mermaid
graph TD
    subgraph "Sidebar Stores"
        SS["sessions.ts<br/>session rows + selection state"]
        BS["bridges.ts<br/>bridges[]"]
        PS["project.ts / project-refresh.ts<br/>project, workgroup, replica trees"]
        CS["coding-agents.ts, clock.ts,<br/>team-idle-watcher.ts, ..."]
    end

    subgraph "Terminal Store"
        TS["terminal.ts<br/>selection order, connection<br/>generation, live activeSessionId"]
    end

    subgraph "Global"
        GS["settings.ts<br/>AppSettings signal"]
        VR["voice-recorder.ts<br/>recordingId, processing, error, level"]
        TST["toasts.ts<br/>toast host"]
    end

    style SS fill:#16213e,stroke:#0f3460,color:#fff
    style BS fill:#16213e,stroke:#0f3460,color:#fff
    style TS fill:#16213e,stroke:#0f3460,color:#fff
    style GS fill:#533483,stroke:#fff,color:#fff
    style VR fill:#533483,stroke:#fff,color:#fff
```

---

## 8. Persistence: Files on Disk

The config directory is **per instance**: `<binary folder>/.<binary stem>/`, next to the executable (portable), with a legacy `$HOME` fallback when `current_exe()` is unavailable. Example: `C:\tools\agentscommander.exe` → `C:\tools\.agentscommander\`. A differently named binary gets its own isolated config dir. See [Directory layout](directory-layout.md) for the full on-disk contract.

```mermaid
graph TD
    subgraph "Per-instance config dir (next to binary)"
        SETTINGS["settings.json<br/>Shell, agents, bots,<br/>voice config, window prefs"]
        SESSIONS["sessions.json<br/>Session registry<br/>for restore on startup"]
        MASTER["master-token.txt<br/>host credential"]
        INJECTED["injected-messages.toml<br/>+ .default.toml"]
        LOG["app.log, harness.log,<br/>activity.jsonl"]
        DAEMON["daemon.pid<br/>stale-session detection"]
    end

    subgraph "Per-project .ac/ root"
        AGENTS["_agent_&lt;id&gt;/<br/>Role.md + memory/plans/skills"]
        TEAMS["_team_&lt;name&gt;/<br/>config.json"]
        WGS["wg-&lt;N&gt;-&lt;team&gt;/<br/>__agent_* replicas, repo-*,<br/>messaging/, TASK.md"]
        LOOPS["_loop_&lt;id&gt;/config.toml"]
        SEED["seed-manifest.json,<br/>.gitignore sweep"]
    end

    TEAMS -->|"roster"| WGS
    AGENTS -->|"replicated into"| WGS

    style SETTINGS fill:#0f3460,stroke:#53a8b6,color:#fff
    style TEAMS fill:#e94560,stroke:#fff,color:#fff
    style WGS fill:#533483,stroke:#fff,color:#fff
```

Inter-agent messaging is file-based: senders write Markdown into `<workgroup>/messaging/` and the daemon mailbox delivers it to the recipient's session PTY.

---

## 9. Threading Model

```mermaid
graph TD
    subgraph "std::thread (native)"
        T1["PTY Read Loop<br/>(1 per session)<br/>Blocking read → emit"]
        T2["IdleDetector Watcher<br/>(1 global)<br/>500ms poll loop"]
        T3["GitWatcher<br/>(1 global)<br/>5s poll"]
        T4["Resource watchdog<br/>process sampling"]
    end

    subgraph "tokio async tasks"
        T5["Telegram Output Task<br/>(1 per bridge)<br/>vt100 pipeline → send"]
        T6["Telegram Poll Task<br/>(1 per bridge)<br/>Long-poll getUpdates"]
        T7["Loops scheduler<br/>cron ticks + delivery"]
        T8["API server<br/>control-plane HTTP"]
        T9["All Tauri Commands<br/>async fn handlers"]
    end

    subgraph "Synchronization"
        M1["std::Mutex<br/>PtyManager, OutputSenderMap,<br/>IdleDetector, DetachedSessions"]
        M2["async RwLock<br/>SessionManager, AppSettings"]
        M3["async Mutex<br/>TelegramBridgeManager"]
    end

    T1 -->|"lock()"| M1
    T2 -->|"lock()"| M1
    T5 -->|"lock()"| M1
    T9 -->|".await"| M2
    T9 -->|".await"| M3

    style T1 fill:#e94560,stroke:#fff,color:#fff
    style T2 fill:#e94560,stroke:#fff,color:#fff
    style T3 fill:#e94560,stroke:#fff,color:#fff
    style T5 fill:#0f3460,stroke:#53a8b6,color:#fff
    style T6 fill:#0f3460,stroke:#53a8b6,color:#fff
    style T9 fill:#0f3460,stroke:#53a8b6,color:#fff
```

---

## 10. File Index

### Rust Backend (`src-tauri/src/`)

| File | Purpose |
|------|---------|
| `main.rs` | Thin shim → `lib::run()` |
| `lib.rs` | App bootstrap, state init, window creation, session restore, command registration |
| `errors.rs` | `AppError` enum (thiserror) |
| `path_identity.rs`, `path_utils.rs` | Path canonicalization, identity helpers |
| `logging.rs` | Logger setup, live log-level control |
| `shutdown.rs` | Ordered daemon shutdown |
| `update_check.rs` | npm publish version check |
| `session/session.rs` | `Session`, `SessionInfo`, `SessionStatus` structs |
| `session/manager.rs` | `SessionManager`: records, stable order, pending creates, canonical selection state |
| `session/selection.rs` | Selection contract, coordinator, eligibility policy, process epoch, revision, publication |
| `session/profile.rs` | Coding-agent profile resolution per session |
| `session/auto_close.rs` | Idle auto-close clock |
| `session/context_alerts.rs`, `session/warnings.rs` | Context alerts, session warnings |
| `session/purge_guard.rs` | `purge-wg` busy gate |
| `pty/manager.rs` | `PtyManager`: spawn, read loop, write, resize, kill |
| `pty/backend.rs` | Local vs container transport selection |
| `pty/local_backend.rs` | ConPTY/pipes backend |
| `pty/container_backend.rs` | Docker container transport |
| `pty/container_runtime.rs`, `docker_runtime.rs` | Container runtime discovery |
| `pty/container_credentials.rs`, `container_tokens.rs`, `container_paths.rs`, `container_repos.rs` | Container credential/token/path/repo plumbing |
| `pty/idle_detector.rs` | 2.5s silence detection (default), idle/busy events |
| `pty/git_watcher.rs` | 5s branch polling via `git rev-parse` |
| `pty/inject.rs` | Logical clear and exact PTY input injection |
| `pty/job.rs`, `pty/spawn_diagnostics.rs` | Spawn orchestration and diagnostics |
| `pty/output.rs` | PTY output dispatch |
| `pty/context_scrape/` | Context scraper engine |
| `pty/watchers/` | Context-scrape watchers (#1171) |
| `pty/terminal_snapshot/` | Terminal snapshot rendering |
| `telegram/types.rs` | `TelegramBotConfig`, `BridgeInfo`, `BridgeStatus` |
| `telegram/api.rs` | `send_message()`, `get_updates()` |
| `telegram/manager.rs` | `TelegramBridgeManager`, `OutputSenderMap` |
| `telegram/bridge.rs` | vt100 pipeline, `RowTracker`, agent filters, output/poll tasks |
| `telegram/redact.rs` | Secret redaction for bridge output |
| `telegram/claude_watcher.rs`, `codex_watcher.rs`, `gemini_watcher.rs` | Per-agent screen watchers |
| `telegram/jsonl_kernel.rs` | JSONL-based watcher kernel |
| `phone/types.rs` | `OutboxMessage`, PTY-input protocol types |
| `phone/mailbox.rs` | Per-session token authorization, action dispatch |
| `phone/messaging.rs` | Message pump and delivery |
| `phone/consumption.rs` | Message consumption tracking |
| `phone/terminal_snapshot.rs` | Snapshot request/response plumbing |
| `config/mod.rs` | `config_dir()`: per-instance next-to-binary resolution |
| `config/settings.rs` | `AppSettings`, `AgentConfig`, load/save JSON |
| `config/teams.rs` | Team discovery, FQNs, routing rules |
| `config/projects.rs` | Dual-path project registry |
| `config/workspace.rs` | Workspace discovery |
| `config/root_agent.rs` | Root Agent layout and identity |
| `config/sessions_persistence.rs` | `PersistedSession`, snapshot/restore |
| `config/coding_agent_profiles.rs` | Profile matrix resolution |
| `config/coding_agents_catalog.rs` | Catalog of coding agents |
| `config/config_seed.rs`, `seed_manifest.rs` | Config-folder seeding |
| `config/seed_manifest.rs` | Seed manifest + project gate |
| `config/agent_memory.rs`, `agent_creation.rs` | Agent matrix layout and creation |
| `config/project_settings.rs`, `config/loops.rs` | Project settings, Loop config |
| `config/injected_messages.rs` | Injected PTY message templates |
| `config/activity_log.rs` | Activity log |
| `config/coordinator_clocks.rs` | Coordinator idle clocks |
| `config/replica_identity.rs` | Replica identity verification |
| `config/session_context.rs` | Session context reading |
| `config/seeded_context_templates.rs` | Context template seeding |
| `config/archive_gate.rs`, `instance_gitignore.rs`, `daemon_pid.rs`, `placeholders.rs`, `profile.rs` | Supporting config |
| `commands/session.rs` | create, destroy, switch, rename, list, selection |
| `commands/pty.rs` | pty_write, pty_resize |
| `commands/config.rs` | get/update_settings, save_debug_logs |
| `commands/telegram.rs` | attach, detach, list_bridges, get_bridge, send_test |
| `commands/window.rs` | detach_terminal, close_detached_terminal |
| `commands/repos.rs` | search_repos |
| `commands/voice.rs` | voice_transcribe (Gemini API) |
| `commands/ac_discovery.rs` | branch/repo discovery |
| `commands/task.rs` | `TASK.md` operations |
| `commands/loops.rs` | Project Loop CRUD |
| `commands/resource_monitor.rs` | resource snapshots + thresholds |
| `commands/screenshot.rs` | window screenshot capture |
| `commands/spec_board.rs` | Spec Board operations |
| `commands/entity_creation.rs`, `agent_creator.rs` | agents/teams/workgroups creation |
| `commands/role_templates.rs` | role-template picker |
| `commands/project_settings.rs` | per-project settings |
| `commands/non_stop.rs` | non-stop mode |
| `commands/wg_delete_diagnostic.rs` | workgroup delete diagnostics |
| `commands/testability.rs` | test-only bridges |
| `cli/` | CLI verbs: send, list-peers, terminal-snapshot, coding-agent, api-client, window-list, window-screenshot, ... |
| `api/` | Control-plane API server: auth, audit, dispatcher, handlers, message store |
| `loops/` | Loop scheduler, delivery, events, non-stop watchdog |
| `resource_monitor/` | Process registry, watchdog, per-window reporting |
| `screenshot/` | Screenshot capture (Windows) |
| `voice/` | Voice transcription tracking |
| `web/` | Embedded web server, WebSocket broadcast, embedded auth |
| `network/` | Network helpers |
| `testability/` | Test-only reset, window info, UI automation verbs |

### Frontend (`src/`)

| File | Purpose |
|------|---------|
| `main.tsx` | Entry, window routing by `?window=` param |
| `shared/types.ts` | TypeScript interfaces and the invariant-bearing `SessionSelection` union |
| `shared/session-selection.ts` | Runtime decoder for untrusted selection hydration and events |
| `shared/ipc.ts` | API wrappers, decoded selection hydration, event listeners |
| `shared/transport.ts` | Transport abstraction (Tauri vs WebSocket) |
| `shared/transport-tauri.ts`, `shared/transport-ws.ts` | Concrete transports |
| `shared/shortcuts.ts` | Global keyboard shortcuts (Ctrl+Shift+W/R) |
| `shared/constants.ts` | Window type constants |
| `shared/voice-recorder.ts` | Mic recording → Gemini → PTY inject |
| `shared/console-capture.ts` | Console monkey-patch, 500 entries buffer |
| `shared/stores/settings.ts` | Global `AppSettings` signal |
| `shared/stores/toasts.ts` | Toast host state |
| `shared/stores/resourceMonitor.ts` | Resource monitor state |
| `shared/screenshot-hotkey.ts`, `shared/zoom.ts`, `shared/window-geometry.ts` | Window helpers |
| `sidebar/App.tsx` | Sidebar root: events, shortcuts, bridge subs |
| `sidebar/stores/sessions.ts` | Session rows plus authoritative selection state |
| `sidebar/stores/bridges.ts` | `bridges[]` reactive store |
| `sidebar/stores/project.ts` | Project/workgroup/replica tree |
| `sidebar/components/ProjectPanel.tsx` | Projects, workgroups and replicas → `SessionItem` |
| `sidebar/components/WorkgroupGroupRail.tsx` | Rail with favorites, groups, raise-hand |
| `sidebar/components/SessionItem.tsx` | Status dot, name, git branch, mic, telegram, detach, close |
| `sidebar/components/ActionBar.tsx` | Project creation + Settings gear |
| `sidebar/components/SettingsModal.tsx` | Settings tabs: General, Agents, Integrations, Watchers, API clients, ... |
| `sidebar/components/OpenAgentModal.tsx` | Repo search → agent picker → launch |
| `sidebar/components/NewTeamModal.tsx`, `EditTeamModal.tsx`, `NewWorkgroupModal.tsx`, `NewEntityAgentModal.tsx` | Entity creation |
| `sidebar/components/NewLoopModal.tsx`, `EditLoopModal.tsx` | Loop CRUD UI |
| `sidebar/components/ArchivedProjectsModal.tsx`, `AutoUnarchiveModal.tsx` | Archive handling |
| `sidebar/components/RestartPromptModal.tsx`, `OnboardingModal.tsx`, `RootAgentBanner.tsx`, `WebServerMenu.tsx` | App chrome |
| `terminal/App.tsx` | Terminal root: authoritative selection reconciliation and detached mode |
| `terminal/stores/terminal.ts` | Ordered selection state, connection generation, binding state, live-only `activeSessionId` |
| `terminal/components/TerminalView.tsx` | xterm.js multi-session container, WebGL, FitAddon |
| `terminal/components/Titlebar.tsx` | Session name, shell, DETACHED badge |
| `terminal/components/StatusBar.tsx` | Launch command, mic button, clear input, watchers |
| `terminal/components/LastPrompt.tsx` | Last command display per session |
| `terminal/components/WorkgroupTask.tsx` | TASK.md title/status |
| `terminal/components/TaskCleanConfirmModal.tsx` | Task clean confirmation |
| `guide/` | Guide window app (hints, tutorial) |
| `resource-monitor/` | Resource Monitor window app |
| `watchers/` | Watchers window app |
| `screenshot-overlay/` | Screenshot selection overlay |
| `spec-board/` | Spec Board window app |
| `browser/` | Non-Tauri browser build |

### Config Files

| File | Purpose |
|------|---------|
| `src-tauri/tauri.conf.json` | Tauri config, app version, window defs, capabilities |
| `src-tauri/Cargo.toml` | Rust dependencies |
| `package.json` | Frontend deps, scripts (`tauri dev`, `kill-dev`) |
| `vite.config.ts` | Vite config, `__APP_VERSION__` injection from tauri.conf.json |
