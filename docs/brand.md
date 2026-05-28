# Brand

This is the contributor reference for AgentsCommander's visual identity. The
source of truth for colors and type lives in
`src/sidebar/styles/variables.css` and `src/terminal/styles/variables.css`.
This doc explains intent — `variables.css` is what ships.

If you are adding a new surface (a modal, a panel, a doc page, a social
asset) read this once, then pick tokens from the locked palette below. Do not
introduce new colors.

## Palette

Theme name: **Noir** (dark default). A light theme exists in `variables.css`
but the dark Noir palette is the public face of the app — every screenshot,
the OG card, and the demo GIF render in Noir.

### Base

| Token | Hex | Use |
|---|---|---|
| Background | `#0a0a0f` | App canvas. Sidebar background. Terminal background. Default fill behind everything. |
| Surface | `#0e0e18` | Raised panels, modal bodies, picker rows, tooltips. One step above background to imply elevation without a shadow. |
| Foreground | `#d0d0d8` | Default body text in the UI chrome. **Not** terminal text — that is `#e8e8e8`, a half-step brighter so PTY output reads on top of `#0a0a0f`. |
| Accent (cyan) | `#00d4ff` | Brand. Active selection, focus ring, terminal cursor, toolbar action color, link, statusbar accent. Use sparingly — one accent per region. |

### Status dots

The four status colors below are load-bearing UI. They appear next to every
session in the sidebar. Do not repurpose them for non-status decoration; a
green dot must always mean "waiting for human input."

| Token | Hex | State | Meaning |
|---|---|---|---|
| Waiting | `#22c55e` | Green | Agent finished its turn and is waiting for human input. Highest-priority "look here." |
| Pending | `#eab308` | Amber | Agent is queued / has work pending but no live PTY activity yet. |
| Running | `#3a7bff` | Blue | Agent is actively running — PTY output streaming. |
| Exited | `#ff3b5c` | Red | Session exited (clean or crash). Detail in the row tooltip. |

Two additional status tokens exist in `variables.css` but are rarely surfaced
in marketing material: `idle` (`#555566`) for sessions with no recent
activity, and `offline` (`rgba(255,255,255,0.25)`) for disconnected bridges.

### Reserved chrome colors

These exist in `variables.css` and round out the system. Treat as locked.

| Token | Hex | Use |
|---|---|---|
| Foreground (dim) | `#6a6a78` | Secondary text, captions, footer help. |
| Titlebar bg | `#08080d` | Custom titlebar — a hair darker than `--sidebar-bg` to anchor the window. |
| Titlebar fg | `#888898` | Title text, window-control icons. |
| Sidebar hover | `#12121e` | Row hover state. |
| Sidebar active | `#161628` | Selected row. |
| Sidebar border | `rgba(255,255,255,0.06)` | Hairline dividers. Never solid. |
| Toolbar btn bg | `rgba(0,212,255,0.10)` | Tinted accent surface for primary actions. |
| Toolbar btn hover | `rgba(0,212,255,0.20)` | Same, on hover. |
| Close-button hover | `#e81123` | Windows-standard red. Do not change. |

## Typography

Two stacks. Both fall back through web-safe families so the app keeps its
character even if the brand fonts are missing.

### UI stack

```
"Geist", "Outfit", "General Sans", "Segoe UI", sans-serif
```

Used for every non-terminal surface: sidebar, modals, settings, docs widgets,
the wordmark in marketing assets.

| Size token | px | Use |
|---|---|---|
| `--font-size-sm` | 11 | Footer hints, status captions. |
| `--font-size-md` | 13 | Sidebar rows, body text in modals. |
| `--font-size-lg` | 14 | Section labels, agent names. |

Weight scale: 400 (body) · 500 (active row) · 600 (section labels) · 700
(wordmark, hero copy). No 800/900 — too loud for the Noir surface.

### Terminal stack

```
"Cascadia Code", "JetBrains Mono", "Fira Code", monospace
```

Used only inside the xterm.js panes. Default size 14px. Ligatures on — they
ship by default in Cascadia Code and JetBrains Mono.

## Logo

The mark is a stylized helmet/visor — a single figure with a glowing cyan
visor stripe. It reads as "an operator at the controls," matching the
"command center" pitch. The mark is locked. There is no wordmark lockup
file; the wordmark is set in Geist Bold at runtime in marketing assets.

### Files

| File | Purpose |
|---|---|
| `src-tauri/icons/icon.png` | 512×512 canonical raster master. Use this for any new asset. |
| `src-tauri/icons/icon.icns` | macOS bundle icon. |
| `src-tauri/icons/icon.ico` | Windows bundle icon. |
| `src-tauri/icons/128x128.png`, `128x128@2x.png`, `64x64.png`, `32x32.png` | Sized rasters for installers + window icons. |
| `src-tauri/icons/Square*Logo.png` | Microsoft Store sized variants. Do not hand-edit. |
| `src-tauri/icons/android/`, `src-tauri/icons/ios/` | Mobile bundles, generated. |
| `src-tauri/icons/icon.svg` | **Stale.** This file is an old portal-arch design from the pre-rename era. Do **not** use it. PR tracking its removal: see issues. |

### Do

- Use `icon.png` (or one of the sized PNGs) for any new asset — README hero,
  social previews, slide decks.
- Render the mark on dark backgrounds. The visor glow needs `#0a0a0f` or
  `#0e0e18` underneath to read.
- Pair with the wordmark in Geist Bold when a wordmark is needed. The
  wordmark is **"AgentsCommander"** — one word, two capitals.
- Pad the mark by ≥10% of its width on every side. The visor glow extends
  past the helmet silhouette; cropping tight kills the glow.

### Don't

- Don't recolor the mark. The blue is part of the brand; replacing it
  with green/red/etc. communicates the wrong thing.
- Don't add gradients, drop shadows, or outer glows on top of the existing
  glow. The mark already has its own light.
- Don't rotate, skew, or distort the mark.
- Don't render the mark on light backgrounds without first inverting to a
  light-canvas variant. (No such variant exists today; commission one before
  shipping a white-bg surface.)
- Don't use `icon.svg`. It is a vestigial old design and does not match the
  rasters. Use the PNG masters.
- Don't write the name as "Agents Commander" (two words), "agentscommander"
  (all lowercase outside URLs), or "AC" in marketing copy. The product name
  is **AgentsCommander**.

## Voice

Technical, dry-witted, dev-to-dev. No marketing buzzwords. Active second
person. Present tense. Confident, not hyperbolic. We are talking to someone
who already runs Claude Code in a terminal — they don't need to be
convinced AI agents are exciting; they need to know what this thing
actually does and whether it will save them time today.

### Banned vocabulary

`revolutionary`, `unleash`, `supercharge`, `next-gen`, `AI-powered`,
`game-changing`, `blazing-fast`, `seamless`, `magical`, `agentic`,
`empower`, `synergy`, `cutting-edge`, `state-of-the-art`, `disrupt`.

If a sentence still works after deleting one of these words, the word was
not earning its keep. If the sentence breaks, rewrite the sentence — don't
keep the word.

### Examples

| Do | Don't |
|---|---|
| You orchestrate the coding agents you already use. | Revolutionize your workflow with AI-powered autonomous agents. |
| Each agent gets a real PTY. You watch every step in xterm.js. | Seamless step-by-step visibility into your agentic workflows. |
| Agents coordinate by writing markdown files to each other — files you can `cat`, `git diff`, and audit. | Unleash next-gen multi-agent collaboration with our magical coordination layer. |
| Pick Claude Code for the architect role, Codex for the dev role. No vendor lock-in. | Cutting-edge cross-model orchestration empowers you to choose. |
| Built on Windows, runs on Linux, works on macOS — file issues, we'll fix them. | Blazing-fast cross-platform support out of the box. |

### Tone tells

- We say `cat` and `git diff`, not "audit your agent state." We name the
  command.
- We say "Claude Code, Codex, Gemini," not "leading AI coding assistants."
  We name the product.
- Numbers beat adjectives. "60–90% token reduction with RTK" beats
  "massive token savings."
- Footguns and limits in the open. "macOS not yet tested" out-converts
  silence on macOS.

---

— Last reviewed: 2026-05-27. Tokens cross-checked against `variables.css`.
If you change a token in `variables.css`, update this file in the same PR.
