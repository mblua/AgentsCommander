use super::*;

mod attachments;
mod gate;
mod history;
mod screen;

fn fanout() -> SessionIoFanout {
    SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
    )
}

fn feed(fanout: &SessionIoFanout, id: Uuid, chunks: &[&[u8]]) {
    let token = fanout.registration_token_for_test(id);
    for chunk in chunks {
        fanout.handle_output(&token, &id.to_string(), chunk.to_vec());
    }
}

fn session(fanout: &SessionIoFanout) -> Uuid {
    let id = Uuid::new_v4();
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register test session");
    id
}

const WINDOW: &str = "main";

fn activation_data(fanout: &SessionIoFanout, id: Uuid, include_history: bool) -> Vec<u8> {
    fanout
        .activate_terminal_output(id, WINDOW, include_history)
        .expect("attach")
        .expect("snapshot on attach")
        .data
}
