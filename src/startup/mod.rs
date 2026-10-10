mod collision;
mod logs;
mod readiness;
mod retry;

pub use logs::CapturedOutput;
pub(crate) use readiness::{StartupExpectation, StartupOutcome, wait_for_ready};
pub(crate) use retry::RetryPolicy;
use tokio_process_tools::{BroadcastOutputStream, ReliableWithBackpressure, ReplayEnabled};

/// The stdout/stderr stream type of the spawned `cargo leptos` process.
///
/// Must match the stream configuration in `app::spawn_with_log_capture`.
pub(crate) type OutputStream = BroadcastOutputStream<ReliableWithBackpressure, ReplayEnabled>;
