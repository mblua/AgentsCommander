use super::*;

#[test]
fn prepared_identity_pair_serializer_cleanup_and_split_parity() {
    use crate::config::local_config_io::{read_config_pair_physical, PhysicalState};
    let decision_seeds = [
        None,
        Some(concat!(
            r#"{"unknown":[1],"tooling":{"lastCodingAgent":"tracked","selectionLocked":true,"#,
            r#""configurationRef":{"keep":1}}}"#
        )),
    ];
    let mut marked = serde_json::json!({
        "unknownState": null,
        "tooling": {"lastCodingAgent": "state", "codingAgents": {"a": {"unknown": 7}}}
    });
    stamp_split_marker(marked.as_object_mut().unwrap());
    let marked = marked.to_string();
    for decisions_seed in decision_seeds {
        for state_seed in [
            None,
            Some(concat!(
                r#"{"unknownState":null,"#,
                r#""tooling":{"lastCodingAgent":"state","codingAgents":{"a":{"unknown":7}}}}"#
            )),
            Some(marked.as_str()),
        ] {
            for changed in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let old_dir = temp.path().join("old");
                let new_dir = temp.path().join("new");
                std::fs::create_dir(&old_dir).unwrap();
                std::fs::create_dir(&new_dir).unwrap();
                let old = old_dir.join("config.json");
                let new = new_dir.join("config.json");
                for dir in [&old_dir, &new_dir] {
                    if let Some(bytes) = decisions_seed {
                        std::fs::write(dir.join("config.json"), bytes).unwrap();
                    }
                    if let Some(bytes) = state_seed {
                        std::fs::write(state_file_path(dir), bytes).unwrap();
                    }
                }
                let before = [
                    read_config_pair_physical(&new).unwrap(),
                    read_config_pair_physical(&state_file_path(&new_dir)).unwrap(),
                ];
                let mutate = |d: &mut JsonMap, s: &mut JsonMap| {
                    if changed {
                        d.entry("tooling")
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .unwrap()
                            .insert(
                                "configurationRef".into(),
                                serde_json::json!({"version":1,"owned":"new"}),
                            );
                        s.entry("tooling")
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .unwrap()
                            .insert("lastCodingAgent".into(), serde_json::json!("new"));
                    }
                    Ok(())
                };
                let plan = prepare_identity_reference_pair(before.clone(), mutate).unwrap();
                assert_eq!(
                    [
                        read_config_pair_physical(&new).unwrap(),
                        read_config_pair_physical(&state_file_path(&new_dir)).unwrap()
                    ],
                    before
                );
                assert_eq!(
                    std::fs::read_dir(&new_dir).unwrap().count(),
                    usize::from(decisions_seed.is_some()) + usize::from(state_seed.is_some())
                );
                let observed = std::cell::RefCell::new(Vec::new());
                update_agent_config_with_stage(
                    &old,
                    Some(&temp.path().join("journal")),
                    &|stage| {
                        let index = match stage {
                            "after_cleanup_state_publish" => 1,
                            "after_cleanup_tracked_write" => 2,
                            "after_caller_state_publish" => 3,
                            "after_caller_decisions_write" => 4,
                            _ => panic!("unknown stage"),
                        };
                        observed.borrow_mut().push((
                            index,
                            [
                                read_config_pair_physical(&old).unwrap(),
                                read_config_pair_physical(&state_file_path(&old_dir)).unwrap(),
                            ],
                        ));
                    },
                    mutate,
                )
                .unwrap();
                for (index, tuple) in observed.into_inner() {
                    assert_eq!(tuple, plan.stages()[index]);
                }
                execute_identity_reference_pair(&new, &plan).unwrap();
                assert_eq!(
                    [
                        read_config_pair_physical(&new).unwrap(),
                        read_config_pair_physical(&state_file_path(&new_dir)).unwrap()
                    ],
                    [
                        read_config_pair_physical(&old).unwrap(),
                        read_config_pair_physical(&state_file_path(&old_dir)).unwrap()
                    ]
                );
                if let PhysicalState::Bytes { bytes, .. } = &plan.stages()[4][0] {
                    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                    if decisions_seed.is_some() {
                        assert_eq!(value["unknown"], serde_json::json!([1]));
                        assert_eq!(value["tooling"]["selectionLocked"], true);
                    }
                }
            }
        }
    }
}

#[test]
fn prepared_identity_malformed_tooling_zero_write() {
    use crate::config::local_config_io::PhysicalState;
    for (d, s) in [
        (r#"{"tooling":null}"#, r#"{}"#),
        (r#"{}"#, r#"{"tooling":[]}"#),
    ] {
        assert!(prepare_identity_reference_pair(
            [
                PhysicalState::from_bytes(d.as_bytes().to_vec()),
                PhysicalState::from_bytes(s.as_bytes().to_vec())
            ],
            |_, _| panic!("cleanup must reject before caller")
        )
        .is_err());
    }
}
