use super::*;

fn request_name(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012x}.json")
}

fn oracle(names: &[String], cursor: &str) -> Vec<String> {
    let mut ordered = names.to_vec();
    ordered.sort();
    let after: Vec<_> = ordered
        .iter()
        .filter(|name| name.as_str() > cursor)
        .cloned()
        .collect();
    if after.is_empty() {
        ordered
            .into_iter()
            .filter(|name| name.as_str() <= cursor)
            .take(MAX_PER_POLL)
            .collect()
    } else {
        after.into_iter().take(MAX_PER_POLL).collect()
    }
}

fn assert_selection(
    names: Vec<String>,
    cursor: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    for name in names.iter().rev() {
        std::fs::write(directory.path().join(name), b"")?;
    }
    for name in [
        "invalid.json",
        "00000000-0000-1000-8000-000000000001.json",
        "noise",
    ] {
        std::fs::write(directory.path().join(name), b"")?;
    }
    let identity = crate::path_identity::verify_directory(directory.path())?;
    let mut scanner = SnapshotMailboxScanner::delivery_only();
    if let Some(value) = &cursor {
        scanner.cursors.insert(identity.object_id, value.clone());
    }
    let expected = oracle(&names, cursor.as_deref().unwrap_or_default());
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        Some(expected.clone())
    );
    let expected_cursor = expected.last().cloned().or(cursor);
    assert_eq!(
        scanner.cursors.get(&identity.object_id),
        expected_cursor.as_ref()
    );
    Ok(())
}

#[test]
fn selection_matches_sorted_oracle() -> Result<(), Box<dyn std::error::Error>> {
    assert_selection(Vec::new(), None)?;
    assert_selection(Vec::new(), Some(request_name(7)))?;
    assert_selection(vec![request_name(7)], Some(request_name(7)))?;
    assert_selection(
        (1..=700).map(request_name).collect(),
        Some(request_name(350)),
    )?;
    assert_selection(
        (1..=700).map(request_name).collect(),
        Some(request_name(700)),
    )?;
    assert_selection((1..=700).map(request_name).collect(), None)?;
    assert_selection(
        vec![request_name(1), request_name(8)],
        Some(request_name(7)),
    )?;
    Ok(())
}

#[test]
fn directory_capacity_and_open_failure_preserve_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let identity = crate::path_identity::verify_directory(directory.path())?;
    let mut scanner = SnapshotMailboxScanner::delivery_only();
    let cursor = request_name(10);
    scanner.cursors.insert(identity.object_id, cursor.clone());
    let names: Vec<_> = (1..=MAX_DIRECTORY_ENTRIES).map(request_name).collect();
    for name in names.iter().rev() {
        std::fs::write(directory.path().join(name), b"")?;
    }
    let expected = oracle(&names, &cursor);
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        Some(expected.clone())
    );
    assert_eq!(scanner.cursors.get(&identity.object_id), expected.last());
    let before = scanner.cursors.clone();
    std::fs::write(directory.path().join("overflow"), b"")?;
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        None
    );
    assert_eq!(scanner.cursors, before);
    assert_eq!(
        scanner.select_requests(&directory.path().join("missing"), identity.object_id),
        None
    );
    assert_eq!(scanner.cursors, before);
    Ok(())
}

#[test]
fn empty_directory_keeps_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let identity = crate::path_identity::verify_directory(directory.path())?;
    let mut scanner = SnapshotMailboxScanner::delivery_only();
    let cursor = request_name(7);
    scanner.cursors.insert(identity.object_id, cursor.clone());
    assert_eq!(
        scanner.select_requests(directory.path(), identity.object_id),
        Some(Vec::new())
    );
    assert_eq!(scanner.cursors.get(&identity.object_id), Some(&cursor));
    Ok(())
}
