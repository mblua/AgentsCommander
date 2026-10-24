use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use super::{LEGACY_LOG, LEGACY_PAYLOAD, LEGACY_TOKENS, PROFILE_ENTRIES};

/// Entry-level census of one profile entry. The only filesystem contact is
/// `fs::symlink_metadata`, which neither opens the object nor follows a link
/// or reparse point.
fn profile_entry_kind(home: &Path, name: &str) -> Result<Option<&'static str>, String> {
    let path = home.join(name);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "symlink_metadata {} failed with {:?}: {error}",
            path.display(),
            error.kind()
        )),
        Ok(metadata) => {
            let file_type = metadata.file_type();
            // Symlink first: a Windows junction or directory symlink reports both.
            Ok(Some(if file_type.is_symlink() {
                "symlink"
            } else if file_type.is_dir() {
                "dir"
            } else if file_type.is_file() {
                "file"
            } else {
                "other"
            }))
        }
    }
}

pub(crate) fn profile_census(
    home: &Path,
) -> Result<[(&'static str, Option<&'static str>); 3], String> {
    let entry = |name: &'static str| profile_entry_kind(home, name).map(|kind| (name, kind));
    Ok([
        entry(PROFILE_ENTRIES[0])?,
        entry(PROFILE_ENTRIES[1])?,
        entry(PROFILE_ENTRIES[2])?,
    ])
}

/// Snapshot invariance at entry level only: it does not observe the interval
/// between the two censuses, nor the contents of a present entry.
pub(crate) fn census_unchanged(
    pre: &[(&'static str, Option<&'static str>); 3],
    post: &[(&'static str, Option<&'static str>); 3],
) -> Result<(), String> {
    for (before, after) in pre.iter().zip(post.iter()) {
        if before != after {
            return Err(format!(
                "profile entry {} changed: before={:?} after={:?}",
                before.0, before.1, after.1
            ));
        }
    }
    Ok(())
}

/// Positive and inverse battery for the census over a `TempDir` fake profile.
/// It never touches the real profile.
pub(crate) fn refusal_census_self_control() {
    let fake = tempfile::TempDir::new().expect("self-control tempdir");
    let root: &Path = fake.path();
    let dir_entry = root.join(PROFILE_ENTRIES[0]);
    fs::create_dir(&dir_entry).expect("self-control create dir entry");
    fs::write(root.join(PROFILE_ENTRIES[1]), b"").expect("self-control create file entry");

    let pre = profile_census(root).expect("self-control pre-census");
    assert_eq!(
        pre,
        [
            (PROFILE_ENTRIES[0], Some("dir")),
            (PROFILE_ENTRIES[1], Some("file")),
            (PROFILE_ENTRIES[2], None),
        ],
        "self-control pre-census shape"
    );

    let same = profile_census(root).expect("self-control positive re-census");
    census_unchanged(&pre, &same).expect("self-control positive leg must be unchanged");

    fs::remove_dir(&dir_entry).expect("self-control leg 1 remove dir");
    let post = profile_census(root).expect("self-control leg 1 census");
    assert!(
        census_unchanged(&pre, &post).is_err(),
        "leg 1: present becomes absent must be Err"
    );
    fs::create_dir(&dir_entry).expect("self-control leg 1 recreate dir");

    fs::remove_dir(&dir_entry).expect("self-control leg 2 remove dir");
    fs::write(&dir_entry, b"").expect("self-control leg 2 create file");
    let post = profile_census(root).expect("self-control leg 2 census");
    assert!(
        census_unchanged(&pre, &post).is_err(),
        "leg 2: dir becomes file must be Err"
    );
    fs::remove_file(&dir_entry).expect("self-control leg 2 remove file");
    fs::create_dir(&dir_entry).expect("self-control leg 2 recreate dir");

    fs::create_dir(root.join(PROFILE_ENTRIES[2])).expect("self-control leg 3 create dir");
    let post = profile_census(root).expect("self-control leg 3 census");
    assert!(
        census_unchanged(&pre, &post).is_err(),
        "leg 3: absent becomes present must be Err"
    );

    println!("ISSUE1850_REFUSAL_SELFCONTROL_OK entries=3");
}

pub(crate) fn profile_entry_absent(home: &Path, name: &str) -> Result<(), String> {
    let path = home.join(name);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "symlink_metadata {} failed with {:?}: {error}",
            path.display(),
            error.kind()
        )),
        Ok(metadata) => Err(format!(
            "profile entry {} already exists ({:?}); refusing to create, open, clean or adopt it",
            path.display(),
            metadata.file_type()
        )),
    }
}

pub(crate) fn regular_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "{label}: symlink_metadata {} failed: {error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!("{label}: {} is not a regular file", path.display()));
    }
    Ok(())
}

pub(crate) fn regular_file_nonempty(path: &Path, label: &str) -> Result<(), String> {
    regular_file(path, label)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "{label}: symlink_metadata {} failed: {error}",
            path.display()
        )
    })?;
    if metadata.len() == 0 {
        return Err(format!("{label}: {} is empty", path.display()));
    }
    Ok(())
}

pub(crate) fn write_legacy_root(
    root: &Path,
    token: &str,
    session_name: &str,
    working_directory: &Path,
) -> Result<(), String> {
    // Non-recursive: a pre-existing path is a collision, not state to adopt.
    fs::create_dir(root)
        .map_err(|error| format!("create_dir {} failed: {error}", root.display()))?;
    let settings = serde_json::json!({
        "defaultShell": "cmd",
        "defaultShellArgs": [],
        "agents": [],
        "rootToken": token,
        "legacyMarker": token,
    });
    write_json(&root.join("settings.json"), &settings)?;
    write_json(
        &root.join("sessions.json"),
        &serde_json::json!([session_row(
            &format!("{token}-session-id"),
            session_name,
            working_directory
        )]),
    )?;
    for (name, bytes) in [("app.log", LEGACY_LOG), ("payload.bin", LEGACY_PAYLOAD)] {
        fs::write(root.join(name), bytes)
            .map_err(|error| format!("write {} failed: {error}", root.join(name).display()))?;
    }
    let nested = root.join("instances").join("legacy");
    fs::create_dir_all(&nested)
        .map_err(|error| format!("create {} failed: {error}", nested.display()))?;
    fs::write(nested.join("web-token.txt"), token.as_bytes())
        .map_err(|error| format!("write nested legacy token failed: {error}"))?;
    Ok(())
}

pub(crate) fn session_row(id: &str, name: &str, working_directory: &Path) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": working_directory.to_string_lossy(),
    })
}

pub(crate) fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("serialize {} failed: {error}", path.display()))?;
    fs::write(path, text).map_err(|error| format!("write {} failed: {error}", path.display()))
}

pub(crate) fn read_json(path: &Path, case: &str) -> Result<serde_json::Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("{case}: read {} failed: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{case}: {} is not valid JSON: {error}", path.display()))
}

/// Sorted recursive inventory of relative path, entry type and SHA-256 of file
/// bytes. Directories and symlinks carry an empty digest; files carry the hex
/// digest of their bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InventoryEntry {
    relative: String,
    kind: &'static str,
    digest: String,
}

pub(crate) fn snapshot(root: &Path) -> Result<Option<Vec<InventoryEntry>>, String> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "symlink_metadata {} failed: {error}",
            root.display()
        )),
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                let bytes = fs::read(root)
                    .map_err(|error| format!("read {} failed: {error}", root.display()))?;
                return Ok(Some(vec![InventoryEntry {
                    relative: root
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| ".".to_string()),
                    kind: if metadata.file_type().is_file() {
                        "file"
                    } else {
                        "other"
                    },
                    digest: if metadata.file_type().is_file() {
                        sha256_hex(&bytes)
                    } else {
                        String::new()
                    },
                }]));
            }
            let mut entries = Vec::new();
            inventory_into(root, root, &mut entries)?;
            entries.sort_by(|lhs, rhs| lhs.relative.cmp(&rhs.relative));
            Ok(Some(entries))
        }
    }
}

fn inventory_into(root: &Path, dir: &Path, out: &mut Vec<InventoryEntry>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("read_dir {} failed: {error}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("read entry in {} failed: {error}", dir.display()))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("strip prefix failed: {error}"))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("symlink_metadata {} failed: {error}", path.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            out.push(InventoryEntry {
                relative,
                kind: "symlink",
                digest: String::new(),
            });
        } else if file_type.is_dir() {
            out.push(InventoryEntry {
                relative: relative.clone(),
                kind: "dir",
                digest: String::new(),
            });
            inventory_into(root, &path, out)?;
        } else if file_type.is_file() {
            let bytes = fs::read(&path)
                .map_err(|error| format!("read {} failed: {error}", path.display()))?;
            out.push(InventoryEntry {
                relative,
                kind: "file",
                digest: sha256_hex(&bytes),
            });
        } else {
            out.push(InventoryEntry {
                relative,
                kind: "other",
                digest: String::new(),
            });
        }
    }
    Ok(())
}

pub(crate) fn scan_canonical_for_legacy(root: &Path, case: &str) -> Result<(), String> {
    fn walk(root: &Path, dir: &Path, case: &str) -> Result<(), String> {
        let entries = fs::read_dir(dir)
            .map_err(|error| format!("{case}: read_dir {} failed: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("{case}: read entry in {} failed: {error}", dir.display())
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| format!("{case}: strip prefix failed: {error}"))?
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "{case}: symlink_metadata {} failed: {error}",
                    path.display()
                )
            })?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                if relative == "instances/legacy" {
                    return Err(format!(
                        "{case}: legacy instances directory appeared at {relative}"
                    ));
                }
                walk(root, &path, case)?;
            } else if file_type.is_file() {
                let leaf = relative.rsplit('/').next().unwrap_or(relative.as_str());
                if leaf == "payload.bin" {
                    return Err(format!(
                        "{case}: legacy payload entry appeared at {relative}"
                    ));
                }
                let bytes = fs::read(&path)
                    .map_err(|error| format!("{case}: read {} failed: {error}", path.display()))?;
                if bytes == LEGACY_PAYLOAD {
                    return Err(format!(
                        "{case}: legacy payload bytes appeared at {relative}"
                    ));
                }
                let text = String::from_utf8_lossy(&bytes);
                for token in LEGACY_TOKENS {
                    if text.contains(token) {
                        return Err(format!(
                            "{case}: legacy token {token} leaked into {relative}"
                        ));
                    }
                }
                if text.contains("legacyMarker") {
                    return Err(format!("{case}: legacyMarker leaked into {relative}"));
                }
            }
        }
        Ok(())
    }
    walk(root, root, case)
}

/// Test-local SHA-256 (FIPS 180-4). The frozen manifest forbids adding a
/// dependency for the inventory digests, and the plan names only `std`,
/// `tempfile`, `dirs` and `serde_json`.
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (bytes.len() as u64) * 8;
    let mut message = bytes.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}
