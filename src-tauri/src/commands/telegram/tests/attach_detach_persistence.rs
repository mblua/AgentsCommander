/// Issue #295 — full command regression needs a Tauri AppHandle fixture with
/// managed SessionManager/PtyManager/TelegramBridgeState and an injectable
/// sessions.json failure path. Keep the harness shape next to the command
/// code so the ordering contract is explicit until such a fixture exists.
#[test]
#[ignore = "integration: needs Tauri AppHandle + injectable sessions persistence failure"]
fn telegram_attach_detach_persistence_ordering_harness_note() {
    // Fixture shape:
    //   1. Start with a live session and one configured Telegram bot.
    //   2. Make attach persistence fail after TelegramBridgeManager::attach
    //      succeeds; assert telegram_attach returns Err, emits
    //      telegram_bridge_error, rolls back the live bridge, and clears
    //      Session.telegram_bot_id.
    //   3. Let attach persist successfully, then start detach after attach
    //      returns Ok; assert telegram_detach returns Ok only after the
    //      serialized snapshot no longer contains telegramBotId.
    //   4. Assert the final bridge list is empty and the final persisted
    //      row has no telegramBotId.
}
