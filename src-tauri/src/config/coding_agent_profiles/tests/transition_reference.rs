use super::*;
use crate::config::settings::{IdentityReferencePhysicalState, SourceIdentityMapping, SourceRef};

fn fixture() -> (
    SourceRef,
    [IdentityReferencePhysicalState; 2],
    Vec<SourceIdentityMapping>,
) {
    let source: SourceRef = serde_json::from_value(serde_json::json!({
        "family": "agents", "role": "registered", "scope": "instance",
        "kind": "registeredInstance", "contextRoot": "fixture-instance"
    }))
    .unwrap();
    let reference = |cid: &str, pid: &str, key: &str| {
        serde_json::json!({
            "version": 1, "source": source, "calculatedId": cid,
            "requestedProfile": "B", "profileCalculatedId": pid, "legacyEntryHint": key
        })
    };
    let first = reference("cid1:old-a", "pid1:old-a", "a");
    let second = reference("cid1:old-b", "pid1:old-b", "b");
    let third = reference("cid1:stable-c", "pid1:old-c", "c");
    let decisions = serde_json::json!({"unknown": {"null": null}, "tooling": {
        "currentCodingAgent": "a", "profile": "B", "selectionLocked": true,
        "configurationRef": first, "replicaSelectionDefault": {
            "codingAgentId": "b", "requestedProfile": "B", "selectionLocked": true,
            "configurationRef": second
        }
    }});
    let state = serde_json::json!({"unknown": [null, "keep"], "tooling": {"codingAgents": {"c": {"configurationRef": third, "unknown": null}}, "sourceReferences": [{"configurationRef": first, "unknown": true}]}});
    let physical = [decisions, state].map(|value| {
        IdentityReferencePhysicalState::from_bytes(serde_json::to_vec(&value).unwrap())
    });
    let mapping = |key: &str, before: &str, after: &str| SourceIdentityMapping {
        entry_hint: key.into(),
        old_calculated_id: Some(before.into()),
        new_calculated_id: Some(after.into()),
        unavailable: false,
        profiles: std::collections::BTreeMap::from([(
            "B".into(),
            (
                Some(format!("pid1:old-{key}")),
                Some(format!("pid1:new-{key}")),
            ),
        )]),
    };
    (
        source,
        physical,
        vec![
            mapping("a", "cid1:old-a", "cid1:new-a"),
            mapping("b", "cid1:old-b", "cid1:new-b"),
            mapping("c", "cid1:stable-c", "cid1:stable-c"),
        ],
    )
}

#[test]
fn transition_reference_batch_two_cid_pid_only_default_lock_history_unknown() {
    let (source, before, mappings) = fixture();
    let patch = plan_identity_reference_patch(before.clone(), &source, &mappings).unwrap();
    assert_eq!(patch.before, before);
    assert_eq!(
        patch.after[0].pointer("/tooling/configurationRef/calculatedId"),
        Some(&serde_json::json!("cid1:new-a"))
    );
    assert_eq!(
        patch.after[0].pointer("/tooling/replicaSelectionDefault/configurationRef/calculatedId"),
        Some(&serde_json::json!("cid1:new-b"))
    );
    assert_eq!(
        patch.after[1].pointer("/tooling/codingAgents/c/configurationRef/profileCalculatedId"),
        Some(&serde_json::json!("pid1:new-c"))
    );
    assert_eq!(
        patch.after[1].pointer("/tooling/sourceReferences/0/configurationRef/calculatedId"),
        Some(&serde_json::json!("cid1:new-a"))
    );
    let mut expected = [
        identity_reference_value(&before[0]).unwrap(),
        identity_reference_value(&before[1]).unwrap(),
    ];
    for (side, pointer, cid, pid) in [
        (0, "/tooling/configurationRef", "cid1:new-a", "pid1:new-a"),
        (
            0,
            "/tooling/replicaSelectionDefault/configurationRef",
            "cid1:new-b",
            "pid1:new-b",
        ),
        (
            1,
            "/tooling/codingAgents/c/configurationRef",
            "cid1:stable-c",
            "pid1:new-c",
        ),
        (
            1,
            "/tooling/sourceReferences/0/configurationRef",
            "cid1:new-a",
            "pid1:new-a",
        ),
    ] {
        let object = expected[side]
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap();
        object.insert("calculatedId".into(), serde_json::json!(cid));
        object.insert("profileCalculatedId".into(), serde_json::json!(pid));
    }
    assert_eq!(patch.after, expected);
}

#[test]
fn transition_reference_disabled_removed_and_other_source_preserve_every_field() {
    let (source, before, mut mappings) = fixture();
    mappings[0].unavailable = true;
    mappings[1].new_calculated_id = None;
    mappings[2]
        .profiles
        .insert("B".into(), (Some("pid1:old-c".into()), None));
    let original = [
        identity_reference_value(&before[0]).unwrap(),
        identity_reference_value(&before[1]).unwrap(),
    ];
    assert_eq!(
        plan_identity_reference_patch(before.clone(), &source, &mappings)
            .unwrap()
            .after,
        original
    );
    let mut other = source;
    other.context_root = "other-instance".into();
    assert_eq!(
        plan_identity_reference_patch(before, &other, &mappings)
            .unwrap()
            .after,
        original
    );
}

#[test]
fn transition_reference_stale_profile_conflict_leaves_input_physical_unchanged() {
    let (source, before, mut mappings) = fixture();
    let original = before.clone();
    mappings[2].profiles.insert(
        "B".into(),
        (Some("pid1:stale".into()), Some("pid1:new-c".into())),
    );
    assert!(matches!(
        plan_identity_reference_patch(before.clone(), &source, &mappings),
        Err(crate::config::settings::IdentityReferenceError::Conflict)
    ));
    assert_eq!(before, original);
}

#[test]
fn transition_reference_metadata_only_and_absent_sides_remain_unchanged() {
    let (source, before, _) = fixture();
    let original = [
        identity_reference_value(&before[0]).unwrap(),
        identity_reference_value(&before[1]).unwrap(),
    ];
    assert_eq!(
        plan_identity_reference_patch(before, &source, &[])
            .unwrap()
            .after,
        original
    );
    let absent = [
        IdentityReferencePhysicalState::Absent,
        IdentityReferencePhysicalState::Absent,
    ];
    let patch = plan_identity_reference_patch(absent.clone(), &source, &[]).unwrap();
    assert_eq!(patch.before, absent);
    assert_eq!(patch.after, [serde_json::json!({}), serde_json::json!({})]);
}
