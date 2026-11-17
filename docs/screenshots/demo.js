// Demo data for README screenshots (scripts/screenshots.sh). Fictional machines,
// jobs and agent sessions; times are relative to "now" so ETAs look natural.
"use strict";

(() => {
  const T = Math.floor(Date.now() / 1000);
  const h = 3600, m = 60;
  const gpu = (index, name, util, used, total, temp, power, limit, job, node) =>
    ({ index, name, util, mem_used_mb: used * 1024, mem_total_mb: total * 1024, unified: false, temp_c: temp, power_w: power, power_limit_w: limit, job_id: job, node });
  const job = (o) => ({
    user: "alice", raw_state: null, detail: null, queue: "gpu", resources: "gpu:1", array_tasks: null,
    submitted_at: T - 9 * h, started_at: null, est_start_at: null, elapsed_s: null, time_limit_s: 48 * h,
    log_path: null, log_age_s: 20, last_line: null, progress: null, eta: null, actions: ["cancel", "hold", "logs"], ...o,
  });
  const running = (id, name, frac, epoch, step, rate, left, elapsed, node) => job({
    id, name, state: "running", detail: node, started_at: T - elapsed, elapsed_s: elapsed,
    progress: { fraction: frac, label: `epoch ${epoch}/300 · step ${step}/500`, it_per_s: rate, tool_remaining_s: null, throughput_remaining_s: left },
    eta: { remaining_s: left, finish_at: T + left, method: "observed", exceeds_limit: false },
  });

  const snapshot = {
    taken_at: T - 8,
    targets: [
      {
        name: "dgx", kind: "slurm · gpu · ssh dgx", ok: true, error: null, updated_at: T - 8, latency_ms: 112,
        host: { hostname: "dgx-01", cpus: 128, load1: 41.2, mem_used_mb: 612 * 1024, mem_total_mb: 2048 * 1024 },
        gpus: [
          gpu(0, "NVIDIA H100 80GB HBM3", 97, 71, 80, 71, 642, 700, "4182", "dgx-01"),
          gpu(1, "NVIDIA H100 80GB HBM3", 95, 70, 80, 69, 618, 700, "4182", "dgx-01"),
          gpu(2, "NVIDIA H100 80GB HBM3", 88, 52, 80, 66, 560, 700, "4190", "dgx-01"),
          gpu(3, "NVIDIA H100 80GB HBM3", 4, 2, 80, 38, 92, 700, null, "dgx-01"),
        ],
        jobs: [
          running("4182", "finetune-llama", 0.62, 186, 310, 2.4, 3 * h + 12 * m, 5 * h + 20 * m, "dgx-01"),
          running("4190", "eval-sweep", 0.31, 93, 120, 6.1, 5 * h + 41 * m, 2 * h + 34 * m, "dgx-01"),
        ],
      },
      {
        name: "hpc", kind: "slurm · ssh hpc-login", ok: true, error: null, updated_at: T - 21, latency_ms: 236,
        host: { hostname: "login01", cpus: 64, load1: 3.1, mem_used_mb: 92 * 1024, mem_total_mb: 384 * 1024 },
        gpus: [
          gpu(0, "A100-SXM4-80GB", 91, 61, 80, 64, 341, 400, "7731_3", "gpu-a07"),
          gpu(0, "A100-SXM4-80GB", 86, 58, 80, 62, 322, 400, "7731_4", "gpu-a11"),
        ],
        jobs: [
          running("7731_3", "train-vit", 0.78, 234, 150, 5.3, 1 * h + 46 * m, 13 * h + 5 * m, "gpu-a07"),
          running("7731_4", "train-vit", 0.44, 132, 400, 4.9, 8 * h + 15 * m, 7 * h + 2 * m, "gpu-a11"),
          job({ id: "7731_[5-8%2]", name: "train-vit", state: "pending", detail: "(Priority)", array_tasks: 4, est_start_at: T + 2 * h }),
          job({ id: "7740", name: "ablate-dropout", state: "held", detail: "JobHeldUser" }),
        ],
      },
      { name: "modal", kind: "modal", ok: true, error: null, updated_at: T - 40, latency_ms: 840, host: null, gpus: [], jobs: [] },
      {
        name: "local", kind: "gpu · local", ok: true, error: null, updated_at: T - 3, latency_ms: 18,
        host: { hostname: "macbook", cpus: 12, load1: 2.4, mem_used_mb: 21 * 1024, mem_total_mb: 36 * 1024 },
        gpus: [{ index: 0, name: "Apple M4 Max · 40-core", util: 34, mem_used_mb: 21 * 1024, mem_total_mb: 36 * 1024, unified: true, temp_c: null, power_w: null, power_limit_w: null, job_id: null, node: null }],
        jobs: [],
      },
    ],
    agents: [
      { tool: "claude_code", id: "a1", title: "Add retries to the data loader", cwd: "/Users/alice/code/trainer", project: "trainer", branch: "feat/loader-retries", model: "claude-opus-5-5", state: "working", started_at: T - 40 * m, updated_at: T - 6, context_tokens: 142000, context_window: null, last_prompt: "Retry transient S3 errors in the loader, with backoff", last_reply: "Added exponential backoff with jitter; running the loader tests now.", source: "cli", file: "/x", resume: "claude --resume a1" },
      { tool: "codex", id: "c1", title: "Speed up the eval script", cwd: "/Users/alice/code/evals", project: "evals", branch: "main", model: "gpt-5.5-codex", state: "working", started_at: T - 25 * m, updated_at: T - 14, context_tokens: 61000, context_window: 272000, last_prompt: "Speed up the eval script", last_reply: null, source: "codex_cli_rs", file: "/x", resume: "codex resume c1" },
      { tool: "claude_code", id: "a2", title: "Write the results section", cwd: "/Users/alice/papers/vit-scaling", project: "vit-scaling", branch: "draft", model: "claude-opus-5-5", state: "waiting", started_at: T - 2 * h, updated_at: T - 4 * m, context_tokens: 88000, context_window: null, last_prompt: "Draft the results section from the eval tables", last_reply: "Drafted §4 with the three main tables. Want the ablations in the appendix?", source: "cli", file: "/x", resume: "claude --resume a2" },
      { tool: "antigravity", id: "g1", title: "Dashboard for training runs", cwd: "/Users/alice/code/runboard", project: "runboard", branch: null, model: null, state: "waiting", started_at: null, updated_at: T - 17 * m, context_tokens: null, context_window: null, last_prompt: null, last_reply: "The dashboard now streams metrics from the run logs.", source: "42 steps", file: null, resume: null },
      { tool: "claude_code", id: "a3", title: "Fix flaky CI on macOS", cwd: "/Users/alice/code/trainer", project: "trainer", branch: "fix/ci", model: "claude-opus-5-5", state: "idle", started_at: T - 6 * h, updated_at: T - 3 * h, context_tokens: 54000, context_window: null, last_prompt: null, last_reply: null, source: "cli", file: "/x", resume: "claude --resume a3" },
    ],
    usage: [
      { tool: "claude_code", plan: null, limits: [
        { label: "Session (5h)", percent: 46, resets_at: T + 2 * h + 10 * m, severity: "normal" },
        { label: "Weekly", percent: 72, resets_at: T + 3 * 86400, severity: "warning" },
        { label: "Weekly · Opus", percent: 18, resets_at: T + 3 * 86400, severity: "normal" },
      ], limits_as_of: T - 60, tokens_5h: 38_400_000, tokens_today: 61_200_000, sessions_today: 4,
        tokens_by_hour: [0, 0, 0, 0, 0, 0, 0, 0, 0, 1.2e6, 4.8e6, 7.9e6, 6.1e6, 2.2e6, 5.4e6, 9.6e6, 8.8e6, 4.1e6, 0.9e6, 0, 2.6e6, 7.4e6, 9.9e6, 5.3e6] },
      { tool: "codex", plan: "plus", limits: [
        { label: "Session (5h)", percent: 22, resets_at: T + 3 * h, severity: null },
        { label: "Weekly", percent: 31, resets_at: T + 5 * 86400, severity: null },
      ], limits_as_of: T - 30, tokens_5h: 4_100_000, tokens_today: 9_800_000, sessions_today: 2,
        tokens_by_hour: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0.6e6, 1.1e6, 0.4e6, 0, 0, 0.9e6, 1.6e6, 1.2e6, 0, 0, 0, 0.8e6, 1.9e6, 1.3e6] },
    ],
  };

  snapshot.insights = {
    generated_at: T - 95, model: "gemma3:4b", busy: false, error: null,
    headline: "Training is on track; finetune-llama finishes in about 3 hours and one agent needs your answer.",
    notes: [
      { kind: "agent", key: "claude_code::a2", label: "vit-scaling", tone: "warn", text: "Drafted §4 with the three main tables; asks whether the ablations go in the appendix." },
      { kind: "job", key: "dgx::4182", label: "finetune-llama", tone: "ok", text: "Epoch 186/300 at 2.4 it/s, loss still falling; done around " + new Date((T + 3 * h + 12 * m) * 1000).toTimeString().slice(0, 5) + "." },
      { kind: "job", key: "hpc::7731_4", label: "train-vit", tone: "ok", text: "Epoch 132/300 at 4.9 it/s; on pace to finish well inside its 48h limit." },
      { kind: "agent", key: "claude_code::a1", label: "trainer", tone: "ok", text: "Added backoff with jitter to the S3 loader; running the loader tests now." },
      { kind: "agent", key: "codex::c1", label: "evals", tone: "ok", text: "Profiling eval.py; batched the tokenizer calls and is re-running the timing." },
      { kind: "general", key: null, label: "", tone: "warn", text: "GPU 3 on dgx is idle while four train-vit tasks wait on hpc." },
    ],
  };

  const prefs = { tray_title: "count_eta", float_button: true, float_style: "kit", limit_alert: 80, events: { started: true, finished: true, failed: true, limits: true, insights: true } };
  // Six hours of per-machine GPU utilisation, one point per 5-minute poll.
  const walk = (base, wobble, seed) => {
    let v = base, s = seed;
    return Array.from({ length: 73 }, (_, i) => {
      s = (s * 9301 + 49297) % 233280;
      v = Math.max(0, Math.min(100, v + (s / 233280 - 0.5) * wobble + (base - v) * 0.2));
      return [T - (72 - i) * 300, Math.round(v)];
    });
  };
  const ramp = (pts, from, to) => pts.map(([t, v], i) => [t, i < from ? Math.max(0, v - 70) : i < to ? Math.round(v * (i - from) / (to - from)) : v]);
  const mhist = { dgx: ramp(walk(70, 18, 7), 10, 22), hpc: walk(88, 10, 3), local: walk(30, 22, 11) };
  window.DEMO = { snapshot, mhist };
  window.__TAURI__ = {
    core: { invoke: async (cmd) => (cmd === "snapshot" ? snapshot : cmd === "get_prefs" ? prefs : cmd === "float_expand" ? { right: false, down: false } : null) },
    event: { listen: async () => () => {} },
    window: { getCurrentWindow: () => ({ setSize: async () => {}, hide: async () => {}, startDragging: async () => {}, toggleMaximize: async () => {}, onResized: async () => () => {} }) },
    dpi: { LogicalSize: class {} },
  };
})();
