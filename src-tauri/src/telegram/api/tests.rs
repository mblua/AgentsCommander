use super::*;

#[tokio::test]
async fn every_telegram_api_method_acquires_a_network_permit() {
    let network = OutboundNetwork::new_for_tests(1);
    for method in [
        TelegramApiMethod::SendMessage,
        TelegramApiMethod::GetUpdates,
        TelegramApiMethod::GetFile,
        TelegramApiMethod::DownloadFile,
        TelegramApiMethod::SendPhoto,
        TelegramApiMethod::SendDocument,
    ] {
        let permit = acquire_api_permit(&network, method).await.unwrap();
        assert_eq!(network.available_permits_for_tests(), 0);
        drop(permit);
        assert_eq!(network.available_permits_for_tests(), 1);
    }

    assert_eq!(
        network.acquired_labels_for_tests(),
        vec![
            "telegram.send_message",
            "telegram.get_updates",
            "telegram.get_file",
            "telegram.download_file",
            "telegram.send_photo",
            "telegram.send_document",
        ]
    );
}

#[test]
fn send_photo_error_redacts_fake_token() {
    let fake_token = "123456:FAKE_SECRET_TOKEN";
    let error = telegram_request_error_message(&format!(
        "request failed for https://api.telegram.org/bot{fake_token}/sendPhoto"
    ));
    let AppError::Telegram(message) = error else {
        panic!("expected Telegram error");
    };
    assert!(!message.contains(fake_token));
    assert!(message.contains("/bot***/sendPhoto"));
}

#[test]
fn send_document_error_redacts_fake_token() {
    let fake_token = "123456:FAKE_SECRET_TOKEN";
    let error = telegram_request_error_message(&format!(
        "request failed for https://api.telegram.org/bot{fake_token}/sendDocument"
    ));
    let AppError::Telegram(message) = error else {
        panic!("expected Telegram error");
    };
    assert!(!message.contains(fake_token));
    assert!(message.contains("/bot***/sendDocument"));
}
