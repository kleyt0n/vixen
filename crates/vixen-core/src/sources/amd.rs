//! AMD GPUs via `amd-smi` (ROCm 6+): `static --asic` for names, `monitor` for live metrics.
//!
//! JSON shape varies across releases: a top-level list or `{"gpu_data": [...]}`, and
//! values either bare or as `{"value": …, "unit": …}`. Both are accepted.

use crate::model::Gpu;
use anyhow::{Result, bail};
use serde_json::Value;

pub const GPU_SCRIPT: &str = r#"echo "@@AMDGPU"
amd-smi static --asic --json 2>/dev/null
echo "@@AMDMON"
amd-smi monitor -p -t -u -v --json 2>&1
"#;

fn devices(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("gpu_data") {
            Some(Value::Array(a)) => a,
            _ => vec![Value::Object(o)],
        },
        _ => Vec::new(),
    }
}

fn parse_json(lines: &[&str]) -> Option<Value> {
    super::cli::find_json(&lines.join("\n"))
}

/// `12`, `"12.5"`, `{"value": 12, "unit": "%"}` → (12, unit); `"N/A"` → None.
fn value(v: Option<&Value>) -> Option<(f64, String)> {
    match v? {
        Value::Number(n) => Some((n.as_f64()?, String::new())),
        Value::String(s) => {
            let mut it = s.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next().unwrap_or("").to_string()))
        }
        Value::Object(o) => {
            let (n, _) = value(o.get("value"))?;
            Some((n, o.get("unit").and_then(Value::as_str).unwrap_or("").to_string()))
        }
        _ => None,
    }
}

fn mb(v: Option<&Value>) -> Option<u64> {
    let (n, unit) = value(v)?;
    let mb = match unit.to_ascii_uppercase().as_str() {
        "GB" | "GIB" => n * 1024.0,
        "KB" | "KIB" => n / 1024.0,
        "B" => n / 1_048_576.0,
        _ => n, // MB (the default)
    };
    Some(mb.round() as u64)
}

fn index(d: &Value) -> Option<u32> {
    value(d.get("gpu")).map(|(n, _)| n as u32)
}

pub fn parse_gpus(asic: &[&str], monitor: &[&str]) -> Result<Vec<Gpu>> {
    let Some(mon) = parse_json(monitor) else {
        let msg = monitor.iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("no output");
        bail!("amd-smi: {msg}");
    };
    let names: Vec<(u32, String)> = parse_json(asic)
        .map(devices)
        .unwrap_or_default()
        .iter()
        .filter_map(|d| {
            let name = d.get("asic")?.get("market_name")?.as_str()?.trim();
            Some((index(d)?, name.to_string()))
        })
        .filter(|(_, n)| !n.is_empty() && n != "N/A")
        .collect();
    let mut gpus = Vec::new();
    for (i, d) in devices(mon).iter().enumerate() {
        let index = index(d).unwrap_or(i as u32);
        let name = names.iter().find(|(g, _)| *g == index).map(|(_, n)| n.as_str()).unwrap_or("GPU");
        let lower = name.to_ascii_lowercase();
        let branded = ["amd", "radeon", "instinct"].iter().any(|b| lower.contains(b));
        gpus.push(Gpu {
            index,
            name: if branded { name.to_string() } else { format!("AMD {name}") },
            util: value(d.get("gfx")).map(|(n, _)| n as f32),
            mem_used_mb: mb(d.get("vram_used")),
            mem_total_mb: mb(d.get("vram_total")),
            temp_c: value(d.get("hotspot_temperature").or(d.get("edge_temperature"))).map(|(n, _)| n as f32),
            power_w: value(d.get("power_usage")).map(|(n, _)| n as f32),
            ..Default::default()
        });
    }
    Ok(gpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_wrappers() {
        let asic = [
            r#"[{"gpu": 0, "asic": {"market_name": "AMD Instinct MI300X", "vendor_id": "0x1002"}},"#,
            r#" {"gpu": 1, "asic": {"market_name": "MI250X"}}]"#,
        ];
        let mon = [r#"{"gpu_data": [
            {"gpu": 0, "power_usage": {"value": 640, "unit": "W"}, "hotspot_temperature": {"value": 71, "unit": "C"},
             "gfx": {"value": 98, "unit": "%"}, "vram_used": {"value": 150000, "unit": "MB"}, "vram_total": {"value": 196592, "unit": "MB"}},
            {"gpu": 1, "power_usage": "N/A", "hotspot_temperature": 40, "gfx": 0,
             "vram_used": {"value": 0.5, "unit": "GB"}, "vram_total": {"value": 64.0, "unit": "GB"}}
        ]}"#];
        let g = parse_gpus(&asic, &mon).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "AMD Instinct MI300X");
        assert_eq!((g[0].util, g[0].mem_used_mb, g[0].mem_total_mb), (Some(98.0), Some(150_000), Some(196_592)));
        assert_eq!((g[0].temp_c, g[0].power_w), (Some(71.0), Some(640.0)));
        assert_eq!(g[1].name, "AMD MI250X");
        assert_eq!((g[1].power_w, g[1].mem_used_mb, g[1].mem_total_mb), (None, Some(512), Some(65_536)));
    }

    #[test]
    fn error_text() {
        assert!(parse_gpus(&[], &["ERROR:root:Unable to detect any GPU devices"]).is_err());
        let warned = ["[WARNING] driver mismatch", r#"[{"gpu": 0, "gfx": 5}]"#];
        assert_eq!(parse_gpus(&[], &warned).unwrap()[0].util, Some(5.0));
    }
}
