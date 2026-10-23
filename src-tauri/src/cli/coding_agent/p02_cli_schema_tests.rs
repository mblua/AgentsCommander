use super::*;
use serde_json::json;

#[test]
fn p02_direct_and_queued_updates_share_canonical_serializer_and_effect() {
    let settings = AppSettings {
        agents: vec![serde_json::from_value(
            json!({"id":"own","label":"Own","color":"#123456","command":"tool --before"}),
        )
        .unwrap()],
        ..AppSettings::default()
    };
    let op: CodingAgentOp = serde_json::from_value(
        json!({"kind":"update","id":"own","patch":{"command":"tool --after"}}),
    )
    .unwrap();
    let request = CodingAgentRequest {
        request_id: "p02".into(),
        created_at_ms: 1,
        op: op.clone(),
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["op"]["patch"]["baseCommand"], "tool --after");
    assert!(wire["op"]["patch"].get("command").is_none());
    let queued: CodingAgentRequest = serde_json::from_value(wire).unwrap();
    let direct =
        apply_and_save_direct(settings.clone(), &op, |settings| Ok(settings.clone())).unwrap();
    let queued =
        apply_and_save_direct(settings, &queued.op, |settings| Ok(settings.clone())).unwrap();
    assert_eq!(direct.agent.as_ref().unwrap().command, "tool --after");
    assert_eq!(
        serde_json::to_value(direct.agent).unwrap(),
        serde_json::to_value(queued.agent).unwrap()
    );
}
