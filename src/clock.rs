//! Monotonic nanosecond clock.
//!
//! Latency instrumentation needs a monotonic source so a wall-clock jump can
//! never produce a negative latency or a bogus percentile. We anchor on the
//! first use and report nanoseconds since that anchor.

use std::sync::OnceLock;
use std::time::Instant;

fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// Nanoseconds elapsed since process start (monotonic).
#[inline]
pub fn now_ns() -> u128 {
    origin().elapsed().as_nanos()
}
