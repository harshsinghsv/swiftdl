use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use swiftdl_engine::header::{HeaderMap, HeaderName, HeaderValue};
use swiftdl_engine::{CancellationToken, Outcome, Progress, Request, SpeedLimiter};

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

    /// Speed limit in KiB/s (0 = unlimited)
    #[arg(short, long, default_value_t = 0)]
    limit: u64,

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

async fn run(args: Args) -> Result<Option<PathBuf>> {
    let headers = parse_headers(&args.headers)?;
    let client = swiftdl_engine::build_client()?;
    let remote = swiftdl_engine::probe(&client, &args.url, &headers).await?;
    let dest = match args.output {
        Some(p) if p.is_dir() => swiftdl_engine::resolve_dest(&p, &remote.filename),
        Some(p) => p,
        None => swiftdl_engine::resolve_dest(&std::env::current_dir()?, &remote.filename),
    };

    eprintln!("File:   {}", dest.display());
    match remote.size {
        Some(size) => eprintln!("Size:   {} ({size} bytes)", HumanBytes(size)),
        None => eprintln!("Size:   unknown"),
    }
    if !remote.resumable {
        eprintln!("Server does not support ranges: using one connection, no resume.");
    }

    let progress = Arc::new(Progress::default());
    let cancel = CancellationToken::new();
    let bar = ProgressBar::new(remote.size.unwrap_or(0));
    bar.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes:>10}/{total_bytes} {binary_bytes_per_sec:>12}  ETA {eta:>4}  {msg}",
        )?
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    let ticker = {
        let (progress, bar) = (progress.clone(), bar.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(200));
            let mut first = true;
            loop {
                tick.tick().await;
                let done = progress.downloaded.load(Ordering::Relaxed);
                if first && done > 0 {
                    // Resumed: start the bar where we left off so the speed
                    // estimate isn't skewed by the jump.
                    bar.reset();
                    first = false;
                }
                bar.set_position(done);
                bar.set_message(format!("{} conns", progress.connections.load(Ordering::Relaxed)));
            }
        })
    };
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
        });
    }

    let req = Request {
        url: args.url,
        dest: dest.clone(),
        connections: args.connections as usize,
        headers,
        restart: args.restart,
    };
    let limiter = Arc::new(SpeedLimiter::new(args.limit * 1024));
    let outcome = swiftdl_engine::run(&client, &req, progress.clone(), cancel, limiter).await;
    ticker.abort();
    bar.set_position(progress.downloaded.load(Ordering::Relaxed));
    match outcome {
        Ok(Outcome::Completed) => {
            bar.finish_with_message("done");
            Ok(Some(dest))
        }
        Ok(Outcome::Paused) => {
            bar.abandon_with_message("paused");
            Ok(None)
        }
        Err(e) => {
            bar.abandon_with_message("failed");
            Err(e.context("download stopped; progress saved, run the same command to resume"))
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(Some(path)) => {
            eprintln!("Saved to {}", path.display());
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("Paused. Run the same command again to resume.");
            ExitCode::from(130)
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
