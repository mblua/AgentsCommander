use crate::errors::AppError;

#[derive(Debug, serde::Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}

#[derive(Debug, serde::Deserialize)]
struct Message {
    text: Option<String>,
    voice: Option<Voice>,
    chat: Chat,
    from: Option<User>,
}

#[derive(Debug, serde::Deserialize)]
struct Chat {
    id: i64,
}

#[derive(Debug, serde::Deserialize)]
struct User {
    first_name: String,
}

#[derive(Debug, serde::Deserialize)]
struct Voice {
    file_id: String,
}

pub enum TelegramContent {
    Text(String),
    Voice { file_id: String },
}

pub struct TelegramUpdate {
    pub update_id: i64,
    pub content: TelegramContent,
    pub from_name: String,
    pub chat_id: i64,
}

pub async fn send_message(
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    text: &str,
) -> Result<(), AppError> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        }))
        .send()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    let body: TelegramResponse<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    if !body.ok {
        return Err(AppError::Telegram(
            body.description
                .unwrap_or_else(|| "Unknown Telegram error".to_string()),
        ));
    }

    Ok(())
}

pub async fn get_updates(
    client: &reqwest::Client,
    token: &str,
    offset: i64,
    timeout: u64,
) -> Result<Vec<TelegramUpdate>, AppError> {
    let url = format!("https://api.telegram.org/bot{}/getUpdates", token);
    let resp = client
        .get(&url)
        .query(&[
            ("offset", offset.to_string()),
            ("timeout", timeout.to_string()),
        ])
        .send()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    let body: TelegramResponse<Vec<Update>> = resp
        .json()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    if !body.ok {
        return Err(AppError::Telegram(
            body.description
                .unwrap_or_else(|| "Unknown Telegram error".to_string()),
        ));
    }

    let updates = body
        .result
        .unwrap_or_default()
        .into_iter()
        .filter_map(|u| {
            let msg = u.message?;
            let from_name = msg.from
                .map(|f| f.first_name)
                .unwrap_or_else(|| "Unknown".to_string());
            let chat_id = msg.chat.id;
            let update_id = u.update_id;

            if let Some(text) = msg.text {
                Some(TelegramUpdate {
                    update_id,
                    content: TelegramContent::Text(text),
                    from_name,
                    chat_id,
                })
            } else if let Some(voice) = msg.voice {
                Some(TelegramUpdate {
                    update_id,
                    content: TelegramContent::Voice { file_id: voice.file_id },
                    from_name,
                    chat_id,
                })
            } else {
                None
            }
        })
        .collect();

    Ok(updates)
}

pub async fn get_file(
    client: &reqwest::Client,
    token: &str,
    file_id: &str,
) -> Result<String, AppError> {
    let url = format!("https://api.telegram.org/bot{}/getFile", token);
    let resp = client
        .get(&url)
        .query(&[("file_id", file_id)])
        .send()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    let body: TelegramResponse<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    if !body.ok {
        return Err(AppError::Telegram(
            body.description.unwrap_or_else(|| "getFile failed".to_string()),
        ));
    }

    body.result
        .and_then(|v| v.get("file_path")?.as_str().map(String::from))
        .ok_or_else(|| AppError::Telegram("Missing file_path in getFile response".to_string()))
}

pub async fn download_file(
    client: &reqwest::Client,
    token: &str,
    file_path: &str,
) -> Result<Vec<u8>, AppError> {
    let url = format!("https://api.telegram.org/file/bot{}/{}", token, file_path);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(AppError::Telegram(format!("Download failed: {}", resp.status())));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| AppError::Telegram(e.to_string()))
}
