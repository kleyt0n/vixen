//! Command execution on the local machine or over SSH.
//!
//! Scripts are fed to `sh -s` on stdin, so nothing needs shell-quoting for the
//! transport. SSH always runs with `BatchMode=yes` so it can never hang on a
//! password prompt; if the host has no `ControlMaster` configured, vixen adds
//! its own multiplexed connection so repeated polls skip the handshake.

use anyhow::{Context, Result, bail};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::config::home;

#[derive(Debug)]
pub struct Exec {
    host: Option<String>,
    extra: Vec<String>,
    mux: OnceCell<Vec<String>>,
}

impl Exec {
    pub fn new(host: Option<String>, extra: Vec<String>) -> Self {
        Self { host, extra, mux: OnceCell::new() }
    }

    pub fn local() -> Self {
        Self::new(None, Vec::new())
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }

    /// Run a POSIX shell script and return stdout.
    pub async fn sh(&self, script: &str, timeout: Duration) -> Result<String> {
        let mut cmd = match &self.host {
            None => {
                // Windows has no /bin/sh; a `sh` on PATH (Git Bash, MSYS) works for local targets.
                let mut c = Command::new(if cfg!(windows) { "sh" } else { "/bin/sh" });
                c.arg("-s");
                c
            }
            Some(host) => {
                let mut c = Command::new("ssh");
                c.args(SSH_BASE).args(self.mux_args(host).await).args(&self.extra);
                c.arg(host).arg("sh -s");
                c
            }
        };
        let res = run(&mut cmd, Some(script), timeout).await;
        match &self.host {
            Some(h) => res.map_err(|e| anyhow::anyhow!(e.to_string().replace("<host>", h))),
            None => res,
        }
    }

    /// Multiplexing options, only when the user's ssh config has none.
    async fn mux_args(&self, host: &str) -> &[String] {
        self.mux
            .get_or_init(|| async {
                if cfg!(windows) {
                    return Vec::new(); // Windows OpenSSH has no ControlMaster
                }
                let mut probe = Command::new("ssh");
                probe.arg("-G").args(&self.extra).arg(host);
                let cfg = run(&mut probe, None, Duration::from_secs(5)).await.unwrap_or_default();
                let configured =
                    cfg.lines().any(|l| l.strip_prefix("controlmaster ").is_some_and(|v| v != "false" && v != "no"));
                if configured {
                    return Vec::new();
                }
                let dir = home().join(".cache/vixen");
                let _ = std::fs::create_dir_all(&dir);
                vec![
                    "-o".into(),
                    "ControlMaster=auto".into(),
                    "-o".into(),
                    format!("ControlPath={}/%C", dir.display()),
                    "-o".into(),
                    "ControlPersist=600".into(),
                ]
            })
            .await
    }
}

const SSH_BASE: &[&str] =
    &["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "LogLevel=ERROR"];

/// Run a local program (used for CLIs like `modal`).
pub async fn program(argv: &[String], timeout: Duration) -> Result<String> {
    let (bin, args) = argv.split_first().context("empty command")?;
    let mut cmd = Command::new(bin);
    cmd.args(args);
    run(&mut cmd, None, timeout).await
}

/// Run a local program for at most `window`, returning whatever it printed (for streaming CLIs).
pub async fn program_for(argv: &[String], window: Duration) -> Result<String> {
    use tokio::io::AsyncReadExt;
    let (bin, args) = argv.split_first().context("empty command")?;
    let mut child = Command::new(bin)
        .args(args)
        .env("PATH", search_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("cannot start {bin}"))?;
    let mut buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = tokio::time::timeout(window, out.read_to_end(&mut buf)).await;
    }
    let _ = child.kill().await;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn run(cmd: &mut Command, stdin: Option<&str>, timeout: Duration) -> Result<String> {
    cmd.env("PATH", search_path())
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().with_context(|| format!("cannot start {:?}", cmd.as_std().get_program()))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input.as_bytes()).await?;
        drop(pipe);
    }
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(r) => r?,
        Err(_) => bail!("timed out after {}s", timeout.as_secs()),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.ends_with("succeeded!")) // skip `nc -z` chatter from ssh Match exec
        .unwrap_or("")
        .to_string();
    let code = out.status.code().unwrap_or(-1);
    if code == 255 && cmd.as_std().get_program() == "ssh" {
        if msg.contains("Permission denied") || msg.contains("Host key") || msg.is_empty() {
            bail!("ssh auth needed — run `ssh <host>` once in a terminal ({msg})");
        }
        bail!("ssh: {msg}");
    }
    bail!("exit {code}: {}", if msg.is_empty() { "no output" } else { &msg })
}

/// GUI apps on macOS start with a minimal PATH; add the usual tool locations.
fn search_path() -> &'static std::ffi::OsStr {
    static PATH: OnceLock<std::ffi::OsString> = OnceLock::new();
    PATH.get_or_init(|| {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
        if cfg!(unix) {
            let h = home();
            for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin"]
                .map(std::path::PathBuf::from)
                .into_iter()
                .chain([h.join(".local/bin"), h.join(".cargo/bin")])
            {
                if !parts.contains(&extra) {
                    parts.push(extra);
                }
            }
        }
        std::env::join_paths(parts).unwrap_or(current)
    })
}

/// Quote a string for POSIX sh.
pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Split script output into `@@NAME arg…` sections.
pub fn sections(out: &str) -> Vec<(&str, &str, Vec<&str>)> {
    let mut res: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            res.push((name, arg.trim(), Vec::new()));
        } else if let Some(cur) = res.last_mut() {
            cur.2.push(line);
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(shq("a'b"), r"'a'\''b'");
    }

    #[test]
    fn splits_sections() {
        let s = sections("junk\n@@A 1\nx\ny\n@@B\nz\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], ("A", "1", vec!["x", "y"]));
        assert_eq!(s[1].2, vec!["z"]);
    }

    #[tokio::test]
    async fn local_sh() {
        let out = Exec::local().sh("echo hi", Duration::from_secs(5)).await.unwrap();
        assert_eq!(out.trim(), "hi");
    }
}
