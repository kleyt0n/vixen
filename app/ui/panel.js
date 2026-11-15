// Menu-bar panel: running jobs, queued jobs by machine, and machine/GPU health.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = appWindow;
const { LogicalSize } = window.__TAURI__.dpi;

const WIDTH = 600;
const MAX_H = 600;
const MIN_H = 300;
const SIZE_KEY = "panel-size";

// A size the user dragged to; while set, the panel stops auto-fitting its content.
let userSize = null;
try { userSize = JSON.parse(localStorage.getItem(SIZE_KEY)); } catch {}
const GROUP_ROWS = 40; // rows rendered per expanded queue group

const $ = (sel) => document.querySelector(sel);
let snap = null;
let error = null;
const expanded = new Set(); // queue groups the user opened

const isQueued = (j) => j.state === "pending" || j.state === "held";
const isRunning = (j) => j.state === "running" || j.state === "completing";

// Only touch the DOM when markup changed, so scroll positions and hover survive polls.
const last = new Map();
function setHTML(el, html) {
  if (last.get(el) === html) return;
  last.set(el, html);
  el.innerHTML = html;
}

const render = whenVisible(renderNow);
function renderNow() {
  const ts = snap?.targets ?? [];
  const jobs = ts.flatMap((t) => t.jobs.map((j) => ({ t: t.name, j })));
  const running = jobs.filter(({ j }) => isRunning(j));
  const queued = jobs.filter(({ j }) => isQueued(j));
  const gpus = ts.flatMap((t) => t.gpus);
  const busy = gpus.filter((g) => (g.util ?? 0) >= 10).length;
  const avg = gpus.length ? gpus.reduce((a, g) => a + (g.util ?? 0), 0) / gpus.length : null;
  const next = running.filter(({ j }) => j.eta).sort((a, b) => a.j.eta.finish_at - b.j.eta.finish_at)[0];
  const down = ts.filter((t) => !t.ok && t.error).length;

  const sub = $("#p-sub");
  sub.textContent = error ? "config error" : down ? `${down} unreachable` : "";
  sub.className = error || down ? "bad" : "";
  const stat = (v, l) => `<span class="p-stat"><b>${v}</b><span>${l}</span></span>`;
  const agents = (snap?.agents ?? []).filter((a) => a.state !== "idle");
  const working = agents.filter((a) => a.state === "working").length;
  setHTML($("#p-stats"), [
    agents.length ? stat(working, `agent${working === 1 ? "" : "s"}${agents.length > working ? ` · ${agents.length - working} waiting` : ""}`) : "",
    stat(running.length, "running"),
    stat(queued.length, "queued"),
    gpus.length ? stat(`${busy}/${gpus.length}`, `GPUs${avg != null ? ` · ${avg.toFixed(0)}%` : ""}`) : "",
    next ? stat(`<span data-eta="${next.j.eta.finish_at}"></span>`, "next") : "",
  ].join(""));
  setHTML($("#p-banner"), error ? `<div class="banner"><span class="msg">${esc(error)}</span></div>` : "");

  // Running: soonest finish first; jobs without an ETA after.
  running.sort((a, b) => (a.j.eta?.finish_at ?? Infinity) - (b.j.eta?.finish_at ?? Infinity));
  $("#n-running").textContent = running.length || "";
  setHTML($("#p-jobs"), running.length
    ? running.map(({ t, j }) => jobCard(t, j)).join("")
    : `<div class="p-empty">${ts.length ? "No running jobs." : "Waiting for the first poll…"}</div>`);

  $("#n-agents").textContent = agents.length || "";
  const usage = snap?.usage ?? [];
  $("#p-agents-sec").hidden = !agents.length && !usage.length;
  setHTML($("#p-usage"), usage.map((u) => `<div class="pu">${toolChip(u.tool, "sm")}<div class="pu-bars">${u.limits.map((l) => limitBar(l, true)).join("")}</div><span>${fmtTokens(u.tokens_today)}<br><small>today</small></span></div>`).join(""));
  setHTML($("#p-agents"), agents.slice(0, 8).map(agentRow).join(""));

  $("#n-queued").textContent = queued.length || "";
  setHTML($("#p-queue"), ts.filter((t) => t.jobs.some(isQueued)).map(queueGroup).join("") || `<div class="p-empty">Nothing queued.</div>`);

  $("#n-machines").textContent = ts.length || "";
  setHTML($("#p-machines"), ts.map(machine).join("") || `<div class="p-empty">No targets. Open Settings to add one.</div>`);
  $("#p-updated").dataset.ago = snap?.taken_at ?? 0;
  tick();
  fit();
}

function jobCard(t, j) {
  const p = j.progress;
  const warn = j.eta?.exceeds_limit;
  const left = j.time_limit_s != null && j.elapsed_s != null ? Math.max(0, j.time_limit_s - j.elapsed_s) : null;
  const eta = j.eta
    ? `<span class="pj-eta ${warn ? "warn" : ""}" title="Estimated time remaining">${j.eta.method === "elapsed" ? "~" : ""}<span data-eta="${j.eta.finish_at}"></span></span>`
    : `<span class="pj-eta faint">${j.elapsed_s != null ? human(j.elapsed_s) : ""}</span>`;
  const label = [t, p?.label, p?.it_per_s ? `${p.it_per_s.toFixed(2)} it/s` : null].filter(Boolean).join(" · ");
  const right = j.eta
    ? warn ? `<span class="warn">⚠ past limit · ${esc(clock(j.eta.finish_at))}</span>` : `<span>done ${esc(clock(j.eta.finish_at))}</span>`
    : `<span>${left != null ? `${human(left)} left` : "no limit"}</span>`;
  return `<div class="pj" data-target="${esc(t)}" title="${esc(`${j.name} · ${j.id}`)}">
    <div class="pj-top"><span><span class="pj-name">${esc(j.name)}</span><span class="pj-id">${esc(j.id)}</span></span>${eta}</div>
    ${p ? `<div class="pj-bar"><div class="track"><div class="fill ${warn ? "red" : "green"}" style="width:${(p.fraction * 100).toFixed(2)}%"></div></div><span class="pct">${pct(p.fraction)}</span></div>` : ""}
    <div class="pj-meta"><span>${esc(p ? label : `${t}${j.last_line ? ` · ${j.last_line}` : ""}`)}</span>${right}</div>
  </div>`;
}

function agentRow(a) {
  const place = [a.project, a.branch].filter(Boolean).join(" · ");
  return `<div class="pa" data-agent="${esc(`${a.tool}::${a.id}`)}" title="${esc(a.title)}">
    ${toolChip(a.tool, "sm")}
    <span class="name">${esc(a.title)}</span><span class="st ${a.state}">${a.state}</span>
    <span class="meta">${esc(place)}${place ? " · " : ""}<span data-since="${a.updated_at}"></span></span></div>`;
}

function queueGroup(t) {
  const q = t.jobs.filter(isQueued);
  const pending = q.filter((j) => j.state === "pending").length;
  const held = q.length - pending;
  const tasks = q.reduce((n, j) => n + (j.array_tasks ?? 1), 0);
  const open = expanded.has(t.name);
  const pills = [pending && `<span class="pill pending">${pending} pending</span>`, held && `<span class="pill held">${held} held</span>`].filter(Boolean).join("");
  let html = `<div class="qg ${open ? "open" : ""}">
    <button class="qg-head" data-group="${esc(t.name)}" title="${tasks !== q.length ? `${tasks} array tasks` : ""}">
      <span class="chev">▶</span><span class="qg-name">${esc(t.name)}</span><span class="qg-counts">${pills}</span></button>`;
  if (open) {
    // Pending (soonest start first) before held.
    q.sort((a, b) => (a.state === b.state ? (a.est_start_at ?? Infinity) - (b.est_start_at ?? Infinity) : a.state === "pending" ? -1 : 1));
    html += `<div class="qg-rows">${q.slice(0, GROUP_ROWS).map((j) => queueRow(t.name, j)).join("")}`;
    if (q.length > GROUP_ROWS) html += `<button class="qg-more" data-target="${esc(t.name)}">+${q.length - GROUP_ROWS} more in Vixen →</button>`;
    html += `</div>`;
  }
  return html + `</div>`;
}

function queueRow(t, j) {
  const when = j.est_start_at ? `starts ~${clock(j.est_start_at)}` : j.detail ?? (j.submitted_at ? `queued ${human(now() - j.submitted_at)}` : "");
  return `<div class="pq" data-target="${esc(t)}" title="${esc(`${j.name} · ${j.id}${j.detail ? ` · ${j.detail}` : ""}`)}">
    <span class="pq-name"><span class="dot ${j.state}"></span>${esc(j.name)}<span class="pq-id">${esc(j.id)}${j.array_tasks ? ` ×${j.array_tasks}` : ""}</span></span>
    <span class="pq-when">${esc(when)}</span></div>`;
}

function machine(t) {
  const dot = t.ok ? "ok" : t.error && t.updated_at ? "stale" : t.error ? "down" : "wait";
  const r = t.jobs.filter(isRunning).length;
  const q = t.jobs.filter(isQueued).length;
  const jobs = [r && `${r} running`, q && `${q} queued`].filter(Boolean).join(" · ");
  let detail = "";
  if (t.error && !t.updated_at) {
    detail = `<div class="pm-detail err" title="${esc(t.error)}">${esc(t.error)}</div>`;
  } else if (t.gpus.length) {
    detail = `<div class="pm-detail">${t.gpus.map((g) => {
      const u = g.util ?? 0;
      return `<span class="pg" title="${esc(`${g.name}${g.job_id ? ` · job ${g.job_id}` : ""}`)}">${vendorIcon(g.name, "sm")}<span class="track"><span class="fill ${level(u)}" style="display:block;width:${u}%"></span></span><span>${g.util == null ? "–" : `${u.toFixed(0)}%`}</span></span>`;
    }).join("")}</div>`;
  }
  return `<div class="pm" data-target="${esc(t.name)}" title="${esc(t.kind)}">
    <span class="dot ${dot}"></span><span class="pm-name">${esc(t.name)}</span><span class="pm-jobs">${jobs || (t.ok ? "idle" : "")}</span>
    ${detail}</div>`;
}

function tick() {
  if (document.hidden) return;
  const n = now();
  document.querySelectorAll("[data-eta]").forEach((el) => (el.textContent = human(Number(el.dataset.eta) - n)));
  document.querySelectorAll("[data-since]").forEach((el) => (el.textContent = `${human(n - Number(el.dataset.since))} ago`));
  document.querySelectorAll("[data-ago]").forEach((el) => {
    const t = Number(el.dataset.ago);
    el.textContent = t ? `updated ${human(n - t)} ago` : "";
  });
}
setInterval(tick, 1000);

// Size the window to the taller column, within limits; beyond that each column scrolls.
// A user-chosen size wins until they double-click the header.
let lastSet = null;
function fit() {
  requestAnimationFrame(() => {
    let w = WIDTH, h;
    if (userSize) {
      ({ w, h } = userSize);
    } else {
      const body = $(".p-body");
      const chrome = $(".panel").offsetHeight - body.offsetHeight;
      const content = Math.max($("#p-left").scrollHeight, $("#p-right").scrollHeight);
      h = Math.round(Math.min(MAX_H, Math.max(MIN_H, chrome + content + 2)));
    }
    if (lastSet && Math.abs(lastSet.w - w) < 3 && Math.abs(lastSet.h - h) < 3) return;
    lastSet = { w, h };
    win.setSize(new LogicalSize(w, h)).catch(() => {});
  });
}

// Remember sizes the user drags to (ignore our own setSize calls).
let resizeTimer;
win.onResized(() => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    const w = window.innerWidth, h = window.innerHeight;
    if (lastSet && Math.abs(lastSet.w - w) < 3 && Math.abs(lastSet.h - h) < 3) return;
    userSize = lastSet = { w, h };
    try { localStorage.setItem(SIZE_KEY, JSON.stringify(userSize)); } catch {}
  }, 250);
});

draggable($(".p-head"), () => {
  userSize = null;
  lastSet = null;
  try { localStorage.removeItem(SIZE_KEY); } catch {}
  fit();
});

document.addEventListener("click", (e) => {
  const a = e.target.closest("[data-a]")?.dataset.a;
  if (a) return invoke("panel_action", { action: a });
  const g = e.target.closest("[data-group]")?.dataset.group;
  if (g) {
    expanded.has(g) ? expanded.delete(g) : expanded.add(g);
    return render();
  }
  const ag = e.target.closest("[data-agent]")?.dataset.agent;
  if (ag) return invoke("panel_action", { action: "open", target: `agent:${ag}` });
  const t = e.target.closest("[data-target]")?.dataset.target;
  if (t) invoke("panel_action", { action: "open", target: t });
});
$("#p-refresh").addEventListener("click", (e) => {
  e.stopPropagation();
  const b = e.currentTarget;
  b.classList.add("spin");
  setTimeout(() => b.classList.remove("spin"), 900);
  invoke("refresh", { target: null });
});
document.addEventListener("keydown", (e) => e.key === "Escape" && win.hide());

listen("snapshot", (e) => { snap = e.payload; error = null; render(); });
listen("config-error", (e) => { error = e.payload; render(); });
listen("panel-shown", () => { renderNow(); });
invoke("snapshot").then((s) => { snap = s; render(); }).catch((e) => { error = String(e); render(); });
