//! Cooperative shutdown primitive.
//!
//! A single `watch` channel is fanned out to every runtime task. Tasks poll
//! [`Shutdown::cancelled`] inside their `select!` loops, so shutdown is
//! cooperative and never races state mutation.

use tokio::sync::watch;

/// Cloneable read handle given to runtime tasks.
#[derive(Clone)]
pub struct Shutdown {
    rx: watch::Receiver<bool>,
}

/// Single-owner write handle used by the supervisor.
pub struct ShutdownHandle {
    tx: watch::Sender<bool>,
}

/// Create a linked shutdown handle / reader pair.
pub fn shutdown_channel() -> (ShutdownHandle, Shutdown) {
    let (tx, rx) = watch::channel(false);
    (ShutdownHandle { tx }, Shutdown { rx })
}

impl ShutdownHandle {
    /// Signal every holder of a [`Shutdown`] to stop.
    pub fn trigger(&self) {
        // Ignore send errors: a missing receiver simply means every task has
        // already stopped, which is the desired end state.
        let _ = self.tx.send(true);
    }
}

impl Shutdown {
    /// Resolves as soon as shutdown has been requested.
    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        if *rx.borrow() {
            return;
        }
        // `changed()` returns when the value differs from the last observed.
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    }

    /// Non-blocking check for shutdown.
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trigger_then_cancelled_resolves() {
        let (handle, shutdown) = shutdown_channel();
        assert!(!shutdown.is_cancelled());
        handle.trigger();
        shutdown.cancelled().await;
        assert!(shutdown.is_cancelled());
    }
}
