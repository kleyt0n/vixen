//! `vixen` — terminal client.

mod render;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Write};
use vixen_core::{Action, Config, Monitor, control};

#[derive(Parser)]
#[command(
    name = "vixen",
    version,
    about = "Monitor GPUs and jobs across SSH hosts, Slurm, SGE, Modal, Vast.ai and SkyPilot"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-shot status of every target (default).
    Status {
        /// Only these targets.
        targets: Vec<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Show every pending/held job.
        #[arg(short, long)]
        all: bool,
    },
    /// Live view; ETAs sharpen as progress is observed across polls.
    Watch {
        targets: Vec<String>,
        #[arg(short, long)]
        all: bool,
    },
    /// Show the tail of a job's output.
    Logs {
        target: String,
        job: String,
        #[arg(short = 'n', long, default_value_t = 40)]
        lines: u32,
    },
    /// Cancel a job (scancel / qdel / kill / modal app stop), or stop a Vast.ai instance or SkyPilot cluster.
    Cancel {
        target: String,
        job: String,
        /// Skip confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Hold a pending job.
    Hold { target: String, job: String },
    /// Release a held job.
    Release { target: String, job: String },
    /// List configured targets.
    Targets,
    /// Create ~/.config/vixen/config.toml from the example.
    Init,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("vixen: {e:#}");
        std::process::exit(1);
    }
}

fn select(mut cfg: Config, names: &[String]) -> Result<Config> {
    for n in names {
        cfg.target(n)?;
    }
    if !names.is_empty() {
        cfg.targets.retain(|t| names.contains(&t.name));
        cfg.targets.iter_mut().for_each(|t| t.enabled = true);
    }
    Ok(cfg)
}

async fn run(cli: Cli) -> Result<()> {
    let cmd = cli.cmd.unwrap_or(Cmd::Status { targets: vec![], json: false, all: false });
    if let Cmd::Init = cmd {
        let (path, created) = Config::init()?;
        println!("{} {}", if created { "created" } else { "exists:" }, path.display());
        return Ok(());
    }
    let cfg = Config::load()?;
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();

    match cmd {
        Cmd::Status { targets, json, all } => {
            let snap = Monitor::once(&select(cfg, &targets)?).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                print!("{}", render::snapshot(&snap, color, limit(all)));
            }
        }
        Cmd::Watch { targets, all } => {
            let monitor = Monitor::start(select(cfg, &targets)?);
            let mut rx = monitor.subscribe();
            print!("\x1b[?1049h\x1b[?25l"); // alt screen, hide cursor
            let restore = || print!("\x1b[?25h\x1b[?1049l");
            loop {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() { break; }
                        let snap = rx.borrow_and_update().clone();
                        print!("\x1b[H\x1b[2J{}", render::snapshot(&snap, color, limit(all)));
                        println!("{}", render::dim("ctrl-c to quit", color));
                        std::io::stdout().flush()?;
                    }
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            restore();
        }
        Cmd::Logs { target, job, lines } => {
            print!("{}", control::logs(cfg.target(&target)?, &job, None, lines).await?);
        }
        Cmd::Cancel { target, job, yes } => {
            let t = cfg.target(&target)?;
            if !yes && !confirm(&format!("cancel {job} on {target}?"))? {
                bail!("aborted");
            }
            println!("{}", control::act(t, &job, Action::Cancel).await?);
        }
        Cmd::Hold { target, job } => println!("{}", control::act(cfg.target(&target)?, &job, Action::Hold).await?),
        Cmd::Release { target, job } => {
            println!("{}", control::act(cfg.target(&target)?, &job, Action::Release).await?)
        }
        Cmd::Targets => {
            for t in &cfg.targets {
                let off = if t.enabled { "" } else { "  (disabled)" };
                println!("{:<14} {:<32} every {}s{off}", t.name, t.describe(), t.interval(&cfg));
            }
        }
        Cmd::Init => unreachable!(),
    }
    Ok(())
}

fn limit(all: bool) -> Option<usize> {
    (!all).then_some(6)
}

fn confirm(q: &str) -> Result<bool> {
    print!("{q} [y/N] ");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(matches!(s.trim(), "y" | "Y" | "yes"))
}
