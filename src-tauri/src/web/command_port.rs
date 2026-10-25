use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

pub(crate) trait WebCommandDispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        id: u64,
        cmd: &'a str,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>>;

    fn binary<'a>(&'a self, data: &'a [u8]) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

#[derive(Clone)]
pub(crate) struct ManagedWebCommandDispatcher(pub Arc<dyn WebCommandDispatcher>);
