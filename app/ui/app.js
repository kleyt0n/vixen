// Vixen UI. All data comes from the Rust side as `snapshot` events.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
const S = {
  snap: null,
  error: null,
  view: "all",          // "all" | target name
  filter: "all",        // "active" | "queued" | "all"
  query: "",
  open: new Set(),      // expanded job keys
  groups: new Set(),    // expanded queued groups
  hist: new Map(),      // gpu key → util samples
  seen: new Map(),      // target → last updated_at (to sample history once per poll)
  log: null,            // {target, id, path, name}
  confirm: null,
};
const HIST = 90;
const QUEUE_PREVIEW = 6;

// ---- data helpers -----------------------------------------------------------

const targets = () => (S.snap?.targets ?? []).filter((t) => S.view === "all" || t.name === S.view);
const key = (t, j) => `${t}::${j.id}`;
const isQueued = (j) => j.state === "pending" || j.state === "held";

function allJobs() {
  const q = S.query.toLowerCase();
  const out = [];
  for (const t of targets()) {
    for (const j of t.jobs) {
      if (S.filter === "active" && isQueued(j)) continue;
      if (S.filter === "queued" && !isQueued(j)) continue;
      if (q && !`${j.id} ${j.name} ${j.user ?? ""} ${j.detail ?? ""} ${t.name}`.toLowerCase().includes(q)) continue;
      out.push({ t: t.name, j });
    }
  }
  return out;
}

function sampleHistory() {
  for (const t of S.snap?.targets ?? []) {
    if (!t.ok || S.seen.get(t.name) === t.updated_at) continue;
    S.seen.set(t.name, t.updated_at);
    for (const g of t.gpus) {
      const k = `${t.name}/${g.job_id ?? ""}/${g.index}`;
      const h = S.hist.get(k) ?? [];
      h.push(g.util ?? 0);
      if (h.length > HIST) h.shift();
      S.hist.set(k, h);
    }
  }
}

// ---- rendering --------------------------------------------------------------

let frame = 0;
const render = whenVisible(renderNow);
function renderNow() {
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    renderNav();
    renderHeader();
    renderBanner();
    renderGpus();
    renderJobs();
    tick();
  });
}

const cache = new Map();
function setHTML(el, html) {
  if (cache.get(el) === html) return;
  cache.set(el, html);
  el.innerHTML = html;
}

function targetDot(t) {
  if (t.ok) return "ok";
  if (t.error && t.updated_at) return "stale";
  if (t.error) return "down";
  return "wait";
}

function renderNav() {
  const ts = S.snap?.targets ?? [];
  const running = (t) => t.jobs.filter((j) => j.state === "running" || j.state === "completing").length;
  const all = ts.reduce((n, t) => n + running(t), 0);
  const on = (v) => (!CFG.open && S.view === v ? "on" : "");
  let html = `<button class="nav-item ${on("all")}" data-view="all">
      <span class="dot ${ts.every((t) => t.ok) && ts.length ? "ok" : ts.some((t) => !t.ok && t.error) ? "stale" : "wait"}"></span>
      <span class="nav-name">Overview</span><span class="nav-badge">${all || ""}</span></button>
    <div class="nav-label">Targets</div>`;
  for (const t of ts) {
    const r = running(t), q = t.jobs.filter(isQueued).length;
    const badge = [r ? `${r}▸` : "", q ? `${q}` : ""].filter(Boolean).join(" · ");
    html += `<button class="nav-item ${on(t.name)}" data-view="${esc(t.name)}" title="${esc(t.error ?? `${t.kind} · ${t.latency_ms} ms`)}">
      <span class="dot ${targetDot(t)}"></span>
      <span class="nav-name">${esc(t.name)}</span><span class="nav-badge">${badge}</span>
      <span class="nav-kind">${esc(t.kind)}</span></button>`;
  }
  setHTML($("#targets"), html);
}

function renderHeader() {
  const ts = targets();
  const t = S.view === "all" ? null : ts[0];
  $("#title").textContent = t ? t.name : "Overview";
  const sub = t
    ? [t.kind, t.host ? `${t.host.hostname} · load ${t.host.load1.toFixed(1)}/${t.host.cpus} · RAM ${gb(t.host.mem_used_mb)}/${gb(t.host.mem_total_mb)}` : ""].filter(Boolean).join("  ·  ")
    : `${ts.length} targets`;
  $("#subtitle").title = sub;
  $("#subtitle").innerHTML = `${esc(sub)}  ·  <span data-ago="${t ? t.updated_at : S.snap?.taken_at ?? 0}"></span>`;

  const gpus = ts.flatMap((t) => t.gpus);
  const busy = gpus.filter((g) => (g.util ?? 0) >= 10).length;
  const avg = gpus.length ? gpus.reduce((a, g) => a + (g.util ?? 0), 0) / gpus.length : null;
  const jobs = ts.flatMap((t) => t.jobs);
  const running = jobs.filter((j) => j.state === "running").length;
  const queued = jobs.filter(isQueued).length;
  const next = jobs.filter((j) => j.eta).sort((a, b) => a.eta.finish_at - b.eta.finish_at)[0];
  const stats = [
    gpus.length ? [`${busy}/${gpus.length}`, "GPUs busy"] : null,
    avg != null ? [`${avg.toFixed(0)}%`, "avg util"] : null,
    [running, "running"],
    [queued, "queued"],
    next ? [`<span data-eta="${next.eta.finish_at}"></span>`, `next · ${esc(next.name)}`] : null,
  ].filter(Boolean);
  setHTML($("#stats"), stats.map(([v, l]) => `<div class="stat"><b>${v}</b><span>${l}</span></div>`).join(""));
}

function renderBanner() {
  if (S.error) {
    setHTML($("#banner"), `<div class="banner"><b>Config</b><span class="msg">${esc(S.error)}</span>
      <button class="primary" data-act="settings">Open Settings</button></div>`);
    return;
  }
  const html = targets()
    .filter((t) => t.error)
    .map((t) => `<div class="banner ${t.updated_at ? "warn" : ""}"><b>${esc(t.name)}</b>
      <span class="msg">${esc(t.error)}${t.updated_at ? ` <span class="muted">— showing data from <span data-ago="${t.updated_at}"></span></span>` : ""}</span>
      <button data-act="refresh-target" data-target="${esc(t.name)}">Retry</button></div>`)
    .join("");
  setHTML($("#banner"), html);
}

function sparkline(samples) {
  if (!samples || samples.length < 2) return `<svg class="spark" viewBox="0 0 100 30" preserveAspectRatio="none"></svg>`;
  const n = HIST - 1;
  const off = HIST - samples.length;
  const pts = samples.map((v, i) => `${(((off + i) / n) * 100).toFixed(2)},${(29 - (v / 100) * 28).toFixed(2)}`);
  const first = pts[0].split(",")[0];
  return `<svg class="spark" viewBox="0 0 100 30" preserveAspectRatio="none">
    <path class="area" d="M${first},30 L${pts.join(" L")} L100,30 Z"/>
    <path class="line" d="M${pts.join(" L")}"/></svg>`;
}

function renderGpus() {
  const cards = [];
  for (const t of targets()) {
    for (const g of t.gpus) {
      const util = g.util ?? 0;
      const memF = g.mem_total_mb ? (g.mem_used_mb ?? 0) / g.mem_total_mb : 0;
      const powF = g.power_limit_w ? g.power_w / g.power_limit_w : null;
      const where = [S.view === "all" ? t.name : null, g.job_id ? `job ${g.job_id}` : null, g.node].filter(Boolean).join(" · ");
      cards.push(`<div class="gpu">
        <div class="gpu-top"><div class="gpu-id">${vendorIcon(g.name)}<div style="min-width:0"><div class="gpu-name">${esc(g.name)}</div>
          <div class="gpu-where">${esc(where || `gpu ${g.index}`)}</div></div></div>
          <div class="gpu-util">${g.util == null ? "–" : util.toFixed(0)}<small>%</small></div></div>
        ${sparkline(S.hist.get(`${t.name}/${g.job_id ?? ""}/${g.index}`))}
        <div class="meters">
          <div class="meter"><span>${g.unified ? "RAM" : "VRAM"}</span><div class="track"><div class="fill ${level(memF * 100)}" style="width:${(memF * 100).toFixed(1)}%"></div></div>
            <span>${gb(g.mem_used_mb)} / ${gb(g.mem_total_mb)}</span></div>
          ${powF != null ? `<div class="meter"><span>Power</span><div class="track"><div class="fill ${powF > 1 ? "red" : "green"}" style="width:${Math.min(100, powF * 100).toFixed(1)}%"></div></div>
            <span>${g.power_w.toFixed(0)} / ${g.power_limit_w.toFixed(0)} W</span></div>` : ""}
        </div>
        <div class="gpu-foot">
          ${g.temp_c != null ? `<span class="${g.temp_c >= 85 ? "hot" : ""}">${g.temp_c.toFixed(0)} °C</span>` : ""}
          ${g.power_w != null && powF == null ? `<span>${g.power_w.toFixed(0)} W</span>` : ""}
          ${g.unified ? `<span>unified memory</span>` : ""}
        </div></div>`);
    }
  }
  $("#gpu-count").textContent = cards.length || "";
  setHTML($("#gpus"), cards.join("") || `<div class="empty">No GPU telemetry for this view. Set <span class="mono">host_gpus = true</span> or <span class="mono">job_gpus = true</span> on a target.</div>`);
}

function jobRow(t, j) {
  const k = key(t, j);
  const open = S.open.has(k);
  const id = j.array_tasks ? `${j.id} ×${j.array_tasks}` : j.id;
  const sub2 = [S.view === "all" ? t : null, j.queue, j.user].filter(Boolean).join(" · ");

  let prog = `<span class="faint">—</span>`;
  if (j.progress) {
    const f = j.progress.fraction;
    prog = `<div class="prog"><div class="track"><div class="fill green" style="width:${(f * 100).toFixed(2)}%"></div></div>
      <span class="pct">${pct(f)}</span></div><div class="l2">${esc(j.progress.label)}${j.progress.it_per_s ? ` · ${j.progress.it_per_s.toFixed(2)} it/s` : ""}</div>`;
  } else if (j.state === "pending" && j.est_start_at) {
    prog = `<div class="l1 muted">starts ~${esc(clock(j.est_start_at))}</div>`;
  } else if (j.detail && isQueued(j)) {
    prog = `<div class="l1 muted">${esc(j.detail)}</div>`;
  } else if (j.last_line) {
    prog = `<div class="l1 muted mono small" title="${esc(j.last_line)}">${esc(j.last_line)}</div>`;
  }

  let eta = `<span class="faint">—</span>`;
  if (j.eta) {
    const approx = j.eta.method === "elapsed";
    eta = `<b class="${j.eta.exceeds_limit ? "warn" : ""}" title="ETA: ${esc(METHOD[j.eta.method])}">${approx ? '<span class="approx">~</span>' : ""}<span data-eta="${j.eta.finish_at}"></span></b>
      <div class="l2 ${j.eta.exceeds_limit ? "eta warn" : ""}">${j.eta.exceeds_limit ? "⚠ past limit · " : ""}${esc(clock(j.eta.finish_at))}</div>`;
  }

  const left = j.time_limit_s != null && j.elapsed_s != null ? Math.max(0, j.time_limit_s - j.elapsed_s) : null;
  const time = j.elapsed_s != null
    ? `<div class="l1 num">${human(j.elapsed_s)}</div><div class="l2">${left != null ? `${human(left)} left` : "no limit"}</div>`
    : `<div class="l1 muted">${j.time_limit_s ? `limit ${human(j.time_limit_s)}` : "—"}</div>${j.submitted_at ? `<div class="l2">queued ${human(now() - j.submitted_at)}</div>` : ""}`;

  const silent = j.state === "running" && j.log_age_s > 600 ? ` · <span class="eta warn">log silent ${human(j.log_age_s)}</span>` : "";
  let html = `<div class="job ${open ? "open" : ""}" data-key="${esc(k)}">
    <div class="cell"><span class="pill ${j.state}">${esc(j.state)}</span></div>
    <div class="cell"><div class="l1 name" title="${esc(j.name)}">${esc(j.name)}</div><div class="l2 mono">${esc(id)}${sub2 ? ` · ${esc(sub2)}` : ""}${silent}</div></div>
    <div class="cell">${prog}</div>
    <div class="cell eta">${eta}</div>
    <div class="cell">${time}</div>
    <div class="cell res"><div class="l1">${esc(j.resources ?? "")}</div><div class="l2">${esc(j.state === "running" ? j.detail ?? "" : "")}</div></div>
    <div class="cell"><button class="more" data-act="toggle" title="Details">${open ? "▾" : "▸"}</button></div>
  </div>`;
  if (open) html += detail(t, j);
  return html;
}

function detail(t, j) {
  const rows = [
    ["Target", t],
    ["Job", `${j.id}${j.raw_state ? ` · ${j.raw_state}` : ""}`],
    j.started_at && ["Started", `${clock(j.started_at)} (${human(now() - j.started_at)} ago)`],
    j.submitted_at && ["Submitted", clock(j.submitted_at)],
    j.est_start_at && ["Est. start", clock(j.est_start_at)],
    j.time_limit_s && ["Time limit", human(j.time_limit_s)],
    j.eta && ["ETA method", METHOD[j.eta.method]],
    j.resources && ["Resources", j.resources],
    j.detail && [isQueued(j) ? "Reason" : "Node", j.detail],
    j.log_path && ["Log", j.log_path],
    j.log_age_s != null && ["Log written", `${human(j.log_age_s)} ago`],
  ].filter(Boolean);
  const btn = (a, label, cls = "") => `<button data-act="${a}" class="${cls}">${label}</button>`;
  const acts = j.actions
    .map((a) => ({ logs: btn("logs", "View log"), hold: btn("hold", "Hold"), release: btn("release", "Release"), cancel: btn("cancel", "Cancel job…", "danger-text") })[a])
    .join("");
  return `<div class="detail" data-key="${esc(key(t, j))}">
    <dl>${rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join("")}</dl>
    ${j.last_line ? `<div class="lastline">${esc(j.last_line)}</div>` : ""}
    <div class="detail-actions">${acts}</div></div>`;
}

function renderJobs() {
  const rows = allJobs();
  $("#job-count").textContent = rows.length || "";
  if (!rows.length) {
    const any = targets().some((t) => t.jobs.length);
    return setHTML($("#jobs"), `<div class="empty pad">${any ? "No jobs match this filter." : "No jobs."}</div>`);
  }
  const head = `<div class="job-cols"><div>State</div><div>Job</div><div>Progress</div><div>ETA</div><div>Time</div><div class="res">Resources</div><div></div></div>`;
  const active = rows.filter(({ j }) => !isQueued(j));
  let html = head + active.map(({ t, j }) => jobRow(t, j)).join("");
  for (const state of ["pending", "held"]) {
    const group = rows.filter(({ j }) => j.state === state);
    if (!group.length) continue;
    const forced = S.filter === "queued" || S.query;
    const open = forced || S.groups.has(state) || group.length <= QUEUE_PREVIEW;
    const shown = open ? group : [];
    const tasks = group.reduce((n, { j }) => n + (j.array_tasks ?? 1), 0);
    if (!forced) {
      html += `<button class="group ${open ? "open" : ""}" data-act="group" data-group="${state}">
        <span class="chev">▸</span>${state === "held" ? "Held" : "Pending"} · ${group.length} job${group.length === 1 ? "" : "s"}${tasks !== group.length ? ` (${tasks} tasks)` : ""}
        ${state === "held" && group.every(({ j }) => j.actions.includes("release")) ? `<span class="gbtn faint">expand to release individually</span>` : ""}</button>`;
    }
    html += shown.map(({ t, j }) => jobRow(t, j)).join("");
  }
  setHTML($("#jobs"), html);
}

function tick() {
  if (document.hidden) return;
  const n = now();
  document.querySelectorAll("[data-eta]").forEach((el) => {
    el.textContent = human(Number(el.dataset.eta) - n);
  });
  document.querySelectorAll("[data-ago]").forEach((el) => {
    const t = Number(el.dataset.ago);
    el.textContent = t ? `updated ${human(n - t)} ago` : "connecting…";
  });
}
setInterval(tick, 1000);

// ---- actions ----------------------------------------------------------------

function findJob(k) {
  const [t, ...rest] = k.split("::");
  const id = rest.join("::");
  const target = S.snap?.targets.find((x) => x.name === t);
  return { t, kind: target?.kind, j: target?.jobs.find((j) => j.id === id) };
}

let toastTimer;
function toast(msg, err = false) {
  const el = $("#toast");
  el.textContent = msg;
  el.className = `toast ${err ? "err" : ""}`;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), err ? 6000 : 3000);
}

async function act(t, j, action) {
  try {
    const out = await invoke("job_action", { target: t, id: j.id, action });
    toast(out);
  } catch (e) {
    toast(String(e), true);
  }
}

function confirmDialog(title, body, okLabel) {
  $("#modal-title").textContent = title;
  $("#modal-body").textContent = body;
  $("#modal-ok").textContent = okLabel;
  $("#modal").hidden = false;
  $("#modal-ok").focus();
  return new Promise((resolve) => (S.confirm = resolve));
}
function closeConfirm(ok) {
  $("#modal").hidden = true;
  S.confirm?.(ok);
  S.confirm = null;
}

async function openLog(t, j) {
  S.log = { target: t, id: j.id, path: j.log_path ?? null, name: j.name };
  $("#sheet-title").textContent = `${j.name}`;
  $("#sheet-sub").textContent = `${t} · ${j.id}${j.log_path ? ` · ${j.log_path}` : ""}`;
  $("#sheet").hidden = false;
  await loadLog();
}

async function loadLog() {
  if (!S.log) return;
  const body = $("#sheet-body");
  const btn = $("#sheet-refresh");
  btn.classList.add("spin");
  try {
    const { target, id, path } = S.log;
    const text = await invoke("job_logs", { target, id, path, lines: Number($("#sheet-lines").value) });
    const stick = body.scrollTop + body.clientHeight >= body.scrollHeight - 30 || !body.textContent;
    body.textContent = text || "(empty)";
    if (stick) body.scrollTop = body.scrollHeight;
  } catch (e) {
    body.textContent = String(e);
  } finally {
    btn.classList.remove("spin");
  }
}

async function refreshAll(target = null) {
  const btn = $("#btn-refresh");
  btn.classList.add("spin");
  setTimeout(() => btn.classList.remove("spin"), 900);
  await invoke("refresh", { target });
}

// ---- events -----------------------------------------------------------------

$("#targets").addEventListener("click", (e) => {
  const b = e.target.closest("[data-view]");
  if (!b) return;
  S.view = b.dataset.view;
  closeSettings();
  render();
});

$("#filter").addEventListener("click", (e) => {
  const b = e.target.closest("[data-f]");
  if (!b) return;
  S.filter = b.dataset.f;
  document.querySelectorAll("#filter button").forEach((x) => x.classList.toggle("on", x === b));
  render();
});

$("#search").addEventListener("input", (e) => {
  S.query = e.target.value.trim();
  render();
});

$("#banner").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-act]");
  if (!b) return;
  if (b.dataset.act === "settings") openSettings();
  if (b.dataset.act === "refresh-target") await refreshAll(b.dataset.target);
});

$("#jobs").addEventListener("click", async (e) => {
  const actBtn = e.target.closest("[data-act]");
  if (actBtn?.dataset.act === "group") {
    const g = actBtn.dataset.group;
    S.groups.has(g) ? S.groups.delete(g) : S.groups.add(g);
    return render();
  }
  const row = e.target.closest("[data-key]");
  if (!row) return;
  const k = row.dataset.key;
  const { t, kind, j } = findJob(k);
  const a = actBtn?.dataset.act;
  if (!a || a === "toggle") {
    if (row.classList.contains("detail") && !a) return; // clicks inside detail don't collapse
    S.open.has(k) ? S.open.delete(k) : S.open.add(k);
    return render();
  }
  if (!j) return;
  if (a === "logs") return openLog(t, j);
  if (a === "cancel") {
    // Vast instances and SkyPilot clusters are stopped (disks kept), not destroyed.
    const stops = kind === "vast.ai" || (kind === "skypilot" && !/^\d+$/.test(j.id));
    const ok = stops
      ? await confirmDialog(`Stop “${j.name}”?`, `${j.id} on ${t} will be stopped. Its disk is kept and storage is still billed.`, "Stop")
      : await confirmDialog(`Cancel “${j.name}”?`, `Job ${j.id} on ${t} will be terminated. This cannot be undone.`, "Cancel job");
    if (ok) await act(t, j, "cancel");
    return;
  }
  await act(t, j, a);
});

$("#btn-refresh").addEventListener("click", () => refreshAll());
$("#btn-settings").addEventListener("click", () => (CFG.open ? closeSettings() : openSettings()));
$("#sheet-close").addEventListener("click", () => { $("#sheet").hidden = true; S.log = null; });
$("#sheet-refresh").addEventListener("click", loadLog);
$("#sheet-lines").addEventListener("change", loadLog);
$("#modal-ok").addEventListener("click", () => closeConfirm(true));
$("#modal-cancel").addEventListener("click", () => closeConfirm(false));
$("#modal").addEventListener("click", (e) => e.target.id === "modal" && closeConfirm(false));

document.addEventListener("keydown", (e) => {
  if (e.metaKey && e.key === ",") { e.preventDefault(); return openSettings(); }
  if (e.metaKey && e.key === "s" && CFG.open) { e.preventDefault(); return saveSettings(); }
  if (e.key === "Escape") {
    if (!$("#modal").hidden) return closeConfirm(false);
    if (!$("#sheet").hidden) { $("#sheet").hidden = true; S.log = null; return; }
    if (S.query) { $("#search").value = ""; S.query = ""; return render(); }
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "r") { e.preventDefault(); refreshAll(); }
  if (e.key === "/" && !CFG.open && !/^(INPUT|SELECT|TEXTAREA)$/.test(document.activeElement?.tagName)) { e.preventDefault(); $("#search").focus(); }
  if (/^[1-9]$/.test(e.key) && e.metaKey) {
    const ts = S.snap?.targets ?? [];
    const i = Number(e.key) - 1;
    S.view = i === 0 ? "all" : ts[i - 1]?.name ?? S.view;
    closeSettings();
    render();
  }
});

// Auto-refresh the open log while its job is running.
setInterval(() => {
  if (!S.log || $("#sheet").hidden) return;
  const { j } = findJob(`${S.log.target}::${S.log.id}`);
  if (j?.state === "running") loadLog();
}, 10000);

// ---- settings ---------------------------------------------------------------
// Edits a draft copy of the config; Save validates on both sides, writes the
// TOML file and restarts polling.

const CFG = {
  open: false,
  path: "",
  exists: true,
  saved: null,   // last saved config (with defaults filled in)
  draft: null,
  sel: 0,        // selected target index
  tests: new WeakMap(), // target object → test result html
};

const TARGET_DEFAULTS = {
  kind: "host", host: null, scheduler: "none", host_gpus: false, job_gpus: false,
  all_users: false, interval: null, command: null, ssh_args: [], enabled: true,
};
const clone = (x) => JSON.parse(JSON.stringify(x));
const withDefaults = (cfg) => ({
  interval: cfg.interval ?? 300,
  tail_bytes: cfg.tail_bytes ?? 6144,
  targets: (cfg.targets ?? cfg.target ?? []).map((t) => ({ ...clone(TARGET_DEFAULTS), ...t })),
});
// Cloud kinds polled through a local CLI: kind → [label, default command, has uvx fallback].
const CLOUDS = {
  modal: ["Modal", "modal", true],
  vast: ["Vast.ai", "vastai", true],
  skypilot: ["SkyPilot", "sky", false],
};
const isCloud = (m) => m in CLOUDS;
const mode = (t) => (isCloud(t.kind) ? t.kind : t.host != null ? "ssh" : "local");
const isDirty = () => CFG.draft && JSON.stringify(CFG.draft) !== JSON.stringify(CFG.saved);

function describeTarget(t) {
  if (isCloud(t.kind)) return CLOUDS[t.kind][0];
  return [
    mode(t) === "ssh" ? t.host || "ssh" : "This Mac",
    t.scheduler !== "none" ? (t.scheduler === "sge" ? "SGE" : "Slurm") : null,
    t.host_gpus || t.job_gpus ? "GPUs" : null,
  ].filter(Boolean).join(" · ");
}

function validate(cfg) {
  const errs = { general: {}, targets: [] };
  if (!(cfg.interval >= 2)) errs.general.interval = "At least 2 seconds";
  if (!(cfg.tail_bytes >= 1024)) errs.general.tail = "At least 1 KB";
  const names = new Map();
  cfg.targets.forEach((t) => names.set(t.name.trim(), (names.get(t.name.trim()) ?? 0) + 1));
  cfg.targets.forEach((t, i) => {
    const e = {};
    if (!t.name.trim()) e.name = "Required";
    else if (names.get(t.name.trim()) > 1) e.name = "Another target has this name";
    if (mode(t) === "ssh" && !t.host?.trim()) e.host = "Required — an alias from ~/.ssh/config";
    if (t.interval != null && !(t.interval >= 2)) e.interval = "At least 2 seconds";
    if (Object.keys(e).length) errs.targets[i] = e;
  });
  errs.count = Object.keys(errs.general).length + errs.targets.filter(Boolean).length;
  return errs;
}

async function openSettings() {
  CFG.open = true;
  $("#view-dash").hidden = true;
  $("#view-settings").hidden = false;
  $("#sheet").hidden = true;
  if (!CFG.draft) await loadSettings();
  renderSettings();
  render();
}

function closeSettings() {
  if (!CFG.open) return;
  CFG.open = false;
  $("#view-settings").hidden = true;
  $("#view-dash").hidden = false;
  updateSettingsChrome();
  render();
}

async function loadSettings() {
  try {
    const r = await invoke("settings");
    CFG.path = r.path;
    CFG.exists = r.exists;
    CFG.saved = withDefaults(r.config);
    CFG.draft = clone(CFG.saved);
    CFG.sel = 0;
    $("#ssh-hosts").innerHTML = r.ssh_hosts.map((h) => `<option value="${esc(h)}">`).join("");
  } catch (e) {
    toast(String(e), true);
    CFG.saved = withDefaults({});
    CFG.draft = clone(CFG.saved);
  }
}

async function saveSettings() {
  if (!CFG.draft) return;
  const errs = validate(CFG.draft);
  if (errs.count) return toast("Fix the highlighted fields first", true);
  const btn = $("#set-save");
  btn.disabled = true;
  try {
    const saved = await invoke("save_settings", { config: CFG.draft });
    CFG.saved = withDefaults(saved);
    CFG.draft = clone(CFG.saved);
    CFG.exists = true;
    S.error = null;
    S.seen.clear();
    toast("Saved · monitoring restarted");
  } catch (e) {
    toast(String(e), true);
  }
  renderSettings();
}

function renderSettings() {
  const d = CFG.draft;
  if (!d) return;
  $("#set-path").textContent = CFG.path;
  $("#set-path").title = CFG.path;
  $("#g-interval").value = d.interval;
  $("#g-tail").value = Math.round(d.tail_bytes / 1024);
  CFG.sel = Math.min(CFG.sel, Math.max(0, d.targets.length - 1));
  renderTargetList();
  renderTargetForm();
  updateSettingsChrome();
}

function renderTargetList() {
  const d = CFG.draft;
  const errs = validate(d);
  $("#t-list").innerHTML = d.targets.length
    ? d.targets.map((t, i) => `<button class="t-item ${i === CFG.sel ? "on" : ""} ${t.enabled ? "" : "off"}" data-i="${i}">
        <span class="dot ${errs.targets[i] ? "down" : t.enabled ? "ok" : ""}"></span>
        <span class="t-name">${esc(t.name.trim() || "Untitled")}</span>
        <span class="t-desc">${esc(describeTarget(t))}</span></button>`).join("")
    : `<div class="empty pad">No targets yet.</div>`;
}

const row = (title, help, control, err = "") => `<div class="row">
  <div><div class="row-title">${title}</div>${help ? `<div class="row-help">${help}</div>` : ""}
  <div class="field-err" data-err="${err}"></div></div><div class="ctl">${control}</div></div>`;
const toggle = (f, on, disabled = false) =>
  `<label class="switch ${disabled ? "disabled" : ""}"><input type="checkbox" data-f="${f}" ${on ? "checked" : ""} ${disabled ? "disabled" : ""}><span></span></label>`;
const seg = (attr, value, opts) => `<div class="segmented">${opts
  .map(([v, label]) => `<button data-${attr}="${v}" class="${v === value ? "on" : ""}">${label}</button>`).join("")}</div>`;

function renderTargetForm() {
  const t = CFG.draft.targets[CFG.sel];
  const form = $("#t-form");
  if (!t) {
    form.innerHTML = `<div class="empty pad">Add a target to start monitoring a machine, cluster or cloud.</div>`;
    return;
  }
  const m = mode(t);
  const slurm = t.scheduler === "slurm";
  let html = `<div class="form-head">
      <div style="min-width:0"><div class="form-title" id="f-title">${esc(t.name.trim() || "Untitled")}</div>
      <div class="row-help" id="f-desc">${esc(describeTarget(t))}</div></div>
      <div class="form-enabled"><span class="muted">${t.enabled ? "Enabled" : "Disabled"}</span>${toggle("enabled", t.enabled)}</div>
    </div>
    <h3 class="sub">Connection</h3>
    <div class="group-box">
      ${row("Name", "Shown in the sidebar and used by the CLI.", `<input data-f="name" value="${esc(t.name)}" spellcheck="false" autocomplete="off">`, "name")}
      ${row("Type", "", seg("mode", m, [["ssh", "SSH host"], ["local", "This Mac"], ...Object.entries(CLOUDS).map(([k, [label]]) => [k, label])]))}`;
  if (m === "ssh") {
    html += row("SSH host", "An alias from ~/.ssh/config. Proxies and jump hosts are handled by ssh.",
      `<input data-f="host" value="${esc(t.host ?? "")}" list="ssh-hosts" placeholder="e.g. dgx" spellcheck="false" autocomplete="off">`, "host");
    html += row("SSH options", "Extra arguments passed to ssh. Optional.",
      `<input data-f="ssh_args" value="${esc(t.ssh_args.join(" "))}" placeholder="-p 2222" spellcheck="false" autocomplete="off">`);
  }
  if (isCloud(m)) {
    const [, cmd, uvx] = CLOUDS[m];
    const help = uvx
      ? `Falls back to <span class="mono">uvx ${cmd}</span> when empty and ${cmd} isn't installed.`
      : `Leave empty to use <span class="mono">${cmd}</span> from your PATH.`;
    html += row("CLI command", help,
      `<input data-f="command" value="${esc(t.command ?? "")}" placeholder="${cmd}" spellcheck="false" autocomplete="off">`);
  }
  html += `</div>`;
  if (!isCloud(m)) {
    html += `<h3 class="sub">Monitoring</h3><div class="group-box">
      ${row("Scheduler", "Lists your jobs with progress, ETAs and controls.", seg("sched", t.scheduler, [["none", "None"], ["slurm", "Slurm"], ["sge", "SGE"]]))}
      ${row("Machine GPUs", m === "local" ? "Apple GPU utilisation on this Mac." : "Runs nvidia-smi (or amd-smi) on the host itself.", toggle("host_gpus", t.host_gpus))}
      ${row("GPUs inside jobs", slurm ? "Runs nvidia-smi in your running jobs via srun --overlap." : "Requires Slurm.", toggle("job_gpus", t.job_gpus, !slurm))}
      ${row("All users", "Show everyone's jobs, not only yours.", toggle("all_users", t.all_users))}
    </div>`;
  }
  html += `<h3 class="sub">Polling</h3><div class="group-box">
      ${row("Interval", `Leave empty to use the default (${CFG.draft.interval} s).`,
        `<div class="unit"><input data-f="interval" type="number" min="2" value="${t.interval ?? ""}" placeholder="${CFG.draft.interval}"><span>sec</span></div>`, "interval")}
    </div>
    <div class="form-foot">
      <button data-t="test">Test connection</button>
      <span class="test-result" id="t-test">${CFG.tests.get(t) ?? ""}</span>
      <span class="spacer"></span>
      <button class="ghost icon" data-t="up" title="Move up" ${CFG.sel === 0 ? "disabled" : ""}>↑</button>
      <button class="ghost icon" data-t="down" title="Move down" ${CFG.sel === CFG.draft.targets.length - 1 ? "disabled" : ""}>↓</button>
      <button class="ghost" data-t="dup">Duplicate</button>
      <button class="ghost danger-text" data-t="del">Delete</button>
    </div>`;
  form.innerHTML = html;
  showErrors();
}

function showErrors() {
  const errs = validate(CFG.draft);
  const te = errs.targets[CFG.sel] ?? {};
  document.querySelectorAll("#t-form [data-err]").forEach((el) => {
    const msg = te[el.dataset.err] ?? "";
    el.textContent = msg;
    el.closest(".row").classList.toggle("invalid", !!msg);
  });
  [["#g-interval", errs.general.interval], ["#g-tail", errs.general.tail]].forEach(([sel, msg]) => {
    $(sel).closest(".row").classList.toggle("invalid", !!msg);
    $(sel).title = msg ?? "";
  });
  return errs;
}

function updateSettingsChrome() {
  const dirty = isDirty();
  const errs = CFG.draft ? validate(CFG.draft) : { count: 0 };
  $("#set-save").disabled = !(dirty || !CFG.exists) || errs.count > 0;
  $("#set-revert").disabled = !dirty;
  $("#settings-dirty").textContent = dirty ? "●" : "";
  $("#btn-settings").classList.toggle("on", CFG.open);
  $("#set-error").innerHTML = !CFG.exists
    ? `<div class="banner warn"><b>No config yet</b><span class="msg">These are example targets. Edit them and press Save to create the file.</span></div>`
    : "";
}

// Text edits update the draft in place; structural edits re-render the form.
function changed(structural = false) {
  if (structural) renderTargetForm();
  else {
    const t = CFG.draft.targets[CFG.sel];
    if (t) {
      $("#f-title").textContent = t.name.trim() || "Untitled";
      $("#f-desc").textContent = describeTarget(t);
    }
    showErrors();
  }
  renderTargetList();
  updateSettingsChrome();
}

$("#t-form").addEventListener("input", (e) => {
  const f = e.target.dataset.f;
  const t = CFG.draft.targets[CFG.sel];
  if (!f || !t || e.target.type === "checkbox") return;
  const v = e.target.value;
  if (f === "name") t.name = v;
  if (f === "host") t.host = v;
  if (f === "command") t.command = v.trim() ? v : null;
  if (f === "ssh_args") t.ssh_args = v.split(/\s+/).filter(Boolean);
  if (f === "interval") t.interval = v === "" ? null : Number(v);
  changed();
});

$("#t-form").addEventListener("change", (e) => {
  const f = e.target.dataset.f;
  const t = CFG.draft.targets[CFG.sel];
  if (!f || !t || e.target.type !== "checkbox") return;
  t[f] = e.target.checked;
  changed(true);
});

$("#t-form").addEventListener("click", async (e) => {
  const b = e.target.closest("button");
  const t = CFG.draft.targets[CFG.sel];
  if (!b || !t) return;
  if (b.dataset.mode) {
    const m = b.dataset.mode;
    if (isCloud(m) && t.kind !== m) Object.assign(t, { kind: m, host: null, command: null, scheduler: "none", host_gpus: false, job_gpus: false, ssh_args: [] });
    if (m === "ssh") Object.assign(t, { kind: "host", host: t.host ?? "", command: null });
    if (m === "local") Object.assign(t, { kind: "host", host: null, command: null, ssh_args: [] });
    return changed(true);
  }
  if (b.dataset.sched) {
    t.scheduler = b.dataset.sched;
    if (t.scheduler !== "slurm") t.job_gpus = false;
    return changed(true);
  }
  const a = b.dataset.t;
  const list = CFG.draft.targets;
  const i = CFG.sel;
  if (a === "up" && i > 0) { [list[i - 1], list[i]] = [list[i], list[i - 1]]; CFG.sel--; return changed(true); }
  if (a === "down" && i < list.length - 1) { [list[i + 1], list[i]] = [list[i], list[i + 1]]; CFG.sel++; return changed(true); }
  if (a === "dup") {
    const copy = { ...clone(t), name: uniqueName(`${t.name.trim() || "target"} copy`) };
    list.splice(i + 1, 0, copy);
    CFG.sel = i + 1;
    return changed(true);
  }
  if (a === "del") {
    const ok = await confirmDialog(`Delete “${t.name.trim() || "Untitled"}”?`, "The target is removed when you save.", "Delete");
    if (!ok) return;
    list.splice(i, 1);
    CFG.sel = Math.max(0, i - 1);
    return changed(true);
  }
  if (a === "test") {
    const out = $("#t-test");
    out.innerHTML = `<span class="muted">Connecting…</span>`;
    b.disabled = true;
    let html;
    try {
      const r = await invoke("test_target", { target: t });
      html = r.ok
        ? `<span class="ok">✓</span> ${r.latency_ms} ms · ${r.gpus.length} GPU${r.gpus.length === 1 ? "" : "s"} · ${r.jobs.length} job${r.jobs.length === 1 ? "" : "s"}`
        : `<span class="bad">✕ ${esc(r.error ?? "failed")}</span>`;
    } catch (err) {
      html = `<span class="bad">✕ ${esc(String(err))}</span>`;
    }
    CFG.tests.set(t, html);
    b.disabled = false;
    if (CFG.draft.targets[CFG.sel] === t) $("#t-test").innerHTML = html;
  }
});

function uniqueName(base) {
  const taken = new Set(CFG.draft.targets.map((t) => t.name.trim()));
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base} ${n}`)) return `${base} ${n}`;
}

$("#t-list").addEventListener("click", (e) => {
  const b = e.target.closest("[data-i]");
  if (!b) return;
  CFG.sel = Number(b.dataset.i);
  renderTargetList();
  renderTargetForm();
});

$("#t-add").addEventListener("click", () => {
  CFG.draft.targets.push({ ...clone(TARGET_DEFAULTS), name: uniqueName("new-target"), host: "", host_gpus: true });
  CFG.sel = CFG.draft.targets.length - 1;
  changed(true);
  $('#t-form [data-f="name"]')?.select();
});

$("#g-interval").addEventListener("input", (e) => {
  CFG.draft.interval = e.target.value === "" ? 0 : Number(e.target.value);
  changed(true);
});
$("#g-tail").addEventListener("input", (e) => {
  CFG.draft.tail_bytes = Math.round(Number(e.target.value || 0) * 1024);
  changed();
});
$("#set-save").addEventListener("click", saveSettings);
$("#set-revert").addEventListener("click", () => {
  CFG.draft = clone(CFG.saved);
  renderSettings();
});
$("#set-open").addEventListener("click", () => invoke("open_config").catch((e) => toast(String(e), true)));

// ---- window -----------------------------------------------------------------

const zoom = () => appWindow.toggleMaximize().catch(() => {});
document.querySelectorAll(".toolbar, .traffic, .brand").forEach((el) => draggable(el, zoom));

// ---- boot -------------------------------------------------------------------

listen("snapshot", (e) => {
  S.snap = e.payload;
  S.error = null;
  sampleHistory();
  render();
});
listen("open-settings", () => openSettings());
listen("focus-target", (e) => {
  S.view = (S.snap?.targets ?? []).some((t) => t.name === e.payload) ? e.payload : "all";
  closeSettings();
  render();
});
listen("config-error", (e) => {
  S.error = e.payload;
  render();
});

invoke("snapshot")
  .then((snap) => { S.snap = snap; sampleHistory(); render(); })
  .catch((e) => { S.error = String(e); render(); });
