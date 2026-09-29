# Desktop wallpapers

AgentsCommander desktop wallpapers, built from the logo
(`src-tauri/icons/icon.png`). Each one is a 2560x1440 PNG.

| File | Layout |
|---|---|
| `agentscommander-wallpaper-v1-horizontal-2560x1440.png` | Helmet on the left, name on the right, faint starfield. |
| `agentscommander-wallpaper-v2-minimal-2560x1440.png` | **Default.** Small helmet and name in the bottom-right corner, faint starfield. |
| `agentscommander-wallpaper-v3-centered-2560x1440.png` | Helmet centered, name below, faint starfield. |

The text uses Bahnschrift, a Microsoft font bundled with Windows.

## Regenerate

`generate-wallpapers.py` rebuilds all three PNGs in this folder. It is a
manual tool, not part of any build or CI.

You need:

- Windows, with the Bahnschrift font at `C:/Windows/Fonts/bahnschrift.ttf`.
- Python 3 with Pillow (`pip install pillow`).

Run it from the repository root:

```
python docs/assets/wallpapers/generate-wallpapers.py
```
