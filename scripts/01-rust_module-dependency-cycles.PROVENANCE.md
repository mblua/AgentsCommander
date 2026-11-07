# Provenance of `01-rust_module-dependency-cycles.mjs`

## Historical origin

- Source: `repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs`
- `TOOL_VERSION 1.1.1`
- `SCHEMA_VERSION 1`
- Copied: 2026-09-24 (#2463, epic #2447), vault commit `b912dbc9b48880ac0a0a15317d0a53bcf714bb82`
- SHA-256 of the source file at copy time: `4181fe8474dd491210979d86c5a21babdefb811b6299a726afcdf492237abd88`

## Maintained application fork

The copy is no longer byte identical to the vault source. It is a maintained fork of this
repository, changed by reviewed fixes: #3991 (child use paths never bind to a same-name
crate-root module), #3995 (compound test-cfg arcs excluded) and #3998 (child paths resolve
against the current module, so parent-to-child arcs are recorded).

- `TOOL_VERSION` stays `1.1.1` and `SCHEMA_VERSION` stays `1`.
- SHA-256 after #3998: `01d847b9ecfa327b4a3c12326be06c4cafcb43719d78c9893bfbdb30a950c6fe`

Edit it here, through a reviewed fix with tests, and update the hash above. Do not re-copy
the vault source over it: that would drop these fixes. The vault is not refreshed from here.

After any change, run `node --test scripts/module-child-paths.test.mjs`,
`npm run check:cycles:self`, `npm run check:cycles` and `npm run check:arcs`;
the last validates and regenerates ignored `src-tauri/module-arcs.txt`. Never stage the record.
CI publishes validated tested-tree bytes and a receipt; see `docs/testing/module-arcs.md`.
