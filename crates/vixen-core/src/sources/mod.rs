//! Backends. Each poll is at most two round trips per target:
//!
//! 1. **status** — one composed shell script: clock, host stats, GPUs,
//!    scheduler queue and log paths for active jobs;
//! 2. **detail** — log tails (and in-job GPU samples) for active jobs.

pub mod apple;
pub mod modal;
pub mod nvidia;
pub mod sge;
pub mod slurm;

use crate::config::{Kind, Scheduler, Target};
use crate::exec::{Exec, sections, shq};
use crate::model::{Gpu, HostStats, Job};
use crate::progress;
use crate::timefmt::RemoteClock;
use anyhow::{Result, bail};
use std::time::Duration;

/// Raw poll result before ETA estimation.
#[derive(Debug, Default)]
pub struct Poll {
    pub clock: Option<RemoteClock>,
    pub host: Option<HostStats>,
    pub gpus: Vec<Gpu>,
    pub jobs: Vec<Job>,
}

/// A log file to tail in phase 2. If `path` is a directory, `dir_file` is appended (SGE).
#[derive(Debug, Clone)]
pub struct TailSpec {
    pub job: usize,
    pub path: String,
    pub dir_file: Option<String>,
}

/// A Slurm job whose GPUs should be sampled from inside the allocation.
#[derive(Debug, Clone)]
pub struct JobGpuSpec {
    pub job: usize,
    pub numeric_id: String,
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(40);
const DETAIL_TIMEOUT: Duration = Duration::from_secs(40);

fn local_mac(t: &Target) -> bool {
    cfg!(target_os = "macos") && t.host.is_none()
}

pub async fn poll(t: &Target, exec: &Exec, tail_bytes: u32) -> Result<Poll> {
    if t.kind == Kind::Modal {
        return modal::poll(t).await;
    }

    // ---- phase 1: status ---------------------------------------------------
    let mut script = String::from("set +e\necho \"@@TZ $(date +%s) $(date +%z)\"\necho \"@@USER $(id -un)\"\n");
    let mac = local_mac(t);
    script.push_str(if mac { apple::SCRIPT } else { nvidia::HOST_SCRIPT });
    if t.host_gpus {
        script.push_str(if mac { apple::GPU_SCRIPT } else { nvidia::GPU_SCRIPT });
    }
    match t.scheduler {
        Scheduler::Slurm => script.push_str(&slurm::script(t.all_users)),
        Scheduler::Sge => script.push_str(&sge::script(t.all_users)),
        Scheduler::None => {}
    }

    script.push_str("exit 0\n");
    let out = exec.sh(&script, STATUS_TIMEOUT).await?;
    let mut poll = Poll::default();
    let mut me = String::new();
    let mut tails: Vec<TailSpec> = Vec::new();
    let mut job_gpus: Vec<JobGpuSpec> = Vec::new();
    let mut slurm_state = slurm::Parsed::default();
    let mut sge_state = sge::Parsed::default();

    let secs = sections(&out);
    for (name, arg, lines) in &secs {
        match *name {
            "TZ" => poll.clock = Some(RemoteClock::parse(arg)),
            "USER" => me = arg.to_string(),
            "ERR" => {
                bail!("{}", std::iter::once(*arg).chain(lines.iter().copied()).collect::<Vec<_>>().join(" ").trim())
            }
            _ => {}
        }
    }
    let clock = poll.clock.unwrap_or_else(RemoteClock::local);
    for (name, arg, lines) in &secs {
        match *name {
            "HOST" => poll.host = nvidia::parse_host(arg, lines),
            "MACHOST" => poll.host = apple::parse_host(arg, lines),
            "GPU" => poll.gpus = nvidia::parse_gpus(lines)?,
            "APPLEGPU" => poll.gpus = apple::parse_gpus(lines),
            "PROC" if t.scheduler == Scheduler::None => {
                for (job, log) in nvidia::parse_procs(lines, &poll.gpus, &me, t.all_users, clock.now) {
                    if let Some(path) = log {
                        tails.push(TailSpec { job: poll.jobs.len(), path, dir_file: None });
                    }
                    poll.jobs.push(job);
                }
            }
            "SQUEUE" => slurm_state.queue = lines.iter().map(|s| s.to_string()).collect(),
            "SCONTROL" => slurm_state.details = lines.iter().map(|s| s.to_string()).collect(),
            "QSTAT" => sge_state.xml = lines.join("\n"),
            "QSTATJ" => sge_state.detail = lines.join("\n"),
            _ => {}
        }
    }

    // Unified-memory GPUs (GB10) report N/A — fall back to system RAM.
    if let Some(h) = &poll.host {
        for g in poll.gpus.iter_mut().filter(|g| g.mem_total_mb.is_none()) {
            g.unified = true;
            g.mem_used_mb = g.mem_used_mb.or(Some(h.mem_used_mb));
            g.mem_total_mb = Some(h.mem_total_mb);
        }
    }

    match t.scheduler {
        Scheduler::Slurm => {
            let (jobs, tl, jg) = slurm::build(&slurm_state, &clock, t.job_gpus);
            poll.jobs = jobs;
            tails = tl;
            job_gpus = jg;
        }
        Scheduler::Sge => {
            let (jobs, tl) = sge::build(&sge_state, &clock)?;
            poll.jobs = jobs;
            tails = tl;
        }
        Scheduler::None => {}
    }

    // ---- phase 2: detail ---------------------------------------------------
    if tails.is_empty() && job_gpus.is_empty() {
        return Ok(poll);
    }
    let script = detail_script(&tails, &job_gpus, tail_bytes);
    match exec.sh(&script, DETAIL_TIMEOUT).await {
        Ok(out) => apply_detail(&mut poll, &out, &tails, &job_gpus)?,
        Err(e) => {
            // Status is still useful without logs; surface the problem on the jobs.
            for spec in &tails {
                poll.jobs[spec.job].last_line = Some(format!("(log unavailable: {e})"));
            }
        }
    }
    Ok(poll)
}

fn detail_script(tails: &[TailSpec], gpus: &[JobGpuSpec], tail_bytes: u32) -> String {
    let mut s = String::from("set +e\nnow=$(date +%s)\n");
    if !gpus.is_empty() {
        s.push_str("d=$(mktemp -d)\n");
        for (i, g) in gpus.iter().enumerate() {
            s.push_str(&format!(
                "(timeout 15 srun --jobid={} --overlap -N1 -n1 --quiet {} > \"$d/{i}\" 2>/dev/null) &\n",
                shq(&g.numeric_id),
                nvidia::QUERY
            ));
        }
    }
    for (i, t) in tails.iter().enumerate() {
        s.push_str(&format!("f={}\n", shq(&t.path)));
        if let Some(name) = &t.dir_file {
            s.push_str(&format!("[ -d \"$f\" ] && f=\"$f\"/{}\n", shq(name)));
        }
        s.push_str(&format!(
            "echo \"@@TAIL {i} $f\"\nif [ -r \"$f\" ]; then m=$(stat -c %Y \"$f\" 2>/dev/null || stat -f %m \"$f\"); \
             echo \"$((now - m))\"; tail -c {tail_bytes} \"$f\"; echo; fi\n"
        ));
    }
    if !gpus.is_empty() {
        s.push_str("wait\n");
        for i in 0..gpus.len() {
            s.push_str(&format!("echo \"@@JOBGPU {i}\"; cat \"$d/{i}\"\n"));
        }
        s.push_str("rm -rf \"$d\"\n");
    }
    s.push_str("exit 0\n");
    s
}

fn apply_detail(poll: &mut Poll, out: &str, tails: &[TailSpec], gpus: &[JobGpuSpec]) -> Result<()> {
    for (name, arg, lines) in sections(out) {
        let (idx, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let Ok(idx) = idx.parse::<usize>() else { continue };
        match name {
            "TAIL" => {
                let Some(spec) = tails.get(idx) else { continue };
                let job = &mut poll.jobs[spec.job];
                if !rest.is_empty() {
                    job.log_path = Some(rest.to_string());
                }
                let Some((age, body)) = lines.split_first() else {
                    job.last_line = Some("(log not readable)".into());
                    continue;
                };
                job.log_age_s = age.trim().parse().ok();
                let clean = progress::clean_lines(&body.join("\n"));
                job.progress = progress::parse(&clean);
                job.last_line = clean.last().map(|l| truncate(l, 240));
            }
            "JOBGPU" => {
                let Some(spec) = gpus.get(idx) else { continue };
                let job = &poll.jobs[spec.job];
                let (id, node) = (job.id.clone(), job.detail.clone());
                for mut g in nvidia::parse_gpus(&lines).unwrap_or_default() {
                    g.job_id = Some(id.clone());
                    g.node = node.clone();
                    poll.gpus.push(g);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).chain(std::iter::once('…')).collect() }
}
