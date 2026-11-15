<p align="center">
  <img src="app/assets/logo.svg" width="88" alt="vixen">
</p>

<h1 align="center">vixen</h1>

<p align="center">GPU and job monitoring for SSH hosts, Slurm, SGE, Modal, Vast.ai, SkyPilot and your Mac.<br>
A native macOS menu-bar app and a CLI, on one fast Rust core.</p>

---

## Features

- **Fast**: at most one SSH round trip per poll, plus one more only when jobs are running. Reuses your `ControlMaster`.
- **ETAs**: reads tqdm, `Epoch a/b`, `step a/b` and `it/s` from your logs, and warns when a job will overrun its wall time.
- **Control**: cancel, hold, release and live logs across Slurm, SGE, plain processes and Modal. Stop Vast.ai instances and SkyPilot clusters, and cancel SkyPilot managed jobs.
- **In-job GPUs**: shows real compute-node utilisation through `srun --overlap nvidia-smi`.

## Install

**macOS / Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/kleyt0n/vixen/main/scripts/install.sh | sh
```

**Windows** (PowerShell)

```powershell
irm https://raw.githubusercontent.com/kleyt0n/vixen/main/scripts/install.ps1 | iex
```

The installer puts the `vixen` CLI in `~/.local/bin` (on Windows, `%LOCALAPPDATA%\vixen\bin`). On macOS it also installs **Vixen.app**, and on every platform it writes a starter config. Pin a version with `VIXEN_VERSION=v0.1.0`. Downloads are checked against the release's `SHA256SUMS`.

**From source:** `make install` builds the CLI and the app (the app needs `cargo install tauri-cli --version "^2" --locked`).

## Configure

Edit targets in the app's **Settings**, or by hand. Hosts are aliases from `~/.ssh/config`. See [`config/example.toml`](config/example.toml) for every option.

```toml
interval = 300          # seconds (5 min)

[[target]]
name      = "dgx"
host      = "dgx"
scheduler = "slurm"     # slurm | sge | none
host_gpus = true

[[target]]
name      = "hpc"
host      = "hpc-login"
scheduler = "slurm"
job_gpus  = true        # GPU stats inside your running jobs

[[target]]
name = "modal"
kind = "modal"

[[target]]
name = "vast"
kind = "vast"       # vastai CLI; GPU utilisation without SSH

[[target]]
name = "sky"
kind = "skypilot"   # sky CLI with `-o json` (checked on 0.14); clusters + managed jobs
```

Any NVIDIA or AMD machine (via `nvidia-smi` or `amd-smi`) is just an SSH host with `host_gpus = true`.

## Use

```sh
vixen                          # status
vixen watch                    # live
vixen logs    <target> <job>
vixen cancel  <target> <job>
vixen hold    <target> <job>
vixen release <target> <job>
```

The app has three parts:

- **Window**: GPU cards with vendor badges, and a job table with live ETA countdowns.
- **Menu bar**: shows `3 ▸ 2h 31m`. Click it for a panel listing running jobs with progress and ETA, and each machine with its GPUs.
- **Settings** (`⌘,`): edit targets, test each connection and save. The app writes `config.toml` and keeps the previous version as `.bak`.

You get a notification when a job ends or is about to overrun its time limit.

## Layout

```
crates/vixen-core   config, SSH, backends, ETA, job control
crates/vixen-cli    vixen command
app/src-tauri        macOS app
app/ui               HTML/CSS/JS, no build step
```

`make help` lists all build tasks. Pushing a `v*` tag builds and publishes a release (see `.github/workflows/release.yml`).

## License

MIT
