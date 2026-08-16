//! `api-client` host CLI verb (#791 §5.4): mint / revoke / list control-plane
//! API client tokens. All subcommands require HOST AUTHORITY (the master or
//! root token) so a container (which has no master token by construction)
//! cannot self-mint. Mint binds a token to exactly one replica FQN and rejects
//! the Root Agent (§0.5 HIGH-3).

use clap::{Args, Subcommand};
use uuid::Uuid;

use crate::api::auth;

#[derive(Args)]
#[command(after_help = "\
AUTHORITY: every subcommand requires the master/root token (host authority). A \
container cannot self-mint (it has no master token by construction).\n\n\
MINT prints the secret token exactly ONCE on stdout as JSON; capture it then. \
The registry stores only a SHA-256 hash. The bound replica identity is derived \
from --root; the Root Agent is rejected (API-excluded in increment 1).")]
pub struct ApiClientArgs {
    #[command(subcommand)]
    pub command: ApiClientCommand,
}

#[derive(Subcommand)]
pub enum ApiClientCommand {
    /// Mint a new scoped, revocable API client token bound to a replica.
    Mint(MintArgs),
    /// Revoke a client by id (takes effect on the next API request).
    Revoke(RevokeArgs),
    /// List registered clients (secrets and hashes are never shown).
    List(ListArgs),
}

#[derive(Args)]
pub struct MintArgs {
    /// Host authority token (master/root). Required.
    #[arg(long)]
    pub token: Option<String>,
    /// Replica working directory to bind the token to (the identity source).
    #[arg(long)]
    pub root: String,
    /// Comma-separated scopes: send, list-peers-lean, session-transport,
    /// pty-input, terminal-snapshot. Manually minted privileged scopes never
    /// gain live authority; both require a matching automatic container binding.
    #[arg(long)]
    pub scopes: String,
    /// Optional human label.
    #[arg(long)]
    pub label: Option<String>,
    /// Optional RFC3339 expiry (e.g. 2026-12-31T00:00:00Z).
    #[arg(long)]
    pub expires: Option<String>,
}

#[derive(Args)]
pub struct RevokeArgs {
    /// Host authority token (master/root). Required.
    #[arg(long)]
    pub token: Option<String>,
    /// The clientId to revoke.
    #[arg(long = "client-id")]
    pub client_id: String,
}

#[derive(Args)]
pub struct ListArgs {
    /// Host authority token (master/root). Required.
    #[arg(long)]
    pub token: Option<String>,
}

pub fn execute(args: ApiClientArgs) -> i32 {
    match args.command {
        ApiClientCommand::Mint(a) => mint(a),
        ApiClientCommand::Revoke(a) => revoke(a),
        ApiClientCommand::List(a) => list(a),
    }
}

/// True only for the master/root token. `validate_cli_token` returns `is_root`
/// for both the persisted `root_token` and `master-token.txt`; a session UUID
/// passes shape validation but returns `is_root = false`, so it is rejected.
fn host_authority_ok(token: &Option<String>) -> bool {
    matches!(crate::cli::validate_cli_token(token), Ok((_, true)))
}

fn registry_path() -> Result<std::path::PathBuf, i32> {
    match crate::config::config_dir() {
        Some(d) => Ok(d.join(auth::REGISTRY_FILENAME)),
        None => {
            eprintln!("Error: cannot resolve the host config directory.");
            Err(1)
        }
    }
}

fn mint(a: MintArgs) -> i32 {
    if !host_authority_ok(&a.token) {
        eprintln!(
            "Error: `api-client mint` requires the master/root token (host authority). A container cannot self-mint."
        );
        return 1;
    }

    // Derive + gate the bound identity (§0.5 HIGH-3 mint-time guard).
    let bound_fqn = crate::config::teams::agent_fqn_from_path(&a.root);
    if crate::config::root_agent::is_root_agent_path(&a.root)
        || crate::config::root_agent::is_root_agent_target(&bound_fqn)
        || bound_fqn == crate::config::root_agent::ROOT_AGENT_SENDER
    {
        eprintln!(
            "Error: cannot mint an API client bound to the Root Agent; root-agent is API-excluded in increment 1."
        );
        return 1;
    }

    let scopes: Vec<String> = a
        .scopes
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if let Err(e) = auth::validate_scopes(&scopes) {
        eprintln!("Error: {}", e);
        return 1;
    }

    let path = match registry_path() {
        Ok(p) => p,
        Err(code) => return code,
    };

    let client_id = Uuid::new_v4().to_string();
    let secret = Uuid::new_v4().to_string();
    let issued_at = chrono::Utc::now().to_rfc3339();

    match auth::mint(
        &path,
        auth::MintRequest {
            client_id,
            secret,
            label: a.label.unwrap_or_default(),
            bound_root: a.root.clone(),
            bound_fqn: bound_fqn.clone(),
            scopes: scopes.clone(),
            issued_at,
            expires_at: a.expires,
            bound_session_id: None,
            credential_generation: None,
        },
    ) {
        Ok(out) => {
            crate::api::audit::record(&out.client_id, &bound_fqn, "mint", "ok");
            let output = serde_json::json!({
                "clientId": out.client_id,
                "token": out.secret,
                "boundFqn": bound_fqn,
                "boundRoot": a.root,
                "scopes": scopes,
                "note": "Store this token now; it is shown only once. The registry keeps only a hash. A manually requested pty-input scope does not grant actuation without an automatically bound live container session."
            });
            crate::cli_println!(
                "{}",
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string())
            );
            0
        }
        Err(e) => {
            eprintln!("Error: mint failed: {}", e);
            1
        }
    }
}

fn revoke(a: RevokeArgs) -> i32 {
    if !host_authority_ok(&a.token) {
        eprintln!("Error: `api-client revoke` requires the master/root token (host authority).");
        return 1;
    }
    let path = match registry_path() {
        Ok(p) => p,
        Err(code) => return code,
    };
    match auth::revoke(&path, &a.client_id) {
        Ok(true) => {
            crate::api::audit::record(&a.client_id, "-", "revoke", "ok");
            crate::cli_println!("Revoked client '{}'.", a.client_id);
            0
        }
        Ok(false) => {
            eprintln!("Error: no client with id '{}'.", a.client_id);
            1
        }
        Err(e) => {
            eprintln!("Error: revoke failed: {}", e);
            1
        }
    }
}

fn list(a: ListArgs) -> i32 {
    if !host_authority_ok(&a.token) {
        eprintln!("Error: `api-client list` requires the master/root token (host authority).");
        return 1;
    }
    let path = match registry_path() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let snapshot = auth::list_with_status(&path);
    let output = list_output(&snapshot);
    match serde_json::to_string_pretty(&output) {
        Ok(json) => {
            crate::cli_println!("{}", json);
            if snapshot.problem.is_some() {
                1
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("Error: failed to serialize client list: {}", e);
            1
        }
    }
}

fn redacted_clients(registry: &auth::ApiClientRegistry) -> Vec<serde_json::Value> {
    registry
        .clients
        .iter()
        .map(|c| {
            serde_json::json!({
                "clientId": c.client_id,
                "label": c.label,
                "boundFqn": c.bound_fqn,
                "boundRoot": c.bound_root,
                "scopes": c.scopes,
                "issuedAt": c.issued_at,
                "expiresAt": c.expires_at,
                "revoked": c.revoked,
            })
        })
        .collect()
}

fn list_output(snapshot: &auth::RegistrySnapshot) -> serde_json::Value {
    let clients = redacted_clients(&snapshot.registry);
    if let Some(problem) = &snapshot.problem {
        serde_json::json!({
            "registryStatus": problem.status,
            "error": problem.message,
            "clients": clients,
        })
    } else {
        serde_json::Value::Array(clients)
    }
}

#[cfg(test)]
mod tests {
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
}
