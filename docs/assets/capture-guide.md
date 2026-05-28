# Capture guide: README hero, demo GIF, feature screenshots

This is the design spec for the visual assets that must be captured from
the running app. The other deliverables in this push (`docs/brand.md`,
`docs/assets/og-card.png`, `docs/assets/badges.md`) are static; they
ship in this PR. The captures below need someone with the GUI app
running on a real desktop.

If you are the human running the captures: read all sections once, set up
the environment in **§1**, then work through §2–§7 in order.

## 1. Environment

### Display

- **Resolution:** 2560×1440 or higher (preferably a 2880×1800 retina
  panel). Captures are saved at 2880×1800; anything below requires
  upscaling and loses crispness.
- **Display scaling:** **100%**. Windows DPI scaling above 100% breaks
  pixel-precise framing. Set in Settings → System → Display → Scale.
- **Theme:** macOS / Windows / GNOME dark mode (the wallpaper visible
  behind a windowed capture should be dark).
- **Wallpaper:** plain dark grey or black if any of the desktop will be
  visible in the capture. No personal photos, no LLM-generated art.

### App

- Build: latest commit on `chore/313-public-push` (i.e. version
  **0.8.43**).
- App theme: **Noir** (default, do not switch to the light theme for
  marketing captures).
- Window mode: unified `main` window (sidebar + terminal in one). This
  is the only mode after the unified-window rewrite (`lib.rs:643`).
- Window size at capture time: **1600×1000** on screen. The capture
  tool will export at the spec resolution; the on-screen size keeps
  the row heights and font sizes proportional.

### Fonts (critical)

Install these system-wide before capturing. Without them the renderer
falls through to Segoe UI / Consolas and the captures lose brand fidelity.

- **Geist** (UI): https://vercel.com/font (download Geist Sans)
- **Cascadia Code** (terminal): bundled with Windows Terminal; install
  manually on macOS/Linux if missing

If you cannot install Geist, fall back to **Inter Variable** as the closest
substitute. Note the substitution in the commit message so a re-shoot can
be planned.

### Tools

| Job | Recommended | Alternatives |
|---|---|---|
| Single PNG screenshot | **ShareX** (Win) / built-in Screenshot (macOS Cmd+Shift+4) | Greenshot, Snipping Tool |
| Animated GIF | **ScreenToGif** (Win) / **gifski** (cross-platform) | Kap (macOS), peek (Linux) |
| MP4 screencast | **OBS Studio** | Windows Game Bar, QuickTime (macOS) |
| GIF compression | **gifski** | `ffmpeg -i in.mp4 -vf "fps=15,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse" out.gif` |

### Test data

You need a workspace that produces the visuals these captures require.
Reproducible setup:

```
# In a fresh AgentsCommander instance:
1. Create a project pointing at a real local repo (any working Rust/Node app).
2. Create a workgroup named "demo-wg" with 3 agents:
   - architect       (Claude Code profile)
   - dev-rust        (Codex profile)
   - reviewer        (Claude Code profile)
3. In a second project, add one more session running Gemini.
4. Open settings → Telegram → wire one bot to one session (the "monitored" one)
   if you want telegram-bridge.png to be authentic.
```

All session names and project names should be in English. No "proyecto",
no "agente", no `_es` suffixes.

## 2. `docs/screenshots/hero.png`

The single highest-leverage asset in the push. First-viewport of the
README.

### Frame

- **Window:** the entire app window, including the custom titlebar.
- **Visible regions:** sidebar (left) + 2 terminal panes (right, split
  vertically OR a single pane if split is not implemented; confirm
  with dev-rust which is the canonical layout).
- **Crop:** include 8–12px of shadow/halo around the window if the
  capture tool catches it, otherwise crop tight to the window chrome.

### Sidebar state

- ≥3 agent sessions visible.
- Status mix: **at least 1 green (waiting)**, **at least 1
  yellow/pending** or **blue/running**. Never all green and never all
  red; the mix is the point.
- Recommended row composition (top → bottom):
  1. `architect` · Claude Code · status green (waiting)
  2. `dev-rust` · Codex · status blue (running)
  3. `reviewer` · Claude Code · status amber (pending)
- One project header expanded above the rows. Project name: **a real
  repo name** the audience will recognize as a real repo (`repo-app`,
  `repo-website`, etc.), not `demo` or `test`.

### Terminal state

- Pane 1: Claude Code mid-output. Show 8–12 lines of plausible agent
  output (a diff, a file write, a tool call). Cursor visible at the
  end of the last line.
- Pane 2: Codex mid-output. Similar density. Different content so the
  audience can see the two are doing different things.
- **No real secrets, API keys, or paths under `C:\Users\<realname>\`
  visible.** Scrub before capturing.

### Export

- 2880×1800 PNG: `docs/screenshots/hero.png`
- (optional) 1440×900 PNG: `docs/screenshots/hero@1x.png` (the
  filename suffix matches the retina convention readers expect)

## 3. `docs/screenshots/demo.gif` + `docs/screenshots/demo.mp4`

### Length + budget

- **8–15 seconds** total. Closer to 10 is ideal: long enough to land
  the workflow, short enough to autoplay on a Twitter feed.
- **GIF ≤ 4 MB.** Hard limit for README inline rendering performance.
- **MP4 unconstrained but ≤ 6 MB** is target. GitHub re-encodes for
  embedded playback.

### Script (frame-by-frame)

| t | Beat | What's on screen |
|---|---|---|
| 0.0s | Cold start | Empty AC window. Sidebar shows the "Demo" workgroup with no sessions. |
| 0.5s | Cursor moves to "New session" | Pointer hovers the "+" button or whatever the actual entry point is. |
| 1.0s | Open agent picker | `AgentPickerModal` appears. Cursor over the `architect` row. |
| 1.8s | Click `architect` (Claude Code) | Modal closes. New sidebar row appears with a blue (running) dot. Terminal pane spins up. |
| 3.0s | Open agent picker again | Cursor goes back to "+". Modal opens. |
| 3.6s | Click `dev-rust` (Codex) | Second row appears. Both sessions running (blue dots). Terminal split or swap to show the new one. |
| 5.0s | Both running | Both terminals stream output for ~3s. Different content. |
| 8.5s | First session finishes | `architect` row's dot transitions blue → green. Maybe a subtle pop/highlight. |
| 9.5s | Pointer goes to green dot | Cursor moves over the green row. |
| 10.5s | Click the green session | Terminal pane swaps to show the `architect` output, waiting at a prompt. |
| 11.5s | Hold for 1s | End frame: green dot prominent, terminal showing "waiting for input" cue. |

### Recording

- Frame rate: **15–24 fps**. 30 fps is wasted bandwidth for GIF.
- Resolution: capture at on-screen 1600×1000, export GIF at 1440 wide
  (downscaling smooths palette quantization).
- Cursor: visible (it's part of the demo: the click is the verb).
- Mouse highlight: optional cyan ring effect to draw attention. Do not
  overuse: one or two clicks deserve it, not all of them.

### GIF encoding

```bash
# Record to MP4 first, then encode.
gifski --fps 18 --width 1440 --quality 80 -o docs/screenshots/demo.gif demo.mp4
```

If gifski produces > 4 MB, drop fps to 15 and re-encode. If still over,
trim the recording to 10s.

### Naming

- `docs/screenshots/demo.gif` (GitHub-embedded)
- `docs/screenshots/demo.mp4` (pristine source, also used as video embed
  for paths that prefer mp4)

## 4. `docs/screenshots/agent-picker.png`

### What it shows

The `AgentPickerModal` component (`src/sidebar/components/AgentPickerModal.tsx`)
fully open, with a populated list. This is the surface that loads the
vendored `agency-agents` snapshot.

### Frame

- The modal centered in the app window. Include some of the dimmed
  sidebar background so it reads as a modal, not a standalone.
- Modal width per CSS; do not resize manually.

### Modal state

- Title: `Launch new-session` (or whatever the user actually typed for
  the placeholder session name, pick a realistic name).
- Agent list: ≥6 rows visible without scroll. Roles to feature
  (in alphabetical order, since the component sorts):
  1. `architect`
  2. `dev-rust`
  3. `developer-advocate`
  4. `growth-hacker`
  5. `tech-lead`
  6. `technical-writer`
  7. `ui-designer`
  8. `ux-architect`
- Highlighted row: `tech-lead` or `ui-designer` (the highlight is the
  cyan accent, show it working).
- Color badges: each agent's `agent.color` visible to the left of the
  name.
- Footer text visible: `↑↓ navigate    ↵ launch    esc close`.

### Export

- 2880×1800 PNG: `docs/screenshots/agent-picker.png`

## 5. `docs/screenshots/multi-agent-panel.png`

### What it shows

The sidebar focus image. The "watch your team" hero of the multi-agent
story.

### Frame

- Sidebar only: crop tightly to the left panel. Do NOT include the
  terminal.
- Width: full sidebar width as the app renders it.
- Height: enough vertical space for the project header + 4–6 session
  rows + footer/status row.

### Sidebar state

- **At least one project** expanded showing 4+ sessions.
- Status distribution (mandatory mix):
  - 1 green (waiting): top
  - 1 blue (running)
  - 1 amber (pending)
  - 1 red (exited)
- Agent names span coding-agent products: at least one Claude Code,
  one Codex, one Gemini. This is the "no vendor lock-in" beat.
- Bridge indicator (telegram icon, voice icon) visible on at least one
  row if implemented in the current build.

### Export

- 2880×1800 PNG (the column will be a slice of that; that's fine,
  the right side can be transparent or `#0a0a0f`).
- Filename: `docs/screenshots/multi-agent-panel.png`

## 6. `docs/screenshots/telegram-bridge.png`

### What it shows

The Telegram bridge feature, side-by-side with a real Telegram chat.

### Composition

This is a composite. Two layouts work, pick one:

**Option A: desktop composite**

- Left half: AC window cropped to show the session row with the
  Telegram bridge state indicator + a tooltip or status message.
- Right half: Telegram Desktop showing the same session's chat,
  with the agent's last output visible as a message bubble.
- Both halves at the same vertical scale. A thin cyan line `#00d4ff`
  at 1px between them is optional.

**Option B: phone mockup**

- Left half: AC window crop as above.
- Right half: A clean Android or iOS phone mockup PNG with a
  screenshot of the same Telegram chat inside the phone frame.
- The phone mockup must be a generic device: no Apple/Google
  branding visible (trademark caution).

### Required content in the chat

- ≥2 messages from the agent to the bot: short, plausible (e.g. "Done
  with the refactor. Tests pass. Awaiting review.")
- 1 reply from the user (e.g. "Looks good, merge it.")
- Username on the agent side should match the session name in AC.

### Export

- 2880×1800 PNG: `docs/screenshots/telegram-bridge.png`
- If you mock the phone, save the source `.psd` or `.fig` alongside in
  `docs/assets/sources/` (gitignored) so future revisions are tractable.

## 7. Verification checklist (before committing)

Run through this before `git add docs/screenshots/`:

- [ ] All filenames lowercase, hyphenated, **English**. No Spanish, no
      mixed case, no spaces.
- [ ] All visible UI text in the captures is **English**. Spanish in any
      session name, command, or tooltip is a re-shoot.
- [ ] No real secrets, API keys, real email addresses, or filesystem
      paths revealing personal directories.
- [ ] The Noir theme is in use (dark background). No light-theme shots.
- [ ] All PNG files are ≤ 1.5 MB each. If over, run through
      `pngquant --quality=80-95` or `oxipng -O3`.
- [ ] `demo.gif` ≤ 4 MB. `demo.mp4` ≤ 6 MB.
- [ ] At least 3 of the captures show ≥2 different coding-agent
      products (the cross-coding-agent story).
- [ ] At least 1 capture shows the green "waiting" status dot
      (the "knows when to ping you" story).
- [ ] Cursor is **not** visible in static screenshots. Cursor **is**
      visible in the demo GIF.
- [ ] No personal wallpaper, browser tabs, or other desktop chrome
      bleeding into the capture.

## 8. Where the design specs land if a capture is impossible

If a specific feature is not yet shippable as a capture (e.g. the
Telegram bridge UI moved in a recent refactor), leave the file out of
this PR and open a follow-up issue. Do **not** ship a placeholder
SVG mockup in the screenshots directory; readers will assume it is a
real capture and feel misled when the actual UI differs.

ui-designer, wg-2-community
