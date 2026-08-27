//! Background worker: periodic tasks with graceful shutdown.

use std::sync::Arc;

use crate::experiments::auto_decide_all;
use crate::ports::Repository;

pub struct BackgroundWorker {
    repo: Arc<dyn Repository>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    interval_secs: u64,
}

impl BackgroundWorker {
    pub fn new(
        repo: Arc<dyn Repository>,
        shutdown: tokio::sync::watch::Receiver<bool>,
        interval_secs: u64,
    ) -> Self {
        Self {
            repo,
            shutdown,
            interval_secs,
        }
    }

    /// Spawn the auto-decider loop as a background task.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(self.interval_secs));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut shutdown = self.shutdown;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = auto_decide_all(&*self.repo).await {
                            tracing::warn!(error = %e, "auto-decider tick failed");
                        }
                    }
                    _ = shutdown.changed() => {
                        tracing::info!("background worker shutting down");
                        break;
                    }
                }
            }
        })
    }
}
