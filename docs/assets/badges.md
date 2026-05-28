# README badges row: spec

Helper file for whoever rewrites the README hero block. Copy the markdown
under "Final block" into the README directly under the H1, above the
30-second pitch.

## Constraints

- **Exactly 6 badges.** More than 6 turns the hero into a sticker wall and
  hurts first-impression credibility.
- All badges are shields.io. No custom hosting, no third-party badge
  services that can break the README if they go down.
- Style is locked to `flat-square` for the whole row: a single visual
  rhythm.
- Color is locked to the brand cyan (`00d4ff`) for status/info badges and
  shields.io defaults for true categorical badges (license, language).
- Wording is English. No localization in badge text.

## Order (left → right)

The order is deliberate: identity → quality → trust → community → stack.

1. **GitHub Release**: what version is current.
2. **Build**: is it green right now.
3. **License**: can I actually use this.
4. **Stars**: social proof at a glance.
5. **Code-signed (SignPath)**: the trust signal that separates this from
   a weekend project on a stranger's GitHub.
6. **Stack**: Rust + Tauri 2, in one combined badge to save a slot.

## Final block (paste into README)

```markdown
[![GitHub release](https://img.shields.io/github/v/release/mblua/AgentsCommander?style=flat-square&color=00d4ff&label=release)](https://github.com/mblua/AgentsCommander/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/mblua/AgentsCommander/release.yml?style=flat-square&label=build)](https://github.com/mblua/AgentsCommander/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/mblua/AgentsCommander?style=flat-square&color=00d4ff&label=stars)](https://github.com/mblua/AgentsCommander/stargazers)
[![Code-signed · SignPath](https://img.shields.io/badge/code--signed-SignPath-00d4ff?style=flat-square&logo=windows)](CODE_SIGNING_POLICY.md)
[![Built with Rust + Tauri 2](https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%202-dea584?style=flat-square&logo=rust)](https://tauri.app)
```

## Verification checklist (before merging README)

- [ ] Repo slug `mblua/AgentsCommander` matches the actual GitHub URL.
      Change in all 4 shields URLs above if the owner/repo differs.
- [ ] `release.yml` is the correct workflow filename in `.github/workflows/`.
      Update the URL if the workflow file is named differently.
- [ ] All 6 badges render in the GitHub README preview (paste, push, reload).
      Broken badge images degrade trust faster than no badge.
- [ ] The 6 badges fit on **one row** on a 1280-wide viewport. If they wrap
      to two lines, the row reads as cluttered; drop the stack badge first.
- [ ] No badge links to a 404. `CODE_SIGNING_POLICY.md` must exist at repo
      root (it does, as of 2026-05-27).

## Future additions (out of scope for this round)

If we later want to surface these, here are the badge specs ready to drop in,
but only if we remove one from the row above. Six is the cap.

- **Discord** (only after ~500 stars per growth-hacker rec):
  `https://img.shields.io/discord/<server_id>?style=flat-square&color=00d4ff&logo=discord&label=discord`
- **Total downloads** (once release-asset counts are non-trivial):
  `https://img.shields.io/github/downloads/mblua/AgentsCommander/total?style=flat-square&color=00d4ff&label=downloads`
- **Last commit** (only useful if velocity drops and we want to signal
  active development, usually noise):
  `https://img.shields.io/github/last-commit/mblua/AgentsCommander?style=flat-square`

ui-designer, wg-2-community
