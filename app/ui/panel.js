// Menu-bar panel: summary tiles, what needs you, a timeline of running work,
// plan limits and machines, in one narrow column.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = appWindow;
const { LogicalSize } = window.__TAURI__.dpi;

const WIDTH = 380;
const MAX_H = 640;
const MIN_H = 260;
const SIZE_KEY = "panel-size-v2";
const NEEDS_MAX = 4;

const $ = (sel) => document.querySelector(sel);
let snap = null;
let error = null;

// A size the user dragged to; while set, the panel stops auto-fitting its content.
let userSize = null;
try { userSize = JSON.parse(localStorage.getItem(SIZE_KEY)); } catch {}

const isQueued = (j) => j.state === "pending" || j.state === "held";
const isRunning = (j) => j.state === "running" || j.state === "completing";
const plural = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;
const LIMIT_SHORT = { "Session (5h)": "5h", Weekly: "week" };
const shortLimit = (label) => LIMIT_SHORT[label] ?? label.replace(/^Weekly · /, "wk ");

// Only touch the DOM when markup changed, so scroll position and hover survive polls.
const last = new Map();
function setHTML(el, html) {
  if (last.get(el) === html) return;
  last.set(el, html);
  el.innerHTML = html;
}

const render = whenVisible(renderNow);
function renderNow() {
  const ts = snap?.targets ?? [];
  const agents = snap?.agents ?? [];
  const n = now();
  const jobs = ts.flatMap((t) => t.jobs.map((j) => ({ t: t.name, j })));
  const running = jobs.filter(({ j }) => isRunning(j))
    .sort((a, b) => (a.j.eta?.finish_at ?? Infinity) - (b.j.eta?.finish_at ?? Infinity));
  const queued = jobs.filter(({ j }) => isQueued(j));
  const waiting = agents.filter((a) => a.state === "waiting");
  const working = agents.filter((a) => a.state === "working");
  const gpus = ts.flatMap((t) => t.gpus);
  const busy = gpus.filter((g) => (g.util ?? 0) >= 10).length;
  const avg = gpus.length ? Math.round(gpus.reduce((s, g) => s + (g.util ?? 0), 0) / gpus.length) : null;
  const needsItems = needsYou(snap);
  const next = running.find(({ j }) => j.eta);

  // Header: the one thing that matters most, in words.
  const asks = needsItems.length;
  const agentAsks = needsItems.filter((i) => i.kind === "agent").length;
  $("#p-title").textContent = error ? "Config needs attention"
    : agentAsks && agentAsks === asks ? `${plural(agentAsks, "agent")} waiting for you`
    : asks ? `${plural(asks, "thing")} need${asks === 1 ? "s" : ""} you`
    : "All clear";
  $("#p-sub").textContent = [
    running.length ? `${plural(running.length, "job")} running` : "no jobs running",
    next ? `next done in ${human(next.j.eta.finish_at - n)}` : null,
  ].filter(Boolean).join(" · ");

  setHTML($("#p-tiles"), [
    `<div class="tile ${waiting.length ? "hot" : ""}" data-a="open" data-target="agents" title="${working.length} working · ${waiting.length} waiting"><b>${waiting.length}</b><span>agents waiting</span></div>`,
    `<div class="tile" data-a="open" data-target="" title="${queued.length} queued"><b>${running.length}</b><span>jobs running</span></div>`,
    `<div class="tile" data-a="open" data-target="" title="${avg != null ? `average ${avg}%` : ""}"><b>${gpus.length ? `${busy}/${gpus.length}` : "–"}</b><span>GPUs busy</span></div>`,
  ].join(""));
  setHTML($("#p-banner"), error ? `<div class="banner"><span class="msg">${esc(error)}</span></div>` : "");

  // Insights: the headline, then what needs a look (or the first notes when all is well).
  const ins = snap?.insights;
  const shown = ins?.headline || ins?.error;
  $("#sec-insight").hidden = !shown;
  if (shown) {
    $("#p-ins-when").innerHTML = ins.busy ? "updating…" : ins.generated_at ? `<span data-since="${ins.generated_at}"></span>` : "";
    const flagged = ins.notes.filter((x) => x.tone !== "ok");
    const notes = (flagged.length ? flagged : ins.notes).slice(0, 3);
    const nav = (x) => x.kind === "agent" ? `data-agent="${esc(x.key)}"` : x.kind === "job" ? `data-target="${esc(x.key.split("::")[0])}"` : "";
    setHTML($("#p-insight"), (ins.headline ? `<p class="ins-headline">${esc(ins.headline)}</p>` : `<p class="ins-headline faint">${esc(ins.error)}</p>`)
      + notes.map((x) => `<div class="ins-note ${x.tone}" ${nav(x)}><span class="ins-dot"></span>${x.label ? `<b>${esc(x.label)}</b>` : ""}<span>${esc(x.text)}</span></div>`).join(""));
  }

  // Needs you: waiting agents, machines down, jobs about to overrun or failed. Each can be dismissed.
  const x = (it) => `<button class="dismiss" data-dismiss="${esc(it.key)}" title="Dismiss">✕</button>`;
  const needs = needsItems.map((it) => {
    if (it.kind === "agent") {
      const a = it.a;
      return `<div class="need" data-agent="${esc(`${a.tool}::${a.id}`)}" title="${esc(a.title)}">${toolChip(a.tool, "sm")}
        <span class="n">${esc(a.title)}</span><span class="m">${esc([a.project, a.branch].filter(Boolean).join(" · "))} · <span data-since="${a.updated_at}"></span></span>
        <span class="acts">${a.resume ? `<button data-resume="${esc(`${a.tool}::${a.id}`)}">Resume</button>` : ""}${x(it)}</span></div>`;
    }
    if (it.kind === "down") {
      return `<div class="need" data-target="${esc(it.t.name)}"><span class="ico down">!</span>
        <span class="n">${esc(it.t.name)} unreachable</span><span class="m" title="${esc(it.t.error)}">${esc(it.t.error)}</span><span class="acts">${x(it)}</span></div>`;
    }
    const what = it.kind === "limit" ? `will pass its limit · ETA ${clock(it.j.eta.finish_at)}` : "failed";
    return `<div class="need" data-target="${esc(it.t.name)}"><span class="ico ${it.kind === "failed" ? "down" : "warn"}">${it.kind === "failed" ? "✕" : "⏱"}</span>
      <span class="n">${esc(it.j.name)} ${it.kind === "failed" ? "failed" : "will pass its limit"}</span><span class="m">${esc(it.t.name)} · ${esc(what)}</span><span class="acts">${x(it)}</span></div>`;
  });
  $("#sec-needs").hidden = !needs.length;
  $("#p-needs-clear").hidden = needs.length < 2;
  setHTML($("#p-needs"), needs.slice(0, NEEDS_MAX).join("") + (needs.length > NEEDS_MAX ? `<div class="p-more">+${needs.length - NEEDS_MAX} more</div>` : ""));

  // Running: a timeline — agents working now, then jobs by finish time.
  $("#p-next").textContent = next ? `next ${human(next.j.eta.finish_at - n)}` : "";
  const nowRows = working.map((a) => `<div class="ti" data-agent="${esc(`${a.tool}::${a.id}`)}" title="${esc(a.title)}">
      <div class="time">now</div><span class="node now"></span>
      <div><div class="row1"><span class="n"><span class="who">${toolChip(a.tool, "sm")}</span>${esc(a.title)}</span></div>
      <div class="m">${esc([a.project, a.branch].filter(Boolean).join(" · "))} · working</div></div></div>`);
  const jobRows = running.map(({ t, j }) => {
    const p = j.progress;
    const warn = j.eta?.exceeds_limit;
    const when = j.eta ? clock(j.eta.finish_at) : "";
    const time = when ? when.replace(/^tomorrow /, "").replace(/^\w{3} \d+ \w{3} /, "") : "–";
    const day = !when ? "" : when.startsWith("tomorrow") ? "tomorrow" : /^\w{3} /.test(when) && !/^\d/.test(when) ? when.split(" ").slice(0, 3).join(" ") : "today";
    return `<div class="ti" data-target="${esc(t)}" title="${esc(`${j.name} · ${j.id}`)}">
      <div class="time">${esc(time)}<small>${esc(day)}</small></div><span class="node ${warn ? "warn" : ""}"></span>
      <div><div class="row1"><span class="n">${esc(j.name)}<i>${esc(j.id)}</i></span>
        <span class="left ${warn ? "warn" : ""}">${j.eta ? `<span data-eta="${j.eta.finish_at}"></span>` : j.elapsed_s != null ? human(j.elapsed_s) : ""}</span></div>
      ${p ? `<div class="track"><div class="fill ${warn ? "red" : "green"}" style="width:${(p.fraction * 100).toFixed(1)}%"></div></div>` : ""}
      <div class="m">${esc([t, p ? pct(p.fraction) : null, p?.label, p?.it_per_s ? `${p.it_per_s.toFixed(2)} it/s` : null].filter(Boolean).join(" · "))}</div></div></div>`;
  });
  setHTML($("#p-timeline"), [...nowRows, ...jobRows].join("") || `<div class="p-empty">Nothing running.</div>`);
  const qByTarget = {};
  queued.forEach(({ t }) => (qByTarget[t] = (qByTarget[t] ?? 0) + 1));
  const qEntries = Object.entries(qByTarget);
  setHTML($("#p-queued"), qEntries.length
    ? `<span data-target="${esc(qEntries[0][0])}">+ ${queued.length} queued · ${qEntries.map(([t, c]) => `${esc(t)} ${c}`).join(", ")}</span>` : "");

  // Limits: one line per tool.
  const usage = snap?.usage ?? [];
  $("#sec-limits").hidden = !usage.length;
  setHTML($("#p-limits"), usage.map((u) => `<div class="lim" data-a="open" data-target="agents">${toolChip(u.tool, "sm")}
    <div class="pills">${u.limits.map((l) => {
      const stale = l.resets_at && l.resets_at <= n;
      const lvl = stale ? "stale" : l.percent >= 90 ? "red" : l.percent >= 70 ? "amber" : "";
      const tip = `${l.label}: ${stale ? "reset since" : `${Math.round(l.percent)}%${l.resets_at ? ` · resets in ${human(l.resets_at - n)}` : ""}`}`;
      return `<span class="pill2 ${lvl}" title="${esc(tip)}">${esc(shortLimit(l.label))} <b>${stale ? "–" : `${Math.round(l.percent)}%`}</b></span>`;
    }).join("") || `<span class="faint small">no limits recorded</span>`}</div>
    <span class="tok">${fmtTokens(u.tokens_today)} today</span></div>`).join(""));

  // Machines: compact chips.
  setHTML($("#p-machines"), ts.map((t) => {
    const r = t.jobs.filter(isRunning).length;
    const top = t.gpus.length ? Math.max(...t.gpus.map((g) => g.util ?? 0)) : null;
    const dot = t.ok ? "ok" : t.error && t.updated_at ? "stale" : t.error ? "down" : "wait";
    const info = [r ? `${r}▸` : null, top != null ? `${Math.round(top)}%` : null].filter(Boolean).join(" ");
    return `<span class="chip" data-target="${esc(t.name)}" title="${esc(t.error ?? t.kind)}"><span class="dot ${dot}"></span>${esc(t.name)}${info ? `<span>${info}</span>` : ""}</span>`;
  }).join("") || `<div class="p-empty">No targets. Open Settings to add one.</div>`);

  $("#p-updated").dataset.ago = snap?.taken_at ?? 0;
  tick();
  fit();
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

// Fit the window to its content (up to MAX_H); a size the user dragged to wins
// until they double-click the header.
let lastSet = null;
function fit() {
  requestAnimationFrame(() => {
    let w = WIDTH, h;
    if (userSize) {
      ({ w, h } = userSize);
    } else {
      const panel = $(".panel");
      const scroll = $("#p-scroll");
      const chrome = panel.offsetHeight - scroll.offsetHeight;
      h = Math.round(Math.min(MAX_H, Math.max(MIN_H, chrome + scroll.scrollHeight + 2)));
    }
    if (lastSet && Math.abs(lastSet.w - w) < 3 && Math.abs(lastSet.h - h) < 3) return;
    lastSet = { w, h };
    win.setSize(new LogicalSize(w, h)).catch(() => {});
  });
}

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
  const dis = e.target.closest("[data-dismiss]");
  if (dis) { e.stopPropagation(); dismissNeeds([dis.dataset.dismiss]); return renderNow(); }
  if (e.target.closest("#p-needs-clear")) { dismissNeeds(needsYou(snap).map((i) => i.key)); return renderNow(); }
  const resume = e.target.closest("[data-resume]")?.dataset.resume;
  if (resume) {
    e.stopPropagation();
    const [tool, ...rest] = resume.split("::");
    return invoke("agent_action", { tool, id: rest.join("::"), action: "resume" }).catch(() => {});
  }
  const agent = e.target.closest("[data-agent]")?.dataset.agent;
  if (agent) return invoke("panel_action", { action: "open", target: `agent:${agent}` });
  const a = e.target.closest("[data-a]");
  const t = e.target.closest("[data-target]")?.dataset.target;
  if (a?.dataset.a === "open" && t === "agents") return invoke("panel_action", { action: "open", target: "agent:" });
  if (a) return invoke("panel_action", { action: a.dataset.a, target: t || null });
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

onDismissChange(() => renderNow());
listen("snapshot", (e) => { snap = e.payload; error = null; render(); });
listen("config-error", (e) => { error = e.payload; render(); });
listen("panel-shown", () => renderNow());
invoke("snapshot").then((s) => { snap = s; render(); }).catch((e) => { error = String(e); render(); });
