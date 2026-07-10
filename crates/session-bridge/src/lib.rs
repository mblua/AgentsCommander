use std::env;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

pub const TRANSPORT_PROTOCOL_VERSION: u32 = 2;
const REGISTRATION_TOKEN_ENV: &str = "AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN";

type BridgeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum HostToBridgeTextFrame {
    Resize { version: u32, cols: u16, rows: u16 },
    Terminate { version: u32 },
    Ping { version: u32 },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum BridgeToHostTextFrame {
    Hello {
        version: u32,
        #[serde(rename = "sessionId")]
        session_id: Uuid,
        root: String,
    },
    Status {
        version: u32,
        status: Option<String>,
    },
    Exit {
        version: u32,
        code: i32,
    },
    Pong {
        version: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub api_url: String,
    pub api_token: String,
    pub session_id: Uuid,
    pub registration_ticket: String,
    pub host_root: String,
    pub workdir: String,
    pub command: String,
    pub args: Vec<String>,
    pub child_env: Vec<(String, String)>,
    pub env_unset: Vec<String>,
    pub cols: u16,
    pub rows: u16,
}

impl BridgeConfig {
    pub fn from_env() -> BridgeResult<Self> {
        let api_url = required_env("AGENTSCOMMANDER_API_URL")?;
        let api_token = required_env("AGENTSCOMMANDER_API_TOKEN")?;
        let session_id = required_env("AGENTSCOMMANDER_SESSION_ID")?.parse::<Uuid>()?;
        let registration_ticket = required_env("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN")?;
        let host_root = required_env("AGENTSCOMMANDER_ROOT")?;
        let workdir =
            env::var("AGENTSCOMMANDER_BRIDGE_WORKDIR").unwrap_or_else(|_| "/workspace".to_string());
        let command = required_env("AGENTSCOMMANDER_BRIDGE_COMMAND")?;
        let args = parse_json_env("AGENTSCOMMANDER_BRIDGE_ARGS_JSON")?.unwrap_or_default();
        let child_env = parse_json_env("AGENTSCOMMANDER_BRIDGE_ENV_JSON")?.unwrap_or_default();
        let env_unset =
            parse_json_env("AGENTSCOMMANDER_BRIDGE_ENV_UNSET_JSON")?.unwrap_or_default();
        let cols = parse_u16_env("AGENTSCOMMANDER_BRIDGE_COLS", 120)?;
        let rows = parse_u16_env("AGENTSCOMMANDER_BRIDGE_ROWS", 30)?;

        Ok(Self {
            api_url,
            api_token,
            session_id,
            registration_ticket,
            host_root,
            workdir,
            command,
            args,
            child_env,
            env_unset,
            cols,
            rows,
        })
    }

    pub fn transport_url(&self) -> String {
        let base = self.api_url.trim_end_matches('/');
        let scheme = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{}", rest)
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{}", rest)
        } else {
            format!("ws://{}", base)
        };
        format!(
            "{}/api/v1/session-transport?sessionId={}&ticket={}",
            scheme, self.session_id, self.registration_ticket
        )
    }
}

fn required_env(key: &str) -> BridgeResult<String> {
    let value = env::var(key)?;
    if value.trim().is_empty() {
        Err(format!("{key} is empty").into())
    } else {
        Ok(value)
    }
}

fn parse_json_env<T>(key: &str) -> BridgeResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(serde_json::from_str(&value)?)),
        _ => Ok(None),
    }
}

fn parse_u16_env(key: &str, fallback: u16) -> BridgeResult<u16> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value.parse()?),
        _ => Ok(fallback),
    }
}

pub async fn run_from_env() -> BridgeResult<()> {
    run_bridge(BridgeConfig::from_env()?).await
}

pub async fn run_bridge(config: BridgeConfig) -> BridgeResult<()> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: config.rows,
        cols: config.cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new(&config.command);
    for arg in &config.args {
        command.arg(arg);
    }
    command.cwd(&config.workdir);
    command.env("TERM", "xterm-256color");
    apply_bridge_env(&mut command, &config.child_env, &config.env_unset);

    let child = pair.slave.spawn_command(command)?;
    let child = Arc::new(Mutex::new(child));
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
    let mut reader = pair.master.try_clone_reader()?;

    let (pty_tx, mut pty_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if pty_tx.blocking_send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let (exit_tx, mut exit_rx) = mpsc::channel::<i32>(1);
    {
        let child = Arc::clone(&child);
        std::thread::spawn(move || loop {
            let status = {
                let mut child = child.lock().unwrap();
                child.try_wait()
            };
            match status {
                Ok(Some(status)) => {
                    let _ = exit_tx.blocking_send(status.exit_code() as i32);
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => {
                    let _ = exit_tx.blocking_send(1);
                    break;
                }
            }
        });
    }

    let mut request = config.transport_url().into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.api_token))?,
    );
    let (ws, _) = connect_async(request).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    let hello = BridgeToHostTextFrame::Hello {
        version: TRANSPORT_PROTOCOL_VERSION,
        session_id: config.session_id,
        root: config.host_root.clone(),
    };
    ws_tx
        .send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await?;

    loop {
        tokio::select! {
            Some(data) = pty_rx.recv() => {
                ws_tx.send(Message::Binary(data.into())).await?;
            }
            Some(code) = exit_rx.recv() => {
                let frame = BridgeToHostTextFrame::Exit {
                    version: TRANSPORT_PROTOCOL_VERSION,
                    code,
                };
                let _ = ws_tx.send(Message::Text(serde_json::to_string(&frame)?.into())).await;
                break;
            }
            incoming = ws_rx.next() => {
                let Some(message) = incoming else {
                    break;
                };
                if !handle_host_message(message?, pair.master.as_ref(), &writer, &child).await? {
                    break;
                }
            }
        }
    }

    terminate_child(&child);
    Ok(())
}

trait BridgeEnvTarget {
    fn set_env(&mut self, key: &str, value: &str);
    fn remove_env(&mut self, key: &str);
}

impl BridgeEnvTarget for CommandBuilder {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }

    fn remove_env(&mut self, key: &str) {
        self.env_remove(key);
    }
}

fn apply_bridge_env<T: BridgeEnvTarget>(
    target: &mut T,
    child_env: &[(String, String)],
    env_unset: &[String],
) {
    apply_bridge_env_inner(target, child_env, env_unset, true);
}

fn apply_bridge_env_inner<T: BridgeEnvTarget>(
    target: &mut T,
    child_env: &[(String, String)],
    env_unset: &[String],
    assert_disjoint: bool,
) {
    if assert_disjoint {
        debug_assert!(
            !env_unset
                .iter()
                .any(|unset| child_env.iter().any(|(key, _)| key == unset)),
            "env_unset and child_env should be disjoint"
        );
    }
    for (key, value) in child_env {
        target.set_env(key, value);
    }
    for key in env_unset {
        target.remove_env(key);
    }
    target.remove_env(REGISTRATION_TOKEN_ENV);
}

async fn handle_host_message(
    message: Message,
    master: &(dyn portable_pty::MasterPty + Send),
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    child: &Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
) -> BridgeResult<bool> {
    match message {
        Message::Binary(data) => {
            writer.lock().unwrap().write_all(&data)?;
            Ok(true)
        }
        Message::Text(text) => {
            let frame: HostToBridgeTextFrame = serde_json::from_str(text.as_str())?;
            match frame {
                HostToBridgeTextFrame::Resize {
                    version,
                    cols,
                    rows,
                } if version == TRANSPORT_PROTOCOL_VERSION => {
                    master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                    Ok(true)
                }
                HostToBridgeTextFrame::Terminate { version }
                    if version == TRANSPORT_PROTOCOL_VERSION =>
                {
                    terminate_child(child);
                    Ok(false)
                }
                HostToBridgeTextFrame::Ping { version }
                    if version == TRANSPORT_PROTOCOL_VERSION =>
                {
                    Ok(true)
                }
                _ => Ok(false),
            }
        }
        Message::Ping(_) | Message::Pong(_) => Ok(true),
        Message::Close(_) => Ok(false),
        Message::Frame(_) => Ok(true),
    }
}

fn terminate_child(child: &Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>) {
    if let Ok(mut child) = child.lock() {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    impl BridgeEnvTarget for BTreeMap<String, String> {
        fn set_env(&mut self, key: &str, value: &str) {
            self.insert(key.to_string(), value.to_string());
        }

        fn remove_env(&mut self, key: &str) {
            self.remove(key);
        }
    }

    #[test]
    fn transport_url_maps_http_to_ws() {
        let config = BridgeConfig {
            api_url: "http://host.docker.internal:8765/".to_string(),
            api_token: "secret".to_string(),
            session_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            registration_ticket: "ticket".to_string(),
            host_root: "C:/root".to_string(),
            workdir: "/workspace".to_string(),
            command: "sh".to_string(),
            args: Vec::new(),
            child_env: Vec::new(),
            env_unset: Vec::new(),
            cols: 80,
            rows: 24,
        };

        assert_eq!(
            config.transport_url(),
            "ws://host.docker.internal:8765/api/v1/session-transport?sessionId=11111111-1111-4111-8111-111111111111&ticket=ticket"
        );
    }

    #[test]
    fn protocol_frames_match_host_shape() {
        let hello = BridgeToHostTextFrame::Hello {
            version: TRANSPORT_PROTOCOL_VERSION,
            session_id: Uuid::nil(),
            root: "C:/root".to_string(),
        };
        let json = serde_json::to_value(hello).unwrap();
        assert_eq!(json["type"], "hello");
        assert_eq!(json["sessionId"], Uuid::nil().to_string());

        let resize: HostToBridgeTextFrame = serde_json::from_value(serde_json::json!({
            "type": "resize",
            "version": TRANSPORT_PROTOCOL_VERSION,
            "cols": 100,
            "rows": 40
        }))
        .unwrap();
        assert_eq!(
            resize,
            HostToBridgeTextFrame::Resize {
                version: TRANSPORT_PROTOCOL_VERSION,
                cols: 100,
                rows: 40
            }
        );
    }

    #[test]
    fn env_unset_applies_after_child_env() {
        let mut env = BTreeMap::from([("CODEX_HOME".to_string(), "/opt/codex".to_string())]);
        apply_bridge_env_inner(
            &mut env,
            &[("CODEX_HOME".to_string(), "/workspace/.codex".to_string())],
            &["CODEX_HOME".to_string()],
            false,
        );

        assert_eq!(env.get("CODEX_HOME"), None);
    }

    #[test]
    fn registration_token_is_removed_after_child_env() {
        let mut env = BTreeMap::new();
        apply_bridge_env(
            &mut env,
            &[(
                REGISTRATION_TOKEN_ENV.to_string(),
                "should-not-survive".to_string(),
            )],
            &[],
        );

        assert_eq!(env.get(REGISTRATION_TOKEN_ENV), None);
    }
}
