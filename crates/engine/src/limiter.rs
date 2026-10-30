//! Global speed limit shared by every connection of every download.
//!
//! A token bucket that may go into debt: a caller takes the bytes it just
//! received and sleeps off any deficit. That keeps the hot path to one short
//! mutex hold and works for any number of concurrent connections.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct SpeedLimiter {
    /// Bytes per second; 0 means unlimited.
    rate: AtomicU64,
    bucket: Mutex<(f64, Instant)>,
}

impl SpeedLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self { rate: AtomicU64::new(bytes_per_sec), bucket: Mutex::new((0.0, Instant::now())) }
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub async fn take(&self, bytes: usize) {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return;
        }
        let wait = {
            let mut b = self.bucket.lock().unwrap();
            let now = Instant::now();
            let refill = now.duration_since(b.1).as_secs_f64() * rate as f64;
            // Allow at most one second of burst.
            b.0 = (b.0 + refill).min(rate as f64) - bytes as f64;
            b.1 = now;
            if b.0 < 0.0 { Duration::from_secs_f64(-b.0 / rate as f64) } else { Duration::ZERO }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}
