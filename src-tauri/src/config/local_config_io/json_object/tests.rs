use super::*;
use serde_json::json;
use std::cell::Cell;

fn assert_no_stage(directory: &Path) {
    let stages: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(stages.is_empty(), "unexpected stages: {stages:?}");
}

#[test]
fn e2992_p52_r1_a_duplicates_reject_before_mutation_and_publication() {
    let documents = [
        r#"{"x":1,"x":2}"#,
        r#"{"opaque":1,"opaque":2}"#,
        r#"{"opaque":{"x":1,"x":2}}"#,
        r#"{"opaque":[{"x":1,"x":2}]}"#,
        r#"{"opaque":{"x":1,"\u0078":2}}"#,
    ];
    for document in documents {
        for allow_create in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            // Dot components make an accidental canonicalization visible in the error.
            let path = temp.path().join(".").join("input.json");
            std::fs::write(&path, document).unwrap();
            let mutations = Cell::new(0);
            let publications = Cell::new(0);

            let error = update_config_json_object_with_publish(
                &path,
                allow_create,
                |_| {
                    mutations.set(mutations.get() + 1);
                    Ok(())
                },
                |_, _| {
                    publications.set(publications.get() + 1);
                    Ok(())
                },
            )
            .unwrap_err();

            assert_eq!(error, format!("duplicateJsonMember: {}", path.display()));
            assert_eq!(mutations.get(), 0);
            assert_eq!(publications.get(), 0);
            assert_eq!(std::fs::read(&path).unwrap(), document.as_bytes());
            assert_no_stage(temp.path());
        }
    }
}

#[test]
fn e2992_p52_r1_a_duplicate_cause_contains_no_member_or_value() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.json");
    let document = r#"{"opaque":"private-first","opaque":"private-second"}"#;
    std::fs::write(&path, document).unwrap();

    let error = update_config_json_object_with_publish(
        &path,
        false,
        |_| panic!("duplicate reached mutation"),
        |_, _| panic!("duplicate reached publication"),
    )
    .unwrap_err();

    assert_eq!(error, format!("duplicateJsonMember: {}", path.display()));
    for secret in ["opaque", "private-first", "private-second"] {
        assert!(!error.contains(secret), "member or value leaked");
    }
    assert_eq!(std::fs::read(&path).unwrap(), document.as_bytes());
    assert_no_stage(temp.path());
}

#[test]
fn e2992_p52_r1_a_distinct_members_and_unknown_fields_survive() {
    let documents = [
        r#"{"opaque":{"x":1},"other":{"x":2}}"#,
        r#"{"opaque":[{"x":1},{"x":2}]}"#,
        r#"{"a.b":1,"a":{"b":2}}"#,
        r#"{"x":1,"X":2}"#,
        r#"{"é":1,"e\u0301":2}"#,
    ];
    for document in documents {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input.json");
        std::fs::write(&path, document).unwrap();
        let mut expected: Value = serde_json::from_str(document).unwrap();
        expected["touched"] = json!(true);
        let mutations = Cell::new(0);
        let publications = Cell::new(0);

        let value = update_config_json_object_with_publish(
            &path,
            false,
            |object| {
                mutations.set(mutations.get() + 1);
                object.insert("touched".into(), json!(true));
                Ok(())
            },
            |stage, destination| {
                publications.set(publications.get() + 1);
                publish_temp_config(stage, destination)
            },
        )
        .unwrap();

        assert_eq!(value, expected);
        assert_eq!(mutations.get(), 1);
        assert_eq!(publications.get(), 1);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_no_stage(temp.path());
    }
}

#[test]
fn e2992_p52_r1_a_malformed_json_keeps_native_parse_error() {
    for document in [
        "{ invalid",
        r#"{"x":1,"x":2,"#,
        r#"{"x":1,"x":2} trailing"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input.json");
        std::fs::write(&path, document).unwrap();
        let native_error = serde_json::from_str::<Value>(document).unwrap_err();

        let error = update_config_json_object_with_publish(
            &path,
            false,
            |_| panic!("malformed JSON reached mutation"),
            |_, _| panic!("malformed JSON reached publication"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            format!("Failed to parse {}: {}", path.display(), native_error)
        );
        assert_eq!(std::fs::read(&path).unwrap(), document.as_bytes());
        assert_no_stage(temp.path());
    }
}

#[test]
fn e2992_p52_r1_a_nonobjects_keep_ordinary_rejection() {
    for document in ["[]", "null", "1", "true", r#""text""#] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input.json");
        std::fs::write(&path, document).unwrap();

        let error = update_config_json_object_with_publish(
            &path,
            false,
            |_| panic!("nonobject reached mutation"),
            |_, _| panic!("nonobject reached publication"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            format!("Local config {} must be a JSON object", path.display())
        );
        assert_eq!(std::fs::read(&path).unwrap(), document.as_bytes());
        assert_no_stage(temp.path());
    }
}

#[test]
fn e2992_p52_r1_a_missing_without_creation_keeps_ordinary_rejection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.json");

    let error = update_config_json_object_with_publish(
        &path,
        false,
        |_| panic!("missing file reached mutation"),
        |_, _| panic!("missing file reached publication"),
    )
    .unwrap_err();

    assert_eq!(
        error,
        format!("Local config {} does not exist", path.display())
    );
    assert!(!path.exists());
    assert_no_stage(temp.path());
}

#[test]
fn e2992_p52_r1_a_missing_with_creation_publishes_mutated_object() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.json");
    let publications = Cell::new(0);

    let value = update_config_json_object_with_publish(
        &path,
        true,
        |object| {
            assert!(object.is_empty());
            object.insert("opaque".into(), json!({"x": 1}));
            Ok(())
        },
        |stage, destination| {
            publications.set(publications.get() + 1);
            publish_temp_config(stage, destination)
        },
    )
    .unwrap();

    assert_eq!(value, json!({"opaque": {"x": 1}}));
    assert_eq!(publications.get(), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        value
    );
    assert_no_stage(temp.path());
}

#[test]
fn e2992_p52_r1_a_failed_publication_cleans_stage_and_preserves_original() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.json");
    let document = b"{ \"opaque\": { \"x\": 1 } }\n";
    std::fs::write(&path, document).unwrap();
    let publications = Cell::new(0);

    let error = update_config_json_object_with_publish(
        &path,
        false,
        |object| {
            object.insert("opaque".into(), json!({"x": 2}));
            Ok(())
        },
        |stage, destination| {
            publications.set(publications.get() + 1);
            assert_eq!(destination, path.as_path());
            assert_eq!(
                serde_json::from_slice::<Value>(&std::fs::read(stage).unwrap()).unwrap(),
                json!({"opaque": {"x": 2}})
            );
            assert_eq!(std::fs::read(destination).unwrap(), document);
            Err("forced publish failure".into())
        },
    )
    .unwrap_err();

    assert_eq!(error, "forced publish failure");
    assert_eq!(publications.get(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), document);
    assert_no_stage(temp.path());
}

#[test]
fn e2992_p52_r1_a_read_error_keeps_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.json");
    let document = [0xff, 0xfe];
    std::fs::write(&path, document).unwrap();
    let native_error = std::fs::read_to_string(&path).unwrap_err();

    let error = update_config_json_object_with_publish(
        &path,
        false,
        |_| panic!("unreadable input reached mutation"),
        |_, _| panic!("unreadable input reached publication"),
    )
    .unwrap_err();

    assert_eq!(
        error,
        format!("Failed to read {}: {}", path.display(), native_error)
    );
    assert_eq!(std::fs::read(&path).unwrap(), document);
    assert_no_stage(temp.path());
}
