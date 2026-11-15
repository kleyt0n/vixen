//! Vast.ai instances via `vastai show instances --raw`.
//!
//! Each instance becomes a job, and running instances also get a GPU card from
//! the utilisation Vast reports, so no SSH is needed.

use super::{Poll, cli};
use crate::config::Target;
use crate::model::{Action, Gpu, Job, JobState};
use crate::timefmt::RemoteClock;
use anyhow::{Result, bail};
use serde_json::Value;
use std::time::Duration;

pub async fn poll(t: &Target) -> Result<Poll> {
    let out = cli::run(t, &["show", "instances", "--raw"], Duration::from_secs(40)).await?;
    let clock = RemoteClock::local();
    let (jobs, gpus) = parse(&out, clock.now)?;
    Ok(Poll { clock: Some(clock), jobs, gpus, ..Default::default() })
}

fn id_of(v: &Value) -> Option<String> {
    match v.get("id")? {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

pub fn parse(out: &str, now: i64) -> Result<(Vec<Job>, Vec<Gpu>)> {
    let v = cli::json(out, "vastai show instances --raw")?;
    let Value::Array(rows) = v else {
        bail!("vastai: {}", cli::str_field(&v, &["msg", "error"]).unwrap_or("unexpected response"));
    };
    let (mut jobs, mut gpus) = (Vec::new(), Vec::new());
    for row in &rows {
        let Some(id) = id_of(row) else { continue };
        // `null` actual_status means the instance is still being provisioned.
        let raw = cli::str_field(row, &["actual_status"]).unwrap_or("provisioning").to_string();
        let st = match raw.as_str() {
            "running" => JobState::Running,
            "provisioning" | "created" | "loading" | "rebooting" | "scheduling" => JobState::Pending,
            "stopped" | "frozen" => JobState::Held,
            "exited" | "unknown" | "offline" => JobState::Failed,
            _ => JobState::Other,
        };
        let gpu_name = cli::str_field(row, &["gpu_name"]).unwrap_or("GPU");
        let n = cli::num_field(row, &["num_gpus"]).unwrap_or(1.0).max(1.0) as u32;
        let label = cli::str_field(row, &["label"]);
        let mut job = Job::new(&id, label.unwrap_or(&format!("{n}× {gpu_name}")), st);
        job.raw_state = raw;
        let mut res = format!("{n}× {gpu_name}");
        if let Some(dph) = cli::num_field(row, &["dph_total"]) {
            res.push_str(&format!(" · ${dph:.2}/h"));
        }
        job.resources = Some(res);
        job.detail = cli::str_field(row, &["status_msg"]).map(|s| super::truncate(s.trim(), 200));
        job.started_at = cli::num_field(row, &["start_date"]).map(|s| s as i64);
        if st == JobState::Running {
            job.elapsed_s = job.started_at.map(|s| (now - s).max(0) as u64);
        }
        job.actions = if st == JobState::Running { vec![Action::Logs, Action::Cancel] } else { vec![Action::Logs] };

        if st == JobState::Running {
            // Vast reports one utilisation figure per instance, so one card per instance.
            let per_gpu_mb = cli::num_field(row, &["gpu_ram"]);
            gpus.push(Gpu {
                index: gpus.len() as u32,
                name: if n > 1 { format!("{gpu_name} ×{n}") } else { gpu_name.to_string() },
                util: cli::num_field(row, &["gpu_util"]).map(|u| u as f32),
                mem_total_mb: per_gpu_mb.map(|m| (m * n as f64) as u64),
                temp_c: cli::num_field(row, &["gpu_temp"]).map(|c| c as f32),
                job_id: Some(id.clone()),
                node: Some(label.map(String::from).unwrap_or_else(|| format!("instance {id}"))),
                ..Default::default()
            });
        }
        jobs.push(job);
    }
    Ok((jobs, gpus))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_instances() {
        let out = r#"[notice] Update available: vastai 1.12
[
  {"id": 101, "label": "train-llm", "actual_status": "running", "num_gpus": 4, "gpu_name": "RTX 4090",
   "gpu_util": 87.5, "gpu_temp": 66, "gpu_ram": 24564, "dph_total": 1.6123, "start_date": 1794650400.5,
   "status_msg": "success, running pytorch/pytorch"},
  {"id": 102, "label": null, "actual_status": null, "num_gpus": 1, "gpu_name": "H100 SXM", "dph_total": 2.5},
  {"id": 103, "label": "old", "actual_status": "stopped", "num_gpus": 1, "gpu_name": "A100 PCIE"}
]"#;
        let (jobs, gpus) = parse(out, 1_794_657_600).unwrap();
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].name, "train-llm");
        assert_eq!(jobs[0].state, JobState::Running);
        assert_eq!(jobs[0].elapsed_s, Some(7200));
        assert_eq!(jobs[0].resources.as_deref(), Some("4× RTX 4090 · $1.61/h"));
        assert_eq!(jobs[1].state, JobState::Pending);
        assert_eq!(jobs[1].name, "1× H100 SXM");
        assert_eq!(jobs[2].state, JobState::Held);
        assert_eq!(jobs[2].actions, vec![Action::Logs]);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "RTX 4090 ×4");
        assert_eq!(gpus[0].util, Some(87.5));
        assert_eq!(gpus[0].mem_total_mb, Some(98_256));
        assert_eq!(gpus[0].job_id.as_deref(), Some("101"));
    }

    #[test]
    fn api_error() {
        assert!(parse(r#"{"success": false, "msg": "Invalid API key"}"#, 0).is_err());
        assert!(parse("failed with error 401: Unauthorized", 0).is_err());
    }
}
