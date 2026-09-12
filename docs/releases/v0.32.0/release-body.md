# Agents Commander v0.32.0

### Added

- **Linux/X11 screenshot capture is available**, with documented capture commands and native build dependencies. ([#1915](https://github.com/mblua/AgentsCommander/issues/1915), [#1916](https://github.com/mblua/AgentsCommander/issues/1916))
- **Muse sessions can resume the latest workspace session automatically** on supported local macOS/Linux launches. Explicit fresh starts and configured arguments retain their own behavior. ([#1873](https://github.com/mblua/AgentsCommander/issues/1873))
- **Blocking-menu patterns now have shipped, local and remote files.** Existing settings arrays are exported once into the local overlay without overwriting operator entries. An enabled-by-default Settings checkbox controls the background startup download, throttled to once per 24 hours. Downloaded files are validated before replacing the cache; failures retain the prior copy, and new patterns apply on the next start. Local overrides take precedence. ([#1905](https://github.com/mblua/AgentsCommander/issues/1905), [#1925](https://github.com/mblua/AgentsCommander/issues/1925))
- **Coding-agent catalog availability and diagnostics are visible in Settings and Quick Configuration**, with a Reload catalog action and guards against stale selections when the primary project changes. Catalog consumers and the startup updater use persisted catalog data without a bundled fallback. ([#1963](https://github.com/mblua/AgentsCommander/issues/1963), [#1964](https://github.com/mblua/AgentsCommander/issues/1964), [#1965](https://github.com/mblua/AgentsCommander/issues/1965), [#1966](https://github.com/mblua/AgentsCommander/issues/1966), [#1967](https://github.com/mblua/AgentsCommander/issues/1967))

### Changed

- **Normal executables now store configuration in the user's home directory, under `.agentscommander`.** An explicit `AGENTSCOMMANDER_CONFIG_DIR` still wins. Unsuffixed executables no longer select an adjacent portable directory, and no old configuration is discovered, copied or migrated automatically. Preserve the active configuration before updating and select its location explicitly when needed. Suffixed instances retain their existing location rules. ([#1868](https://github.com/mblua/AgentsCommander/issues/1868))
- **The Root Agent banner uses its right-click menu for actions**, removing the five hover buttons while retaining the Telegram indicator. ([#1896](https://github.com/mblua/AgentsCommander/issues/1896))
- **Documentation now covers native Linux package installation, platform-specific shell guidance, the Orchestrator alias, and outbound network calls.** The obsolete codebase-memory cache-directory recommendation was removed. ([#1840](https://github.com/mblua/AgentsCommander/issues/1840), [#1951](https://github.com/mblua/AgentsCommander/issues/1951), [#1900](https://github.com/mblua/AgentsCommander/issues/1900), [#1924](https://github.com/mblua/AgentsCommander/issues/1924), [#1897](https://github.com/mblua/AgentsCommander/issues/1897))
- **The Cargo package is named `agentscommander`.** Native regression CI separates focused release builds from test execution, aligns cache targets, and checks the served-path inventory. ([#1934](https://github.com/mblua/AgentsCommander/issues/1934), [#1929](https://github.com/mblua/AgentsCommander/issues/1929), [#1974](https://github.com/mblua/AgentsCommander/issues/1974), [#1946](https://github.com/mblua/AgentsCommander/issues/1946))

### Fixed

- **Concurrent local configuration writes are serialized across processes** to protect persisted settings. ([#1938](https://github.com/mblua/AgentsCommander/issues/1938))
- **Integration tests isolate unsuffixed binaries from the user's configuration.** The built-in coding-agent support table consistently controls catalog filtering and seeding. ([#1867](https://github.com/mblua/AgentsCommander/issues/1867), [#1912](https://github.com/mblua/AgentsCommander/issues/1912))

### Security

- Updated the container base images to Node `22.23.2-trixie-slim` and Debian `13.6-slim`. ([#1931](https://github.com/mblua/AgentsCommander/pull/1931), [#1933](https://github.com/mblua/AgentsCommander/pull/1933))

## Included scope

- docs: docs: route Linux installs to native packages (refs #1840) ([#1840](https://github.com/mblua/AgentsCommander/issues/1840), [PR #1899](https://github.com/mblua/AgentsCommander/pull/1899))
- maintenance: test: #1867 isolate unsuffixed integration binaries ([#1867](https://github.com/mblua/AgentsCommander/issues/1867), [PR #1889](https://github.com/mblua/AgentsCommander/pull/1889))
- fix: fix(config): #1868 canonical HOME for unsuffixed executables, no migration ([#1868](https://github.com/mblua/AgentsCommander/issues/1868), [PR #1917](https://github.com/mblua/AgentsCommander/pull/1917))
- feature: feat(session): add first-class Muse workspace-latest automatic resume (#1873) ([#1873](https://github.com/mblua/AgentsCommander/issues/1873), [PR #1911](https://github.com/mblua/AgentsCommander/pull/1911))
- fix: fix(sidebar): drop the Root Agent banner's five hover buttons (#1896) ([#1896](https://github.com/mblua/AgentsCommander/issues/1896), [PR #1903](https://github.com/mblua/AgentsCommander/pull/1903))
- docs: docs: drop the CBM_CACHE_DIR recommendation from the codebase-memory-mcp page (#1897) ([#1897](https://github.com/mblua/AgentsCommander/issues/1897), [PR #1898](https://github.com/mblua/AgentsCommander/pull/1898))
- docs: docs(glossary): explain Orchestrator alias, refs #1900 ([#1900](https://github.com/mblua/AgentsCommander/issues/1900), [PR #1902](https://github.com/mblua/AgentsCommander/pull/1902))
- feature: feat(settings): blocking-menus store, shipped file and .local overlay (#1906) ([#1906](https://github.com/mblua/AgentsCommander/issues/1906), [PR #1910](https://github.com/mblua/AgentsCommander/pull/1910))
- feature: feat(menu-guard): read blocking menus through the BlockingMenusStore (#1907) ([#1907](https://github.com/mblua/AgentsCommander/issues/1907), [PR #1913](https://github.com/mblua/AgentsCommander/pull/1913))
- feature: feat(settings): one-shot migration of blockingMenus into settings-blocking-menus.local.json (#1908) ([#1908](https://github.com/mblua/AgentsCommander/issues/1908), [PR #1921](https://github.com/mblua/AgentsCommander/pull/1921))
- docs: docs: document settings-blocking-menus.json and the .local overlay (#1909) ([#1909](https://github.com/mblua/AgentsCommander/issues/1909), [PR #1923](https://github.com/mblua/AgentsCommander/pull/1923))
- feature: feat(catalog): add built-in coding-agent support switch (#1912) ([#1912](https://github.com/mblua/AgentsCommander/issues/1912), [PR #1922](https://github.com/mblua/AgentsCommander/pull/1922))
- maintenance: ci(workflows): install Linux xcap native build deps; add rust-linux-release-parity job (refs #1914) ([#1914](https://github.com/mblua/AgentsCommander/issues/1914), [PR #1920](https://github.com/mblua/AgentsCommander/pull/1920))
- feature: feat(screenshot): Linux/X11 screenshot runtime (closes #1915) ([#1915](https://github.com/mblua/AgentsCommander/issues/1915), [PR #1926](https://github.com/mblua/AgentsCommander/pull/1926))
- docs: docs(1916): document Linux/X11 screenshot capture ([#1916](https://github.com/mblua/AgentsCommander/issues/1916), [PR #1950](https://github.com/mblua/AgentsCommander/pull/1950))
- docs: docs(privacy): document every outbound network call, refs #1924 ([#1924](https://github.com/mblua/AgentsCommander/issues/1924), [PR #1928](https://github.com/mblua/AgentsCommander/pull/1928))
- maintenance: ci: run focused release assertions from compiled harness (refs #1929) ([#1929](https://github.com/mblua/AgentsCommander/issues/1929), [PR #1975](https://github.com/mblua/AgentsCommander/pull/1975))
- security: [Snyk] Security upgrade node from 22-bookworm-slim to 22.23.2-trixie-slim ([#1931](https://github.com/mblua/AgentsCommander/issues/1931), [PR #1931](https://github.com/mblua/AgentsCommander/pull/1931))
- security: [Snyk] Security upgrade debian from bookworm-slim to 13.6-slim ([#1933](https://github.com/mblua/AgentsCommander/issues/1933), [PR #1933](https://github.com/mblua/AgentsCommander/pull/1933))
- maintenance: chore: rename Cargo package to agentscommander (#1934) ([#1934](https://github.com/mblua/AgentsCommander/issues/1934), [PR #1957](https://github.com/mblua/AgentsCommander/pull/1957))
- feature: feat(config): serialize local config writes across processes (refs #1938) ([#1938](https://github.com/mblua/AgentsCommander/issues/1938), [PR #1953](https://github.com/mblua/AgentsCommander/pull/1953))
- feature: feat(settings): remote blocking-menu layer, read side (refs #1925, closes #1944) ([#1944](https://github.com/mblua/AgentsCommander/issues/1944), [PR #1958](https://github.com/mblua/AgentsCommander/pull/1958))
- feature: feat(settings): remoteBlockingMenusEnabled settings flag (refs #1925, closes #1945) ([#1945](https://github.com/mblua/AgentsCommander/issues/1945), [PR #1962](https://github.com/mblua/AgentsCommander/pull/1962))
- maintenance: ci: served-path inventory, check-served-paths gate and CODEOWNERS (refs #1925, closes #1946) ([#1946](https://github.com/mblua/AgentsCommander/issues/1946), [PR #1970](https://github.com/mblua/AgentsCommander/pull/1970))
- feature: feat(settings): checkbox for remote blocking-menu pattern downloads (refs #1925, closes #1947) ([#1947](https://github.com/mblua/AgentsCommander/issues/1947), [PR #1973](https://github.com/mblua/AgentsCommander/pull/1973))
- docs: docs(#1948): document remote blocking-menu patterns ([#1948](https://github.com/mblua/AgentsCommander/issues/1948), [PR #1978](https://github.com/mblua/AgentsCommander/pull/1978))
- feature: feat(#1949): download remote blocking-menu patterns at startup ([#1949](https://github.com/mblua/AgentsCommander/issues/1949), [PR #1983](https://github.com/mblua/AgentsCommander/pull/1983))
- fix: fix(settings): platform-specific shell guidance (#1951) ([#1951](https://github.com/mblua/AgentsCommander/issues/1951), [PR #1954](https://github.com/mblua/AgentsCommander/pull/1954))
- feature: feat: expose persisted coding-agent catalog report (#1963) ([#1963](https://github.com/mblua/AgentsCommander/issues/1963), [PR #1972](https://github.com/mblua/AgentsCommander/pull/1972))
- feature: feat(ipc): expose typed catalog report binding (refs #1964) ([#1964](https://github.com/mblua/AgentsCommander/issues/1964), [PR #1977](https://github.com/mblua/AgentsCommander/pull/1977))
- feature: feat(catalog): show availability and guard stale selections (refs #1965) ([#1965](https://github.com/mblua/AgentsCommander/issues/1965), [PR #1979](https://github.com/mblua/AgentsCommander/pull/1979))
- feature: feat(catalog): invalidate presets on primary project changes (refs #1966) ([#1966](https://github.com/mblua/AgentsCommander/issues/1966), [PR #1980](https://github.com/mblua/AgentsCommander/pull/1980))
- fix: fix(catalog): use persisted data across consumers (refs #1967) ([#1967](https://github.com/mblua/AgentsCommander/issues/1967), [PR #1981](https://github.com/mblua/AgentsCommander/pull/1981))
- maintenance: ci: align native regression cache targets (refs #1974) ([#1974](https://github.com/mblua/AgentsCommander/issues/1974), [PR #1976](https://github.com/mblua/AgentsCommander/pull/1976))

## Install from npm

```text
npx @mblua/agentscommander@0.32.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/1985
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.31.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
