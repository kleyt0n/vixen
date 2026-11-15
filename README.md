<p align="center">
  <img src="app/assets/logo.svg" width="72" alt="vixen">
</p>

<h1 align="center">vixen</h1>

<p align="center">Your GPUs, jobs and coding agents, in the menu bar.</p>

<p align="center">
  <img src="app/assets/foxes/kit.svg" width="56" alt="Kit">
  <img src="app/assets/foxes/snow.svg" width="56" alt="Snow">
  <img src="app/assets/foxes/sleepy.svg" width="56" alt="Sleepy">
  <img src="app/assets/foxes/mini.svg" width="56" alt="Mini">
</p>

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/kleyt0n/vixen/main/scripts/install.sh | sh     # macOS / Linux
```

```powershell
irm https://raw.githubusercontent.com/kleyt0n/vixen/main/scripts/install.ps1 | iex        # Windows
```

## What it does

- **Compute**: GPUs and jobs on SSH hosts, Slurm, SGE, Modal, Vast.ai, SkyPilot and your Mac. ETAs read from your training logs; cancel, hold and release jobs.
- **Agents**: Claude Code, Codex and Antigravity sessions, shown as working, waiting or idle, with plan limits and token use. Resume any session in one click.
- **Fox**: an optional floating fox that tells you when a job starts, finishes or fails, or when a limit runs high.

## Configure

Edit targets in **Settings** (`⌘,`), or in `~/.config/vixen/config.toml`:

```toml
[[target]]
name      = "dgx"
host      = "dgx"        # alias from ~/.ssh/config
scheduler = "slurm"
host_gpus = true
```

Every option is in [`config/example.toml`](config/example.toml).

## CLI

```sh
vixen                    # status
vixen watch              # live
vixen agents             # coding agents
vixen logs   <target> <job>
vixen cancel <target> <job>
```

## License

MIT
