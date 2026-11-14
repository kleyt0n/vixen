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

// Fox mark (app/assets/fox.svg) in currentColor: white in dark mode, near-black in light.
const LOGO_DEFS = `<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>
  <mask id="fox-cut" maskUnits="userSpaceOnUse" x="0" y="0" width="24" height="24"> <path d="M3.5 2.8 9 8.3H15L20.5 2.8V12.9L12 21.4 3.5 12.9Z" fill="#fff" stroke="#fff" stroke-width="0.8" stroke-linejoin="round"/> </mask>
  <symbol id="logo" viewBox="1.5 1.5 21 21"><rect width="24" height="24" mask="url(#fox-cut)"/></symbol>
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
