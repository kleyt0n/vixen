// Vixen UI. All data comes from the Rust side as `snapshot` events.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
const S = {
  snap: null,
  error: null,
  page: "home",         // "home" | "compute" | "agents"
  tool: "all",          // agents page: "all" | tool id
  agentFilter: "active",// "active" (working + waiting) | "all"
  agentQuery: "",
  agentOpen: new Set(), // expanded agent rows
  view: "all",          // compute page: "all" | target name
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
    if (S.page === "home") renderHome();
    if (S.page === "agents") renderAgents();
    if (S.page === "compute") {
      renderHeader();
      renderBanner();
      renderGpus();
      renderJobs();
    }
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

// ---- navigation -------------------------------------------------------------

const PAGES = ["home", "compute", "agents"];
function showPage(page) {
  S.page = page;
  if (CFG.open) closeSettings();
  for (const p of PAGES) $(`#view-${p === "compute" ? "dash" : p}`).hidden = p !== page;
  render();
}

const agentList = () => S.snap?.agents ?? [];
const running = (t) => t.jobs.filter((j) => j.state === "running" || j.state === "completing").length;

function renderNav() {
  const ts = S.snap?.targets ?? [];
  const ags = agentList();
  const on = (page, ok = true) => (!CFG.open && S.page === page && ok ? "on" : "");
  const jobsRunning = ts.reduce((n, t) => n + running(t), 0);
  const working = ags.filter((a) => a.state === "working").length;
  const waiting = ags.filter((a) => a.state === "waiting").length;
  const attn = attention().length;
  const health = ts.every((t) => t.ok) && ts.length ? "ok" : ts.some((t) => !t.ok && t.error) ? "stale" : "wait";

  let html = `<button class="nav-item ${on("home")}" data-page="home">
      <span class="nav-ico">⌂</span><span class="nav-name">Home</span>
      <span class="nav-badge ${attn ? "hot" : ""}">${attn || ""}</span></button>
    <div class="nav-label">Compute</div>
    <button class="nav-item ${on("compute", S.view === "all")}" data-page="compute" data-view="all">
      <span class="dot ${health}"></span><span class="nav-name">All machines</span><span class="nav-badge">${jobsRunning || ""}</span></button>`;
  for (const t of ts) {
    const r = running(t), q = t.jobs.filter(isQueued).length;
    const badge = [r ? `${r}▸` : "", q ? `${q}` : ""].filter(Boolean).join(" · ");
    html += `<button class="nav-item sub ${on("compute", S.view === t.name)}" data-page="compute" data-view="${esc(t.name)}" title="${esc(t.error ?? `${t.kind} · ${t.latency_ms} ms`)}">
      <span class="dot ${targetDot(t)}"></span>
      <span class="nav-name">${esc(t.name)}</span><span class="nav-badge">${badge}</span></button>`;
  }
  html += `<div class="nav-label">Agents</div>
    <button class="nav-item ${on("agents", S.tool === "all")}" data-page="agents" data-tool="all">
      <span class="dot ${working ? "ok" : waiting ? "stale" : ""}"></span><span class="nav-name">All agents</span>
      <span class="nav-badge">${[working && `${working}▸`, waiting && `${waiting}◆`].filter(Boolean).join(" · ")}</span></button>`;
  for (const id of Object.keys(TOOLS)) {
    const mine = ags.filter((a) => a.tool === id);
    if (!mine.length) continue;
    const w = mine.filter((a) => a.state === "working").length, q = mine.filter((a) => a.state === "waiting").length;
    html += `<button class="nav-item sub ${on("agents", S.tool === id)}" data-page="agents" data-tool="${id}">
      ${toolChip(id, "sm")}<span class="nav-name">${TOOLS[id].label}</span>
      <span class="nav-badge">${[w && `${w}▸`, q && `${q}◆`].filter(Boolean).join(" · ") || mine.length}</span></button>`;
  }
  setHTML($("#targets"), html);
}

// ---- agents -----------------------------------------------------------------

const agentKey = (a) => `${a.tool}::${a.id}`;
const kTokens = fmtTokens;
const STATE_LABEL = { working: "Working", waiting: "Waiting for you", idle: "Idle" };

function agentRow(a) {
  const k = agentKey(a);
  const open = S.agentOpen.has(k);
  const place = [a.project, a.branch].filter(Boolean).join(" · ");
  const ctx = a.context_tokens == null ? "" : a.context_window
    ? `<div class="ctx" title="${kTokens(a.context_tokens)} of ${kTokens(a.context_window)} context"><div class="track"><div class="fill ${a.context_tokens / a.context_window > 0.85 ? "amber" : "blue"}" style="width:${Math.min(100, (100 * a.context_tokens) / a.context_window).toFixed(1)}%"></div></div><span>${kTokens(a.context_tokens)}</span></div>`
    : `<span class="faint">${kTokens(a.context_tokens)} ctx</span>`;
  let html = `<div class="agent ${open ? "open" : ""}" data-akey="${esc(k)}">
    ${toolChip(a.tool)}
    <div class="cell"><div class="l1 name" title="${esc(a.title)}">${esc(a.title)}</div>
      <div class="l2">${esc(place || a.cwd || "")}${a.model ? ` · ${esc(a.model)}` : ""}</div></div>
    <div class="cell"><span class="pill ${a.state === "working" ? "running" : a.state === "waiting" ? "held" : ""}">${a.state}</span></div>
    <div class="cell num muted"><span data-since="${a.updated_at}"></span></div>
    <div class="cell">${ctx}</div>
    <div class="cell"><button class="more" data-aact="toggle">${open ? "▾" : "▸"}</button></div>
  </div>`;
  if (open) {
    const rows = [
      ["Tool", `${TOOLS[a.tool]?.label ?? a.tool}${a.source ? ` · ${a.source}` : ""}`],
      a.cwd && ["Folder", a.cwd],
      a.branch && ["Branch", a.branch],
      a.model && ["Model", a.model],
      a.context_tokens != null && ["Context", `${kTokens(a.context_tokens)}${a.context_window ? ` of ${kTokens(a.context_window)}` : ""} tokens`],
      a.started_at && ["Started", `${clock(a.started_at)}`],
      ["Session", a.id],
    ].filter(Boolean);
    const btn = (act, label) => `<button data-aact="${act}">${label}</button>`;
    html += `<div class="detail agent-detail" data-akey="${esc(k)}">
      <dl>${rows.map(([kk, v]) => `<dt>${esc(kk)}</dt><dd>${esc(v)}</dd>`).join("")}</dl>
      ${a.last_prompt ? `<div class="bubble you"><b>You</b>${esc(a.last_prompt)}</div>` : ""}
      ${a.last_reply ? `<div class="bubble"><b>${esc(TOOLS[a.tool]?.label ?? "Agent")}</b>${esc(a.last_reply)}</div>` : ""}
      <div class="detail-actions">
        ${a.resume ? btn("resume", "Resume in Terminal") : ""}
        ${a.tool === "antigravity" ? btn("app", "Open Antigravity") : ""}
        ${a.cwd ? btn("folder", "Open folder") : ""}
        ${a.file ? btn("reveal", "Reveal transcript") : ""}
      </div></div>`;
  }
  return html;
}

const usageList = () => S.snap?.usage ?? [];
const REFRESH_HINT = {
  claude_code: "Turn on Live Claude limits in Settings, or run /usage in Claude Code",
  codex: "Codex updates these as you use it",
};

function usageCard(u) {
  const label = TOOLS[u.tool]?.label ?? u.tool;
  const stale = u.limits.some((l) => l.resets_at && l.resets_at <= now());
  const asOf = u.limits_as_of ? `limits as of <span data-since="${u.limits_as_of}"></span>` : "";
  return `<div class="usage-card">
    <div class="uc-head">${toolChip(u.tool, "lg")}<div class="cell"><div class="l1 name">${esc(label)}</div>
      <div class="l2">${[u.plan && esc(u.plan), asOf].filter(Boolean).join(" · ")}</div></div></div>
    <div class="limits">${u.limits.map((l) => limitBar(l)).join("") || `<div class="faint small">No plan limits recorded yet.</div>`}</div>
    ${stale ? `<div class="uc-note">Some limits have reset since. ${esc(REFRESH_HINT[u.tool] ?? "")}.</div>` : ""}
    <div class="uc-tokens">
      <div><b>${fmtTokens(u.tokens_5h)}</b><span>tokens · last 5h</span></div>
      <div><b>${fmtTokens(u.tokens_today)}</b><span>tokens · today</span></div>
      <div><b>${u.sessions_today}</b><span>sessions today</span></div>
    </div></div>`;
}

function renderAgents() {
  const usage = usageList().filter((u) => S.tool === "all" || u.tool === S.tool);
  setHTML($("#usage"), usage.map(usageCard).join(""));
  const q = S.agentQuery.toLowerCase();
  const list = agentList()
    .filter((a) => S.tool === "all" || a.tool === S.tool)
    .filter((a) => S.agentFilter === "all" || a.state !== "idle")
    .filter((a) => !q || `${a.title} ${a.project ?? ""} ${a.branch ?? ""} ${a.cwd ?? ""} ${a.model ?? ""}`.toLowerCase().includes(q));
  const all = agentList().filter((a) => S.tool === "all" || a.tool === S.tool);
  $("#agents-title").textContent = S.tool === "all" ? "Agents" : TOOLS[S.tool]?.label ?? S.tool;
  const w = all.filter((a) => a.state === "working").length, wt = all.filter((a) => a.state === "waiting").length;
  $("#agents-sub").textContent = `${w} working · ${wt} waiting · ${all.length} sessions`;
  if (!list.length) {
    const hint = all.length
      ? `No ${S.agentFilter === "active" ? "active " : ""}sessions match. ${S.agentFilter === "active" ? "Switch to All to see idle ones." : ""}`
      : "No agent sessions in the window. Start Claude Code, Codex or Antigravity and they appear here.";
    return setHTML($("#agents"), `<div class="empty pad">${hint}</div>`);
  }
  let html = "";
  for (const st of ["working", "waiting", "idle"]) {
    const group = list.filter((a) => a.state === st);
    if (!group.length) continue;
    html += `<div class="agent-group"><h2 class="section-title">${STATE_LABEL[st]} <span class="count">${group.length}</span></h2>
      <div class="agent-list">${group.map(agentRow).join("")}</div></div>`;
  }
  setHTML($("#agents"), html);
}

// ---- home -------------------------------------------------------------------

/// Things that want a human (see needsYou in common.js), minus dismissed ones.
const attention = () => needsYou(S.snap);
const dismissBtn = (it) => `<button class="dismiss" data-dismiss="${esc(it.key)}" title="Dismiss">✕</button>`;

function attnRow(it) {
  if (it.kind === "agent") {
    const a = it.a;
    return `<div class="attn-row" data-akey="${esc(agentKey(a))}">${toolChip(a.tool)}
      <div class="cell"><div class="l1 name">${esc(a.title)}</div><div class="l2">${esc(TOOLS[a.tool]?.label)} is waiting for you · ${esc(a.project ?? "")} · <span data-since="${a.updated_at}"></span></div></div>
      <div class="attn-acts">${a.resume ? `<button data-aact="resume">Resume</button>` : `<button data-aact="open">Open</button>`}${dismissBtn(it)}</div></div>`;
  }
  if (it.kind === "down") {
    return `<div class="attn-row" data-target="${esc(it.t.name)}"><span class="attn-ico down">!</span>
      <div class="cell"><div class="l1 name">${esc(it.t.name)} is unreachable</div><div class="l2">${esc(it.t.error)}</div></div>
      <div class="attn-acts"><button data-hact="retry">Retry</button>${dismissBtn(it)}</div></div>`;
  }
  const j = it.j;
  const msg = it.kind === "limit" ? `will pass its time limit · ETA ${clock(j.eta.finish_at)}` : "failed";
  return `<div class="attn-row" data-target="${esc(it.t.name)}"><span class="attn-ico warn">⏱</span>
    <div class="cell"><div class="l1 name">${esc(j.name)} <span class="faint mono small">${esc(j.id)}</span></div><div class="l2">${esc(it.t.name)} · ${msg}</div></div>
    <div class="attn-acts"><button data-hact="job">View</button>${dismissBtn(it)}</div></div>`;
}

function renderHome() {
  const ts = S.snap?.targets ?? [];
  const ags = agentList();
  const items = attention();
  $("#home-sub").innerHTML = `${ts.length} machines · ${ags.length} agent sessions · <span data-ago="${S.snap?.taken_at ?? 0}"></span>`;
  $("#attn-count").innerHTML = items.length ? `${items.length} <button class="link" data-dismiss-all>Clear all</button>` : "";
  setHTML($("#attn"), items.length ? items.map(attnRow).join("") : `<div class="all-clear">✓ Nothing needs you right now.</div>`);

  // Compute card
  const gpus = ts.flatMap((t) => t.gpus);
  const busy = gpus.filter((g) => (g.util ?? 0) >= 10).length;
  const avg = gpus.length ? gpus.reduce((x, g) => x + (g.util ?? 0), 0) / gpus.length : null;
  const jobs = ts.flatMap((t) => t.jobs.map((j) => ({ t: t.name, j })));
  const run = jobs.filter(({ j }) => j.state === "running").sort((a, b) => (a.j.eta?.finish_at ?? Infinity) - (b.j.eta?.finish_at ?? Infinity));
  const queued = jobs.filter(({ j }) => isQueued(j)).length;
  const stat = (v, l) => `<div class="hstat"><b>${v}</b><span>${l}</span></div>`;
  setHTML($("#home-compute"), `<div class="card-head"><h2>Compute</h2><button class="ghost" data-go="compute">Open →</button></div>
    <div class="hstats">${stat(gpus.length ? `${busy}/${gpus.length}` : "–", "GPUs busy")}${stat(avg != null ? `${avg.toFixed(0)}%` : "–", "avg util")}${stat(run.length, "running")}${stat(queued, "queued")}</div>
    <div class="mini-list">${run.slice(0, 5).map(({ t, j }) => `<div class="mini" data-go="compute" data-view="${esc(t)}">
        <div class="mini-top"><span class="name">${esc(j.name)} <span class="faint mono small">${esc(j.id)}</span></span>${j.eta ? `<b class="${j.eta.exceeds_limit ? "warn" : ""}"><span data-eta="${j.eta.finish_at}"></span></b>` : ""}</div>
        ${j.progress ? `<div class="track"><div class="fill green" style="width:${(j.progress.fraction * 100).toFixed(1)}%"></div></div>` : ""}
        <div class="l2">${esc(t)}${j.progress ? ` · ${pct(j.progress.fraction)}` : ""}</div></div>`).join("") || `<div class="empty">No jobs running.</div>`}</div>`);

  // Agents card
  const working = ags.filter((a) => a.state === "working");
  const waiting = ags.filter((a) => a.state === "waiting").length;
  const today = ags.filter((a) => now() - a.updated_at < 86400).length;
  setHTML($("#home-agents"), `<div class="card-head"><h2>Agents</h2><button class="ghost" data-go="agents">Open →</button></div>
    <div class="hstats">${stat(working.length, "working")}${stat(waiting, "waiting")}${stat(today, "sessions today")}${stat(new Set(ags.map((a) => a.project).filter(Boolean)).size, "projects")}</div>
    <div class="home-limits">${usageList().map((u) => `<div class="hl">${toolChip(u.tool, "sm")}<div class="hl-bars">${u.limits.map((l) => limitBar(l, true)).join("")}</div><span class="faint small">${fmtTokens(u.tokens_today)} today</span></div>`).join("")}</div>
    <div class="mini-list">${working.slice(0, 5).map((a) => `<div class="mini agent-mini" data-go="agents" data-akey="${esc(agentKey(a))}">
        ${toolChip(a.tool, "sm")}<div class="cell"><div class="mini-top"><span class="name">${esc(a.title)}</span><span class="faint small"><span data-since="${a.updated_at}"></span></span></div>
        <div class="l2">${esc([a.project, a.branch].filter(Boolean).join(" · "))}</div></div></div>`).join("") || `<div class="empty">No agents working.</div>`}</div>`);
}

async function agentAct(k, action) {
  const a = agentList().find((x) => agentKey(x) === k);
  if (!a) return;
  if (action === "open") { S.tool = "all"; S.agentOpen.add(k); return showPage("agents"); }
  try { toast(await invoke("agent_action", { tool: a.tool, id: a.id, action })); }
  catch (e) { toast(String(e), true); }
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
  document.querySelectorAll("[data-since]").forEach((el) => {
    const t = Number(el.dataset.since);
    el.textContent = t ? `${human(n - t)} ago` : "";
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
  const b = e.target.closest("[data-page]");
  if (!b) return;
  if (b.dataset.view) S.view = b.dataset.view;
  if (b.dataset.tool) S.tool = b.dataset.tool;
  showPage(b.dataset.page);
});

$("#agents").addEventListener("click", (e) => {
  const row = e.target.closest("[data-akey]");
  if (!row) return;
  const k = row.dataset.akey;
  const act = e.target.closest("[data-aact]")?.dataset.aact;
  if (act && act !== "toggle") return agentAct(k, act);
  if (row.classList.contains("agent-detail") && !act) return;
  S.agentOpen.has(k) ? S.agentOpen.delete(k) : S.agentOpen.add(k);
  render();
});
$("#agent-filter").addEventListener("click", (e) => {
  const b = e.target.closest("[data-af]");
  if (!b) return;
  S.agentFilter = b.dataset.af;
  document.querySelectorAll("#agent-filter button").forEach((x) => x.classList.toggle("on", x === b));
  render();
});
$("#agent-search").addEventListener("input", (e) => { S.agentQuery = e.target.value.trim(); render(); });

$("#view-home").addEventListener("click", (e) => {
  const dis = e.target.closest("[data-dismiss]");
  if (dis) { dismissNeeds([dis.dataset.dismiss]); return render(); }
  if (e.target.closest("[data-dismiss-all]")) { dismissNeeds(attention().map((i) => i.key)); return render(); }
  const act = e.target.closest("[data-aact]");
  const akey = e.target.closest("[data-akey]")?.dataset.akey;
  if (act && akey) return agentAct(akey, act.dataset.aact);
  const h = e.target.closest("[data-hact]");
  const tname = e.target.closest("[data-target]")?.dataset.target;
  if (h?.dataset.hact === "retry") return refreshAll(tname);
  if (h?.dataset.hact === "job" || (tname && !h)) { S.view = tname; return showPage("compute"); }
  const go = e.target.closest("[data-go]");
  if (go) {
    if (go.dataset.view) S.view = go.dataset.view;
    if (akey) { S.tool = "all"; S.agentOpen.add(akey); }
    return showPage(go.dataset.go);
  }
});
$("#home-refresh").addEventListener("click", () => refreshAll());

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
  if (e.key === "/" && !CFG.open && !/^(INPUT|SELECT|TEXTAREA)$/.test(document.activeElement?.tagName)) {
    e.preventDefault();
    (S.page === "agents" ? $("#agent-search") : $("#search")).focus();
  }
  if (/^[1-3]$/.test(e.key) && e.metaKey) {
    e.preventDefault();
    showPage(PAGES[Number(e.key) - 1]);
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
const AGENT_DEFAULTS = { enabled: true, window_hours: 24, interval: 5, claude_code: true, codex: true, antigravity: true, live_claude_limits: false };
const withDefaults = (cfg) => ({
  interval: cfg.interval ?? 300,
  tail_bytes: cfg.tail_bytes ?? 6144,
  agents: { ...AGENT_DEFAULTS, ...(cfg.agents ?? {}) },
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
  if (!(cfg.agents?.window_hours >= 1)) errs.general.window = "At least 1 hour";
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

// App preferences (menu-bar title, floating button) live on the Rust side and apply instantly.
let PREFS = null;
async function loadPrefs() {
  try { PREFS = await invoke("get_prefs"); }
  catch { PREFS = { tray_title: "count_eta", float_button: false, float_style: "kit", limit_alert: 80, events: { started: true, finished: true, failed: true, limits: true } }; }
  renderPrefs();
}
function renderPrefs() {
  if (!PREFS) return;
  document.querySelectorAll("#pref-tray [data-tray]").forEach((b) => b.classList.toggle("on", b.dataset.tray === PREFS.tray_title));
  $("#pref-float").checked = !!PREFS.float_button;
  document.querySelectorAll("#pref-events [data-ev]").forEach((el) => (el.checked = !!PREFS.events?.[el.dataset.ev]));
  if (document.activeElement !== $("#pref-limit")) $("#pref-limit").value = PREFS.limit_alert ?? 80;
  const style = PREFS.float_style || "kit";
  setHTML($("#pref-fox"), Object.entries(FOXES).map(([id, f]) =>
    `<button class="fox-card ${id === style ? "on" : ""}" data-fox="${id}" title="${esc(f.note)}">${foxSvg(id, "fox-prev")}<span>${esc(f.name)}</span></button>`).join(""));
  $("#pref-fox").classList.toggle("off", !PREFS.float_button);
}
async function savePrefs(change) {
  PREFS = { ...PREFS, ...change };
  renderPrefs();
  await invoke("set_prefs", { prefs: PREFS }).catch((e) => toast(String(e), true));
}
$("#pref-tray").addEventListener("click", (e) => {
  const b = e.target.closest("[data-tray]");
  if (b) savePrefs({ tray_title: b.dataset.tray });
});
$("#pref-float").addEventListener("change", (e) => savePrefs({ float_button: e.target.checked }));
$("#pref-events").addEventListener("change", (e) => {
  const k = e.target.dataset.ev;
  if (k) savePrefs({ events: { ...PREFS.events, [k]: e.target.checked } });
});
$("#pref-limit").addEventListener("change", (e) => {
  const v = Math.max(0, Math.min(100, Math.round(Number(e.target.value) || 0)));
  e.target.value = v;
  savePrefs({ limit_alert: v });
});
$("#pref-fox").addEventListener("click", (e) => {
  const b = e.target.closest("[data-fox]");
  // Picking a fox also turns the floating button on.
  if (b) savePrefs({ float_style: b.dataset.fox, float_button: true });
});

async function openSettings() {
  loadPrefs();
  CFG.open = true;
  for (const p of PAGES) $(`#view-${p === "compute" ? "dash" : p}`).hidden = true;
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
  $(`#view-${S.page === "compute" ? "dash" : S.page}`).hidden = false;
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
  document.querySelectorAll("#agent-cfg [data-ag]").forEach((el) => {
    const v = d.agents[el.dataset.ag];
    if (el.type === "checkbox") el.checked = !!v;
    else el.value = v;
    if (el.dataset.ag !== "enabled") el.disabled = !d.agents.enabled;
  });
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

$("#agent-cfg").addEventListener("input", (e) => {
  const f = e.target.dataset.ag;
  if (!f || !CFG.draft) return;
  CFG.draft.agents[f] = e.target.type === "checkbox" ? e.target.checked : Number(e.target.value);
  if (f === "enabled") renderSettings();
  else updateSettingsChrome();
});

onDismissChange(() => render());

// ---- boot -------------------------------------------------------------------

listen("snapshot", (e) => {
  S.snap = e.payload;
  S.error = null;
  sampleHistory();
  render();
});
listen("open-settings", () => openSettings());
listen("focus-target", (e) => {
  const p = String(e.payload);
  if (p.startsWith("agent:")) {
    S.tool = "all";
    S.agentOpen.add(p.slice(6));
    return showPage("agents");
  }
  S.view = (S.snap?.targets ?? []).some((t) => t.name === p) ? p : "all";
  showPage("compute");
});
listen("config-error", (e) => {
  S.error = e.payload;
  render();
});

invoke("snapshot")
  .then((snap) => { S.snap = snap; sampleHistory(); render(); })
  .catch((e) => { S.error = String(e); render(); });
