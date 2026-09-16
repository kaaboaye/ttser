pub mod audio;
pub mod config;
mod history;
pub mod runtime;
pub mod speech;
pub mod state;

/// Inserts one complete transcript at the current keyboard focus.
///
/// Implementations own platform details, including clipboard lifetime and paste
/// shortcuts. An error can follow a partial insertion, so callers must not retry
/// automatically. Implementations must never submit the destination form.
pub trait TextOutput: Send + 'static {
    fn insert(&mut self, text: &str) -> anyhow::Result<()>;
}
