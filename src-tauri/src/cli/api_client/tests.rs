use super::*;
use clap::Parser;

#[test]
fn mint_parses_scopes_and_root() {
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "api-client",
        "mint",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "C:/proj/.ac/wg-1/__agent_dev",
        "--scopes",
        "send,list-peers-lean",
        "--label",
        "docker:agent-1",
    ])
    .expect("api-client mint should parse");
    match parsed.command {
        Some(crate::cli::Commands::ApiClient(ApiClientArgs {
            command: ApiClientCommand::Mint(a),
        })) => {
            assert_eq!(a.scopes, "send,list-peers-lean");
            assert_eq!(a.root, "C:/proj/.ac/wg-1/__agent_dev");
            assert_eq!(a.label.as_deref(), Some("docker:agent-1"));
        }
        _ => panic!("expected api-client mint"),
    }
}

#[test]
fn revoke_parses_client_id() {
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "api-client",
        "revoke",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--client-id",
        "abc",
    ])
    .expect("api-client revoke should parse");
    match parsed.command {
        Some(crate::cli::Commands::ApiClient(ApiClientArgs {
            command: ApiClientCommand::Revoke(a),
        })) => assert_eq!(a.client_id, "abc"),
        _ => panic!("expected api-client revoke"),
    }
}

#[test]
fn list_output_surfaces_malformed_registry_status() {
    let snapshot = auth::RegistrySnapshot {
        registry: auth::ApiClientRegistry::default(),
        problem: Some(auth::RegistryLoadProblem {
            status: "malformed",
            message: "api-clients.json is malformed: expected value".to_string(),
        }),
    };

    let output = list_output(&snapshot);

    assert_eq!(output["registryStatus"], "malformed");
    assert!(output["error"].as_str().unwrap().contains("malformed"));
    assert_eq!(output["clients"].as_array().unwrap().len(), 0);
}
