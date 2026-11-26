use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::time::timeout;

use crate::service::{Routes, Service, Shutdown, Trigger, spawn};

const WAIT: Duration = Duration::from_secs(5);

async fn settle(shutdown: &Shutdown) -> bool {
    timeout(WAIT, shutdown.wait()).await.is_ok()
}

#[tokio::test]
async fn should_wake_every_subscriber_when_the_trigger_fires() {
    let trigger = Trigger::new();
    let early = trigger.subscribe();
    assert!(!early.is_triggered());
    trigger.fire();
    let late = trigger.subscribe();
    assert!(early.is_triggered() && late.is_triggered());
    assert!(settle(&early).await && settle(&late).await);
}

#[tokio::test]
async fn should_stop_waiting_when_the_trigger_is_dropped() {
    let trigger = Trigger::new();
    let shutdown = trigger.subscribe();
    drop(trigger);
    assert!(settle(&shutdown).await);
}

#[tokio::test]
async fn should_hand_the_task_a_shutdown_it_can_stop_on() {
    let trigger = Trigger::new();
    let handle = spawn("worker", &trigger.subscribe(), |shutdown| async move {
        shutdown.wait().await;
    });
    trigger.fire();
    let outcome = timeout(WAIT, handle).await.expect("task stops");
    assert!(matches!(outcome, Ok(Ok(()))));
}

#[tokio::test]
async fn should_report_the_panic_and_keep_running_when_a_task_panics() {
    let trigger = Trigger::new();
    let shutdown = trigger.subscribe();
    let panicked = spawn("broken", &shutdown, |_| async { panic!("boom") });
    let outcome = timeout(WAIT, panicked).await.expect("task ends");
    assert!(matches!(outcome, Ok(Err(error)) if error.is_panic()));
    let healthy = spawn("healthy", &shutdown, |_| async {});
    let outcome = timeout(WAIT, healthy).await.expect("task ends");
    assert!(matches!(outcome, Ok(Ok(()))));
}

#[cfg(unix)]
#[tokio::test]
async fn should_fire_when_the_process_gets_sigterm() {
    let trigger = Trigger::new();
    let shutdown = trigger.subscribe();
    let listening = tokio::spawn(trigger.fire_on_signal());
    let status = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success());
    assert!(settle(&shutdown).await);
    timeout(WAIT, listening)
        .await
        .expect("listener ends")
        .expect("listener does not panic");
}

struct Indexer {
    stopped: Arc<AtomicBool>,
}

impl Service for Indexer {
    fn name(&self) -> &'static str {
        "indexer"
    }

    fn build_router(self: Arc<Self>) -> Routes {
        Routes::new()
    }

    fn spawn_workers(self: Arc<Self>, shutdown: Shutdown) {
        spawn("indexer", &shutdown, move |shutdown| async move {
            shutdown.wait().await;
            self.stopped.store(true, Ordering::SeqCst);
        });
    }
}

struct Idle;

impl Service for Idle {
    fn name(&self) -> &'static str {
        "idle"
    }

    fn build_router(self: Arc<Self>) -> Routes {
        Routes::new()
    }
}

#[tokio::test]
async fn should_stop_service_workers_when_main_fires_the_trigger() {
    let trigger = Trigger::new();
    let stopped = Arc::new(AtomicBool::new(false));
    let services: Vec<Arc<dyn Service>> = vec![
        Arc::new(Indexer {
            stopped: Arc::clone(&stopped),
        }),
        Arc::new(Idle),
    ];
    for service in services {
        service.spawn_workers(trigger.subscribe());
    }
    trigger.fire();
    let stops = async {
        while !stopped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    };
    assert!(timeout(WAIT, stops).await.is_ok());
}
