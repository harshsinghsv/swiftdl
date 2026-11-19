use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches, Parser};
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
#[command(version, about, disable_help_flag = true, disable_version_flag = true)]
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

    // The built-in flags, declared so their help text can be translated too.
    /// Print help
    #[arg(id = "help", short, long, action = clap::ArgAction::Help, help = "Print help")]
    #[allow(dead_code)]
    help: Option<bool>,

    /// Print version
    #[arg(id = "version", short = 'V', long, action = clap::ArgAction::Version, help = "Print version")]
    #[allow(dead_code)]
    version: Option<bool>,
}

/// The terminal's language: SWIFDL_LANG wins, then the usual Unix variables.
/// (Windows terminals rarely set LANG, so SWIFDL_LANG is the way in.)
fn zh() -> bool {
    let v = std::env::var("SWIFDL_LANG")
        .ok()
        .or_else(|| std::env::var("LC_ALL").ok())
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default();
    v.to_ascii_lowercase().starts_with("zh")
}

/// A fixed message in the terminal's language.
fn t(en: &'static str, cn: &'static str) -> &'static str {
    if zh() { cn } else { en }
}

fn parse_headers(raw: &[String]) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for h in raw {
        let (name, value) = h.split_once(':').with_context(|| {
            if zh() { format!("请求头格式应为 '名称: 值'，收到的是 {h:?}") } else { format!("header must be 'Name: value', got {h:?}") }
        })?;
        map.append(
            HeaderName::from_bytes(name.trim().as_bytes()).with_context(|| {
                if zh() { format!("请求头名称无效：{h:?}") } else { format!("bad header name in {h:?}") }
            })?,
            HeaderValue::from_str(value.trim()).with_context(|| {
                if zh() { format!("请求头内容无效：{h:?}") } else { format!("bad header value in {h:?}") }
            })?,
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

    eprintln!("{}", if zh() { format!("文件：  {}", dest.display()) } else { format!("File:   {}", dest.display()) });
    match remote.size {
        Some(size) if zh() => eprintln!("大小：  {}（{size} 字节）", HumanBytes(size)),
        Some(size) => eprintln!("Size:   {} ({size} bytes)", HumanBytes(size)),
        None => eprintln!("{}", t("Size:   unknown", "大小：  未知")),
    }
    if !remote.resumable {
        eprintln!("{}", t("Server does not support ranges: using one connection, no resume.", "服务器不支持分段下载：将使用单连接，无法续传。"));
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
                bar.set_message(if zh() {
                    format!("{} 个连接", progress.connections.load(Ordering::Relaxed))
                } else {
                    format!("{} conns", progress.connections.load(Ordering::Relaxed))
                });
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
        mirrors: vec![],
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
            bar.finish_with_message(t("done", "完成"));
            Ok(Some(dest))
        }
        Ok(Outcome::Paused) => {
            bar.abandon_with_message(t("paused", "已暂停"));
            Ok(None)
        }
        Err(e) => {
            bar.abandon_with_message(t("failed", "失败"));
            Err(e.context(t("download stopped; progress saved, run the same command to resume", "下载已中断；进度已保存，重新运行同一条命令即可续传")))
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut cmd = Args::command();
    if zh() {
        // clap's own headings are English; a template and explicit headings
        // make the whole help screen Chinese.
        cmd = cmd
            .about("swiftdl：快速、可续传的多连接下载器")
            .help_template("{about-with-newline}\n用法: {usage}\n\n{all-args}{after-help}")
            .mut_arg("url", |a| a.help("要下载的 URL").help_heading("参数"))
            .mut_arg("output", |a| a.help("输出文件或目录（默认使用服务器的文件名，保存在当前目录）").help_heading("选项"))
            .mut_arg("connections", |a| a.help("并行连接数").help_heading("选项"))
            .mut_arg("headers", |a| a.help("附加请求头，例如 -H \"Cookie: session=abc\"（可重复）").help_heading("选项"))
            .mut_arg("limit", |a| a.help("速度限制，单位 KiB/s（0 表示不限速）").help_heading("选项"))
            .mut_arg("restart", |a| a.help("忽略已保存的进度，从头开始").help_heading("选项"))
            .mut_arg("help", |a| a.help("显示帮助信息").help_heading("选项"))
            .mut_arg("version", |a| a.help("显示版本号").help_heading("选项"));
    }
    let args = Args::from_arg_matches(&cmd.get_matches()).expect("clap already validated these");
    match run(args).await {
        Ok(Some(path)) => {
            eprintln!("{}", if zh() { format!("已保存到 {}", path.display()) } else { format!("Saved to {}", path.display()) });
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("{}", t("Paused. Run the same command again to resume.", "已暂停。重新运行同一条命令即可续传。"));
            ExitCode::from(130)
        }
        Err(e) => {
            eprintln!("{}: {e:#}", t("error", "错误"));
            ExitCode::FAILURE
        }
    }
}
