// Shared by the main window (app.js) and the menu-bar panel (panel.js).
"use strict";

// ---- formatting -------------------------------------------------------------

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const now = () => Math.floor(Date.now() / 1000);

function human(s) {
  if (s == null) return "";
  s = Math.max(0, Math.round(s));
  const d = Math.floor(s / 86400), h = Math.floor(s / 3600) % 24, m = Math.floor(s / 60) % 60;
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m) return `${m}m`;
  return `${s}s`;
}

function clock(ts) {
  if (!ts) return "";
  const d = new Date(ts * 1000), today = new Date();
  const hm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  if (d.toDateString() === today.toDateString()) return hm;
  const tomorrow = new Date(today); tomorrow.setDate(today.getDate() + 1);
  if (d.toDateString() === tomorrow.toDateString()) return `tomorrow ${hm}`;
  return `${d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })} ${hm}`;
}

const gb = (mb) => (mb == null ? "–" : mb >= 10240 ? `${(mb / 1024).toFixed(0)}G` : `${(mb / 1024).toFixed(1)}G`);
const pct = (f) => `${(f * 100).toFixed(f < 0.1 ? 1 : 0)}%`;
const level = (v) => (v >= 90 ? "red" : v >= 70 ? "amber" : "green");
const METHOD = {
  observed: "measured from progress across polls",
  throughput: "remaining steps ÷ logged it/s",
  tool: "remaining time printed by the progress bar",
  elapsed: "elapsed ÷ progress (rough: assumes the job started at 0%)",
};

// ---- brand ------------------------------------------------------------------

// Fox silhouette (app/assets/fox.svg) in currentColor: white in dark mode, near-black in light.
const LOGO_DEFS = `<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>
  <mask id="fox-cut" maskUnits="userSpaceOnUse" x="0" y="0" width="64" height="64"><path fill="#fff" d="M40 53C47 53 55 49 58 41 60.5 34.5 58.5 27 54 24.5 52.5 31 48.5 36 42 39.5 37.5 42 36 48 40 53Z"/><ellipse fill="#fff" cx="40.5" cy="45.5" rx="5" ry="6.5"/><ellipse fill="#fff" cx="42" cy="41" rx="5" ry="4.5"/><ellipse cx="30" cy="47.5" rx="13" ry="11.5" fill="#fff" stroke="#000" stroke-width="2.2"/><ellipse fill="#fff" cx="24.5" cy="58" rx="4.2" ry="2.8"/><ellipse fill="#fff" cx="35.5" cy="58" rx="4.2" ry="2.8"/><path fill="#fff" d="M13.5 22 15.5 4.5C19.5 6.5 24 10.5 27 15ZM46.5 22 44.5 4.5C40.5 6.5 36 10.5 33 15Z"/><path d="M30 12.5C40.5 12.5 47.5 19 47.5 27 47.5 29.4 47 31.4 46 33.2L50.6 36.2 44.4 37.6C40.8 40.4 35.8 42 30 42S19.2 40.4 15.6 37.6L9.4 36.2 14 33.2C13 31.4 12.5 29.4 12.5 27 12.5 19 19.5 12.5 30 12.5Z" fill="#fff" stroke="#000" stroke-width="2.2"/></mask>
  <symbol id="logo" viewBox="8 2.5 54 60"><rect width="64" height="64" mask="url(#fox-cut)"/></symbol>
</defs></svg>`;
document.body.insertAdjacentHTML("afterbegin", LOGO_DEFS);

// ---- GPU vendors ------------------------------------------------------------

// Explicit vendor names win; bare NVIDIA model numbers (A100, H100, GB10…) are the fallback.
const VENDORS = [
  ["nvidia", "NVIDIA", /nvidia|geforce|quadro|tesla|\brtx|\bgtx|dgx/i],
  ["apple", "Apple", /apple|\bm\d( pro| max| ultra)?\b/i],
  ["amd", "AMD", /\bamd\b|radeon|instinct|\bmi\d{2,3}/i],
  ["intel", "Intel", /intel|\barc\b|gaudi/i],
  ["nvidia", "NVIDIA", /\b[ahlbv]\d{2,3}\b|\bgb\d{2,3}|gh200/i],
];
// Brand marks from Simple Icons (CC0, simple-icons@13.21.0); trademarks of their owners.
const mark = (d) => `<svg viewBox="0 0 24 24"><path fill="#fff" d="${d}"/></svg>`;
const VENDOR_MARK = {
  nvidia: mark("M8.948 8.798v-1.43a6.7 6.7 0 0 1 .424-.018c3.922-.124 6.493 3.374 6.493 3.374s-2.774 3.851-5.75 3.851c-.398 0-.787-.062-1.158-.185v-4.346c1.528.185 1.837.857 2.747 2.385l2.04-1.714s-1.492-1.952-4-1.952a6.016 6.016 0 0 0-.796.035m0-4.735v2.138l.424-.027c5.45-.185 9.01 4.47 9.01 4.47s-4.08 4.964-8.33 4.964c-.37 0-.733-.035-1.095-.097v1.325c.3.035.61.062.91.062 3.957 0 6.82-2.023 9.593-4.408.459.371 2.34 1.263 2.73 1.652-2.633 2.208-8.772 3.984-12.253 3.984-.335 0-.653-.018-.971-.053v1.864H24V4.063zm0 10.326v1.131c-3.657-.654-4.673-4.46-4.673-4.46s1.758-1.944 4.673-2.262v1.237H8.94c-1.528-.186-2.73 1.245-2.73 1.245s.68 2.412 2.739 3.11M2.456 10.9s2.164-3.197 6.5-3.533V6.201C4.153 6.59 0 10.653 0 10.653s2.35 6.802 8.948 7.42v-1.237c-4.84-.6-6.492-5.936-6.492-5.936z"),
  apple: mark("M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701"),
  amd: mark("M18.324 9.137l1.559 1.56h2.556v2.557L24 14.814V9.137zM2 9.52l-2 4.96h1.309l.37-.982H3.9l.408.982h1.338L3.432 9.52zm4.209 0v4.955h1.238v-3.092l1.338 1.562h.188l1.338-1.556v3.091h1.238V9.52H10.47l-1.592 1.845L7.287 9.52zm6.283 0v4.96h2.057c1.979 0 2.88-1.046 2.88-2.472 0-1.36-.937-2.488-2.747-2.488zm1.237.91h.792c1.17 0 1.63.711 1.63 1.57 0 .728-.372 1.572-1.616 1.572h-.806zm-10.985.273l.791 1.932H2.008zm17.137.307l-1.604 1.603v2.25h2.246l1.604-1.607h-2.246z"),
  intel: mark("M20.42 7.345v9.18h1.651v-9.18zM0 7.475v1.737h1.737V7.474zm9.78.352v6.053c0 .513.044.945.13 1.292.087.34.235.618.44.828.203.21.475.359.803.451.334.093.754.136 1.255.136h.216v-1.533c-.24 0-.445-.012-.593-.037a.672.672 0 0 1-.39-.173.693.693 0 0 1-.173-.377 4.002 4.002 0 0 1-.037-.606v-2.182h1.193v-1.416h-1.193V7.827zm-3.505 2.312c-.396 0-.76.08-1.082.241-.327.161-.6.384-.822.668l-.087.117v-.902H2.658v6.256h1.639v-3.214c.018-.588.16-1.02.433-1.299.29-.297.642-.445 1.044-.445.476 0 .841.149 1.082.433.235.284.359.686.359 1.2v3.324h1.663V12.97c.006-.89-.229-1.595-.686-2.09-.458-.495-1.1-.742-1.917-.742zm10.065.006a3.252 3.252 0 0 0-2.306.946c-.29.29-.525.637-.692 1.033a3.145 3.145 0 0 0-.254 1.273c0 .452.08.878.241 1.274.161.395.39.742.674 1.032.284.29.637.526 1.045.693.408.173.86.26 1.342.26 1.397 0 2.262-.637 2.782-1.23l-1.187-.904c-.248.297-.841.699-1.583.699-.464 0-.847-.105-1.138-.321a1.588 1.588 0 0 1-.593-.872l-.019-.056h4.915v-.587c0-.451-.08-.872-.235-1.267a3.393 3.393 0 0 0-.661-1.033 3.013 3.013 0 0 0-1.02-.692 3.345 3.345 0 0 0-1.311-.248zm-16.297.118v6.256h1.651v-6.256zm16.278 1.286c1.132 0 1.664.797 1.664 1.255l-3.32.006c0-.458.525-1.255 1.656-1.261zm7.073 3.814a.606.606 0 0 0-.606.606.606.606 0 0 0 .606.606.606.606 0 0 0 .606-.606.606.606 0 0 0-.606-.606zm-.008.105a.5.5 0 0 1 .002 0 .5.5 0 0 1 .5.501.5.5 0 0 1-.5.5.5.5 0 0 1-.5-.5.5.5 0 0 1 .498-.5zm-.233.155v.699h.13v-.285h.093l.173.285h.136l-.18-.297a.191.191 0 0 0 .118-.056c.03-.03.05-.074.05-.136 0-.068-.02-.117-.063-.154-.037-.038-.105-.056-.185-.056zm.13.099h.154c.019 0 .037.006.056.012a.064.064 0 0 1 .037.031c.013.013.012.031.012.056a.124.124 0 0 1-.012.055.164.164 0 0 1-.037.031c-.019.006-.037.013-.056.013h-.154Z"),
  gpu: `<svg viewBox="0 0 24 24"><path fill="#fff" fill-rule="evenodd" d="M6 6h12v12H6Zm2 2v8h8V8ZM9 3h1.6v2.4H9Zm4.4 0H15v2.4h-1.6ZM9 18.6h1.6V21H9Zm4.4 0H15V21h-1.6ZM3 9h2.4v1.6H3Zm0 4.4h2.4V15H3Zm15.6-4.4H21v1.6h-2.4Zm0 4.4H21V15h-2.4Z"/></svg>`,
};

function vendorOf(name) {
  for (const [id, label, re] of VENDORS) if (re.test(name ?? "")) return { id, label };
  return { id: "gpu", label: "GPU" };
}

function vendorIcon(name, size = "") {
  const v = vendorOf(name);
  return `<span class="vendor v-${v.id} ${size}" title="${v.label}">${VENDOR_MARK[v.id]}</span>`;
}

// ---- code agents --------------------------------------------------------------

// Brand marks from Simple Icons (CC0, simple-icons@15.6.0); trademarks of their owners.
// Antigravity has no Simple Icons mark; it shows the Google Gemini mark it runs on.
const TOOLS = {
  claude_code: { label: "Claude Code", mark: "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" },
  codex: { label: "Codex", mark: "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z" },
  antigravity: { label: "Antigravity", mark: "M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81" },
};

function toolChip(id, size = "") {
  const t = TOOLS[id];
  const mark = t ? `<svg viewBox="0 0 24 24"><path fill="#fff" d="${t.mark}"/></svg>` : "?";
  return `<span class="tool t-${id} ${size}" title="${t?.label ?? id}">${mark}</span>`;
}

const fmtTokens = (n) => (n >= 1e9 ? `${(n / 1e9).toFixed(1)}B` : n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : `${n}`);

/// One limit as a labelled bar: green < 70 %, amber < 90 %, red beyond; faded once its window has reset.
function limitBar(l, compact = false) {
  const n = now();
  const reset = l.resets_at && l.resets_at <= n;
  const lvl = l.percent >= 90 ? "red" : l.percent >= 70 ? "amber" : "green";
  // Once a window has reset, the saved percentage no longer applies: show "–".
  const when = !l.resets_at ? "" : reset ? "reset · not refreshed" : `resets in ${human(l.resets_at - n)}`;
  const shown = reset ? 0 : l.percent;
  return `<div class="limit ${reset ? "stale" : ""} ${compact ? "compact" : ""}" title="${esc(`${l.label}: ${l.percent.toFixed(0)}%${when ? ` · ${when}` : ""}`)}">
    <span class="lname">${esc(l.label)}</span>
    <div class="track"><div class="fill ${lvl}" style="width:${Math.min(100, shown).toFixed(1)}%"></div></div>
    <span class="lpct">${reset ? "–" : `${l.percent.toFixed(0)}%`}</span>${compact ? "" : `<span class="lwhen">${when}</span>`}</div>`;
}

// ---- needs you ----------------------------------------------------------------
// Things that want a human. Shared by the main window's Home and the menu-bar panel,
// so both agree on what's dismissed (stored in localStorage, shared by the app's windows).

const DISMISS_KEY = "vixen-dismissed";
const DISMISS_TTL_S = 7 * 86400;

function readDismissed() {
  try { return JSON.parse(localStorage.getItem(DISMISS_KEY)) ?? {}; } catch { return {}; }
}
function writeDismissed(d) {
  try { localStorage.setItem(DISMISS_KEY, JSON.stringify(d)); } catch {}
}

/// Waiting agents, unreachable machines, jobs past their limit and failed jobs.
/// Each item has a key that changes when the situation does: an agent that works and
/// waits again gets a new key, so a dismissed item comes back only when there's news.
function needsYou(snap) {
  const items = [];
  for (const a of (snap?.agents ?? []).filter((a) => a.state === "waiting")) {
    items.push({ kind: "agent", a, key: `agent:${a.tool}::${a.id}@${a.updated_at}`, at: a.updated_at });
  }
  for (const t of snap?.targets ?? []) {
    if (t.error && !t.updated_at) items.push({ kind: "down", t, key: `down:${t.name}`, at: 0 });
    for (const j of t.jobs) {
      if (j.eta?.exceeds_limit) items.push({ kind: "limit", t, j, key: `late:${t.name}:${j.id}`, at: j.eta.finish_at });
      else if (j.state === "failed") items.push({ kind: "failed", t, j, key: `failed:${t.name}:${j.id}`, at: 0 });
    }
  }
  // Forget dismissals on evidence only: a machine that's back online, or entries older than a week.
  const d = readDismissed();
  const n = now();
  let changed = false;
  for (const [k, at] of Object.entries(d)) {
    const back = k.startsWith("down:") && (snap?.targets ?? []).some((t) => `down:${t.name}` === k && t.ok);
    if (back || n - at > DISMISS_TTL_S) { delete d[k]; changed = true; }
  }
  if (changed) writeDismissed(d);
  return items.filter((i) => !(i.key in d));
}

function dismissNeeds(keys) {
  const d = readDismissed();
  for (const k of keys) d[k] = now();
  writeDismissed(d);
}

/// Re-render when another window dismisses something.
function onDismissChange(fn) {
  window.addEventListener("storage", (e) => e.key === DISMISS_KEY && fn());
}

// ---- window chrome ----------------------------------------------------------

const appWindow = window.__TAURI__.window.getCurrentWindow();
const INTERACTIVE = "button, input, select, textarea, a, label, [data-no-drag], .segmented, .job, .pj, .pq, .pm, .qg-head";

// Drag the window from empty areas of `el`; optional double-click handler (e.g. zoom).
function draggable(el, onDouble) {
  el.addEventListener("mousedown", (e) => {
    if (e.button !== 0 || e.target.closest(INTERACTIVE)) return;
    if (e.detail === 2 && onDouble) return onDouble(e);
    appWindow.startDragging().catch(() => {});
  });
}

// Render only while visible; hidden windows catch up when shown.
function whenVisible(render) {
  let pending = false;
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && pending) {
      pending = false;
      render();
    }
  });
  return () => {
    if (document.hidden) pending = true;
    else render();
  };
}
