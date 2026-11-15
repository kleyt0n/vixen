//! Job control: cancel / hold / release, and log retrieval.

use crate::config::{Kind, Scheduler, Target};
use crate::exec::{self, Exec, shq};
use crate::model::Action;
use crate::monitor::Poller;
use crate::sources::{cli, skypilot};
use anyhow::{Result, bail};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Job ids are interpolated into shell commands — allow scheduler syntax only.
fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty() && id.len() < 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-[],%".contains(c));
    if !ok {
        bail!("invalid job id `{id}`");
    }
    Ok(())
}

/// SGE `123.4` → (`123`, `-t 4`).
fn sge_id(id: &str) -> String {
    match id.split_once('.') {
        Some((job, task)) => format!("{job} -t {task}"),
        None => id.to_string(),
    }
}

/// Shell command implementing `action` on `id` for this target (None for cloud CLI targets).
pub fn command(t: &Target, id: &str, action: Action) -> Result<Option<String>> {
    check_id(id)?;
    if t.kind.is_cli() {
        return Ok(None);
    }
    // Slurm array throttle `%8` is display-only.
    let mut slurm_id = id.split('%').next().unwrap_or(id).to_string();
    if slurm_id.contains("_[") && !slurm_id.ends_with(']') {
        slurm_id.push(']');
    }
    let cmd = match (t.scheduler, action) {
        (Scheduler::Slurm, Action::Cancel) => format!("scancel {}", shq(&slurm_id)),
        (Scheduler::Slurm, Action::Hold) => format!("scontrol hold {}", shq(&slurm_id)),
        (Scheduler::Slurm, Action::Release) => format!("scontrol release {}", shq(&slurm_id)),
        (Scheduler::Sge, Action::Cancel) => format!("qdel {}", sge_id(id)),
        (Scheduler::Sge, Action::Hold) => format!("qhold {}", sge_id(id)),
        (Scheduler::Sge, Action::Release) => format!("qrls {}", sge_id(id)),
        (Scheduler::None, Action::Cancel) => format!("kill -TERM {}", shq(id)),
        (_, a) => bail!("{a:?} is not supported on `{}`", t.name),
    };
    Ok(Some(cmd))
}

/// Cancel, hold or release a job. Returns the command's output.
pub async fn act(t: &Target, id: &str, action: Action) -> Result<String> {
    if action == Action::Logs {
        bail!("use control::logs");
    }
    match command(t, id, action)? {
        Some(cmd) => {
            let out = Exec::new(t.host.clone(), t.ssh_args.clone()).sh(&format!("{cmd} 2>&1"), TIMEOUT).await?;
            Ok(if out.trim().is_empty() { format!("{action:?} sent: {cmd}") } else { out.trim().to_string() })
        }
        None => {
            if action != Action::Cancel {
                bail!("{action:?} is not supported on {}", t.kind.label());
            }
            let (args, done): (Vec<&str>, String) = cloud_stop(t.kind, id)?;
            let out = cli::run(t, &args, TIMEOUT).await?;
            Ok(if out.trim().is_empty() { done } else { out.trim().to_string() })
        }
    }
}

/// CLI arguments that stop `id` on a cloud target, and a fallback confirmation.
/// Instances and clusters are stopped, not deleted, so their disks survive.
fn cloud_stop(kind: Kind, id: &str) -> Result<(Vec<&str>, String)> {
    Ok(match kind {
        Kind::Modal => (vec!["app", "stop", id], format!("stopped {id}")),
        Kind::Vast => (vec!["stop", "instance", id], format!("stopped instance {id}")),
        Kind::Skypilot if skypilot::is_managed_job(id) => {
            (vec!["jobs", "cancel", "-y", id], format!("cancelled managed job {id}"))
        }
        Kind::Skypilot => (vec!["stop", "-y", id], format!("stopped cluster {id}")),
        Kind::Host => bail!("not a cloud target"),
    })
}

/// Last `lines` lines of a job's output. `path` skips the lookup poll when already known.
pub async fn logs(t: &Target, id: &str, path: Option<String>, lines: u32) -> Result<String> {
    check_id(id)?;
    let n = lines.to_string();
    match t.kind {
        Kind::Host => {}
        // `modal app logs` streams; read what arrives in a short window.
        Kind::Modal => {
            let out = exec::program_for(&cli::argv(t, &["app", "logs", id]), Duration::from_secs(6)).await?;
            return Ok(cli::tail(&out, lines));
        }
        Kind::Vast => {
            let out = cli::run(t, &["logs", id, "--tail", &n], TIMEOUT).await?;
            return Ok(crate::progress::strip_ansi(&cli::tail(&out, lines)));
        }
        Kind::Skypilot => {
            let args: Vec<&str> = if skypilot::is_managed_job(id) {
                vec!["jobs", "logs", "--no-follow", "--tail", &n, id]
            } else {
                vec!["logs", "--no-follow", id]
            };
            let out = cli::run(t, &args, Duration::from_secs(60)).await?;
            return Ok(crate::progress::strip_ansi(&cli::tail(&out, lines)));
        }
    }
    let path = match path {
        Some(p) => p,
        None => {
            let status = Poller::new(t.clone(), 512).poll().await;
            if let Some(e) = status.error {
                bail!(e);
            }
            status
                .jobs
                .into_iter()
                .find(|j| j.id == id)
                .ok_or_else(|| anyhow::anyhow!("job `{id}` not found on `{}`", t.name))?
                .log_path
                .ok_or_else(|| anyhow::anyhow!("job `{id}` has no readable log"))?
        }
    };
    let script = format!("tail -n {lines} {} | tr '\\r' '\\n' | tail -n {lines}", shq(&path));
    let out = Exec::new(t.host.clone(), t.ssh_args.clone()).sh(&script, TIMEOUT).await?;
    Ok(crate::progress::strip_ansi(&out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn target(sched: &str) -> Target {
        Config::parse(&format!("[[target]]\nname='x'\nhost='h'\nscheduler='{sched}'")).unwrap().targets.remove(0)
    }

    #[test]
    fn commands() {
        let s = target("slurm");
        assert_eq!(command(&s, "61680_[14-19%8]", Action::Cancel).unwrap().unwrap(), "scancel '61680_[14-19]'");
        assert_eq!(command(&s, "2302", Action::Release).unwrap().unwrap(), "scontrol release '2302'");
        let g = target("sge");
        assert_eq!(command(&g, "123.4", Action::Cancel).unwrap().unwrap(), "qdel 123 -t 4");
        assert!(command(&s, "1; rm -rf ~", Action::Cancel).is_err());
        assert!(command(&target("none"), "1", Action::Hold).is_err());
    }

    #[test]
    fn cloud_stops() {
        assert_eq!(cloud_stop(Kind::Vast, "101").unwrap().0, ["stop", "instance", "101"]);
        assert_eq!(cloud_stop(Kind::Skypilot, "7").unwrap().0, ["jobs", "cancel", "-y", "7"]);
        assert_eq!(cloud_stop(Kind::Skypilot, "dev-a100").unwrap().0, ["stop", "-y", "dev-a100"]);
    }
}

/// How a job that left the queue ended, from the scheduler's accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    /// e.g. `COMPLETED · exit 0:0`, `FAILED · exit 1:0`, `TIMEOUT`.
    pub detail: String,
}

/// Look up the final state of `id` (Slurm `sacct`, SGE `qacct`). None when unknown.
pub async fn outcome(t: &Target, id: &str) -> Option<Outcome> {
    check_id(id).ok()?;
    let job = id.split('%').next().unwrap_or(id);
    let script = match t.scheduler {
        Scheduler::Slurm => format!("sacct -j {} -X -n -P -o State,ExitCode 2>/dev/null | head -1", shq(job)),
        Scheduler::Sge => format!("qacct -j {} 2>/dev/null | grep -E '^(failed|exit_status) '", sge_id(job)),
        Scheduler::None => return None,
    };
    let out = Exec::new(t.host.clone(), t.ssh_args.clone()).sh(&script, TIMEOUT).await.ok()?;
    match t.scheduler {
        Scheduler::Slurm => parse_sacct(&out),
        Scheduler::Sge => parse_qacct(&out),
        Scheduler::None => None,
    }
}

fn parse_sacct(out: &str) -> Option<Outcome> {
    let (state, code) = out.lines().next()?.split_once('|')?;
    let state = state.split_whitespace().next()?; // "CANCELLED by 123" → CANCELLED
    if matches!(state, "PENDING" | "RUNNING" | "REQUEUED" | "SUSPENDED") {
        return None;
    }
    let code = code.trim();
    let ok = state == "COMPLETED" && code.split(':').next() == Some("0");
    let detail = if code.is_empty() || code == "0:0" { state.to_string() } else { format!("{state} · exit {code}") };
    Some(Outcome { ok, detail })
}

fn parse_qacct(out: &str) -> Option<Outcome> {
    let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string());
    let status = field("exit_status")?;
    let failed = field("failed").unwrap_or_else(|| "0".into());
    let ok = status == "0" && failed.starts_with('0');
    let detail = if ok {
        "exit 0".to_string()
    } else if !failed.starts_with('0') {
        format!("failed: {failed}")
    } else {
        format!("exit {status}")
    };
    Some(Outcome { ok, detail })
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    #[test]
    fn sacct_states() {
        assert_eq!(parse_sacct("COMPLETED|0:0\n"), Some(Outcome { ok: true, detail: "COMPLETED".into() }));
        assert_eq!(parse_sacct("FAILED|1:0"), Some(Outcome { ok: false, detail: "FAILED · exit 1:0".into() }));
        assert_eq!(parse_sacct("CANCELLED by 1234|0:15").map(|o| o.ok), Some(false));
        assert_eq!(parse_sacct("TIMEOUT|0:0"), Some(Outcome { ok: false, detail: "TIMEOUT".into() }));
        assert_eq!(parse_sacct("RUNNING|0:0"), None);
        assert_eq!(parse_sacct(""), None);
    }

    #[test]
    fn qacct_states() {
        assert_eq!(parse_qacct("failed       0\nexit_status  0\n").map(|o| o.ok), Some(true));
        let o = parse_qacct("failed       0\nexit_status  137\n").unwrap();
        assert_eq!((o.ok, o.detail.as_str()), (false, "exit 137"));
        assert_eq!(parse_qacct("failed       100 : assumedly after job\nexit_status  0\n").map(|o| o.ok), Some(false));
    }
}
