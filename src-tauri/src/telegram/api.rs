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

pub struct TelegramUpdate {
    pub update_id: i64,
    pub text: String,
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
            let text = msg.text?;
            Some(TelegramUpdate {
                update_id: u.update_id,
                text,
                from_name: msg
                    .from
                    .map(|f| f.first_name)
                    .unwrap_or_else(|| "Unknown".to_string()),
                chat_id: msg.chat.id,
            })
        })
        .collect();

    Ok(updates)
}
