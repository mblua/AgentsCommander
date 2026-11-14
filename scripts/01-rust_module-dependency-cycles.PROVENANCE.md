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

- TOOL_VERSION is now 1.2.0 under #4013 to identify the parent-to-child arc semantics from #3998; SCHEMA_VERSION stays 1.
- SHA-256 after #3998: `01d847b9ecfa327b4a3c12326be06c4cafcb43719d78c9893bfbdb30a950c6fe`
- SHA-256 after #4013: `76d47fa2af9e9a4656e4b2f4202da289e10fefd8f7c0c96f72c70f307e3b41f8`
- SHA-256 after #4706 B4a: `990177ed1ffa2d970e74761bd7aa3b65a6d37e1370e19754ef0d1e307700d5ce`
- SHA-256 after #4707 B4b: `defe22ef9eaae444b29560f6946449a7eb33570db712a917e2f6112878ce7af8`
- SHA-256 after #4708 B4c: `f175a82daf36e579b4cb20407c2f60118f0440ad32dd45a8a92d87b52d594abd`

Edit it here, through a reviewed fix with tests, and update the hash above. Do not re-copy
the vault source over it: that would drop these fixes. The vault is not refreshed from here.

After any change, run `node --test scripts/module-child-paths.test.mjs`,
`npm run check:cycles:self`, `npm run check:cycles` and `npm run check:arcs`;
the last validates and regenerates ignored `src-tauri/module-arcs.txt`. Never stage the record.
CI publishes validated tested-tree bytes and a receipt; see `docs/testing/module-arcs.md`.
