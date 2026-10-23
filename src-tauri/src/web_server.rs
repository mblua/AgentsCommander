//! Web and API server lifecycle handles managed by the Tauri app.

use std::future::Future;
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::shutdown::ShutdownSignal;
use futures_util::FutureExt;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::web;

pub(crate) const WEB_SERVER_STOP_TIMEOUT_MS: u64 = 5_000;
pub(crate) const WEB_SERVER_START_CANCELLED: &str = "Web server start cancelled by stop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServerLifecycle {
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebServerLifecycleSnapshot {
    pub generation: Option<u64>,
    pub revision: u64,
    pub lifecycle: WebServerLifecycle,
    pub endpoint: Option<(String, u16)>,
}

pub(crate) struct WebServerGeneration {
    generation: u64,
    revision: u64,
    lifecycle: WebServerLifecycle,
    endpoint: Option<(String, u16)>,
    generation_token: CancellationToken,
    shutdown: ShutdownSignal,
    admission: Arc<web::WebSocketAdmission>,
    start_result: watch::Receiver<Option<Result<bool, String>>>,
    stop_result: watch::Receiver<Option<Result<(), String>>>,
    stop_deadline: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct WebServerControl {
    next_generation: u64,
    stopped_revision: u64,
    last_generation: Option<u64>,
    current: Option<WebServerGeneration>,
}

#[derive(Clone)]
pub struct WebServerStartWaiter {
    pub(crate) receiver: watch::Receiver<Option<Result<bool, String>>>,
}

impl WebServerStartWaiter {
    pub async fn wait(mut self) -> Result<bool, String> {
        loop {
            if let Some(result) = self.receiver.borrow().clone() {
                return result;
            }
            self.receiver.changed().await.map_err(|_| {
                "Web server start supervisor channel closed before terminal result".to_string()
            })?;
        }
    }
}

#[derive(Clone)]
pub struct WebServerStopWaiter {
    pub(crate) receiver: watch::Receiver<Option<Result<(), String>>>,
    pub(crate) deadline: Instant,
}

impl WebServerStopWaiter {
    pub async fn wait(mut self) -> Result<(), String> {
        let wait = async {
            loop {
                if let Some(result) = self.receiver.borrow().clone() {
                    return result;
                }
                self.receiver.changed().await.map_err(|_| {
                    "Web server stop supervisor channel closed before terminal state".to_string()
                })?;
            }
        };
        tokio::time::timeout_at(self.deadline.into(), wait)
            .await
            .map_err(|_| "Timed out waiting for web server generation to stop".to_string())?
    }

    #[cfg(test)]
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

#[derive(Clone, Default)]
pub struct WebServerHandle {
    inner: Arc<Mutex<WebServerControl>>,
    /// §1453: ultimo arranque fallido (autostart o comando). Lo consume
    /// get_web_server_owned_status; lo limpian el start exitoso y el stop.
    bind_failure: Arc<std::sync::Mutex<Option<crate::web::StartServerError>>>,
    #[cfg(test)]
    start_output_gate: Arc<Mutex<Option<WebServerStartOutputGate>>>,
}

#[cfg(test)]
pub(crate) struct WebServerStartOutputGate {
    reached: tokio::sync::oneshot::Sender<bool>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl WebServerHandle {
    pub(crate) fn begin_start<F, Fut>(
        &self,
        shutdown: ShutdownSignal,
        factory: F,
    ) -> WebServerStartWaiter
    where
        F: FnOnce(u64, Arc<web::WebSocketAdmission>, CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<tauri::async_runtime::JoinHandle<()>>, String>>
            + Send
            + 'static,
    {
        let mut factory = Some(factory);
        let mut supervisor = None;
        let waiter = {
            let mut inner = self.inner.lock().unwrap();
            match inner.current.as_ref() {
                Some(generation) if generation.lifecycle == WebServerLifecycle::Starting => {
                    WebServerStartWaiter {
                        receiver: generation.start_result.clone(),
                    }
                }
                Some(generation) if generation.lifecycle == WebServerLifecycle::Running => {
                    Self::completed_start_waiter(Ok(true))
                }
                Some(_) => Self::completed_start_waiter(Err(WEB_SERVER_START_CANCELLED.into())),
                None => {
                    inner.next_generation = inner
                        .next_generation
                        .checked_add(1)
                        .expect("web server generation id exhausted");
                    let generation = inner.next_generation;
                    let revision = inner
                        .stopped_revision
                        .checked_add(1)
                        .expect("web server lifecycle revision exhausted");
                    let generation_token = CancellationToken::new();
                    let admission =
                        Arc::new(web::WebSocketAdmission::new(generation_token.clone()));
                    let (start_sender, start_result) = watch::channel(None);
                    let (stop_sender, stop_result) = watch::channel(None);
                    let result = WebServerStartWaiter {
                        receiver: start_result.clone(),
                    };
                    inner.current = Some(WebServerGeneration {
                        generation,
                        revision,
                        lifecycle: WebServerLifecycle::Starting,
                        endpoint: None,
                        generation_token: generation_token.clone(),
                        shutdown: shutdown.clone(),
                        admission: Arc::clone(&admission),
                        start_result,
                        stop_result,
                        stop_deadline: None,
                    });
                    supervisor = Some((
                        generation,
                        admission,
                        generation_token,
                        shutdown,
                        start_sender,
                        stop_sender,
                        factory.take().expect("new generation owns start factory"),
                    ));
                    result
                }
            }
        };

        if let Some((
            generation,
            admission,
            generation_token,
            shutdown,
            start_sender,
            stop_sender,
            factory,
        )) = supervisor
        {
            let handle = self.clone();
            tauri::async_runtime::spawn(async move {
                handle
                    .supervise_generation(
                        generation,
                        admission,
                        generation_token,
                        shutdown,
                        start_sender,
                        stop_sender,
                        factory,
                    )
                    .await;
            });
        }

        waiter
    }

    fn completed_start_waiter(result: Result<bool, String>) -> WebServerStartWaiter {
        let (_sender, receiver) = watch::channel(Some(result));
        WebServerStartWaiter { receiver }
    }

    pub(crate) fn begin_stop(&self) -> Option<WebServerStopWaiter> {
        self.begin_stop_with_timeout(Duration::from_millis(WEB_SERVER_STOP_TIMEOUT_MS))
    }

    pub(crate) fn begin_stop_with_timeout(&self, timeout: Duration) -> Option<WebServerStopWaiter> {
        let mut inner = self.inner.lock().unwrap();
        let Some(generation) = inner.current.as_mut() else {
            self.clear_bind_failure();
            return None;
        };
        if matches!(
            generation.lifecycle,
            WebServerLifecycle::Starting | WebServerLifecycle::Running
        ) {
            generation.lifecycle = WebServerLifecycle::Stopping;
            generation.revision = generation
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
            generation.admission.close();
            generation.generation_token.cancel();
        }
        let deadline = *generation
            .stop_deadline
            .get_or_insert_with(|| Instant::now() + timeout);
        Some(WebServerStopWaiter {
            receiver: generation.stop_result.clone(),
            deadline,
        })
    }

    pub fn publish_effective_endpoint(&self, generation: u64, bind: String, port: u16) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return false;
        };
        if current.generation != generation
            || !matches!(
                current.lifecycle,
                WebServerLifecycle::Starting | WebServerLifecycle::Stopping
            )
        {
            return false;
        }
        let endpoint = (bind, port);
        if current.endpoint.as_ref() != Some(&endpoint) {
            current.endpoint = Some(endpoint);
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
        }
        current.lifecycle == WebServerLifecycle::Starting
    }

    pub fn snapshot(&self) -> WebServerLifecycleSnapshot {
        let inner = self.inner.lock().unwrap();
        Self::snapshot_locked(&inner)
    }

    pub fn snapshot_is_current(&self, snapshot: &WebServerLifecycleSnapshot) -> bool {
        let inner = self.inner.lock().unwrap();
        Self::snapshot_locked(&inner) == *snapshot
    }

    fn snapshot_locked(inner: &WebServerControl) -> WebServerLifecycleSnapshot {
        match inner.current.as_ref() {
            Some(generation) => WebServerLifecycleSnapshot {
                generation: Some(generation.generation),
                revision: generation.revision,
                lifecycle: generation.lifecycle,
                endpoint: generation.endpoint.clone(),
            },
            None => WebServerLifecycleSnapshot {
                generation: inner.last_generation,
                revision: inner.stopped_revision,
                lifecycle: WebServerLifecycle::Stopped,
                endpoint: None,
            },
        }
    }

    fn publish_running(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return false;
        };
        if current.generation != generation
            || current.lifecycle != WebServerLifecycle::Starting
            || current.endpoint.is_none()
            || current.generation_token.is_cancelled()
            || current.shutdown.is_cancelled()
        {
            return false;
        }

        current.lifecycle = WebServerLifecycle::Running;
        current.revision = current
            .revision
            .checked_add(1)
            .expect("web server lifecycle revision exhausted");
        if current.admission.open(&current.shutdown) {
            true
        } else {
            current.lifecycle = WebServerLifecycle::Stopping;
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
            current.admission.close();
            current.generation_token.cancel();
            false
        }
    }

    pub(crate) fn move_generation_to_stopping(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return false;
        };
        if current.generation != generation {
            return false;
        }
        if current.lifecycle != WebServerLifecycle::Stopping {
            current.lifecycle = WebServerLifecycle::Stopping;
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
        }
        current.admission.close();
        current.generation_token.cancel();
        true
    }

    fn transition_start_result_to_stopping(
        &self,
        generation: u64,
        result_if_starting: Result<bool, String>,
    ) -> Result<bool, String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return Err(WEB_SERVER_START_CANCELLED.to_string());
        };
        if current.generation != generation {
            return Err(WEB_SERVER_START_CANCELLED.to_string());
        }

        let result = if current.lifecycle == WebServerLifecycle::Starting {
            result_if_starting
        } else {
            Err(WEB_SERVER_START_CANCELLED.to_string())
        };
        if current.lifecycle != WebServerLifecycle::Stopping {
            current.lifecycle = WebServerLifecycle::Stopping;
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
        }
        current.admission.close();
        current.generation_token.cancel();
        result
    }

    #[cfg(test)]
    pub(crate) fn gate_next_start_output(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<bool>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached_sender, reached_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let previous = self
            .start_output_gate
            .lock()
            .unwrap()
            .replace(WebServerStartOutputGate {
                reached: reached_sender,
                release: release_receiver,
            });
        assert!(
            previous.is_none(),
            "only one start-output gate may be installed"
        );
        (reached_receiver, release_sender)
    }

    #[cfg(test)]
    async fn pause_after_start_output(&self, generation: u64) {
        let gate = self.start_output_gate.lock().unwrap().take();
        let Some(gate) = gate else {
            return;
        };
        let was_starting = {
            let inner = self.inner.lock().unwrap();
            inner.current.as_ref().is_some_and(|current| {
                current.generation == generation
                    && current.lifecycle == WebServerLifecycle::Starting
            })
        };
        let _ = gate.reached.send(was_starting);
        let _ = gate.release.await;
    }

    pub(crate) fn finish_generation(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_ref() else {
            return false;
        };
        if current.generation != generation {
            return false;
        }
        let revision = current
            .revision
            .checked_add(1)
            .expect("web server lifecycle revision exhausted");
        if current.stop_deadline.is_some() {
            self.clear_bind_failure();
        }
        inner.current = None;
        inner.last_generation = Some(generation);
        inner.stopped_revision = revision;
        true
    }

    #[allow(clippy::too_many_arguments)]
    async fn supervise_generation<F, Fut>(
        &self,
        generation: u64,
        admission: Arc<web::WebSocketAdmission>,
        generation_token: CancellationToken,
        shutdown: ShutdownSignal,
        start_sender: watch::Sender<Option<Result<bool, String>>>,
        stop_sender: watch::Sender<Option<Result<(), String>>>,
        factory: F,
    ) where
        F: FnOnce(u64, Arc<web::WebSocketAdmission>, CancellationToken) -> Fut,
        Fut: Future<Output = Result<Option<tauri::async_runtime::JoinHandle<()>>, String>>,
    {
        let start_future = AssertUnwindSafe(factory(
            generation,
            Arc::clone(&admission),
            generation_token.clone(),
        ))
        .catch_unwind();
        tokio::pin!(start_future);

        let start_output = tokio::select! {
            result = &mut start_future => result,
            _ = shutdown.token().cancelled() => {
                self.move_generation_to_stopping(generation);
                start_future.await
            }
        };

        let mut server = match start_output {
            Ok(Ok(Some(server))) => Some(server),
            Ok(Ok(None)) => {
                #[cfg(test)]
                self.pause_after_start_output(generation).await;
                let result = self.transition_start_result_to_stopping(generation, Ok(false));
                let _ = start_sender.send(Some(result));
                None
            }
            Ok(Err(error)) => {
                #[cfg(test)]
                self.pause_after_start_output(generation).await;
                let result = self.transition_start_result_to_stopping(generation, Err(error));
                let _ = start_sender.send(Some(result));
                None
            }
            Err(_) => {
                #[cfg(test)]
                self.pause_after_start_output(generation).await;
                let result = self.transition_start_result_to_stopping(
                    generation,
                    Err("Web server start supervisor panicked".to_string()),
                );
                let _ = start_sender.send(Some(result));
                None
            }
        };

        if let Some(join) = server.as_mut() {
            if self.publish_running(generation) {
                let _ = start_sender.send(Some(Ok(true)));
            } else {
                self.move_generation_to_stopping(generation);
                let _ = start_sender.send(Some(Err(WEB_SERVER_START_CANCELLED.to_string())));
            }
            if let Err(error) = join.await {
                log::error!("[web-server] server task join failed: {}", error);
            }
        }

        self.move_generation_to_stopping(generation);
        admission.wait().await;
        if self.finish_generation(generation) {
            let _ = stop_sender.send(Some(Ok(())));
        }
    }

    pub fn record_bind_failure(&self, failure: crate::web::StartServerError) {
        *self.bind_failure.lock().unwrap() = Some(failure);
    }

    pub fn clear_bind_failure(&self) {
        *self.bind_failure.lock().unwrap() = None;
    }

    pub fn last_bind_failure(&self) -> Option<crate::web::StartServerError> {
        self.bind_failure.lock().unwrap().clone()
    }
}

#[derive(Default)]
pub struct ApiServerHandle {
    inner: Arc<Mutex<Option<ApiServerTask>>>,
}

pub struct ApiServerTask {
    join: tauri::async_runtime::JoinHandle<()>,
    shutdown: CancellationToken,
    bound_addr: SocketAddr,
}

impl ApiServerTask {
    pub fn new(
        join: tauri::async_runtime::JoinHandle<()>,
        shutdown: CancellationToken,
        bound_addr: SocketAddr,
    ) -> Self {
        Self {
            join,
            shutdown,
            bound_addr,
        }
    }
}

impl ApiServerHandle {
    /// #791 - handle to the running control-plane API server task.
    pub fn store_if_idle(&self, task: ApiServerTask) -> Result<bool, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "API server handle lock is poisoned".to_string())?;
        if let Some(stored) = inner
            .as_ref()
            .filter(|stored| !stored.join.inner().is_finished())
        {
            log::debug!(
                "[api-server] start ignored; server already running on {}",
                stored.bound_addr
            );
            task.shutdown.cancel();
            task.join.abort();
            return Ok(false);
        }
        *inner = Some(task);
        Ok(true)
    }

    pub fn has_running(&self) -> Result<bool, String> {
        Ok(self.running_bound_addr()?.is_some())
    }

    pub fn running_bound_addr(&self) -> Result<Option<SocketAddr>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "API server handle lock is poisoned".to_string())?;
        if let Some(stored) = inner
            .as_ref()
            .filter(|stored| !stored.join.inner().is_finished())
        {
            return Ok(Some(stored.bound_addr));
        }
        *inner = None;
        Ok(None)
    }

    pub async fn shutdown_running(&self, timeout: std::time::Duration) -> Result<bool, String> {
        let task = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "API server handle lock is poisoned".to_string())?;
            match inner.as_ref() {
                Some(stored) if stored.join.inner().is_finished() => {
                    *inner = None;
                    return Ok(false);
                }
                Some(_) => inner.take(),
                None => return Ok(false),
            }
        };
        let Some(task) = task else {
            return Ok(false);
        };

        task.shutdown.cancel();
        let mut join = task.join;
        match tokio::time::timeout(timeout, &mut join).await {
            Ok(Ok(())) => Ok(true),
            Ok(Err(err)) => Err(format!("API server task failed during shutdown: {}", err)),
            Err(_) => {
                join.abort();
                let _ = join.await;
                Ok(true)
            }
        }
    }
}
