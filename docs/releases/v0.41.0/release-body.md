# Agents Commander v0.41.0

### Added

- **The Welcome dialog shows which coding agents are installed and can install the missing ones.** Each agent card has a status chip (`Installed`, `Not installed` or `Not needed`) and a `Tested: High|Medium|Low` chip, and the list shows installed agents first. A missing agent with a known install command shows the command, a Copy button and an Install button; the install runs in the background and the card flips to Installed when it succeeds. If it fails, the card says `Install failed.` and a `Show output` button reveals the command, its output and the exit code. The coding-agent catalog has a new optional `installCommands` field, filled for Claude Code, Codex, Pi and OpenCode. ([#2736](https://github.com/mblua/AgentsCommander/issues/2736), [#2738](https://github.com/mblua/AgentsCommander/issues/2738), [#2739](https://github.com/mblua/AgentsCommander/issues/2739), [#2740](https://github.com/mblua/AgentsCommander/issues/2740), [#2741](https://github.com/mblua/AgentsCommander/issues/2741), [#2742](https://github.com/mblua/AgentsCommander/issues/2742), [#2745](https://github.com/mblua/AgentsCommander/issues/2745))
- **Settings > General is split into categories with a search box.** The categories are Appearance, Terminal, Agents, Network & remote access and System; the search box looks across all of them, and a red dot marks a category that holds an invalid value. The window is wider, and on narrow screens the categories become chips. ([#2704](https://github.com/mblua/AgentsCommander/issues/2704))
- **The weekly quota left is shown beside each coding agent name** in the terminal agent picker and in Settings > Coding Agents, when a valid reading exists. ([#2681](https://github.com/mblua/AgentsCommander/issues/2681))
- **Codex weekly quota is supported.** A quota source can now read a remaining percentage, as Codex shows it (`weekly 87% left`), and new Codex agents get a suggested weekly pattern by default, from the catalog or when added by hand. Codex agents that already stored an older pattern keep it; **Use suggested pattern** updates them. ([#2686](https://github.com/mblua/AgentsCommander/issues/2686), [#2687](https://github.com/mblua/AgentsCommander/issues/2687), [#2688](https://github.com/mblua/AgentsCommander/issues/2688), [#2726](https://github.com/mblua/AgentsCommander/issues/2726))
- **A replica row says when its saved coding agent was not found.** When AC adopts another profile for it, an amber notice under the row reads `Saved coding agent not found. Using <agent>, same <configuration|name|command>.` It can be dismissed and stays dismissed while the app runs. ([#2568](https://github.com/mblua/AgentsCommander/issues/2568))
- **The sidebar can be used from the keyboard.** Project headers, team headers, Loop rows, replica rows, active sessions and discovery rows take a Tab stop and open or toggle with Enter or Space; the buttons inside a row keep their own action. ([#2655](https://github.com/mblua/AgentsCommander/issues/2655), [#2657](https://github.com/mblua/AgentsCommander/issues/2657), [#2658](https://github.com/mblua/AgentsCommander/issues/2658), [#2659](https://github.com/mblua/AgentsCommander/issues/2659))

### Changed

- **Config files have new names, and you cannot downgrade past this version.** On the first start, AC renames its settings, blocking-menus and coding-agent catalog files to the layered names of the [file naming convention](docs/reference/file-naming.md), for example `settings.json` to `settings.30.instance.no-git.json`, and moves coding agents and their profiles into `agents.30.instance.no-git.json`. The migration runs once, records what it did in `naming-migration.state.no-git.json`, and never deletes or overwrites a file: when both names exist, the new file wins and the old one is kept as `<old name>.deprecated-<n>.no-git`. There is no compatibility shim. An older build started after the migration finds no settings file, starts from defaults, and leaves your real settings untouched under names it does not know. ([#2470](https://github.com/mblua/AgentsCommander/issues/2470))
- **Project and Loop files are renamed too.** `.ac/project-settings.json` becomes `.ac/settings.50.personal.no-git.json`, and each Loop's `state.json` becomes `loop.state.no-git.json`, under the same once-only migration. AC adds the matching rows to `.ac/.gitignore` before renaming, and the Settings screens and docs use the new names. ([#2703](https://github.com/mblua/AgentsCommander/issues/2703), [#2713](https://github.com/mblua/AgentsCommander/issues/2713), [#2714](https://github.com/mblua/AgentsCommander/issues/2714), [#2715](https://github.com/mblua/AgentsCommander/issues/2715), [#2716](https://github.com/mblua/AgentsCommander/issues/2716), [#2717](https://github.com/mblua/AgentsCommander/issues/2717), [#2718](https://github.com/mblua/AgentsCommander/issues/2718), [#2719](https://github.com/mblua/AgentsCommander/issues/2719), [#2720](https://github.com/mblua/AgentsCommander/issues/2720))
- **The project website is now agentscommander.org.** The npm package homepage and the docs point to it. ([#2764](https://github.com/mblua/AgentsCommander/issues/2764))

### Fixed

- **Agents start again on Linux.** After 0.40.0, agents launched in a terminal on Linux could fail with `not found in PATH`. Every place that starts an agent or a helper now uses the same search path AC already resolved. Windows is unchanged. ([#2684](https://github.com/mblua/AgentsCommander/issues/2684), [#2685](https://github.com/mblua/AgentsCommander/issues/2685))
- **The light theme is readable.** Toasts, the Save button, pending status, sidebar text and many hard-coded dark-only colours now have light-theme values, and the selected-row rail is dark in light mode unless you picked a colour. ([#2744](https://github.com/mblua/AgentsCommander/issues/2744), [#2746](https://github.com/mblua/AgentsCommander/issues/2746), [#2747](https://github.com/mblua/AgentsCommander/issues/2747), [#2748](https://github.com/mblua/AgentsCommander/issues/2748), [#2749](https://github.com/mblua/AgentsCommander/issues/2749), [#2751](https://github.com/mblua/AgentsCommander/issues/2751))
- **A Loop whose room is missing on this machine can be disabled and stops repeating errors.** It shows one notice instead of a toast on every run, and the dialog offers **Disable loop (all machines)**. Enabling it still needs the room. ([#2733](https://github.com/mblua/AgentsCommander/issues/2733))
- **Loops are safer to edit while they run.** Saving a Loop no longer waits for a delivery in progress, config and state files are written atomically and locked across AC processes and the CLI, and a prompt that was delivered but whose state write failed is not delivered again. ([#2678](https://github.com/mblua/AgentsCommander/issues/2678), [#2682](https://github.com/mblua/AgentsCommander/issues/2682), [#2694](https://github.com/mblua/AgentsCommander/issues/2694), [#2695](https://github.com/mblua/AgentsCommander/issues/2695), [#2698](https://github.com/mblua/AgentsCommander/issues/2698))
- **Wake messages are no longer lost on shells that submit with Enter.** The extra line ending that left the message unsent is removed. ([#2586](https://github.com/mblua/AgentsCommander/issues/2586))
- **A newer co-managed capture is no longer dropped** when it arrives right after a previous one was committed. ([#2756](https://github.com/mblua/AgentsCommander/issues/2756))

## Included scope

- maintenance: test(sidebar): add Coding Agent picker testids (#2528) ([#2528](https://github.com/mblua/AgentsCommander/issues/2528), [PR #2706](https://github.com/mblua/AgentsCommander/pull/2706))
- feature: feat(picker): instrument selection timing (refs #2555) ([#2555](https://github.com/mblua/AgentsCommander/issues/2555), [PR #2722](https://github.com/mblua/AgentsCommander/pull/2722))
- feature: feat(sidebar): orphan-adoption notice on replica rows (#2568) ([#2568](https://github.com/mblua/AgentsCommander/issues/2568), [PR #2677](https://github.com/mblua/AgentsCommander/pull/2677))
- fix: fix(inject): strip wake wrap suffix, observe submit seam (#2586 phase 1) ([#2586](https://github.com/mblua/AgentsCommander/issues/2586), [PR #2692](https://github.com/mblua/AgentsCommander/pull/2692))
- maintenance: sidebar: keyboard access for 5 collapsible headers (S1082, #2655 phase A) ([#2657](https://github.com/mblua/AgentsCommander/issues/2657), [PR #2661](https://github.com/mblua/AgentsCommander/pull/2661))
- feature: feat(sidebar): keyboard access for five more ProjectPanel rows (refs #2658) ([#2658](https://github.com/mblua/AgentsCommander/issues/2658), [PR #2734](https://github.com/mblua/AgentsCommander/pull/2734))
- feature: feat(sidebar): keyboard access for nested rows (refs #2659) ([#2659](https://github.com/mblua/AgentsCommander/issues/2659), [PR #2743](https://github.com/mblua/AgentsCommander/pull/2743))
- fix: fix(scripts): resolve git by absolute path in scripts (#2670) ([#2670](https://github.com/mblua/AgentsCommander/issues/2670), [PR #2671](https://github.com/mblua/AgentsCommander/pull/2671))
- maintenance: test(loops): pin state write before audit append (#2679) ([#2679](https://github.com/mblua/AgentsCommander/issues/2679), [PR #2699](https://github.com/mblua/AgentsCommander/pull/2699))
- feature: feat(sidebar): show weekly remaining quota beside agents (refs #2681) ([#2681](https://github.com/mblua/AgentsCommander/issues/2681), [PR #2689](https://github.com/mblua/AgentsCommander/pull/2689))
- fix: fix(loops): serialize Loop config+state writes across processes (#2682) ([#2682](https://github.com/mblua/AgentsCommander/issues/2682), [PR #2700](https://github.com/mblua/AgentsCommander/pull/2700))
- fix: fix(pty): launch agents through the effective search path on Linux (#2684) ([#2684](https://github.com/mblua/AgentsCommander/issues/2684), [PR #2708](https://github.com/mblua/AgentsCommander/pull/2708))
- fix: fix(pty): apply launch search path at every spawn site (#2685) ([#2685](https://github.com/mblua/AgentsCommander/issues/2685), [PR #2721](https://github.com/mblua/AgentsCommander/pull/2721))
- feature: feat(quota): support remaining-percentage screen source (refs #2686) ([#2686](https://github.com/mblua/AgentsCommander/issues/2686), [PR #2705](https://github.com/mblua/AgentsCommander/pull/2705))
- feature: feat: add kind-aware Codex weekly quota contract (refs #2687) ([#2687](https://github.com/mblua/AgentsCommander/issues/2687), [PR #2710](https://github.com/mblua/AgentsCommander/pull/2710))
- feature: feat(#2688): default Codex weekly quota on both creation paths ([#2688](https://github.com/mblua/AgentsCommander/issues/2688), [PR #2723](https://github.com/mblua/AgentsCommander/pull/2723))
- feature: feat(testability): add ui-pointer and ui-key CLI verbs (refs #2690) ([#2690](https://github.com/mblua/AgentsCommander/issues/2690), [PR #2701](https://github.com/mblua/AgentsCommander/pull/2701))
- feature: feat(ui): dispatch pointer and key automation events (refs #2691) ([#2691](https://github.com/mblua/AgentsCommander/issues/2691), [PR #2702](https://github.com/mblua/AgentsCommander/pull/2702))
- fix: fix(loops): atomic config write, replace retry and CAS state write (#2694, phase A of #2678) ([#2694](https://github.com/mblua/AgentsCommander/issues/2694), [PR #2696](https://github.com/mblua/AgentsCommander/pull/2696))
- fix: fix(loops): split scheduler lock so Loop save no longer blocks on delivery (#2695) ([#2695](https://github.com/mblua/AgentsCommander/issues/2695), [PR #2697](https://github.com/mblua/AgentsCommander/pull/2697))
- fix: fix(loops): do not re-deliver a run whose post-delivery state write failed (#2698) ([#2698](https://github.com/mblua/AgentsCommander/issues/2698), [PR #2707](https://github.com/mblua/AgentsCommander/pull/2707))
- feature: feat(settings): General tab categories + cross-category search (#2704) ([#2704](https://github.com/mblua/AgentsCommander/issues/2704), [PR #2732](https://github.com/mblua/AgentsCommander/pull/2732))
- fix: fix(frontend): remove agent-help/ipc dependency cycle (#2711) ([#2711](https://github.com/mblua/AgentsCommander/issues/2711), [PR #2725](https://github.com/mblua/AgentsCommander/pull/2725))
- maintenance: test: support occupied Windows default root (refs #2712) ([#2712](https://github.com/mblua/AgentsCommander/issues/2712), [PR #2730](https://github.com/mblua/AgentsCommander/pull/2730))
- maintenance: refactor(config): file naming phase B1a, migration engine (refs #2703) ([#2713](https://github.com/mblua/AgentsCommander/issues/2713), [PR #2724](https://github.com/mblua/AgentsCommander/pull/2724))
- maintenance: refactor(naming): B1b migrate the instance families to layered names (#2714) ([#2714](https://github.com/mblua/AgentsCommander/issues/2714), [PR #2729](https://github.com/mblua/AgentsCommander/pull/2729))
- maintenance: refactor(#2715): B2 migrate coding-agents catalog family names ([#2715](https://github.com/mblua/AgentsCommander/issues/2715), [PR #2737](https://github.com/mblua/AgentsCommander/pull/2737))
- maintenance: refactor(settings): move agents and codingAgentProfiles to the instance agents file (refs #2716) ([#2716](https://github.com/mblua/AgentsCommander/issues/2716), [PR #2759](https://github.com/mblua/AgentsCommander/pull/2759))
- maintenance: refactor(config): migrate .ac/project-settings.json to layer 50 (refs #2717) ([#2717](https://github.com/mblua/AgentsCommander/issues/2717), [PR #2762](https://github.com/mblua/AgentsCommander/pull/2762))
- maintenance: refactor: file naming phase B4b, migrate loop state files (#2718) ([#2718](https://github.com/mblua/AgentsCommander/issues/2718), [PR #2763](https://github.com/mblua/AgentsCommander/pull/2763))
- maintenance: refactor(ui): B5 name the renamed settings, agents and project files (#2719) ([#2719](https://github.com/mblua/AgentsCommander/issues/2719), [PR #2765](https://github.com/mblua/AgentsCommander/pull/2765))
- docs: docs: name the phase-B config files and record no-downgrade (#2720) ([#2720](https://github.com/mblua/AgentsCommander/issues/2720), [PR #2773](https://github.com/mblua/AgentsCommander/pull/2773))
- fix: fix: Codex weekly quota pattern matches the real lowercase 'weekly' row (#2726) ([#2726](https://github.com/mblua/AgentsCommander/issues/2726), [PR #2727](https://github.com/mblua/AgentsCommander/pull/2727))
- fix: fix(loops): allow disabling loops whose room is missing and stop repeated error toasts (refs #2733) ([#2733](https://github.com/mblua/AgentsCommander/issues/2733), [PR #2735](https://github.com/mblua/AgentsCommander/pull/2735))
- feature: feat(catalog): add optional installCommands field (refs #2736, #2738) ([#2738](https://github.com/mblua/AgentsCommander/issues/2738), [PR #2750](https://github.com/mblua/AgentsCommander/pull/2750))
- feature: #2736 P2 welcome status: tested table and detection IPC (#2739) ([#2739](https://github.com/mblua/AgentsCommander/issues/2739), [PR #2753](https://github.com/mblua/AgentsCommander/pull/2753))
- feature: feat(agent-install): silent install runner (#2740) ([#2740](https://github.com/mblua/AgentsCommander/issues/2740), [PR #2755](https://github.com/mblua/AgentsCommander/pull/2755))
- feature: #2736 P4 Welcome status chips and order (#2741) ([#2741](https://github.com/mblua/AgentsCommander/issues/2741), [PR #2758](https://github.com/mblua/AgentsCommander/pull/2758))
- feature: #2736 P5: Welcome Copy and Install actions (#2742) ([#2742](https://github.com/mblua/AgentsCommander/issues/2742), [PR #2761](https://github.com/mblua/AgentsCommander/pull/2761))
- feature: feat(welcome): disclose captured install failure output (#2745) ([#2745](https://github.com/mblua/AgentsCommander/issues/2745), [PR #2772](https://github.com/mblua/AgentsCommander/pull/2772))
- maintenance: test(theme): light-mode contrast harness and allowlist (#2746) ([#2746](https://github.com/mblua/AgentsCommander/issues/2746), [PR #2752](https://github.com/mblua/AgentsCommander/pull/2752))
- fix: fix(theme): light-mode info toast and Save default contrast (#2747) ([#2747](https://github.com/mblua/AgentsCommander/issues/2747), [PR #2754](https://github.com/mblua/AgentsCommander/pull/2754))
- fix: fix(theme): define light-theme tokens and fix 33 light contrast rules (#2748) ([#2748](https://github.com/mblua/AgentsCommander/issues/2748), [PR #2757](https://github.com/mblua/AgentsCommander/pull/2757))
- fix: fix(theme): light-theme literal colours (Epic #2744 phase D) ([#2749](https://github.com/mblua/AgentsCommander/issues/2749), [PR #2760](https://github.com/mblua/AgentsCommander/pull/2760))
- fix: fix(sidebar): theme-aware selected-row rail default in light mode (#2751) ([#2751](https://github.com/mblua/AgentsCommander/issues/2751), [PR #2769](https://github.com/mblua/AgentsCommander/pull/2769))
- fix: fix(capture): preserve newer co-managed candidates, refs #2756 ([#2756](https://github.com/mblua/AgentsCommander/issues/2756), [PR #2771](https://github.com/mblua/AgentsCommander/pull/2771))
- maintenance: chore(#2764): update website references to agentscommander.org ([#2764](https://github.com/mblua/AgentsCommander/issues/2764), [PR #2766](https://github.com/mblua/AgentsCommander/pull/2766))

## Install from npm

```text
npx @mblua/agentscommander@0.41.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2777
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.40.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
