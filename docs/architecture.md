# Agents Commander — Architecture Map

> Auto-generated from codebase analysis. Version 0.4.6, branch `feature/agents-communication`.

---

## 1. High-Level Architecture

```mermaid
graph TB
    subgraph "Frontend (SolidJS + TypeScript)"
        MAIN["main.tsx<br/>Window Router"]
        MAIN -->|"?window=sidebar"| SB["Sidebar Window"]
        MAIN -->|"?window=terminal"| TM["Terminal Window"]
        MAIN -->|"?detached=true"| DT["Detached Terminal"]
    end

    subgraph "Shared Layer"
        IPC["ipc.ts<br/>All API wrappers"]
        TYPES["types.ts<br/>All interfaces"]
        VOICE["voice-recorder.ts<br/>Mic + Gemini"]
        SETTINGS_STORE["stores/settings.ts<br/>Global settings"]
        SHORTCUTS["shortcuts.ts<br/>Keyboard bindings"]
    end

    subgraph "Rust Backend (Tauri 2.x + tokio)"
        LIB["lib.rs<br/>App bootstrap"]
        CMD["commands/<br/>IPC handlers"]
        SESS["session/<br/>SessionManager"]
        PTY["pty/<br/>PtyManager + IdleDetector"]
        TG["telegram/<br/>Bridge + API"]
        PH["phone/<br/>Inter-agent messaging"]
        CFG["config/<br/>Settings + DarkFactory"]
        GW["pty/git_watcher.rs<br/>Branch polling"]
    end

    subgraph "External"
        SHELL["Shell / Agent Process<br/>(PowerShell, Claude, Codex)"]
        TGAPI["Telegram Bot API"]
        GEMINI["Google Gemini API"]
        FS["Filesystem<br/>~/.agentscommander/"]
    end

    SB <-->|"invoke() / events"| CMD
    TM <-->|"invoke() / events"| CMD
    DT <-->|"invoke() / events"| CMD

    CMD --> SESS
    CMD --> PTY
    CMD --> TG
    CMD --> PH
    CMD --> CFG

    PTY <-->|"ConPTY"| SHELL
    TG <-->|"HTTP"| TGAPI
    VOICE -->|"HTTP"| GEMINI
    CFG <-->|"JSON/TOML"| FS
    PH <-->|"JSON files"| FS

    style SB fill:#16213e,stroke:#0f3460,color:#fff
    style TM fill:#16213e,stroke:#0f3460,color:#fff
    style DT fill:#16213e,stroke:#53a8b6,color:#fff
    style LIB fill:#1a1a2e,stroke:#e94560,color:#fff
    style CMD fill:#0f3460,stroke:#53a8b6,color:#fff
    style SHELL fill:#333,stroke:#888,color:#ccc
    style TGAPI fill:#333,stroke:#0088cc,color:#ccc
    style GEMINI fill:#333,stroke:#d97706,color:#ccc
    style FS fill:#333,stroke:#888,color:#ccc
```

---

## 2. Rust Backend Modules

```mermaid
graph LR
    subgraph "lib.rs — App Bootstrap"
        BOOTSTRAP["State init<br/>Window creation<br/>Command registration<br/>Session restore"]
    end

    subgraph "commands/"
        C_SESSION["session.rs<br/>create, destroy, switch<br/>rename, list, set_last_prompt"]
        C_PTY["pty.rs<br/>pty_write, pty_resize"]
        C_CONFIG["config.rs<br/>get/update_settings<br/>save_debug_logs"]
        C_TELEGRAM["telegram.rs<br/>attach, detach<br/>list_bridges, send_test"]
        C_WINDOW["window.rs<br/>detach_terminal<br/>close_detached"]
        C_REPOS["repos.rs<br/>search_repos"]
        C_VOICE["voice.rs<br/>voice_transcribe"]
        C_DF["dark_factory.rs<br/>get/save_dark_factory"]
        C_PHONE["phone.rs<br/>send, inbox, list, ack"]
    end

    subgraph "session/"
        S_MGR["manager.rs<br/>SessionManager<br/>HashMap + order Vec"]
        S_SESS["session.rs<br/>Session struct<br/>SessionInfo (IPC)"]
    end

    subgraph "pty/"
        P_MGR["manager.rs<br/>PtyManager<br/>spawn, write, resize, kill<br/>PTY read loop (std::thread)"]
        P_IDLE["idle_detector.rs<br/>700ms silence → idle<br/>200ms poll (std::thread)"]
        P_GIT["git_watcher.rs<br/>5s branch poll<br/>(std::thread + inner tokio)"]
    end

    subgraph "telegram/"
        T_MGR["manager.rs<br/>TelegramBridgeManager<br/>OutputSenderMap"]
        T_BRIDGE["bridge.rs<br/>output_task (vt100 pipeline)<br/>poll_task (getUpdates)"]
        T_API["api.rs<br/>send_message, get_updates"]
        T_TYPES["types.rs<br/>BotConfig, BridgeInfo"]
    end

    subgraph "phone/"
        PH_MGR["manager.rs<br/>can_communicate()<br/>send, inbox, ack"]
        PH_TYPES["types.rs<br/>PhoneMessage<br/>Conversation, AgentInfo"]
    end

    subgraph "config/"
        CFG_SET["settings.rs<br/>AppSettings<br/>load/save JSON"]
        CFG_DF["dark_factory.rs<br/>DarkFactoryConfig<br/>Teams + sync_agent_configs"]
        CFG_PERSIST["sessions_persistence.rs<br/>Session snapshot/restore"]
        CFG_MOD["mod.rs<br/>config_dir()<br/>-dev suffix in debug"]
    end

    BOOTSTRAP --> C_SESSION
    BOOTSTRAP --> C_PTY
    BOOTSTRAP --> C_CONFIG
    BOOTSTRAP --> C_TELEGRAM
    BOOTSTRAP --> C_WINDOW
    BOOTSTRAP --> C_REPOS
    BOOTSTRAP --> C_VOICE
    BOOTSTRAP --> C_DF
    BOOTSTRAP --> C_PHONE

    C_SESSION --> S_MGR
    C_SESSION --> P_MGR
    C_SESSION --> T_MGR
    C_SESSION --> CFG_PERSIST
    C_PTY --> P_MGR
    C_CONFIG --> CFG_SET
    C_TELEGRAM --> T_MGR
    C_WINDOW --> S_MGR
    C_DF --> CFG_DF
    C_PHONE --> PH_MGR
    C_PHONE --> CFG_DF
    C_REPOS --> CFG_SET

    T_MGR --> T_BRIDGE
    T_BRIDGE --> T_API
    P_MGR --> P_IDLE
    P_MGR --> T_MGR

    style BOOTSTRAP fill:#e94560,stroke:#fff,color:#fff
    style C_SESSION fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_PTY fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_CONFIG fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_TELEGRAM fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_WINDOW fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_REPOS fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_VOICE fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_DF fill:#0f3460,stroke:#53a8b6,color:#fff
    style C_PHONE fill:#0f3460,stroke:#53a8b6,color:#fff
```

---

## 3. Frontend Components

### 3.1 Sidebar Window

```mermaid
graph TD
    SA["sidebar/App.tsx<br/>Root: shortcuts, events,<br/>settings, bridge subs"]

    SA --> TB["Titlebar.tsx<br/>Drag region, icon, version<br/>DEV badge, minimize, close"]
    SA --> SL["SessionList.tsx<br/>For each session → SessionItem"]
    SA --> TL["Toolbar.tsx<br/>Open Agent + New Session + Settings"]

    SL --> SI["SessionItem.tsx<br/>Status dot, name (inline rename)<br/>git branch, shell path<br/>mic button, detach, telegram, close"]

    TL --> SM["SettingsModal.tsx<br/>4 tabs: General, Agents,<br/>Integrations, Dark Factory"]
    TL --> OA["OpenAgentModal.tsx<br/>Repo search → Agent picker<br/>git pull option → launch"]

    subgraph "Stores"
        SS["sessions.ts<br/>createStore: sessions[], activeId"]
        BS["bridges.ts<br/>createStore: bridges[]"]
    end

    SA --> SS
    SA --> BS
    SI --> SS
    SI --> BS

    style SA fill:#16213e,stroke:#0f3460,color:#fff
    style SM fill:#533483,stroke:#fff,color:#fff
    style OA fill:#533483,stroke:#fff,color:#fff
    style SI fill:#0f3460,stroke:#53a8b6,color:#fff
```

### 3.2 Terminal Window

```mermaid
graph TD
    TA["terminal/App.tsx<br/>Root: session switching,<br/>detached mode support"]

    TA --> TTB["Titlebar.tsx<br/>Session name, shell<br/>DETACHED badge, controls"]
    TA --> LP["LastPrompt.tsx<br/>Last command per session<br/>listens to last_prompt events"]
    TA --> TV["TerminalView.tsx<br/>xterm.js multi-session container<br/>WebGL addon, FitAddon<br/>Map of SessionTerminal instances"]
    TA --> SB["StatusBar.tsx<br/>Full launch command (ellipsis + tooltip)<br/>Mic button (hold-to-record)<br/>Clear input button"]

    subgraph "Store"
        TS["terminal.ts<br/>activeSessionId, name<br/>shell, shellArgs, workingDirectory"]
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
        TYPES["types.ts<br/>Session, AppSettings,<br/>AgentConfig, Team,<br/>PhoneMessage, BridgeInfo..."]

        IPC["ipc.ts<br/>SessionAPI, PtyAPI,<br/>SettingsAPI, ReposAPI,<br/>TelegramAPI, VoiceAPI,<br/>DarkFactoryAPI, PhoneAPI,<br/>DebugAPI, WindowAPI<br/>+ event listeners"]

        VOICE["voice-recorder.ts<br/>MediaRecorder → Gemini<br/>→ PTY write"]

        CONSOLE["console-capture.ts<br/>Monkey-patch console<br/>500 entries buffer"]

        SSTORE["stores/settings.ts<br/>Global settings signal<br/>voiceEnabled computed"]

        SHORTS["shortcuts.ts<br/>Ctrl+Shift+N/W/R"]

        CONST["constants.ts<br/>WINDOW_TYPE,<br/>IS_SIDEBAR, IS_TERMINAL"]
    end

    IPC --> TYPES
    VOICE --> IPC
    SSTORE --> IPC

    style TYPES fill:#533483,stroke:#fff,color:#fff
    style IPC fill:#533483,stroke:#fff,color:#fff
    style VOICE fill:#0f3460,stroke:#53a8b6,color:#fff
```

---

## 4. IPC Contract — All Commands

```mermaid
graph LR
    subgraph "Frontend APIs"
        direction TB
        A1["SessionAPI<br/>create, destroy, switch<br/>rename, list, getActive<br/>setLastPrompt"]
        A2["PtyAPI<br/>write, resize"]
        A3["SettingsAPI<br/>get, update"]
        A4["ReposAPI<br/>search"]
        A5["TelegramAPI<br/>attach, detach<br/>listBridges, getBridge<br/>sendTest"]
        A6["WindowAPI<br/>detach, closeDetached"]
        A7["VoiceAPI<br/>transcribe"]
        A8["DebugAPI<br/>saveLogs"]
        A9["DarkFactoryAPI<br/>get, save"]
        A10["PhoneAPI<br/>sendMessage, getInbox<br/>listAgents, ackMessages"]
    end

    subgraph "Rust Commands"
        direction TB
        R1["commands/session.rs"]
        R2["commands/pty.rs"]
        R3["commands/config.rs"]
        R4["commands/repos.rs"]
        R5["commands/telegram.rs"]
        R6["commands/window.rs"]
        R7["commands/voice.rs"]
        R8["commands/dark_factory.rs"]
        R9["commands/phone.rs"]
    end

    A1 -->|invoke| R1
    A2 -->|invoke| R2
    A3 -->|invoke| R3
    A8 -->|invoke| R3
    A4 -->|invoke| R4
    A5 -->|invoke| R5
    A6 -->|invoke| R6
    A7 -->|invoke| R7
    A9 -->|invoke| R8
    A10 -->|invoke| R9
```

---

## 5. Events — Backend to Frontend

```mermaid
graph LR
    subgraph "Rust emits"
        E1["pty_output<br/>{sessionId, data}"]
        E2["session_created<br/>{SessionInfo}"]
        E3["session_destroyed<br/>{id}"]
        E4["session_switched<br/>{id}"]
        E5["session_renamed<br/>{id, name}"]
        E6["session_idle<br/>{id}"]
        E7["session_busy<br/>{id}"]
        E8["session_git_branch<br/>{sessionId, branch}"]
        E9["last_prompt<br/>{sessionId, text}"]
        E10["telegram_bridge_attached<br/>{BridgeInfo}"]
        E11["telegram_bridge_detached<br/>{sessionId}"]
        E12["telegram_bridge_error<br/>{sessionId, error}"]
        E13["telegram_incoming<br/>{sessionId, text, from}"]
    end

    subgraph "Frontend listeners"
        L1["TerminalView<br/>→ xterm.js write"]
        L2["sidebar/App<br/>→ sessionsStore"]
        L3["terminal/App<br/>→ session switching"]
        L4["LastPrompt<br/>→ command display"]
        L5["SessionItem<br/>→ bridge indicator"]
    end

    E1 --> L1
    E2 --> L2
    E3 --> L2
    E3 --> L3
    E4 --> L2
    E4 --> L3
    E5 --> L2
    E5 --> L3
    E6 --> L2
    E7 --> L2
    E8 --> L2
    E9 --> L4
    E10 --> L5
    E11 --> L5
    E12 --> L5
```

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
    PM->>PM: Open PTY (ConPTY)
    PM->>PM: Start read loop (std::thread)
    CMD-->>SB: emit("session_created")
    CMD-->>TM: emit("session_created")
    SB->>SB: sessionsStore.addSession()
    TM->>TM: TerminalView creates xterm instance
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
    participant CF as ClaudeCodeFilter
    participant TG as Telegram API

    PTY->>CH: try_send(data)
    CH->>VT: process(bytes)
    VT->>RT: update_from_screen()
    Note over RT: Per-row stability tracking
    RT->>RT: Row stable 800ms+?
    RT->>CF: harvest_stable(filter)
    CF->>CF: Reject spinners, chrome,<br/>box-drawing, low-alpha
    CF-->>TG: send_message(clean_text)
    Note over TG: Chunk at 4000 chars<br/>35ms rate limit
```

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
    subgraph "Tauri .manage()"
        SM["SessionManager<br/>Arc&lt;tokio::RwLock&gt;"]
        PM["PtyManager<br/>Arc&lt;std::Mutex&gt;"]
        TBM["TelegramBridgeManager<br/>Arc&lt;tokio::Mutex&gt;"]
        SETT["AppSettings<br/>Arc&lt;tokio::RwLock&gt;"]
        DET["DetachedSessions<br/>Arc&lt;std::Mutex&lt;HashSet&gt;&gt;"]
    end

    subgraph "Shared (not managed)"
        OSM["OutputSenderMap<br/>Arc&lt;std::Mutex&lt;HashMap&gt;&gt;<br/>PTY read → Telegram bridge"]
        AHL["AppHandle via OnceLock<br/>For native thread callbacks"]
        IDLE["IdleDetector<br/>Arc, inner std::Mutex"]
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
        SS["sessionsStore<br/>createStore<br/>sessions[], activeId"]
        BS["bridgesStore<br/>createStore<br/>bridges[]"]
    end

    subgraph "Terminal Store"
        TS["terminalStore<br/>createSignal × 5<br/>sessionId, name, shell, shellArgs, workingDirectory"]
    end

    subgraph "Global"
        GS["settingsStore<br/>createSignal<br/>AppSettings + voiceEnabled"]
        VR["voiceRecorder<br/>Module-level signals<br/>recordingId, processing, error, level"]
    end

    style SS fill:#16213e,stroke:#0f3460,color:#fff
    style BS fill:#16213e,stroke:#0f3460,color:#fff
    style TS fill:#16213e,stroke:#0f3460,color:#fff
    style GS fill:#533483,stroke:#fff,color:#fff
    style VR fill:#533483,stroke:#fff,color:#fff
```

---

## 8. Persistence — Files on Disk

```mermaid
graph TD
    subgraph "~/.agentscommander/ (prod)<br/>~/.agentscommander-dev/ (debug)"
        SETTINGS["settings.json<br/>Shell, agents, bots,<br/>voice config, window prefs"]
        SESSIONS["sessions.json<br/>Persisted sessions<br/>for restore on startup"]
        TEAMS["teams.json<br/>Dark Factory teams<br/>+ coordinators"]
        CONVDIR["conversations/<br/>NNNN-from_to.json<br/>Phone messages"]
        DEBUG["debug-logs.txt<br/>Console capture export"]
    end

    subgraph "Per-agent repo"
        AGENTCFG["&lt;repo&gt;/.agentscommander/<br/>config.json<br/>{teams, isCoordinatorOf}<br/>+ telegram_bot auto-attach"]
    end

    TEAMS -->|"sync_agent_configs()"| AGENTCFG

    style SETTINGS fill:#0f3460,stroke:#53a8b6,color:#fff
    style TEAMS fill:#e94560,stroke:#fff,color:#fff
    style CONVDIR fill:#e94560,stroke:#fff,color:#fff
    style AGENTCFG fill:#533483,stroke:#fff,color:#fff
```

---

## 9. Threading Model

```mermaid
graph TD
    subgraph "std::thread (native)"
        T1["PTY Read Loop<br/>(1 per session)<br/>Blocking read → emit"]
        T2["IdleDetector Watcher<br/>(1 global)<br/>200ms poll loop"]
        T3["GitWatcher<br/>(1 global)<br/>5s poll, own tokio runtime"]
    end

    subgraph "tokio async tasks"
        T4["Telegram Output Task<br/>(1 per bridge)<br/>vt100 pipeline → send"]
        T5["Telegram Poll Task<br/>(1 per bridge)<br/>Long-poll getUpdates"]
        T6["All Tauri Commands<br/>async fn handlers"]
    end

    subgraph "Synchronization"
        M1["std::Mutex<br/>PtyManager, OutputSenderMap,<br/>IdleDetector, DetachedSessions"]
        M2["tokio::RwLock<br/>SessionManager, AppSettings"]
        M3["tokio::Mutex<br/>TelegramBridgeManager"]
    end

    T1 -->|"lock()"| M1
    T2 -->|"lock()"| M1
    T4 -->|"lock()"| M1
    T6 -->|".await"| M2
    T6 -->|".await"| M3

    style T1 fill:#e94560,stroke:#fff,color:#fff
    style T2 fill:#e94560,stroke:#fff,color:#fff
    style T3 fill:#e94560,stroke:#fff,color:#fff
    style T4 fill:#0f3460,stroke:#53a8b6,color:#fff
    style T5 fill:#0f3460,stroke:#53a8b6,color:#fff
    style T6 fill:#0f3460,stroke:#53a8b6,color:#fff
```

---

## 10. File Index

### Rust Backend (`src-tauri/src/`)

| File | Purpose |
|------|---------|
| `main.rs` | Thin shim → `lib::run()` |
| `lib.rs` | App bootstrap, state init, window creation, session restore, command registration |
| `errors.rs` | `AppError` enum (thiserror) |
| `session/session.rs` | `Session`, `SessionInfo`, `SessionStatus` structs |
| `session/manager.rs` | `SessionManager` — CRUD, ordering, active tracking |
| `pty/manager.rs` | `PtyManager` — spawn, read loop, write, resize, kill |
| `pty/idle_detector.rs` | 700ms silence detection, idle/busy events |
| `pty/git_watcher.rs` | 5s branch polling via `git rev-parse` |
| `telegram/types.rs` | `TelegramBotConfig`, `BridgeInfo`, `BridgeStatus` |
| `telegram/api.rs` | `send_message()`, `get_updates()` |
| `telegram/manager.rs` | `TelegramBridgeManager`, `OutputSenderMap` |
| `telegram/bridge.rs` | vt100 pipeline, `RowTracker`, `ClaudeCodeFilter`, output/poll tasks |
| `phone/types.rs` | `PhoneMessage`, `Conversation`, `AgentInfo` |
| `phone/manager.rs` | `can_communicate()`, `send_message()`, `get_inbox()`, `ack_messages()` |
| `config/mod.rs` | `config_dir()` — `-dev` suffix in debug |
| `config/settings.rs` | `AppSettings`, `AgentConfig`, load/save JSON |
| `config/dark_factory.rs` | `DarkFactoryConfig`, `Team`, `TeamMember`, `sync_agent_configs()` |
| `config/sessions_persistence.rs` | `PersistedSession`, snapshot/restore |
| `commands/session.rs` | create, destroy, switch, rename, list, set_last_prompt |
| `commands/pty.rs` | pty_write, pty_resize |
| `commands/config.rs` | get/update_settings, save_debug_logs |
| `commands/telegram.rs` | attach, detach, list_bridges, get_bridge, send_test |
| `commands/window.rs` | detach_terminal, close_detached_terminal |
| `commands/repos.rs` | search_repos (agent detection) |
| `commands/voice.rs` | voice_transcribe (Gemini API) |
| `commands/dark_factory.rs` | get/save_dark_factory |
| `commands/phone.rs` | send, inbox, list, ack |

### Frontend (`src/`)

| File | Purpose |
|------|---------|
| `main.tsx` | Entry, window routing by query param |
| `shared/types.ts` | All TypeScript interfaces |
| `shared/ipc.ts` | All API wrappers + event listeners |
| `shared/shortcuts.ts` | Global keyboard shortcuts (Ctrl+Shift+N/W/R) |
| `shared/constants.ts` | `WINDOW_TYPE`, `IS_SIDEBAR`, `IS_TERMINAL` |
| `shared/voice-recorder.ts` | Mic recording → Gemini → PTY inject |
| `shared/console-capture.ts` | Console monkey-patch, 500 entries buffer |
| `shared/stores/settings.ts` | Global `AppSettings` signal + `voiceEnabled` |
| `sidebar/App.tsx` | Sidebar root — events, shortcuts, bridge subs |
| `sidebar/stores/sessions.ts` | `sessions[]` + `activeId` reactive store |
| `sidebar/stores/bridges.ts` | `bridges[]` reactive store |
| `sidebar/components/Titlebar.tsx` | Drag region, icon, version, controls |
| `sidebar/components/SessionList.tsx` | `<For>` over sessions → `SessionItem` |
| `sidebar/components/SessionItem.tsx` | Status dot, name, git branch, mic, telegram, detach, close |
| `sidebar/components/Toolbar.tsx` | Open Agent + New Session + Settings gear |
| `sidebar/components/SettingsModal.tsx` | 4-tab settings: General, Agents, Integrations, Dark Factory |
| `sidebar/components/OpenAgentModal.tsx` | Repo search → agent picker → launch |
| `terminal/App.tsx` | Terminal root — session switching, detached mode |
| `terminal/stores/terminal.ts` | `activeSessionId`, `name`, `shell`, `shellArgs`, `workingDirectory` signals |
| `terminal/components/TerminalView.tsx` | xterm.js multi-session container, WebGL, FitAddon |
| `terminal/components/Titlebar.tsx` | Session name, shell, DETACHED badge |
| `terminal/components/StatusBar.tsx` | Full launch command (ellipsis + tooltip), mic button, clear input |
| `terminal/components/LastPrompt.tsx` | Last command display per session |

### Config Files

| File | Purpose |
|------|---------|
| `src-tauri/tauri.conf.json` | Tauri config, app version, window defs, capabilities |
| `src-tauri/Cargo.toml` | Rust dependencies (v0.4.6) |
| `package.json` | Frontend deps, scripts (`tauri dev`, `kill-dev`) |
| `vite.config.ts` | Vite config, `__APP_VERSION__` injection from tauri.conf.json |
