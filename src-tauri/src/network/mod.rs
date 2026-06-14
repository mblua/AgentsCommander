use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const OUTBOUND_NETWORK_PERMITS: usize = 64;
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const POOL_MAX_IDLE_PER_HOST: usize = 8;

#[derive(Clone)]
pub struct OutboundNetwork {
    inner: Arc<OutboundNetworkInner>,
}

struct OutboundNetworkInner {
    semaphore: Arc<Semaphore>,
    telegram: reqwest::Client,
    telegram_upload: reqwest::Client,
    gemini: reqwest::Client,
    general: reqwest::Client,
    #[cfg(test)]
    acquired_labels: std::sync::Mutex<Vec<&'static str>>,
}

pub struct OutboundPermit {
    _permit: OwnedSemaphorePermit,
}

impl OutboundNetwork {
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_permit_count(OUTBOUND_NETWORK_PERMITS)
    }

    #[cfg(test)]
    pub fn new_for_tests(permits: usize) -> Self {
        Self::with_permit_count(permits).expect("test reqwest clients should build")
    }

    fn with_permit_count(permits: usize) -> Result<Self, reqwest::Error> {
        Ok(Self {
            inner: Arc::new(OutboundNetworkInner {
                semaphore: Arc::new(Semaphore::new(permits)),
                telegram: build_client(Duration::from_secs(15))?,
                telegram_upload: build_client(Duration::from_secs(60))?,
                gemini: build_client(Duration::from_secs(30))?,
                general: build_client(Duration::from_secs(15))?,
                #[cfg(test)]
                acquired_labels: std::sync::Mutex::new(Vec::new()),
            }),
        })
    }

    pub async fn acquire(&self, label: &'static str) -> Result<OutboundPermit, String> {
        let permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| format!("network limiter closed before {label}"))?;
        #[cfg(test)]
        {
            self.inner
                .acquired_labels
                .lock()
                .expect("network test labels mutex poisoned")
                .push(label);
        }
        Ok(OutboundPermit { _permit: permit })
    }

    pub fn telegram(&self) -> &reqwest::Client {
        &self.inner.telegram
    }

    pub fn telegram_upload(&self) -> &reqwest::Client {
        &self.inner.telegram_upload
    }

    pub fn gemini(&self) -> &reqwest::Client {
        &self.inner.gemini
    }

    pub fn general(&self) -> &reqwest::Client {
        &self.inner.general
    }

    #[cfg(test)]
    pub fn available_permits_for_tests(&self) -> usize {
        self.inner.semaphore.available_permits()
    }

    #[cfg(test)]
    pub fn acquired_labels_for_tests(&self) -> Vec<&'static str> {
        self.inner
            .acquired_labels
            .lock()
            .expect("network test labels mutex poisoned")
            .clone()
    }
}

fn build_client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(timeout)
        .pool_idle_timeout(Some(POOL_IDLE_TIMEOUT))
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .build()
}

pub struct NetworkBackoff {
    attempt: u32,
    min: Duration,
    max: Duration,
    salt: u64,
}

impl NetworkBackoff {
    pub fn new(min: Duration, max: Duration, salt: u64) -> Self {
        Self {
            attempt: 0,
            min,
            max,
            salt,
        }
    }

    pub fn salt_from_str(input: &str) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in input.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn next_delay(&mut self) -> Duration {
        let base_ms = self.min.as_millis() as u64;
        let max_ms = self.max.as_millis() as u64;
        let shift = self.attempt.min(10);
        let exponential = base_ms.saturating_mul(1_u64 << shift);
        let capped = exponential.min(max_ms);
        let jitter = deterministic_jitter_ms(self.attempt, capped, self.salt);
        self.attempt = self.attempt.saturating_add(1);
        if capped >= max_ms {
            Duration::from_millis(max_ms.saturating_sub(jitter))
        } else {
            Duration::from_millis((capped + jitter).min(max_ms))
        }
    }
}

fn deterministic_jitter_ms(attempt: u32, cap_ms: u64, salt: u64) -> u64 {
    if cap_ms == 0 {
        return 0;
    }
    let jitter_cap = (cap_ms / 4).max(1);
    let mixed = salt
        ^ (attempt as u64)
            .wrapping_mul(1_103_515_245)
            .wrapping_add(12_345);
    mixed % jitter_cap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn permit_is_released_on_drop() {
        let network = OutboundNetwork::new_for_tests(1);
        let permit = network.acquire("test").await.unwrap();
        assert_eq!(network.available_permits_for_tests(), 0);
        drop(permit);
        assert_eq!(network.available_permits_for_tests(), 1);
    }

    #[test]
    fn backoff_grows_and_resets_within_cap() {
        let mut backoff =
            NetworkBackoff::new(Duration::from_millis(100), Duration::from_secs(2), 42);
        let first = backoff.next_delay();
        let second = backoff.next_delay();
        let third = backoff.next_delay();
        assert!(first >= Duration::from_millis(100));
        assert!(second >= first);
        assert!(third >= second);
        assert!(third <= Duration::from_secs(2));

        backoff.reset();
        let reset = backoff.next_delay();
        assert_eq!(reset, first);
    }

    #[test]
    fn backoff_salt_desynchronizes_same_attempts() {
        let mut first = NetworkBackoff::new(
            Duration::from_secs(3),
            Duration::from_secs(60),
            NetworkBackoff::salt_from_str("session-a"),
        );
        let mut second = NetworkBackoff::new(
            Duration::from_secs(3),
            Duration::from_secs(60),
            NetworkBackoff::salt_from_str("session-b"),
        );
        let first_delay = first.next_delay();
        let second_delay = second.next_delay();

        assert_ne!(first_delay, second_delay);
        assert!(first_delay >= Duration::from_secs(3));
        assert!(second_delay >= Duration::from_secs(3));
        assert!(first_delay <= Duration::from_secs(60));
        assert!(second_delay <= Duration::from_secs(60));
    }
}
