//! Deterministic, typed errors for the runtime foundation.

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level Neurone error.
///
/// The variants are deliberately coarse: each one names the *stage* that
/// failed so runtime failure handling can stay deterministic (fail closed)
/// without inventing per-call-site error types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("ingestion error: {0}")]
    Ingest(String),

    #[error("event routing error: {0}")]
    Routing(String),

    #[error("runtime task `{task}` failed: {source}")]
    Task {
        task: &'static str,
        #[source]
        source: Box<Error>,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Attach a task name to any error produced by a spawned runtime task.
    pub fn in_task(self, task: &'static str) -> Error {
        Error::Task {
            task,
            source: Box::new(self),
        }
    }
}
