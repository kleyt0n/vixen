// Floating fox: drag to move, click to toggle the summary panel next to it.
// Shows a speech bubble with a little animation when a job starts, finishes or fails, a plan
// limit gets high, or an insight flags something new. Bubbles stay until closed with ✕; more messages queue behind.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const fb = document.getElementById("fb");
const stage = document.getElementById("stage");
const bubble = document.getElementById("bubble");

// Distinguish a click from a drag: moving more than a few pixels starts a window drag.
let press = null;
fb.addEventListener("mousedown", (e) => {
  if (e.button === 0) press = { x: e.screenX, y: e.screenY };
});
window.addEventListener("mousemove", (e) => {
  if (press && Math.hypot(e.screenX - press.x, e.screenY - press.y) > 3) {
    press = null;
    appWindow.startDragging().catch(() => {});
  }
});
window.addEventListener("mouseup", (e) => {
  if (!press) return;
  press = null;
  if (e.target.closest("#fb")) invoke("float_click");
});

function render(snap) {
  const ts = snap?.targets ?? [];
  const running = ts.flatMap((t) => t.jobs).filter((j) => j.state === "running" || j.state === "completing").length;
  const working = (snap?.agents ?? []).filter((a) => a.state === "working").length;
  const down = ts.filter((t) => !t.ok && t.error && !t.updated_at).map((t) => t.name);
  document.getElementById("alert").hidden = !down.length;
  fb.classList.toggle("busy", running + working > 0);
  fb.title = [`Vixen — ${running} jobs running, ${working} agents working`, down.length ? `unreachable: ${down.join(", ")}` : ""]
    .filter(Boolean).join("\n");
}

function setFox(style) {
  document.getElementById("fox").outerHTML = `<span id="fox">${foxSvg(style)}</span>`;
}

// ---- messages ---------------------------------------------------------------
// One at a time; the window grows to fit the bubble and shrinks back when it's closed.

const KIND = {
  start: { icon: "▶", anim: "perk" },
  done: { icon: "✓", anim: "hop" },
  failed: { icon: "✕", anim: "droop" },
  limit: { icon: "!", anim: "wiggle" },
  insight: { icon: "✦", anim: "perk" },
};
const ANIMS = Object.values(KIND).map((k) => k.anim);

// The bubble sits this far from the fox's edge of the window (see .bubble in float.html),
// plus room on the far side for its shadow.
const BUBBLE_ABOVE_FOX = 46;
const BUBBLE_MARGIN = 14;

const queue = [];
let showing = false;

async function showNext() {
  if (showing || !queue.length) return;
  showing = true;
  const m = queue.shift();
  bubble.className = `bubble ${m.kind}`;
  const kind = KIND[m.kind] ?? KIND.done;
  document.getElementById("b-ico").textContent = kind.icon;
  document.getElementById("b-title").textContent = m.title;
  document.getElementById("b-detail").textContent = m.detail ?? "";
  bubble.title = [m.title, m.detail].filter(Boolean).join("\n");
  const meter = document.getElementById("b-meter");
  meter.hidden = m.percent == null;
  if (m.percent != null) {
    meter.firstElementChild.style.width = `${Math.min(100, m.percent)}%`;
    meter.firstElementChild.className = m.percent >= 90 ? "red" : "";
  }
  updateMore();
  // Measure the bubble before growing the window, so the window fits it. Count the "+N more"
  // line even if it's hidden: another message may arrive while this one shows.
  const more = document.getElementById("b-more");
  const moreHidden = more.hidden;
  more.hidden = false;
  bubble.style.visibility = "hidden";
  bubble.hidden = false;
  const height = bubble.offsetHeight + BUBBLE_ABOVE_FOX + BUBBLE_MARGIN;
  bubble.hidden = true;
  bubble.style.visibility = "";
  more.hidden = moreHidden;
  const layout = await invoke("float_expand", { on: true, height }).catch(() => ({ right: false, down: false }));
  stage.classList.toggle("right", !!layout.right);
  stage.classList.toggle("down", !!layout.down);
  bubble.hidden = false;
  fb.classList.remove(...ANIMS);
  void fb.offsetWidth; // restart the animation
  fb.classList.add(kind.anim);
}

function updateMore() {
  const more = document.getElementById("b-more");
  more.hidden = !queue.length;
  more.textContent = `+${queue.length} more`;
}

let hiding = false;
async function hide() {
  if (bubble.hidden || hiding) return;
  hiding = true;
  bubble.classList.add("out");
  await new Promise((r) => setTimeout(r, 250));
  bubble.hidden = true;
  fb.classList.remove(...ANIMS);
  stage.classList.remove("right", "down");
  await invoke("float_expand", { on: false }).catch(() => {});
  hiding = false;
  showing = false;
  showNext();
}

// ✕ dismisses (and shows the next one); clicking the message opens the panel.
document.getElementById("b-close").addEventListener("click", (e) => {
  e.stopPropagation();
  hide();
});
bubble.addEventListener("click", () => invoke("float_click"));

function message(m) {
  queue.push(m);
  if (showing) updateMore();
  showNext();
}

listen("float-event", (e) => message(e.payload));
listen("prefs", (e) => setFox(e.payload.float_style));
invoke("get_prefs").then((p) => setFox(p.float_style)).catch(() => setFox("kit"));
listen("snapshot", (e) => render(e.payload));
invoke("snapshot").then(render).catch(() => {});
