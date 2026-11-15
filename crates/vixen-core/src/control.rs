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
