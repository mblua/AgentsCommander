# Served paths

AgentsCommander binaries fetch every path below from `https://raw.githubusercontent.com/mblua/AgentsCommander/<serving ref>/<path>`, starting with the version named in its row. Released binaries pin these paths forever: moving, renaming or deleting one breaks that fetch on every install still running such a binary, with no staging step.

Rules:

- Append only. Never delete or edit a row, even when no supported binary fetches that path any more.
- Add a row no later than the change that adds a fetch literal under `src-tauri/` or `src/`. A fetch URL must be one plain literal: a URL built from a template or placeholder cannot match a row and fails the check.
- `npm run check:served-paths` (CI job `test-debt`) fails when a row's path is not a tracked file, or when a fetch literal has no row with the same path and serving ref.

| Path | Serving ref | First version that fetched it |
|---|---|---|
| `docs/home-en.md` | `main` | 0.8.43 |
| `remote-resources/blocking-menus/v1/settings-blocking-menus.json` | `main` | first release that ships the #1925 startup download |
