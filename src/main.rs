mod engine;
mod probe;
mod segment;
mod state;

use anyhow::{Context, Result};
use clap::Parser;
use engine::{Options, Outcome};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::path::PathBuf;
use std::process::ExitCode;

/// swiftdl: a fast, resumable, multi-connection downloader.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// URL to download
    url: String,

    /// Output file or directory (default: server's file name, current dir)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Number of parallel connections
    #[arg(short, long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=64))]
    connections: u16,

    /// Extra request header, e.g. -H "Cookie: session=abc" (repeatable)
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Ignore saved progress and start over
    #[arg(long)]
    restart: bool,
}

fn parse_headers(raw: &[String]) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for h in raw {
        let (name, value) = h.split_once(':').with_context(|| format!("header must be 'Name: value', got {h:?}"))?;
        map.append(
            HeaderName::from_bytes(name.trim().as_bytes()).with_context(|| format!("bad header name in {h:?}"))?,
            HeaderValue::from_str(value.trim()).with_context(|| format!("bad header value in {h:?}"))?,
        );
    }
    Ok(map)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let run = async {
        let opts = Options {
            url: args.url,
            output: args.output,
            connections: args.connections as usize,
            headers: parse_headers(&args.headers)?,
            restart: args.restart,
        };
        engine::download(opts).await
    };
    match run.await {
        Ok(Outcome::Completed(path)) => {
            eprintln!("Saved to {}", path.display());
            ExitCode::SUCCESS
        }
        Ok(Outcome::Paused) => {
            eprintln!("Paused. Run the same command again to resume.");
            ExitCode::from(130)
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
