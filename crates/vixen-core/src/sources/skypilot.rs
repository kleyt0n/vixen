//! SkyPilot via `sky status -o json` (clusters) and `sky jobs queue -o json`
//! (managed jobs). This covers every cloud SkyPilot launches on.
//!
//! Job ids: clusters are keyed by name (SkyPilot names start with a letter),
//! and managed jobs by their numeric id, so control can tell them apart.

use super::{Poll, cli};
use crate::config::Target;
use crate::model::{Action, Job, JobState};
use crate::timefmt::RemoteClock;
use anyhow::{Result, bail};
use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(40);

/// A managed job id (numeric) rather than a cluster name.
pub fn is_managed_job(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())
}

pub async fn poll(t: &Target) -> Result<Poll> {
    let (status, queue) = tokio::join!(
        cli::run(t, &["status", "-o", "json"], TIMEOUT),
        cli::run(t, &["jobs", "queue", "--skip-finished", "-o", "json"], TIMEOUT),
    );
    let clock = RemoteClock::local();
    let mut jobs = parse_clusters(&status?, clock.now)?;
    // No jobs controller yet is normal; the cluster list is still useful.
    if let Ok(out) = queue {
        jobs.extend(parse_jobs(&out, clock.now).unwrap_or_default());
    }
    Ok(Poll { clock: Some(clock), jobs, ..Default::default() })
}

fn array(out: &str, what: &str) -> Result<Vec<Value>> {
    match cli::json(out, what)? {
        Value::Array(a) => Ok(a),
        v => bail!("{what}: {}", cli::str_field(&v, &["detail", "error", "message"]).unwrap_or("unexpected response")),
    }
}

pub fn parse_clusters(out: &str, now: i64) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    for c in array(out, "sky status -o json")? {
        let Some(name) = cli::str_field(&c, &["name"]) else { continue };
        let raw = cli::str_field(&c, &["status"]).unwrap_or("").to_string();
        let st = match raw.as_str() {
            "UP" => JobState::Running,
            "INIT" | "PENDING" => JobState::Pending,
            "AUTOSTOPPING" => JobState::Completing,
            "STOPPED" => JobState::Held,
            _ => JobState::Other,
        };
        let mut job = Job::new(name, name, st);
        job.raw_state = format!("cluster · {}", raw.to_lowercase());
        job.user = cli::str_field(&c, &["user_name"]).map(String::from);
        job.resources = cli::str_field(&c, &["resources_str", "accelerators"]).map(String::from);
        job.detail = cli::str_field(&c, &["init_status_reason"]).map(String::from).or_else(|| {
            let infra: Vec<&str> = ["cloud", "region"].iter().filter_map(|k| cli::str_field(&c, &[k])).collect();
            (!infra.is_empty()).then(|| infra.join(" / "))
        });
        job.started_at = cli::num_field(&c, &["launched_at"]).map(|s| s as i64);
        if job.state.is_active() {
            job.elapsed_s = job.started_at.map(|s| (now - s).max(0) as u64);
        }
        job.actions = match st {
            JobState::Running => vec![Action::Logs, Action::Cancel],
            _ => Vec::new(),
        };
        jobs.push(job);
    }
    Ok(jobs)
}

pub fn parse_jobs(out: &str, now: i64) -> Result<Vec<Job>> {
    let mut jobs: Vec<Job> = Vec::new();
    for r in array(out, "sky jobs queue -o json")? {
        let Some(id) = cli::num_field(&r, &["job_id"]).map(|n| (n as u64).to_string()) else { continue };
        // Multi-task jobs have one row per task; show the job once.
        if jobs.iter().any(|j| j.id == id) {
            continue;
        }
        let raw = cli::str_field(&r, &["status"]).unwrap_or("").to_string();
        let st = match raw.as_str() {
            "RUNNING" => JobState::Running,
            "PENDING" | "SUBMITTED" | "STARTING" => JobState::Pending,
            "CANCELLING" | "WINDING_DOWN" => JobState::Completing,
            "SUCCEEDED" | "CANCELLED" => continue,
            s if s.starts_with("FAILED") => JobState::Failed,
            _ => JobState::Other, // RECOVERING
        };
        let name = cli::str_field(&r, &["job_name", "task_name"]).unwrap_or(&id).to_string();
        let mut job = Job::new(&id, name, st);
        job.raw_state = raw.to_lowercase();
        job.user = cli::str_field(&r, &["user_name"]).map(String::from);
        job.resources = cli::str_field(&r, &["cluster_resources", "resources"]).map(String::from);
        job.detail = cli::str_field(&r, &["failure_reason", "details", "infra"]).map(|s| super::truncate(s, 200));
        job.submitted_at = cli::num_field(&r, &["submitted_at"]).map(|s| s as i64);
        job.started_at = cli::num_field(&r, &["start_at"]).filter(|s| *s > 0.0).map(|s| s as i64);
        if job.state.is_active() {
            job.elapsed_s = cli::num_field(&r, &["job_duration"])
                .map(|d| d as u64)
                .or_else(|| job.started_at.map(|s| (now - s).max(0) as u64));
        }
        job.actions = match st {
            JobState::Failed => vec![Action::Logs],
            _ => vec![Action::Logs, Action::Cancel],
        };
        jobs.push(job);
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_657_600;

    #[test]
    fn clusters() {
        let out = r#"[
          {"name": "dev-a100", "launched_at": 1794650400, "status": "UP", "autostop": -1, "to_down": false,
           "cluster_hash": "x", "cluster_ever_up": true, "user_hash": "u", "user_name": "alice", "workspace": "default",
           "is_managed": false, "nodes": 1, "cloud": "Lambda", "region": "us-east-1", "resources_str": "1x Lambda(gpu_1x_a100, {'A100': 1})"},
          {"name": "idle", "launched_at": 1794000000, "status": "STOPPED", "nodes": 1}
        ]"#;
        let jobs = parse_clusters(out, NOW).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].state, JobState::Running);
        assert_eq!(jobs[0].elapsed_s, Some(7200));
        assert_eq!(jobs[0].detail.as_deref(), Some("Lambda / us-east-1"));
        assert_eq!(jobs[0].raw_state, "cluster · up");
        assert_eq!(jobs[1].state, JobState::Held);
        assert!(jobs[1].actions.is_empty());
        assert!(!is_managed_job(&jobs[0].id));
    }

    #[test]
    fn managed_jobs() {
        let out = r#"[
          {"_job_id": 7, "job_id": 7, "task_id": 0, "job_name": "finetune", "status": "RUNNING",
           "submitted_at": 1794640000.2, "start_at": 1794650400.0, "job_duration": 7000.5,
           "cluster_resources": "1x AWS(p4d.24xlarge, {'A100': 8})", "user_name": "alice"},
          {"job_id": 7, "task_id": 1, "job_name": "finetune", "status": "PENDING"},
          {"job_id": 8, "job_name": "sweep", "status": "FAILED_NO_RESOURCE", "failure_reason": "no capacity"},
          {"job_id": 9, "job_name": "done", "status": "SUCCEEDED"}
        ]"#;
        let jobs = parse_jobs(out, NOW).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].id, "7");
        assert_eq!(jobs[0].elapsed_s, Some(7000));
        assert_eq!(jobs[1].state, JobState::Failed);
        assert_eq!(jobs[1].detail.as_deref(), Some("no capacity"));
        assert!(is_managed_job("7"));
    }

    #[test]
    fn api_error() {
        assert!(parse_clusters("sky.exceptions.ApiServerConnectionError: Could not connect", NOW).is_err());
    }
}
