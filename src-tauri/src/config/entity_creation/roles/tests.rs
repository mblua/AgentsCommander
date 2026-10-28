use crate::commands::entity_creation::{
    create_agent_matrix_from_role, CreateAgentMatrixFromRoleArgs,
};

#[test]
fn issue_3294_role_bytes_positive_control() {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join(".ac");
    std::fs::create_dir(&ac).unwrap();
    let bytes: &[u8] = &[239, 187, 191, 82, 111, 108, 101, 0, 255, 254, 128, 13, 10];
    let args = || CreateAgentMatrixFromRoleArgs {
        ac_root: &ac,
        safe_name: "opaque-role",
        role_bytes: bytes,
    };
    let created = create_agent_matrix_from_role(args()).unwrap();
    assert_eq!(std::fs::read(&created.role_path).unwrap(), bytes);
    let config_path = created.agent_dir.join("config.json");
    let config = std::fs::read(&config_path).unwrap();
    assert!(
        create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
            role_bytes: b"replacement",
            ..args()
        })
        .unwrap_err()
        .contains("already exists")
    );
    assert_eq!(std::fs::read(&created.role_path).unwrap(), bytes);
    assert_eq!(std::fs::read(config_path).unwrap(), config);
}
