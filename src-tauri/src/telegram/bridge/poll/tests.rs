use super::*;
use crate::config::settings::TelegramNetworkPollErrorLogging;
use std::future::Future;
use std::pin::Pin;

#[derive(Default)]
struct DeliveryTrace {
    calls: Vec<String>,
    prompt_state: String,
}

struct RecordingRuntime {
    trace: Arc<Mutex<DeliveryTrace>>,
    injection_fails: bool,
}

impl TelegramRuntime for RecordingRuntime {
    fn poll_error_policy<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = TelegramNetworkPollErrorLogging> + Send + 'a>> {
        Box::pin(async { panic!("delivery must not reload poll policy") })
    }

    fn voice_settings<'a>(&'a self) -> Pin<Box<dyn Future<Output = (String, String)> + Send + 'a>> {
        Box::pin(async { panic!("text delivery must not read voice settings") })
    }

    fn inject_text<'a>(
        &'a self,
        id: Uuid,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.trace
                .lock()
                .unwrap()
                .calls
                .push(format!("inject:{id}:{text}"));
            if self.injection_fails {
                Err("synthetic injection failure".into())
            } else {
                Ok(())
            }
        })
    }

    fn note_user_message<'a>(&'a self, id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.trace.lock().unwrap().calls.push(format!("note:{id}"));
        })
    }

    fn set_last_prompt<'a>(
        &'a self,
        session_id_str: &'a str,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let mut trace = self.trace.lock().unwrap();
            assert_eq!(trace.prompt_state, "changed during incoming");
            trace.calls.push(format!("set:{session_id_str}:{prompt}"));
            if Uuid::parse_str(session_id_str).is_ok() {
                trace.prompt_state = prompt.into();
            }
        })
    }

    fn transcribe_audio<'a>(
        &'a self,
        _network: &'a OutboundNetwork,
        _audio: &'a [u8],
        _key: &'a str,
        _model: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async { panic!("text delivery must not transcribe") })
    }
}

struct RecordingEvents(Arc<Mutex<DeliveryTrace>>);

impl TelegramEvents for RecordingEvents {
    fn bridge_error(&self, _session_id: &str, _error: &str, _kind: Option<&str>) {
        panic!("delivery injection failures are logged, not emitted");
    }

    fn incoming(&self, session_id: &str, text: &str, from: &str) {
        let mut trace = self.0.lock().unwrap();
        trace
            .calls
            .push(format!("incoming:{session_id}:{text}:{from}"));
        trace.prompt_state = "changed during incoming".into();
    }

    fn last_prompt(&self, session_id: &str, text: &str) {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("last:{session_id}:{text}"));
    }
}

async fn check_delivery(injection_fails: bool, valid_session: bool) {
    let trace = Arc::new(Mutex::new(DeliveryTrace::default()));
    let runtime = RecordingRuntime {
        trace: Arc::clone(&trace),
        injection_fails,
    };
    let event_port = RecordingEvents(Arc::clone(&trace));
    let id = Uuid::new_v4();
    let session = if valid_session {
        id.to_string()
    } else {
        "inválido-ñ".into()
    };
    let text = "Hola ñ 🦀\nsegunda línea";
    let from = "María 🐴";
    let mut logger = BridgeLogger::new(&session);

    deliver_text(&runtime, &event_port, id, &session, text, from, &mut logger).await;

    let trace = trace.lock().unwrap();
    assert_eq!(
        trace.calls,
        vec![
            format!("inject:{id}:{text}"),
            format!("note:{id}"),
            format!("incoming:{session}:{text}:{from}"),
            format!("set:{session}:[TG] {text}"),
            format!("last:{session}:[TG] {text}"),
        ]
    );
    assert_eq!(
        trace.prompt_state,
        if valid_session {
            format!("[TG] {text}")
        } else {
            "changed during incoming".into()
        }
    );
}

#[tokio::test]
async fn delivery_preserves_order_and_observes_incoming_state_change() {
    check_delivery(false, true).await;
}

#[tokio::test]
async fn delivery_continues_after_injection_error() {
    check_delivery(true, true).await;
}

#[tokio::test]
async fn delivery_emits_final_event_for_invalid_session_identity() {
    check_delivery(false, false).await;
}
