use super::*;

#[tokio::test]
async fn window_screenshot_limiter_queue_is_bounded_and_waiter_drop_releases_admission() {
    let limiter = std::sync::Arc::new(WindowScreenshotLimiter::new());
    let first_admission = match limiter.try_admit() {
        Ok(permit) => permit,
        Err(error) => panic!("first request must be admitted: {error:?}"),
    };
    let first_active = match limiter.acquire_active().await {
        Ok(active) => active,
        Err(error) => panic!("first request must acquire the active slot: {error:?}"),
    };
    let first_lease = WindowScreenshotLease::new(first_admission, first_active);

    let waiting_admission = match limiter.try_admit() {
        Ok(permit) => permit,
        Err(error) => panic!("second request must be admitted: {error:?}"),
    };
    let queued_admission = match limiter.try_admit() {
        Ok(permit) => permit,
        Err(error) => panic!("third request must be admitted: {error:?}"),
    };
    assert!(matches!(
        limiter.try_admit(),
        Err(WindowScreenshotAdmissionError::CaptureBusy)
    ));

    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let waiter_limiter = std::sync::Arc::clone(&limiter);
    let mut waiter = tokio::spawn(async move {
        let _ = started_sender.send(());
        let active = waiter_limiter.acquire_active().await?;
        Ok::<_, WindowScreenshotAdmissionError>(WindowScreenshotLease::new(
            waiting_admission,
            active,
        ))
    });
    assert!(started_receiver.await.is_ok());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiter)
            .await
            .is_err()
    );

    waiter.abort();
    let _ = waiter.await;
    assert!(limiter.try_admit().is_ok());

    drop(queued_admission);
    drop(first_lease);
}
