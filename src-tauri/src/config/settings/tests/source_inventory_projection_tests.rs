use super::super::*;

fn fixture() -> (SourceRef, SourceMembershipTopology) {
    (
        SourceRef {
            family: SourceFamily::Agents,
            role: SourceRole::Registered,
            scope: SourceScope::Instance,
            kind: SourceKind::RegisteredInstance,
            context_root: "C".into(),
        },
        SourceMembershipTopology {
            registry: vec!["P".into(), "Q".into()],
            directories: vec![],
            slots: vec![],
            membership: vec![],
            targets: vec![],
            links: vec![],
        },
    )
}

#[test]
fn source_membership_fixed_encoding_vector_and_ordered_registry() {
    let (source, mut topology) = fixture();
    let bytes =
        source_inventory_projection("C", &source, "source-key", "context", &topology).unwrap();
    assert_eq!(
        bytes,
        include_bytes!("fixtures/p03_member_inventory_c.json")
    );
    let before = source_inventory_digest("C", &source, "source-key", "context", &topology).unwrap();
    topology.registry.reverse();
    assert_ne!(
        before,
        source_inventory_digest("C", &source, "source-key", "context", &topology).unwrap()
    );
}

#[test]
fn source_membership_sets_permute_but_objects_slots_and_source_do_not() {
    let (source, mut topology) = fixture();
    topology.directories = vec![(PathBuf::from("b"), (1, 2)), (PathBuf::from("a"), (1, 1))];
    topology.slots = vec![
        (PathBuf::from("b/.ac"), false),
        (PathBuf::from("a/.ac"), true),
    ];
    topology.links = vec![(
        PathBuf::from("a"),
        "state/sourceReferences/1/configurationRef".into(),
        source.clone(),
        "source-key".into(),
    )];
    let digest = |topology: &SourceMembershipTopology| {
        source_inventory_digest("C", &source, "source-key", "context", topology).unwrap()
    };
    let before = digest(&topology);
    topology.directories.reverse();
    topology.slots.reverse();
    assert_eq!(before, digest(&topology));
    let mut replaced = topology.clone();
    replaced.directories[0].1 .1 += 1;
    assert_ne!(before, digest(&replaced));
    let mut shifted = topology.clone();
    shifted.links[0].1 = "state/sourceReferences/0/configurationRef".into();
    assert_ne!(before, digest(&shifted));
    let mut reassigned = topology.clone();
    reassigned.links[0].2.context_root = "other".into();
    assert_eq!(
        source_inventory_projection("C", &source, "source-key", "context", &reassigned),
        Err(SourceCoordinationError::Invalid)
    );
    let mut contradictory = topology.clone();
    contradictory.slots.push((PathBuf::from("a/.ac"), false));
    assert_eq!(
        source_inventory_projection("C", &source, "source-key", "context", &contradictory),
        Err(SourceCoordinationError::Invalid)
    );
}
