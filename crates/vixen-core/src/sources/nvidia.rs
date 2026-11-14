//! NVIDIA hosts via `nvidia-smi` (+ Linux host stats and GPU processes).

use crate::model::{Action, Gpu, HostStats, Job, JobState};
use anyhow::{Result, bail};

pub const QUERY: &str = "nvidia-smi --query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,\
temperature.gpu,power.draw,power.limit --format=csv,noheader,nounits";

pub const HOST_SCRIPT: &str = r#"echo "@@HOST $(hostname -s) $(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN) $(cut -d' ' -f1 /proc/loadavg 2>/dev/null)"
free -m 2>/dev/null | awk '/^Mem:/{print $2, $3}'
"#;

pub const GPU_SCRIPT: &str = r#"echo "@@GPU"
if command -v nvidia-smi >/dev/null 2>&1; then
nvidia-smi --query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit --format=csv,noheader,nounits 2>&1
else echo "ERR nvidia-smi not found"; fi
echo "@@PROC"
nvidia-smi --query-compute-apps=pid,gpu_uuid,used_memory --format=csv,noheader,nounits 2>/dev/null | while IFS=', ' read -r pid uuid mem; do
  [ -n "$pid" ] || continue
  info=$(ps -o user=,etimes=,args= -p "$pid" 2>/dev/null | head -1) || continue
  log=$(readlink "/proc/$pid/fd/1" 2>/dev/null)
  echo "$pid|$uuid|$mem|$log|$info"
done
"#;

pub fn parse_host(arg: &str, lines: &[&str]) -> Option<HostStats> {
    let mut it = arg.split_whitespace();
    let hostname = it.next()?.to_string();
    let cpus = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let load1 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let mut mem =
        lines.first().map(|l| l.split_whitespace().filter_map(|x| x.parse::<u64>().ok())).into_iter().flatten();
    Some(HostStats {
        hostname,
        cpus,
        load1,
        mem_total_mb: mem.next().unwrap_or(0),
        mem_used_mb: mem.next().unwrap_or(0),
    })
}

fn num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.trim().parse().ok() // "[N/A]", "[Not Supported]" → None
}

pub fn parse_gpus(lines: &[&str]) -> Result<Vec<Gpu>> {
    let mut gpus = Vec::new();
    for line in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 9 {
            if line.starts_with("ERR") || line.contains("failed") || line.contains("NVIDIA-SMI") {
                bail!("{}", line.trim_start_matches("ERR ").trim());
            }
            continue;
        }
        let Some(index) = num(f[0]) else { continue };
        gpus.push(Gpu {
            index,
            name: f[2].trim_start_matches("NVIDIA ").to_string(),
            util: num(f[3]),
            mem_used_mb: num(f[4]),
            mem_total_mb: num(f[5]),
            unified: false,
            temp_c: num(f[6]),
            power_w: num(f[7]),
            power_limit_w: num(f[8]),
            job_id: None,
            node: None,
            uuid: Some(f[1].to_string()),
        });
    }
    Ok(gpus)
}

/// GPU processes as jobs, each with an optional stdout log (from /proc/<pid>/fd/1).
pub fn parse_procs(lines: &[&str], gpus: &[Gpu], me: &str, all_users: bool, now: i64) -> Vec<(Job, Option<String>)> {
    let mut out: Vec<(Job, Option<String>)> = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.splitn(5, '|').collect();
        let [pid, uuid, mem, log, info] = f[..] else { continue };
        let mut it = info.split_whitespace();
        let (Some(user), Some(etimes)) = (it.next(), it.next().and_then(|s| s.parse::<u64>().ok())) else { continue };
        if !all_users && !me.is_empty() && user != me {
            continue;
        }
        let args: Vec<&str> = it.collect();
        let gpu = gpus.iter().find(|g| g.uuid.as_deref() == Some(uuid)).map(|g| g.index);
        // Same PID on several GPUs → one job.
        if let Some((job, _)) = out.iter_mut().find(|(j, _)| j.id == pid) {
            if let (Some(g), Some(r)) = (gpu, job.resources.as_mut()) {
                r.push_str(&format!(" +gpu{g}"));
            }
            continue;
        }
        let short: Vec<String> = args.iter().take(2).map(|a| a.rsplit('/').next().unwrap_or(a).to_string()).collect();
        let mut job = Job::new(pid, short.join(" "), JobState::Running);
        job.user = Some(user.to_string());
        job.raw_state = "process".into();
        job.detail = Some(super::truncate(&args.join(" "), 200));
        job.resources = Some(match gpu {
            Some(g) => format!("gpu{g} · {mem} MiB"),
            None => format!("{mem} MiB"),
        });
        job.elapsed_s = Some(etimes);
        job.started_at = Some(now - etimes as i64);
        job.actions = vec![Action::Cancel];
        let log = log.trim();
        let log = (log.starts_with('/') && !log.starts_with("/dev/")).then(|| log.to_string());
        if log.is_some() {
            job.log_path = log.clone();
            job.actions.push(Action::Logs);
        }
        out.push((job, log));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gb10_unified() {
        let g = parse_gpus(&["0, GPU-4f7b, NVIDIA GB10, 96, [N/A], [N/A], 47, 17.99, [N/A]"]).unwrap();
        assert_eq!(g[0].name, "GB10");
        assert_eq!(g[0].util, Some(96.0));
        assert_eq!(g[0].mem_total_mb, None);
        assert_eq!(g[0].power_w, Some(17.99));
    }

    #[test]
    fn missing_smi_is_error() {
        assert!(parse_gpus(&["ERR nvidia-smi not found"]).is_err());
    }

    #[test]
    fn procs() {
        let gpus = parse_gpus(&["0, GPU-a, NVIDIA H100 80GB HBM3, 99, 70000, 81559, 60, 500.1, 700.00"]).unwrap();
        let lines = [
            "4242|GPU-a|69000|/home/alice/run.log|alice 3600 /usr/bin/python train.py --lr 3e-4",
            "77|GPU-a|10|/dev/pts/1|bob 5 python x.py",
        ];
        let jobs = parse_procs(&lines, &gpus, "alice", false, 10_000);
        assert_eq!(jobs.len(), 1);
        let (j, log) = &jobs[0];
        assert_eq!(j.name, "python train.py");
        assert_eq!(j.resources.as_deref(), Some("gpu0 · 69000 MiB"));
        assert_eq!(log.as_deref(), Some("/home/alice/run.log"));
        assert_eq!(j.started_at, Some(6_400));
    }

    #[test]
    fn host() {
        let h = parse_host("gpu-box 20 0.53", &["124610 5612"]).unwrap();
        assert_eq!((h.cpus, h.mem_total_mb, h.mem_used_mb), (20, 124610, 5612));
    }
}
