# Third-party notices

## Terminal snapshot renderer dependencies

The terminal snapshot feature directly uses the following components under their MIT license options:

- `fontdue` 0.9.3, copyright its contributors, MIT OR Apache-2.0 OR Zlib.
- `png` 0.18.1, copyright its contributors, MIT OR Apache-2.0.
- `base64` 0.22.1, copyright its contributors, MIT OR Apache-2.0.
- `getrandom` 0.3.4, copyright its contributors, MIT OR Apache-2.0.
- `crc32fast` 1.5.0, copyright its contributors, MIT OR Apache-2.0.

The applicable MIT license text is available in each crate's distributed package and permits use, copying, modification, distribution, sublicensing, and sale subject to retaining its copyright and permission notice.

## DejaVu Sans Mono 2.37

AgentsCommander bundles the unmodified `ttf/DejaVuSansMono.ttf` from the upstream DejaVu Fonts 2.37 release for deterministic terminal snapshot rendering.

- Source: <https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-fonts-ttf-2.37.tar.bz2>
- Release archive SHA-256: `fa9ca4d13871dd122f61258a80d01751d603b4d3ee14095d65453b4e846e17d7`
- Font byte length: `340712`
- Font SHA-256: `b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582`
- License SHA-256: `7a083b136e64d064794c3419751e5c7dd10d2f64c108fe5ba161eae5e5958a93`
- Full upstream license: `crates/terminal-snapshot-renderer/assets/LICENSE-DejaVu.txt`

The DejaVu and Bitstream Vera-derived license permits redistribution and embedding. The font is not sold by itself, is bundled unmodified, and no modified font uses a reserved name.
