use super::*;
use serde_json::json;

#[test]
fn p02_patch_presence_types_and_queue_canonical_wire() {
    assert!(serde_json::from_value::<AgentPatch>(json!({}))
        .unwrap()
        .command
        .is_none());
    let patch: AgentPatch =
        serde_json::from_value(json!({"baseCommand":"","command":"\t"})).unwrap();
    assert_eq!(patch.command.as_deref(), Some(""));
    let op = CodingAgentOp::Update {
        id: "own".into(),
        patch,
    };
    let wire = serde_json::to_value(&op).unwrap();
    assert_eq!(wire["patch"]["baseCommand"], "");
    assert!(wire["patch"].get("command").is_none());
    let decoded: CodingAgentOp = serde_json::from_value(wire).unwrap();
    match decoded {
        CodingAgentOp::Update { patch, .. } => assert_eq!(patch.command.as_deref(), Some("")),
        _ => panic!("update"),
    }
    for bad in [json!(null), json!(false), json!(9), json!([]), json!({})] {
        for key in ["command", "baseCommand"] {
            let mut pair = json!({"baseCommand":"new","command":"old"});
            pair[key] = bad.clone();
            assert!(serde_json::from_value::<AgentPatch>(pair).is_err());
        }
    }
}
