use std::env;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use uuid::Uuid;

type HelperResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("agentscommander-api-helper error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> HelperResult<()> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err("missing command".into());
    };
    match command.as_str() {
        "list-peers-lean" => list_peers().await,
        "send" => send(args.collect()).await,
        _ => Err(format!("unknown command '{command}'").into()),
    }
}

fn api_url() -> HelperResult<String> {
    Ok(env::var("AGENTSCOMMANDER_API_URL")?
        .trim_end_matches('/')
        .to_string())
}

fn api_token() -> HelperResult<String> {
    Ok(env::var("AGENTSCOMMANDER_API_TOKEN")?)
}

async fn list_peers() -> HelperResult<()> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{}/api/v1/peers", api_url()?))
        .header(AUTHORIZATION, format!("Bearer {}", api_token()?))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(format!("list-peers-lean failed with {status}: {body}").into());
    }
    println!("{body}");
    Ok(())
}

async fn send(args: Vec<String>) -> HelperResult<()> {
    let mut to = None;
    let mut file = None;
    let mut inline = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--to" => to = iter.next(),
            "--send" => file = iter.next(),
            "--message" => inline = iter.next(),
            "--mode" => {
                let _ = iter.next();
            }
            other => return Err(format!("unknown send argument '{other}'").into()),
        }
    }
    let to = to.ok_or("send requires --to")?;
    if inline.is_some() {
        return Err("API transport v1 does not support send --message; use --send".into());
    }
    let send = file.ok_or("send requires --send")?;

    let request = json!({
        "apiVersion": "1",
        "opId": Uuid::new_v4().to_string(),
        "to": to,
        "message": {
            "send": send
        }
    });
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/api/v1/send", api_url()?))
        .header(AUTHORIZATION, format!("Bearer {}", api_token()?))
        .header(CONTENT_TYPE, "application/json")
        .json(&request)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(format!("send failed with {status}: {text}").into());
    }
    println!("{text}");
    Ok(())
}
