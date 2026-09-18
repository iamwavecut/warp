use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use futures::{FutureExt as _, future};
use parking_lot::Mutex;
use warpui::r#async::FutureExt as _;
use warpui::r#async::executor::Background;

use super::{SaveCoordinator, SaveOperation, save_after_session_update};
use crate::ai::agent_sdk::driver::harness::SavePoint;

#[tokio::test]
async fn session_update_failure_does_not_skip_final_save() {
    let coordinator = SaveCoordinator::default();
    let operations = Mutex::new(Vec::new());
    coordinator
        .finish(
            save_after_session_update(
                async {
                    operations.lock().push("update");
                    Err(anyhow!("transient local persistence failure"))
                },
                async {
                    operations.lock().push("final");
                    Ok(())
                },
            ),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(*operations.lock(), ["update", "final"]);
}

#[tokio::test]
async fn save_error_is_preserved_after_session_update() {
    let result = save_after_session_update(async { Ok(()) }, async {
        Err(anyhow!("local save failed"))
    })
    .await;
    assert_eq!(result.unwrap_err().to_string(), "local save failed");
}

#[tokio::test]
async fn coalesces_saves_without_blocking_other_work() {
    let background = Background::default();
    let coordinator = SaveCoordinator::default();
    let saved = Arc::new(Mutex::new(Vec::new()));
    let (started, starts) = async_channel::unbounded();
    let (release, releases) = async_channel::unbounded();
    let recorded = saved.clone();
    let operation: SaveOperation = Arc::new(move |point| {
        let started = started.clone();
        let releases = releases.clone();
        let recorded = recorded.clone();
        Box::pin(async move {
            started.send(point).await?;
            releases.recv().await?;
            recorded.lock().push(point);
            Ok(())
        })
    });

    coordinator.request(SavePoint::Periodic, operation.clone(), &background);
    assert_eq!(starts.recv().await.unwrap(), SavePoint::Periodic);
    coordinator.request(SavePoint::PostTurn, operation.clone(), &background);
    coordinator.request(SavePoint::Periodic, operation.clone(), &background);
    coordinator.request(SavePoint::PostTurn, operation, &background);
    let (ping, pong) = oneshot::channel();
    background
        .spawn(async move { ping.send(()).unwrap() })
        .detach();
    pong.with_timeout(Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    assert!(starts.is_empty());

    release.send(()).await.unwrap();
    assert_eq!(starts.recv().await.unwrap(), SavePoint::PostTurn);
    release.send(()).await.unwrap();
    coordinator
        .finish(
            async {
                saved.lock().push(SavePoint::Final);
                Ok(())
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(
        *saved.lock(),
        [SavePoint::Periodic, SavePoint::PostTurn, SavePoint::Final]
    );
    assert!(starts.is_empty());
}

#[tokio::test]
async fn cancelled_blocking_capture_cannot_save_after_final_save() {
    let background = Background::default();
    let coordinator = SaveCoordinator::default();
    let saved = Arc::new(Mutex::new(Vec::new()));
    let captured_saves = saved.clone();
    let (started, start) = oneshot::channel();
    let (read_done, read_finished) = oneshot::channel();
    let (release, wait_for_release) = mpsc::channel();
    let read = Mutex::new(Some((started, read_done, wait_for_release)));
    let operation: SaveOperation = Arc::new(move |_| {
        let (started, read_done, wait_for_release) = read.lock().take().unwrap();
        let saved = captured_saves.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                wait_for_release.recv().unwrap();
                read_done.send(()).unwrap();
            })
            .await?;
            saved.lock().push("stale");
            Ok(())
        })
    });
    coordinator.request(SavePoint::Periodic, operation.clone(), &background);
    start.await.unwrap();
    coordinator.request(SavePoint::PostTurn, operation.clone(), &background);

    coordinator
        .finish(
            async {
                saved.lock().push("final");
                Ok(())
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    coordinator.request(SavePoint::PostTurn, operation, &background);
    release.send(()).unwrap();
    read_finished.await.unwrap();

    assert_eq!(*saved.lock(), ["final"]);
}

#[tokio::test]
async fn expired_final_deadline_never_starts_or_rearms_a_save() {
    let coordinator = SaveCoordinator::default();
    let captured = AtomicBool::new(false);
    assert!(
        coordinator
            .finish(
                async {
                    captured.store(true, Ordering::SeqCst);
                    Ok(())
                },
                Duration::ZERO,
            )
            .await
            .is_err()
    );
    assert!(
        coordinator
            .finish(
                async {
                    captured.store(true, Ordering::SeqCst);
                    Ok(())
                },
                Duration::from_secs(30),
            )
            .await
            .is_err()
    );
    assert!(!captured.load(Ordering::SeqCst));
}

#[tokio::test]
async fn final_timeout_cancels_future_before_returning() {
    let coordinator = SaveCoordinator::default();
    let (release, released) = oneshot::channel::<()>();
    let saved = AtomicBool::new(false);
    assert!(
        coordinator
            .finish(
                async {
                    released.await?;
                    saved.store(true, Ordering::SeqCst);
                    Ok(())
                },
                Duration::from_millis(10),
            )
            .await
            .is_err()
    );
    assert!(release.send(()).is_err());
    assert!(!saved.load(Ordering::SeqCst));
}

#[tokio::test]
async fn interrupted_finalizer_still_joins_the_cancelled_worker() {
    let background = Background::default();
    let coordinator = SaveCoordinator::default();
    let (started, start) = oneshot::channel();
    let (release, released) = oneshot::channel::<()>();
    let current = Mutex::new(Some((started, released)));
    let operation: SaveOperation = Arc::new(move |_| {
        let (started, released) = current.lock().take().unwrap();
        Box::pin(async move {
            started.send(()).unwrap();
            released.await?;
            Ok(())
        })
    });
    coordinator.request(SavePoint::Periodic, operation, &background);
    start.await.unwrap();
    assert!(
        coordinator
            .finish(future::pending::<Result<()>>(), Duration::from_secs(5))
            .now_or_never()
            .is_none()
    );

    coordinator
        .finish(
            async {
                assert!(release.send(()).is_err());
                Ok(())
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn final_failure_is_retained_without_repeating_writes() {
    let coordinator = SaveCoordinator::default();
    let result = coordinator
        .finish(
            async { Err(anyhow!("save failed")) },
            Duration::from_secs(5),
        )
        .await;
    assert!(result.is_err());
    assert!(
        coordinator
            .finish(future::pending::<Result<()>>(), Duration::from_secs(5))
            .now_or_never()
            .unwrap()
            .is_err()
    );
}
