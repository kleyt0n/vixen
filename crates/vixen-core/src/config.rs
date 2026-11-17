//! Targets file. Default location: `~/.config/vixen/config.toml`
//! (override with `$VIXEN_CONFIG`).

use crate::model::GpuSource;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Default poll interval in seconds.
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// Bytes read from the end of each job log.
    #[serde(default = "default_tail_bytes")]
    pub tail_bytes: u32,
    /// Code-agent monitoring (local session files).
    #[serde(default, skip_serializing_if = "is_default")]
    pub agents: Agents,
    /// Summaries of runs and agents written by a local model (Ollama).
    #[serde(default, skip_serializing_if = "is_default")]
    pub insights: InsightsConfig,
    #[serde(default, rename = "target", skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Target>,
}

/// `[agents]`: which coding agents to watch on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Agents {
    pub enabled: bool,
    /// Sessions active within this many hours are listed.
    pub window_hours: u64,
    /// Seconds between scans (cheap: only changed files are re-read).
    pub interval: u64,
    pub claude_code: bool,
    pub codex: bool,
    pub antigravity: bool,
    pub opencode: bool,
    /// Fetch Claude plan limits live with Claude Code's saved login (Keychain on macOS)
    /// instead of only reading what `/usage` last saved.
    pub live_claude_limits: bool,
}

impl Default for Agents {
    fn default() -> Self {
        Self {
            enabled: true,
            window_hours: 24,
            interval: 5,
            claude_code: true,
            codex: true,
            antigravity: true,
            opencode: true,
            live_claude_limits: false,
        }
    }
}

impl Agents {
    pub fn watches(&self, tool: crate::model::Tool) -> bool {
        use crate::model::Tool::*;
        self.enabled
            && match tool {
                ClaudeCode => self.claude_code,
                Codex => self.codex,
                Antigravity => self.antigravity,
                OpenCode => self.opencode,
            }
    }
}

/// `[insights]`: short summaries of jobs and agent sessions, written by a model served by Ollama.
/// Off by default. Job and agent details are sent only to the Ollama server at `url`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InsightsConfig {
    pub enabled: bool,
    /// Ollama server, e.g. `http://localhost:11434`.
    pub url: String,
    /// Model name as Ollama lists it (`ollama list`); empty picks the first installed model.
    pub model: String,
    /// Seconds between summaries. A summary is only rewritten when something changed.
    pub interval: u64,
    /// Include the last lines of running jobs' logs (one extra SSH round trip per job).
    pub job_logs: bool,
}

impl Default for InsightsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://localhost:11434".into(),
            model: String::new(),
            interval: 300,
            job_logs: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scheduler {
    #[default]
    None,
    Slurm,
    Sge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A machine reached over SSH (or this Mac when `host` is omitted).
    #[default]
    Host,
    /// Modal apps via the `modal` CLI.
    Modal,
    /// Vast.ai instances via the `vastai` CLI.
    #[serde(alias = "vastai")]
    Vast,
    /// SkyPilot clusters and managed jobs via the `sky` CLI (any cloud SkyPilot supports).
    #[serde(alias = "sky")]
    Skypilot,
}

impl Kind {
    /// Polled through a local cloud CLI rather than a shell on a host.
    pub fn is_cli(self) -> bool {
        self != Kind::Host
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Host => "host",
            Kind::Modal => "modal",
            Kind::Vast => "vast.ai",
            Kind::Skypilot => "skypilot",
        }
    }
}

// Defaults are skipped when serialising so saved files stay minimal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub kind: Kind,
    /// SSH alias/host. Omit for the local machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub scheduler: Scheduler,
    /// Sample GPUs on the host itself (nvidia-smi, or Apple GPU locally).
    #[serde(default, skip_serializing_if = "is_default")]
    pub host_gpus: bool,
    /// Slurm only: sample GPUs inside running jobs via `srun --overlap`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub job_gpus: bool,
    /// Show every user's jobs/processes instead of only yours.
    #[serde(default, skip_serializing_if = "is_default")]
    pub all_users: bool,
    /// Poll interval override (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<u64>,
    /// Cloud targets only: command used to invoke the CLI
    /// (default: `modal` / `vastai` / `sky`; Modal and Vast fall back to `uvx`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Extra `ssh` arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_args: Vec<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}
fn is_true(v: &bool) -> bool {
    *v
}

fn default_interval() -> u64 {
    300
}
fn default_tail_bytes() -> u32 {
    6144
}
fn yes() -> bool {
    true
}

impl Target {
    pub fn interval(&self, cfg: &Config) -> u64 {
        self.interval.unwrap_or(cfg.interval).max(2)
    }

    pub fn describe(&self) -> String {
        match self.kind {
            Kind::Host => {
                let mut parts = Vec::new();
                match self.scheduler {
                    Scheduler::Slurm => parts.push("slurm".to_string()),
                    Scheduler::Sge => parts.push("sge".to_string()),
                    Scheduler::None => {}
                }
                if self.host_gpus {
                    parts.push("gpu".into());
                }
                parts.push(match &self.host {
                    Some(h) => format!("ssh {h}"),
                    None => "local".into(),
                });
                parts.join(" · ")
            }
            k => k.label().into(),
        }
    }

    pub fn gpu_source(&self) -> GpuSource {
        match self.kind {
            Kind::Host if self.host_gpus => GpuSource::Host,
            Kind::Host if self.job_gpus => GpuSource::Jobs,
            Kind::Vast => GpuSource::Instances,
            _ => GpuSource::None,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("VIXEN_CONFIG") {
            return PathBuf::from(p);
        }
        home().join(".config/vixen/config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("no config at {} — run `vixen init` to create one", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.interval < 2 {
            bail!("interval must be at least 2 seconds");
        }
        if self.agents.interval < 1 || self.agents.window_hours < 1 {
            bail!("[agents] interval and window_hours must be at least 1");
        }
        let ins = &self.insights;
        if ins.enabled && ins.interval < 60 {
            bail!("[insights] interval must be at least 60 seconds");
        }
        if !(ins.url.starts_with("http://") || ins.url.starts_with("https://")) || ins.url.contains(char::is_whitespace)
        {
            bail!("[insights] url must be an http:// or https:// address");
        }
        let mut seen = std::collections::HashSet::new();
        for t in &self.targets {
            let name = t.name.trim();
            if name.is_empty() {
                bail!("every target needs a name");
            }
            if !seen.insert(name) {
                bail!("duplicate target name `{name}`");
            }
            if t.job_gpus && t.scheduler != Scheduler::Slurm {
                bail!("target `{name}`: job_gpus requires scheduler = \"slurm\"");
            }
            if t.kind.is_cli() && t.scheduler != Scheduler::None {
                bail!("target `{name}`: {} targets don't use a scheduler", t.kind.label());
            }
            if t.interval.is_some_and(|i| i < 2) {
                bail!("target `{name}`: interval must be at least 2 seconds");
            }
        }
        Ok(())
    }

    /// Trim names and drop empty optional strings (as sent by forms).
    pub fn normalized(mut self) -> Self {
        self.insights.url = self.insights.url.trim().trim_end_matches('/').to_string();
        self.insights.model = self.insights.model.trim().to_string();
        let blank = |o: Option<String>| o.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        for t in &mut self.targets {
            t.name = t.name.trim().to_string();
            t.host = blank(t.host.take());
            t.command = blank(t.command.take());
            t.ssh_args.retain(|a| !a.trim().is_empty());
            if t.kind.is_cli() {
                t.host = None;
                t.scheduler = Scheduler::None;
                t.host_gpus = false;
                t.job_gpus = false;
            }
        }
        self
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(format!("{HEADER}\n{}", toml::to_string_pretty(self)?))
    }

    /// Validate and atomically write to [`Config::path`], keeping the previous file as `.bak`.
    pub fn save(&self) -> Result<PathBuf> {
        self.validate()?;
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = self.to_toml()?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        if path.exists() {
            std::fs::copy(&path, path.with_extension("toml.bak"))?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    pub fn target(&self, name: &str) -> Result<&Target> {
        self.targets.iter().find(|t| t.name == name).with_context(|| format!("unknown target `{name}`"))
    }

    /// Write the bundled example if no config exists yet. Returns the path.
    pub fn init() -> Result<(PathBuf, bool)> {
        let path = Self::path();
        if path.exists() {
            return Ok((path, false));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, EXAMPLE)?;
        Ok((path, true))
    }
}

const HEADER: &str = "# vixen targets. Edited by Vixen → Settings; hand edits are fine too.\n\
# Every option is documented in config/example.toml.\n";

pub const EXAMPLE: &str = include_str!("../../../config/example.toml");

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Concrete `Host` aliases from `~/.ssh/config` (wildcard patterns skipped).
pub fn ssh_hosts() -> Vec<String> {
    let text = std::fs::read_to_string(home().join(".ssh/config")).unwrap_or_default();
    let mut hosts = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if !words.next().is_some_and(|k| k.eq_ignore_ascii_case("host")) {
            continue;
        }
        for h in words {
            if !h.contains(['*', '?', '!']) && !hosts.iter().any(|x| x == h) {
                hosts.push(h.to_string());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_parses() {
        let cfg = Config::parse(EXAMPLE).unwrap();
        assert!(!cfg.targets.is_empty());
    }

    #[test]
    fn round_trips_minimal_toml() {
        let cfg = Config::parse(EXAMPLE).unwrap();
        let text = cfg.to_toml().unwrap();
        assert_eq!(Config::parse(&text).unwrap(), cfg);
        assert!(!text.contains("enabled"), "defaults are omitted:\n{text}");
        assert!(!text.contains("job_gpus = false"));
    }

    #[test]
    fn normalizes_form_input() {
        let mut cfg = Config::parse(EXAMPLE).unwrap();
        cfg.targets[0].name = "  spaced ".into();
        cfg.targets[0].host = Some("  ".into());
        let cfg = cfg.normalized();
        assert_eq!(cfg.targets[0].name, "spaced");
        assert_eq!(cfg.targets[0].host, None);
    }

    #[test]
    fn cloud_kinds() {
        let cfg = Config::parse("[[target]]\nname='v'\nkind='vastai'\n[[target]]\nname='s'\nkind='sky'").unwrap();
        assert_eq!(cfg.targets[0].kind, Kind::Vast);
        assert_eq!(cfg.targets[1].kind, Kind::Skypilot);
        assert_eq!(cfg.targets[1].describe(), "skypilot");
        assert!(Config::parse("[[target]]\nname='v'\nkind='vast'\nscheduler='slurm'").is_err());
        assert!(cfg.to_toml().unwrap().contains("kind = \"skypilot\""));
    }

    #[test]
    fn insights_section() {
        let cfg = Config::parse("[insights]\nenabled = true\nmodel = 'qwen2.5'").unwrap();
        assert!(cfg.insights.enabled);
        assert_eq!(cfg.insights.url, "http://localhost:11434");
        assert!(cfg.to_toml().unwrap().contains("[insights]"));
        assert!(!Config::parse("").unwrap().to_toml().unwrap().contains("insights"), "default is omitted");
        assert!(Config::parse("[insights]\nenabled = true\ninterval = 5").is_err());
        assert!(Config::parse("[insights]\nurl = 'file:///etc/passwd'").is_err());
    }

    #[test]
    fn rejects_bad_targets() {
        let mut cfg = Config::parse(EXAMPLE).unwrap();
        cfg.targets[1].name = cfg.targets[0].name.clone();
        assert!(cfg.validate().is_err());
    }
}
