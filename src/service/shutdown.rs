use std::future::Future;

use tokio::signal;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};
use tracing::Instrument;

pub struct Trigger {
    signal: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct Shutdown {
    signal: watch::Receiver<bool>,
}

impl Default for Trigger {
    fn default() -> Self {
        Self::new()
    }
}

impl Trigger {
    pub fn new() -> Trigger {
        let (signal, _) = watch::channel(false);
        Trigger { signal }
    }

    pub fn subscribe(&self) -> Shutdown {
        Shutdown {
            signal: self.signal.subscribe(),
        }
    }

    pub fn fire(&self) {
        self.signal.send_replace(true);
    }

    pub fn fire_on_signal(self) -> impl Future<Output = ()> + Send + 'static {
        let terminate = listen_for_terminate();
        async move {
            tokio::select! {
                _ = wait_for_interrupt() => tracing::info!("SIGINT received, shutting down"),
                _ = terminate => tracing::info!("SIGTERM received, shutting down"),
            }
            self.fire();
        }
    }
}

impl Shutdown {
    pub fn is_triggered(&self) -> bool {
        *self.signal.borrow()
    }

    pub fn wait(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut signal = self.signal.clone();
        async move {
            let _ = signal.wait_for(|fired| *fired).await;
        }
    }
}

pub fn spawn<F, Fut>(
    name: &'static str,
    shutdown: &Shutdown,
    task: F,
) -> JoinHandle<Result<(), JoinError>>
where
    F: FnOnce(Shutdown) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let running = tokio::spawn(task(shutdown.clone()).in_current_span());
    tokio::spawn(async move {
        let outcome = running.await;
        if let Err(error) = &outcome
            && error.is_panic()
        {
            tracing::error!(task = name, error = %error, "Task panicked");
        }
        outcome
    })
}

async fn wait_for_interrupt() {
    if let Err(error) = signal::ctrl_c().await {
        tracing::error!(error = %error, "Failed to listen for SIGINT");
        std::future::pending::<()>().await;
    }
}

#[cfg(unix)]
fn listen_for_terminate() -> impl Future<Output = ()> + Send + 'static {
    let listener = signal::unix::signal(signal::unix::SignalKind::terminate());
    async move {
        match listener {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(error = %error, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(not(unix))]
fn listen_for_terminate() -> impl Future<Output = ()> + Send + 'static {
    std::future::pending::<()>()
}

#[cfg(test)]
#[path = "../../tests/service/shutdown.rs"]
mod tests;
