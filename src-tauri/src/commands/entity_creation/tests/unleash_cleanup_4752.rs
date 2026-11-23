// Each delete helper clears the Unleash flags of what it removed.
const CALL: &str = "crate::config::unleashed_replicas::forget_under(";

// Text of `fn <name>` up to the next top-level `fn`.
fn fn_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).expect(signature);
    let rest = &source[start..];
    let end = ["\npub(crate) fn ", "\npub(super) fn ", "\npub fn ", "\nfn "]
        .iter()
        .filter_map(|marker| rest[1..].find(marker).map(|i| i + 1))
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn delete_helpers_forget_unleash_flags_once() {
    let cases = [
        (
            include_str!("../workgroup_delete.rs"),
            "pub(crate) fn try_atomic_delete_wg(",
        ),
        (
            include_str!("../workgroup.rs"),
            "pub(crate) fn remove_replica_dir(",
        ),
        (
            include_str!("../agent_matrix.rs"),
            "pub(super) fn remove_staged_agent_delete_targets(",
        ),
    ];
    for (source, signature) in cases {
        assert_eq!(
            fn_body(source, signature).matches(CALL).count(),
            1,
            "{signature}"
        );
    }
}
