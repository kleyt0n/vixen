//! Apple Silicon GPU on this Mac (no sudo): `ioreg` PerformanceStatistics.

use crate::model::{Gpu, HostStats};

pub const SCRIPT: &str = r#"echo "@@MACHOST $(hostname -s) $(sysctl -n hw.ncpu) $(sysctl -n vm.loadavg | awk '{print $2}') $(sysctl -n hw.memsize)"
vm_stat
"#;

pub const GPU_SCRIPT: &str = r#"echo "@@APPLEGPU"
ioreg -r -d 1 -w 0 -c IOAccelerator | grep -E '"model"|"PerformanceStatistics"|"gpu-core-count"'
"#;

pub fn parse_host(arg: &str, lines: &[&str]) -> Option<HostStats> {
    let mut it = arg.split_whitespace();
    let hostname = it.next()?.to_string();
    let cpus = it.next()?.parse().ok()?;
    let load1 = it.next()?.parse().ok()?;
    let total: u64 = it.next()?.parse().ok()?;
    let page: u64 = lines
        .first()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(16_384);
    let pages = |key: &str| -> u64 {
        lines
            .iter()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.rsplit(':').next())
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    };
    // Activity Monitor's "Memory Used" ≈ app (anonymous) + wired + compressed.
    let used = (pages("Anonymous pages") + pages("Pages wired down") + pages("Pages occupied by compressor"))
        .saturating_sub(pages("Pages purgeable"))
        * page;
    let used = if used == 0 { (pages("Pages active") + pages("Pages wired down")) * page } else { used };
    Some(HostStats { hostname, cpus, load1, mem_used_mb: used >> 20, mem_total_mb: total >> 20 })
}

fn stat(line: &str, key: &str) -> Option<u64> {
    let i = line.find(&format!("\"{key}\"="))? + key.len() + 3;
    line[i..].split([',', '}']).next()?.trim().parse().ok()
}

pub fn parse_gpus(lines: &[&str]) -> Vec<Gpu> {
    let model = lines.iter().find(|l| l.contains("\"model\"")).and_then(|l| l.split('"').nth(3)).unwrap_or("Apple GPU");
    let cores = lines
        .iter()
        .find(|l| l.contains("gpu-core-count"))
        .and_then(|l| l.rsplit('=').next()?.trim().parse::<u32>().ok());
    let Some(perf) = lines.iter().find(|l| l.contains("PerformanceStatistics")) else { return Vec::new() };
    vec![Gpu {
        index: 0,
        name: match cores {
            Some(c) => format!("{model} · {c}-core"),
            None => model.to_string(),
        },
        util: stat(perf, "Device Utilization %").map(|v| v as f32),
        mem_used_mb: stat(perf, "In use system memory").map(|b| b >> 20),
        mem_total_mb: None, // filled from host RAM (unified)
        ..Default::default()
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ioreg() {
        let lines = [
            r#"      "PerformanceStatistics" = {"In use system memory (driver)"=0,"Renderer Utilization %"=15,"Device Utilization %"=17,"In use system memory"=593035264}"#,
            r#"      "model" = "Apple M5""#,
            r#"      "gpu-core-count" = 10"#,
        ];
        let g = parse_gpus(&lines);
        assert_eq!(g[0].name, "Apple M5 · 10-core");
        assert_eq!(g[0].util, Some(17.0));
        assert_eq!(g[0].mem_used_mb, Some(565));
    }

    #[test]
    fn parses_host() {
        let lines = [
            "Mach Virtual Memory Statistics: (page size of 16384 bytes)",
            "Pages wired down:      1000.",
            "Anonymous pages:     2000.",
        ];
        let h = parse_host("mac 10 2.73 34359738368", &lines).unwrap();
        assert_eq!(h.mem_total_mb, 32768);
        assert_eq!(h.mem_used_mb, (3000 * 16384) >> 20);
    }
}
