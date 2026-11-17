//! vixen-core: everything the CLI and the macOS app share.
//!
//! * [`agents`]   — code-agent sessions (Claude Code, Codex, Antigravity, OpenCode) from local files
//! * [`config`]   — targets file (`~/.config/vixen/config.toml`)
//! * [`exec`]     — local / SSH command execution (one round trip per poll)
//! * [`sources`]  — per-backend scripts + parsers (nvidia-smi, amd-smi, Slurm, SGE, Modal, Vast.ai, SkyPilot, Apple GPU)
//! * [`progress`] — training-log progress parsing and ETA estimation
//! * [`monitor`]  — concurrent poll loop that publishes [`model::Snapshot`]s
//! * [`control`]  — job actions (cancel, hold, release, logs)
//! * [`insights`] — summaries of jobs and agents written by a local model (Ollama)

pub mod agents;
pub mod config;
pub mod control;
pub mod exec;
pub mod insights;
pub mod model;
pub mod monitor;
pub mod progress;
pub mod sources;
pub mod timefmt;

pub use config::{Config, Target};
pub use model::*;
pub use monitor::Monitor;
