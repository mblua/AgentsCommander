use super::*;

/// #1005 S5 / G3: expected values captured from the BASE-COMMIT blob
/// (`git show 409b7f90:...agency-agents-roles/SKILL.md`), never from this
/// const. LF-folded first (G8) so the pin is checkout-invariant.
#[test]
fn agency_agents_roles_before_token_minimization_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    let lf = AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION.replace("\r\n", "\n");
    assert_eq!(
        lf.len(),
        2578,
        "snapshot must be the 409b7f90 blob, byte for byte"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(lf.as_bytes())),
        "9bc8a2cd565357bfeb85efac224917331e52821aba0b36ae2b831da6aaf657e5",
        "the frozen agency v2 snapshot changed; it must stay byte-identical to what shipped"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(
                normalize_role_text(AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION).as_bytes()
            )
        ),
        "2bcb95d2b57e9ff39f212e487f98195bbc66a0949b86b015bcfb1a2022445a3a",
        "the normalized agency v2 snapshot changed; it is what on-disk copies hash to"
    );
}

/// #1005 S5 failing-first proof for the audit skill's `legacy_snapshots`
/// entry (the list is non-empty for the first time). Goes through
/// `ensure_root_agent_dir_at` so the REAL `DEFAULT_ROOT_SKILLS` table is on
/// trial, not a caller-supplied list.
#[test]
fn frozen_audit_skill_is_repaired_to_current() {
    assert_ne!(
        normalize_role_text(ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION),
        normalize_role_text(default_root_skill("role-skill-boundary-audit").content),
        "S5 rewrite must change the shipped audit skill or the freeze is pointless"
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME);
    std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
    std::fs::write(&skill, ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION)
        .expect("seed old skill");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(&skill).expect("read skill"),
        default_root_skill("role-skill-boundary-audit").content,
        "pristine pre-minimization audit skill must self-repair (legacy_snapshots)"
    );
}

/// #1005 S5 failing-first proof for the agency skill's SECOND
/// `legacy_snapshots` entry, through the REAL table like the audit twin.
#[test]
fn frozen_agency_v2_skill_is_repaired_to_current() {
    assert_ne!(
        normalize_role_text(AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION),
        normalize_role_text(default_root_skill("agency-agents-roles").content),
        "S5 rewrite must change the shipped agency skill or the freeze is pointless"
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skill = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("agency-agents-roles")
        .join(SKILL_MD_FILENAME);
    std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
    std::fs::write(&skill, AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION).expect("seed old skill");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(&skill).expect("read skill"),
        default_root_skill("agency-agents-roles").content,
        "pristine pre-minimization agency skill must self-repair (legacy_snapshots)"
    );
}

#[test]
fn agency_agents_roles_pre_yaml_fix_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};

    // The raw pin. The normalized pin below is blind to exactly what normalization erases, and
    // `session_context::tests` writes this constant VERBATIM to disk as its real-world fixture,
    // so a snapshot that silently lost its trailing newline is no longer the file that test
    // claims to model.
    let lf = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace("\r\n", "\n");
    assert_eq!(
        lf.len(),
        2576,
        "snapshot must be the 646aeac blob, byte for byte"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(lf.as_bytes())),
        "2b1643fc1a290fb2c04f490a27564fe23a155ccf17ed03e9c62f31a5b399cd22",
        "the frozen pre-fix snapshot changed; it must stay byte-identical to what shipped"
    );

    let normalized = normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized.as_bytes())),
        "78f498f0ead4ae5699b207358906f409b75d235738e4cfd2769b4f289089f10d",
        "the normalized snapshot changed; it is what every on-disk broken copy hashes to"
    );
}

/// Anti-drift (#1005 S5, E5.6; replaces
/// `shipped_agency_skill_is_the_snapshot_with_a_quoted_description`, whose
/// old==new-minus-one-quote derivation could not survive a real rewrite).
/// If anyone edits a shipped `SKILL.md` without freezing a new snapshot and
/// extending that skill's `legacy_snapshots`, this fails loudly instead of
/// silently orphaning the migration. Pins the LF-folded form plus the
/// normalized form (G8): shipped content is `include_str!` and
/// `.gitattributes` has no `*.md` rule, so raw bytes differ CRLF/LF per
/// checkout.
#[test]
fn shipped_skills_are_the_pinned_bytes() {
    use sha2::{Digest, Sha256};
    for (dir, len, lf_sha, normalized_sha) in [
        (
            "role-skill-boundary-audit",
            3061,
            "af3e86a69e9a3608843242e666993fcd64a21bf53d4f45a087f421f5a1e36de7",
            "005e72afcfdf6431a8e165c426e6d27aed751c5f56a41534adfef647088acf3a",
        ),
        (
            "agency-agents-roles",
            2464,
            "30431b8b39ba27835210d4fdc78e286a2c590329877ee4d5899e8f560071fb75",
            "ebe59fd5e6607a76cb47d71349473066a448b699e92bfbaca0056c8203f44313",
        ),
    ] {
        let content = default_root_skill(dir).content;
        let lf = content.replace("\r\n", "\n");
        assert_eq!(
            lf.len(),
            len,
            "{dir}: shipped bytes changed; freeze the old bytes as a new snapshot first"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(lf.as_bytes())),
            lf_sha,
            "{dir}: shipped bytes changed; freeze the old bytes as a new snapshot first"
        );
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(normalize_role_text(content).as_bytes())
            ),
            normalized_sha,
            "{dir}: normalized shipped bytes changed; freeze a new snapshot first"
        );
    }
}
