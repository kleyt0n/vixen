//! (Son of) Grid Engine via `qstat -xml -r` + `qstat -j` (log paths).

use super::TailSpec;
use crate::model::{Action, Job, JobState};
use crate::timefmt::{RemoteClock, parse_duration};
use anyhow::{Context, Result};
use std::collections::HashMap;

pub fn script(all_users: bool) -> String {
    let who = if all_users { "'*'" } else { "\"$USER\"" };
    format!(
        r#"if ! q=$(qstat -u {who} -xml -r 2>&1); then echo "@@ERR qstat: $q"; else
echo "@@QSTAT"; printf '%s\n' "$q"
ids=$(qstat -u {who} | awk 'NR>2 && $5 ~ /[rtsSTd]/ {{print $1}}' | sort -u | head -64 | paste -sd, -)
echo "@@QSTATJ"
if [ -n "$ids" ]; then qstat -j "$ids" 2>/dev/null; fi
fi
"#
    )
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub xml: String,
    pub detail: String,
}

fn state(s: &str) -> JobState {
    match () {
        _ if s.contains('E') => JobState::Failed,
        _ if s.contains('h') => JobState::Held,
        _ if s.contains('d') => JobState::Completing,
        _ if s.contains('s') || s.contains('S') || s.contains('T') => JobState::Suspended,
        _ if s.contains('r') || s.contains('t') => JobState::Running,
        _ if s.contains("qw") => JobState::Pending,
        _ => JobState::Other,
    }
}

/// `1-10:1` → 10, `3,5-7:1` → 4.
fn task_count(spec: &str) -> Option<u32> {
    let mut n = 0;
    for part in spec.split(',') {
        let (range, step) = part.split_once(':').unwrap_or((part, "1"));
        let step: u32 = step.parse().ok()?;
        n += match range.split_once('-') {
            Some((a, b)) => (b.parse::<u32>().ok()?.checked_sub(a.parse().ok()?)?) / step.max(1) + 1,
            None => 1,
        };
    }
    Some(n)
}

#[derive(Debug, Default)]
struct JobDetail {
    name: String,
    cwd: Option<String>,
    home: Option<String>,
    stdout: Option<String>,
}

/// Blocks of `qstat -j id1,id2` separated by `=====` lines.
fn parse_detail(text: &str) -> HashMap<String, JobDetail> {
    let mut map = HashMap::new();
    for block in text.split("==============================================================") {
        let mut id = None;
        let mut d = JobDetail::default();
        for line in block.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let v = v.trim();
            match k.trim() {
                "job_number" => id = Some(v.to_string()),
                "job_name" => d.name = v.to_string(),
                "cwd" => d.cwd = Some(v.to_string()),
                "sge_o_home" => d.home = Some(v.to_string()),
                // "NONE:NONE:/path" or "/path"
                "stdout_path_list" => d.stdout = v.rsplit(':').next().map(String::from),
                _ => {}
            }
        }
        if let Some(id) = id {
            map.insert(id, d);
        }
    }
    map
}

/// Returns (path, file appended when path is a directory).
fn log_path(d: &JobDetail, id: &str, task: Option<&str>) -> Option<(String, String)> {
    let base = d.cwd.as_deref().or(d.home.as_deref())?;
    let default_file = match task {
        Some(t) => format!("{}.o{id}.{t}", d.name),
        None => format!("{}.o{id}", d.name),
    };
    let Some(p) = d.stdout.as_deref().filter(|p| !p.is_empty() && *p != "NONE") else {
        return Some((format!("{base}/{default_file}"), default_file));
    };
    let p = p
        .replace("$HOME", d.home.as_deref().unwrap_or(""))
        .replace("$JOB_ID", id)
        .replace("$JOB_NAME", &d.name)
        .replace("$TASK_ID", task.unwrap_or("undefined"));
    let p = if p.starts_with('/') { p } else { format!("{base}/{p}") };
    Some((p, default_file))
}

pub fn build(p: &Parsed, clock: &RemoteClock) -> Result<(Vec<Job>, Vec<TailSpec>)> {
    if p.xml.trim().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let doc = roxmltree::Document::parse(&p.xml).context("qstat -xml output")?;
    let details = parse_detail(&p.detail);
    let mut jobs = Vec::new();
    let mut tails = Vec::new();

    for node in doc.descendants().filter(|n| n.has_tag_name("job_list")) {
        let text = |tag: &str| node.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).map(str::trim);
        let Some(num) = text("JB_job_number") else { continue };
        let raw = text("state").unwrap_or("");
        let st = state(raw);
        let tasks = text("tasks");
        let single_task = tasks.filter(|t| t.chars().all(|c| c.is_ascii_digit()));
        let id = match single_task {
            Some(t) => format!("{num}.{t}"),
            None => num.to_string(),
        };
        let name = text("full_job_name").or(text("JB_name")).unwrap_or("");
        let mut job = Job::new(id, name, st);
        job.raw_state = raw.to_string();
        job.user = text("JB_owner").map(String::from);
        job.array_tasks = tasks.filter(|_| single_task.is_none()).and_then(task_count);

        let mut res = Vec::new();
        for r in node.children().filter(|c| c.has_tag_name("hard_request")) {
            let (Some(k), Some(v)) = (r.attribute("name"), r.text()) else { continue };
            match k {
                "h_rt" => job.time_limit_s = parse_duration(v),
                "gpu" | "gpus" => res.push(format!("gpu:{v}")),
                "mem" | "h_vmem" => res.push(format!("{k}={v}")),
                _ => {}
            }
        }
        if let Some(slots) = text("slots").filter(|s| *s != "1") {
            res.push(format!("{slots} slots"));
        }
        job.resources = Some(res.join(" · ")).filter(|s| !s.is_empty());
        job.queue = text("queue_name").map(|q| q.split('@').next().unwrap_or(q).to_string());
        job.detail = text("queue_name").and_then(|q| q.split_once('@')).map(|(_, h)| h.to_string());
        job.submitted_at = text("JB_submission_time").and_then(|t| clock.timestamp(t));
        job.started_at = text("JAT_start_time").and_then(|t| clock.timestamp(t));
        if st.is_active() || st == JobState::Suspended {
            job.elapsed_s = job.started_at.map(|s| (clock.now - s).max(0) as u64);
        }
        job.actions = match st {
            JobState::Pending => vec![Action::Hold, Action::Cancel],
            JobState::Held => vec![Action::Release, Action::Cancel],
            _ => vec![Action::Cancel],
        };
        if job.elapsed_s.is_some()
            && let Some((path, file)) = details.get(num).and_then(|d| log_path(d, num, single_task))
        {
            job.log_path = Some(path.clone());
            job.actions.push(Action::Logs);
            tails.push(TailSpec { job: jobs.len(), path, dir_file: Some(file) });
        }
        jobs.push(job);
    }
    Ok((jobs, tails))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version='1.0'?>
<job_info xmlns:xsd="http://arc.liv.ac.uk/repos/darcs/sge/source/dist/util/resources/schemas/qstat/qstat.xsd">
  <queue_info>
    <job_list state="running">
      <JB_job_number>123</JB_job_number>
      <JB_name>train</JB_name>
      <full_job_name>train-long-name</full_job_name>
      <JB_owner>alice</JB_owner>
      <state>r</state>
      <JAT_start_time>2026-11-14T08:00:00</JAT_start_time>
      <queue_name>GPU@node-a02</queue_name>
      <slots>8</slots>
      <tasks>4</tasks>
      <hard_request name="h_rt" resource_contribution="0.000000">172800</hard_request>
      <hard_request name="gpu" resource_contribution="0.000000">1</hard_request>
    </job_list>
  </queue_info>
  <job_info>
    <job_list state="pending">
      <JB_job_number>124</JB_job_number>
      <JB_name>eval</JB_name>
      <JB_owner>alice</JB_owner>
      <state>hqw</state>
      <JB_submission_time>2026-11-14T09:00:00</JB_submission_time>
      <slots>1</slots>
      <tasks>1-10:1</tasks>
      <hard_request name="h_rt" resource_contribution="0.000000">2:00:00</hard_request>
    </job_list>
  </job_info>
</job_info>"#;

    const DETAIL: &str = "==============================================================
job_number:                 123
job_name:                   train
sge_o_home:                 /home/alice
cwd:                        /home/alice/proj
stdout_path_list:           NONE:NONE:logs/$JOB_NAME.$JOB_ID.$TASK_ID.out";

    #[test]
    fn parses() {
        let clock = RemoteClock::parse("1763114400 +0000"); // 2025… arbitrary; elapsed clamps ≥ 0
        let (jobs, tails) = build(&Parsed { xml: XML.into(), detail: DETAIL.into() }, &clock).unwrap();
        assert_eq!(jobs.len(), 2);
        let r = &jobs[0];
        assert_eq!(r.id, "123.4");
        assert_eq!(r.name, "train-long-name");
        assert_eq!(r.state, JobState::Running);
        assert_eq!(r.time_limit_s, Some(172_800));
        assert_eq!(r.resources.as_deref(), Some("gpu:1 · 8 slots"));
        assert_eq!(r.detail.as_deref(), Some("node-a02"));
        assert_eq!(tails[0].path, "/home/alice/proj/logs/train.123.4.out");
        let h = &jobs[1];
        assert_eq!(h.state, JobState::Held);
        assert_eq!(h.array_tasks, Some(10));
        assert_eq!(h.time_limit_s, Some(7200));
    }

    #[test]
    fn default_log_location() {
        let d = JobDetail { name: "job".into(), home: Some("/h".into()), ..Default::default() };
        assert_eq!(log_path(&d, "9", None).unwrap().0, "/h/job.o9");
    }
}
