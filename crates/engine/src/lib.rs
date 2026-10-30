//! swiftdl-engine: segmented, resumable, multi-connection HTTP downloads.
//!
//! Front ends (the CLI and the desktop app) build a [`Request`], hand it to
//! [`run`] together with a [`Progress`] they can poll, a cancellation token
//! (pause), and a shared [`SpeedLimiter`].

mod download;
mod limiter;
pub mod probe;
mod segment;
mod state;

pub use download::{Outcome, Progress, Request, build_client, part_path, resolve_dest, run, saved_remaining};
pub use limiter::SpeedLimiter;
pub use probe::{RemoteFile, probe};
pub use reqwest::{Client, header};
pub use tokio_util::sync::CancellationToken;
