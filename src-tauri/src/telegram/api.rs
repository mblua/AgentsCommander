use crate::errors::AppError;
use crate::telegram::redact::redact;

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
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

    let body: TelegramResponse<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

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
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

    let body: TelegramResponse<Vec<Update>> = resp
        .json()
        .await
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

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
            let from_name = msg
                .from
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
                    content: TelegramContent::Voice {
                        file_id: voice.file_id,
                    },
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
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

    let body: TelegramResponse<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

    if !body.ok {
        return Err(AppError::Telegram(
            body.description
                .unwrap_or_else(|| "getFile failed".to_string()),
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
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))?;

    if !resp.status().is_success() {
        return Err(AppError::Telegram(format!(
            "Download failed: {}",
            resp.status()
        )));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| AppError::Telegram(redact(&e.to_string())))
}

/// Telegram `sendPhoto`. Used for static images <= 10 MB whose extension
/// indicates a format Telegram renders inline (`jpg`/`jpeg`/`png`/`webp`).
/// Caller is responsible for size + extension gating and for supplying the
/// matching `mime` string; this primitive is dumb on purpose so callers can
/// swap to `send_document` for the fallback path without redoing validation.
pub async fn send_photo(
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    bytes: Vec<u8>,
    filename: &str,
    mime: &str,
    caption: Option<&str>,
) -> Result<(), AppError> {
    let url = format!("https://api.telegram.org/bot{}/sendPhoto", token);

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat_id.to_string())
        .part("photo", part);

    if let Some(c) = caption {
        form = form.text("caption", c.to_string());
    }

    let resp = client
        .post(&url)
        .multipart(form)
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
                .unwrap_or_else(|| "sendPhoto failed".to_string()),
        ));
    }

    Ok(())
}

/// Telegram `sendDocument`. Used as the fallback for files that exceed the
/// 10 MB `sendPhoto` limit or carry an extension Telegram will not render
/// as a photo. Hard upper bound (50 MB) is enforced by the caller; this
/// primitive will happily forward whatever the caller passes.
pub async fn send_document(
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    bytes: Vec<u8>,
    filename: &str,
    mime: &str,
    caption: Option<&str>,
) -> Result<(), AppError> {
    let url = format!("https://api.telegram.org/bot{}/sendDocument", token);

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| AppError::Telegram(e.to_string()))?;

    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat_id.to_string())
        .part("document", part);

    if let Some(c) = caption {
        form = form.text("caption", c.to_string());
    }

    let resp = client
        .post(&url)
        .multipart(form)
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
                .unwrap_or_else(|| "sendDocument failed".to_string()),
        ));
    }

    Ok(())
}
