//! Serializable data model shared by the CLI and the app UI.

use serde::{Deserialize, Serialize};

/// State of every configured target at one moment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub taken_at: i64,
    pub targets: Vec<TargetStatus>,
    /// Code-agent sessions (Claude Code, Codex, Antigravity, OpenCode…) active within the configured window.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// Per-tool plan limits and token totals.
    #[serde(default)]
    pub usage: Vec<ToolUsage>,
    /// Model-written summary of jobs and agents; `None` when insights are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insights: Option<Insights>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetStatus {
    pub name: String,
    /// Human label, e.g. "slurm · ssh dgx".
    pub kind: String,
    pub ok: bool,
    pub error: Option<String>,
    /// Unix seconds of the last successful poll.
    pub updated_at: i64,
    pub latency_ms: u64,
    pub host: Option<HostStats>,
    pub gpus: Vec<Gpu>,
    pub jobs: Vec<Job>,
    /// Where `gpus` come from, so an empty list can be explained.
    #[serde(default)]
    pub gpu_source: GpuSource,
}

/// How a target reads GPUs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuSource {
    /// It doesn't (no `host_gpus` or `job_gpus`).
    #[default]
    None,
    /// The machine itself (`host_gpus`).
    Host,
    /// Inside your running Slurm jobs (`job_gpus`).
    Jobs,
    /// The instances a cloud CLI lists (Vast.ai).
    Instances,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostStats {
    pub hostname: String,
    pub cpus: u32,
    pub load1: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gpu {
    pub index: u32,
    /// Only used to map processes to GPUs.
    #[serde(skip)]
    pub uuid: Option<String>,
    pub name: String,
    pub util: Option<f32>,
    pub mem_used_mb: Option<u64>,
    pub mem_total_mb: Option<u64>,
    /// Memory is shared with the CPU (GB10 / Apple Silicon); figures come from system RAM.
    pub unified: bool,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    /// Set when the GPU was sampled from inside a scheduler job.
    pub job_id: Option<String>,
    pub node: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Pending,
    Held,
    Completing,
    Suspended,
    Failed,
    Other,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Pending => "pending",
            Self::Held => "held",
            Self::Completing => "completing",
            Self::Suspended => "suspended",
            Self::Failed => "failed",
            Self::Other => "other",
        }
    }
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Completing)
    }
}

/// A unit of work: scheduler job, GPU process or cloud app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub user: Option<String>,
    pub state: JobState,
    /// Scheduler's own state string (e.g. "PENDING", "r", "Eqw").
    pub raw_state: String,
    /// Pending reason or node list.
    pub detail: Option<String>,
    pub queue: Option<String>,
    pub resources: Option<String>,
    /// Number of tasks folded into an array row.
    pub array_tasks: Option<u32>,
    pub submitted_at: Option<i64>,
    pub started_at: Option<i64>,
    /// Scheduler estimate for pending jobs.
    pub est_start_at: Option<i64>,
    pub elapsed_s: Option<u64>,
    pub time_limit_s: Option<u64>,
    pub log_path: Option<String>,
    /// Seconds since the log file was last written.
    pub log_age_s: Option<u64>,
    pub last_line: Option<String>,
    pub progress: Option<Progress>,
    pub eta: Option<Eta>,
    pub actions: Vec<Action>,
}

impl Job {
    pub fn new(id: impl Into<String>, name: impl Into<String>, state: JobState) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            user: None,
            state,
            raw_state: state.label().into(),
            detail: None,
            queue: None,
            resources: None,
            array_tasks: None,
            submitted_at: None,
            started_at: None,
            est_start_at: None,
            elapsed_s: None,
            time_limit_s: None,
            log_path: None,
            log_age_s: None,
            last_line: None,
            progress: None,
            eta: None,
            actions: Vec::new(),
        }
    }

    pub fn time_left_s(&self) -> Option<u64> {
        Some(self.time_limit_s?.saturating_sub(self.elapsed_s?))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Overall completion in [0, 1].
    pub fraction: f64,
    /// e.g. "epoch 136/310 · step 150/500".
    pub label: String,
    /// Iterations per second reported by the job, if any.
    pub it_per_s: Option<f64>,
    /// Remaining seconds printed by the tool itself (tqdm `<mm:ss`), if any.
    pub tool_remaining_s: Option<u64>,
    /// Remaining work ÷ printed throughput, if both are known.
    pub throughput_remaining_s: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtaMethod {
    /// Progress observed across polls (most accurate).
    Observed,
    /// Throughput printed in the log (it/s) × remaining work.
    Throughput,
    /// Remaining time printed by the tool (tqdm).
    Tool,
    /// Elapsed ÷ fraction (assumes the job started at 0%).
    Elapsed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eta {
    pub remaining_s: u64,
    pub finish_at: i64,
    pub method: EtaMethod,
    /// The job will hit its wall-time limit before finishing.
    pub exceeds_limit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Cancel,
    Hold,
    Release,
    Logs,
}

/// A coding-agent tool whose sessions vixen can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    ClaudeCode,
    Codex,
    Antigravity,
    #[serde(rename = "opencode")]
    OpenCode,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::ClaudeCode, Tool::Codex, Tool::Antigravity, Tool::OpenCode];

    pub fn label(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "Claude Code",
            Tool::Codex => "Codex",
            Tool::Antigravity => "Antigravity",
            Tool::OpenCode => "OpenCode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Mid-turn: running tools or generating.
    Working,
    /// Turn finished recently; the agent is waiting for you.
    Waiting,
    /// No activity for a while.
    Idle,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Waiting => "waiting",
            AgentState::Idle => "idle",
        }
    }
}

/// One agent session, read from the tool's own session files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub tool: Tool,
    /// Session id (as the tool names it).
    pub id: String,
    /// Session title, or the last prompt when the tool has none.
    pub title: String,
    pub cwd: Option<String>,
    /// Last path component of `cwd`.
    pub project: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub state: AgentState,
    pub started_at: Option<i64>,
    /// Unix seconds of the last recorded activity.
    pub updated_at: i64,
    /// Tokens in the model's context at the last turn, and the window size when known.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub last_prompt: Option<String>,
    pub last_reply: Option<String>,
    /// Where it runs, e.g. `cli`, `vscode`, `sdk-cli`.
    pub source: Option<String>,
    /// Transcript file, for "reveal".
    pub file: Option<String>,
    /// Shell command that resumes the session in a terminal.
    pub resume: Option<String>,
}

/// One plan limit as the tool reports it, e.g. the 5-hour session or the weekly cap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit {
    /// "Session (5h)", "Weekly", "Weekly · Opus"…
    pub label: String,
    /// 0–100.
    pub percent: f64,
    pub resets_at: Option<i64>,
    /// `normal` / `warning` / `critical` when the tool says so.
    pub severity: Option<String>,
}

/// Limits and token use for one agent tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUsage {
    pub tool: Tool,
    pub plan: Option<String>,
    pub limits: Vec<UsageLimit>,
    /// When the tool last recorded these limits (they may be stale).
    pub limits_as_of: Option<i64>,
    /// Tokens (input + output + cache) used in the last 5 hours and since local midnight.
    pub tokens_5h: u64,
    pub tokens_today: u64,
    pub sessions_today: usize,
    /// Tokens per hour over the last 24 hours, oldest first; the last entry is the current hour.
    #[serde(default)]
    pub tokens_by_hour: Vec<u64>,
}

/// A short, model-written read of what's running: one headline plus a note per job or agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Insights {
    /// Unix seconds of the last successful summary (0: none yet).
    pub generated_at: i64,
    /// Model that wrote it, as Ollama names it.
    pub model: Option<String>,
    pub headline: String,
    pub notes: Vec<Note>,
    /// A summary is being written now.
    pub busy: bool,
    /// Why the last attempt failed (the previous summary, if any, is kept).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    Agent,
    Job,
    General,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Progressing normally.
    Ok,
    /// Worth a look: slow, stalled, near a limit, waiting a long time.
    Warn,
    /// Something is wrong: failed, erroring, will miss its limit.
    Bad,
}

/// One insight about a job or agent session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub kind: NoteKind,
    /// Matches the UI's row keys: `<tool>::<session id>` for agents, `<target>::<job id>` for jobs.
    pub key: Option<String>,
    /// What the note is about, e.g. the job or session name.
    pub label: String,
    pub text: String,
    pub tone: Tone,
}
