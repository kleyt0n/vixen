//! Slurm via `squeue` + `scontrol` (log paths) + optional `srun --overlap nvidia-smi`.

use super::{JobGpuSpec, TailSpec};
use crate::model::{Action, Job, JobState};
use crate::timefmt::{RemoteClock, parse_duration};
use std::collections::HashMap;

const FORMAT: &str = "%i|%j|%u|%T|%M|%l|%D|%b|%R|%S|%V|%P|%A";

pub fn script(all_users: bool) -> String {
    let me = if all_users { "" } else { "--me" };
    format!(
        r#"if ! q=$(squeue {me} -h -o '{FORMAT}' 2>&1); then echo "@@ERR squeue: $q"; else
echo "@@SQUEUE"; printf '%s\n' "$q"
echo "@@SCONTROL"
for j in $(printf '%s\n' "$q" | awk -F'|' '$4=="RUNNING"||$4=="COMPLETING"||$4=="SUSPENDED"||$4=="CONFIGURING"{{print $13}}' | head -64); do
  scontrol -o show job "$j" 2>/dev/null
done
fi
"#
    )
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub queue: Vec<String>,
    pub details: Vec<String>,
}

fn state(raw: &str, reason: &str) -> JobState {
    match raw {
        "RUNNING" | "CONFIGURING" => JobState::Running,
        "PENDING" | "REQUEUED" if reason.contains("Held") => JobState::Held,
        "PENDING" | "REQUEUED" => JobState::Pending,
        "REQUEUE_HOLD" | "RESV_DEL_HOLD" => JobState::Held,
        "COMPLETING" | "STAGE_OUT" => JobState::Completing,
        "SUSPENDED" | "STOPPED" => JobState::Suspended,
        "FAILED" | "NODE_FAIL" | "OUT_OF_MEMORY" | "TIMEOUT" | "BOOT_FAIL" | "DEADLINE" | "PREEMPTED" => {
            JobState::Failed
        }
        _ => JobState::Other,
    }
}

/// `61680_[14-19%8]` → 6, `1_[1,3,5-7]` → 5.
pub fn array_count(id: &str) -> Option<u32> {
    let inner = id.split_once("_[")?.1.trim_end_matches(']');
    let inner = inner.split('%').next()?;
    let mut n = 0;
    for part in inner.split(',') {
        let part = part.split(':').next()?;
        n += match part.split_once('-') {
            Some((a, b)) => b.parse::<u32>().ok()?.checked_sub(a.parse().ok()?)? + 1,
            None => 1,
        };
    }
    Some(n)
}

/// `key=value` tokens of a one-line `scontrol show job`.
fn kv(line: &str) -> HashMap<&str, &str> {
    line.split_whitespace().filter_map(|t| t.split_once('=')).collect()
}

fn expand_path(p: &str, job: &HashMap<&str, &str>) -> String {
    let get = |k: &str| job.get(k).copied().unwrap_or("");
    let task = get("ArrayTaskId");
    let master = if job.contains_key("ArrayJobId") { get("ArrayJobId") } else { get("JobId") };
    p.replace("%%", "\u{0}")
        .replace("%A", master)
        .replace("%a", task)
        .replace("%j", get("JobId"))
        .replace("%x", get("JobName"))
        .replace("%u", get("UserId").split('(').next().unwrap_or(""))
        .replace("%N", get("BatchHost"))
        .replace('\u{0}', "%")
}

pub fn build(p: &Parsed, clock: &RemoteClock, job_gpus: bool) -> (Vec<Job>, Vec<TailSpec>, Vec<JobGpuSpec>) {
    let details: HashMap<String, HashMap<&str, &str>> =
        p.details.iter().map(|l| kv(l)).filter_map(|m| Some((m.get("JobId")?.to_string(), m))).collect();

    let mut jobs = Vec::new();
    let mut tails = Vec::new();
    let mut gpus = Vec::new();
    for line in &p.queue {
        let f: Vec<&str> = line.split('|').collect();
        let [id, name, user, raw, used, limit, nodes, tres, reason_or_nodes, start, submit, part, numeric] = f[..]
        else {
            continue;
        };
        let st = state(raw, reason_or_nodes);
        let mut job = Job::new(id, name, st);
        job.raw_state = raw.to_string();
        job.user = Some(user.to_string());
        job.queue = Some(part.to_string()).filter(|s| !s.is_empty());
        job.detail =
            Some(reason_or_nodes.trim_matches(['(', ')']).to_string()).filter(|s| !s.is_empty() && s != "None");
        let mut res = tres.replace("gres/", "").replace("gres:", "");
        if res == "N/A" {
            res.clear();
        }
        if nodes != "1" {
            res = if res.is_empty() { format!("{nodes} nodes") } else { format!("{res} × {nodes} nodes") };
        }
        job.resources = Some(res).filter(|s| !s.is_empty());
        job.array_tasks = array_count(id);
        job.submitted_at = clock.timestamp(submit);
        job.time_limit_s = parse_duration(limit);
        match st {
            JobState::Running | JobState::Completing | JobState::Suspended => {
                job.elapsed_s = parse_duration(used);
                job.started_at = clock.timestamp(start);
                job.actions = vec![Action::Cancel];
            }
            JobState::Pending => {
                job.est_start_at = clock.timestamp(start);
                job.actions = vec![Action::Hold, Action::Cancel];
            }
            JobState::Held => job.actions = vec![Action::Release, Action::Cancel],
            _ => job.actions = vec![Action::Cancel],
        }
        let idx = jobs.len();
        if let Some(d) = details.get(numeric) {
            if let Some(out) = d.get("StdOut").filter(|s| !s.is_empty() && **s != "/dev/null") {
                let path = expand_path(out, d);
                job.log_path = Some(path.clone());
                job.actions.push(Action::Logs);
                tails.push(TailSpec { job: idx, path, dir_file: None });
            }
            if job_gpus && st == JobState::Running && job.resources.as_deref().is_some_and(|r| r.contains("gpu")) {
                gpus.push(JobGpuSpec { job: idx, numeric_id: numeric.to_string() });
            }
        }
        jobs.push(job);
    }
    (jobs, tails, gpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: &str = "\
61680_[14-19%8]|train-vit|alice|PENDING|0:00|1-23:30:00|1|gres/gpu:1|(Priority)|2026-11-15T00:50:00|2026-11-14T00:47:42|gpu|61680
2302|sweep-a|alice|PENDING|0:00|12:00:00|1|gres/gpu:1|(JobHeldUser)|N/A|2026-10-26T15:51:37|main|2302
61680_13|train-vit|alice|RUNNING|9:08:08|1-23:30:00|1|gres/gpu:1|node-a01|2026-11-14T08:08:20|2026-11-14T00:47:42|gpu|64397";

    const D: &str = "JobId=64397 JobName=train-vit UserId=alice(1) ArrayJobId=61680 ArrayTaskId=13 \
                     StdOut=/home/u/logs/%x-%A_%a.log BatchHost=node-a01";

    #[test]
    fn parses_queue() {
        let p = Parsed { queue: Q.lines().map(String::from).collect(), details: vec![D.into()] };
        let clock = RemoteClock::parse("1763130000 +0000");
        let (jobs, tails, gpus) = build(&p, &clock, true);
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].state, JobState::Pending);
        assert_eq!(jobs[0].array_tasks, Some(6));
        assert!(jobs[0].est_start_at.is_some());
        assert_eq!(jobs[1].state, JobState::Held);
        assert_eq!(jobs[1].actions, vec![Action::Release, Action::Cancel]);
        let r = &jobs[2];
        assert_eq!(r.state, JobState::Running);
        assert_eq!(r.elapsed_s, Some(9 * 3600 + 8 * 60 + 8));
        assert_eq!(r.time_left_s(), Some(47 * 3600 + 30 * 60 - (9 * 3600 + 8 * 60 + 8)));
        assert_eq!(r.resources.as_deref(), Some("gpu:1"));
        assert_eq!(r.detail.as_deref(), Some("node-a01"));
        assert_eq!(tails[0].path, "/home/u/logs/train-vit-61680_13.log");
        assert_eq!(gpus[0].numeric_id, "64397");
    }

    #[test]
    fn arrays() {
        assert_eq!(array_count("1_[1,3,5-7]"), Some(5));
        assert_eq!(array_count("66676_[0-9]"), Some(10));
        assert_eq!(array_count("12"), None);
    }
}
