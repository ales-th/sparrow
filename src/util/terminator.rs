use jagua_rs::Instant;
use std::time::Duration;
use std::sync::Arc;

/// A thread-safe snapshot of a termination check for parallel placement workers.
pub type StopSignal = Arc<dyn Fn() -> bool + Send + Sync>;

/// Generic trait for any struct that can determine if the optimization process should terminate.
pub trait Terminator {
    /// Effective phase duration for time-based tuning; legacy controllers retain their settings.
    fn phase_time_limit(&self, configured: Duration) -> Duration { configured }

    /// Whole-run stop, as distinct from a phase timeout or phase interruption.
    fn run_stopped(&self) -> bool { false }

    /// Optional termination check usable inside parallel workers.
    fn stop_signal(&self) -> Option<StopSignal> { None }

    /// Checks if the termination condition is met
    fn kill(&self) -> bool;

    /// Sets a new timeout duration
    fn new_timeout(&mut self, timeout: Duration);

    /// Returns the instant when a timeout was set, if any
    fn timeout_at(&self) -> Option<Instant>;
}

#[derive(Debug, Clone)]
pub struct BasicTerminator {
    pub timeout: Option<Instant>,
}

impl Default for BasicTerminator {
    fn default() -> Self {
        Self::new()
    }
}

impl BasicTerminator {
    pub fn new() -> Self {
        Self { timeout: None }
    }
}

impl Terminator for BasicTerminator {
    fn stop_signal(&self) -> Option<StopSignal> {
        let timeout = self.timeout;
        Some(Arc::new(move || timeout.is_some_and(|t| Instant::now() >= t)))
    }

    fn kill(&self) -> bool {
        self.timeout.is_some_and(|timeout| Instant::now() > timeout)
    }

    fn new_timeout(&mut self, timeout: Duration){
        self.timeout = Some(Instant::now() + timeout);
    }

    fn timeout_at(&self) -> Option<Instant> {
        self.timeout
    }
}
