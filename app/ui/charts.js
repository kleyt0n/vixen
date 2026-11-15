// Small SVG charts for the Home page: a multi-series line chart (GPU utilisation over
// time) and stacked hourly bars (agent tokens). Colours come from the --series-N
// tokens in styles.css (validated categorical palette, light and dark steps).
"use strict";

const SVGNS = 'xmlns="http://www.w3.org/2000/svg"';
const hhmm = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });

function chartTooltip(el, html, x, y) {
  let tip = el.querySelector(".chart-tip");
  if (!tip) {
    tip = document.createElement("div");
    tip.className = "chart-tip";
    el.appendChild(tip);
  }
  if (html == null) return void (tip.hidden = true);
  tip.innerHTML = html;
  tip.hidden = false;
  const w = tip.offsetWidth;
  tip.style.left = `${Math.max(4, Math.min(el.clientWidth - w - 4, x + 12))}px`;
  tip.style.top = `${Math.max(0, y - 10)}px`;
}

const swatch = (color) => `<span class="sw" style="background:${color}"></span>`;

/// series: [{ name, color, points: [[t, v], …] }], times in unix seconds, v in 0–100.
function lineChart(el, series, from, to) {
  const W = Math.max(280, el.clientWidth), H = 176;
  const m = { l: 34, r: 92, t: 10, b: 22 };
  const iw = W - m.l - m.r, ih = H - m.t - m.b;
  const x = (t) => m.l + ((t - from) / (to - from)) * iw;
  const y = (v) => m.t + ih - (Math.min(100, Math.max(0, v)) / 100) * ih;
  const GAP_S = 20 * 60; // break the line across missing polls

  let svg = `<svg ${SVGNS} width="${W}" height="${H}" class="chart">`;
  for (const v of [0, 50, 100]) {
    svg += `<line class="grid" x1="${m.l}" x2="${m.l + iw}" y1="${y(v)}" y2="${y(v)}"/><text class="axis" x="${m.l - 6}" y="${y(v) + 3.5}" text-anchor="end">${v}%</text>`;
  }
  const span = to - from, step = span > 12 * 3600 ? 6 * 3600 : span > 4 * 3600 ? 2 * 3600 : 3600;
  for (let t = Math.ceil(from / step) * step; t <= to; t += step) {
    svg += `<text class="axis" x="${x(t)}" y="${H - 6}" text-anchor="middle">${hhmm(t)}</text>`;
  }

  const labels = [];
  for (const s of series) {
    const pts = s.points.filter(([t]) => t >= from && t <= to);
    if (!pts.length) continue;
    let d = "", prev = null;
    for (const [t, v] of pts) {
      d += `${!prev || t - prev > GAP_S ? "M" : "L"}${x(t).toFixed(1)},${y(v).toFixed(1)}`;
      prev = t;
    }
    if (pts.length === 1) svg += `<circle cx="${x(pts[0][0])}" cy="${y(pts[0][1])}" r="3" fill="${s.color}"/>`;
    svg += `<path d="${d}" fill="none" stroke="${s.color}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>`;
    const [lt, lv] = pts[pts.length - 1];
    labels.push({ name: s.name, color: s.color, x: x(lt), y: y(lv), v: lv });
  }
  // Direct end labels, nudged apart so they never collide.
  labels.sort((a, b) => a.y - b.y);
  for (let i = 1; i < labels.length; i++) labels[i].y = Math.max(labels[i].y, labels[i - 1].y + 13);
  for (const l of labels) {
    svg += `<circle cx="${l.x}" cy="${l.y}" r="3" fill="${l.color}" class="ring"/><text class="end" x="${l.x + 8}" y="${l.y + 4}">${esc(l.name)} <tspan class="v">${Math.round(l.v)}%</tspan></text>`;
  }
  svg += `<line class="cross" x1="0" x2="0" y1="${m.t}" y2="${m.t + ih}" visibility="hidden"/><rect class="hit" x="${m.l}" y="${m.t}" width="${iw}" height="${ih}"/></svg>`;
  el.innerHTML = svg;

  const hit = el.querySelector(".hit"), cross = el.querySelector(".cross");
  hit.addEventListener("mousemove", (e) => {
    const rect = el.getBoundingClientRect();
    const px = e.clientX - rect.left;
    const t = from + ((px - m.l) / iw) * (to - from);
    const rows = series.map((s) => {
      const near = s.points.reduce((best, p) => (Math.abs(p[0] - t) < Math.abs((best?.[0] ?? Infinity) - t) ? p : best), null);
      return near && Math.abs(near[0] - t) < GAP_S ? { s, near } : null;
    }).filter(Boolean);
    if (!rows.length) return chartTooltip(el, null);
    const at = rows[0].near[0];
    cross.setAttribute("x1", x(at));
    cross.setAttribute("x2", x(at));
    cross.setAttribute("visibility", "visible");
    chartTooltip(el, `<b>${hhmm(at)}</b>${rows.sort((a, b) => b.near[1] - a.near[1])
      .map(({ s, near }) => `<div>${swatch(s.color)}${esc(s.name)}<span>${Math.round(near[1])}%</span></div>`).join("")}`, px, e.clientY - rect.top);
  });
  hit.addEventListener("mouseleave", () => {
    cross.setAttribute("visibility", "hidden");
    chartTooltip(el, null);
  });
}

/// stacks: [{ name, color, values: [24 numbers, oldest first] }]; the last bar is the current hour.
function hourlyBars(el, stacks, now) {
  const W = Math.max(280, el.clientWidth), H = 176;
  const m = { l: 44, r: 8, t: 10, b: 22 };
  const iw = W - m.l - m.r, ih = H - m.t - m.b;
  const totals = Array.from({ length: 24 }, (_, i) => stacks.reduce((s, k) => s + (k.values[i] ?? 0), 0));
  const raw = Math.max(1, ...totals);
  const mag = 10 ** Math.floor(Math.log10(raw));
  const top = Math.ceil(raw / mag / 2) * 2 * mag; // a round top
  const slot = iw / 24, bw = Math.max(3, slot - 3);
  const y = (v) => m.t + ih - (v / top) * ih;
  const hourStart = now - (now % 3600);
  const startOf = (i) => hourStart - (23 - i) * 3600;

  let svg = `<svg ${SVGNS} width="${W}" height="${H}" class="chart">`;
  for (const v of [0, top / 2, top]) {
    svg += `<line class="grid" x1="${m.l}" x2="${m.l + iw}" y1="${y(v)}" y2="${y(v)}"/><text class="axis" x="${m.l - 6}" y="${y(v) + 3.5}" text-anchor="end">${fmtTokens(v)}</text>`;
  }
  for (let i = 0; i < 24; i++) {
    const x0 = m.l + i * slot + (slot - bw) / 2;
    if (i % 6 === 5 || i === 23) svg += `<text class="axis" x="${x0 + bw / 2}" y="${H - 6}" text-anchor="middle">${i === 23 ? "now" : hhmm(startOf(i))}</text>`;
    let base = 0;
    const segs = stacks.map((k) => ({ k, v: k.values[i] ?? 0 })).filter((s) => s.v > 0);
    segs.forEach((s, j) => {
      const y1 = y(base), y0 = y(base + s.v);
      const h = Math.max(1, y1 - y0 - (j < segs.length - 1 ? 2 : 0)); // 2px surface gap between segments
      const r = j === segs.length - 1 ? Math.min(4, h, bw / 2) : 0; // rounded data-end on the top segment
      svg += r
        ? `<path d="M${x0},${y1} V${y0 + r} Q${x0},${y0} ${x0 + r},${y0} H${x0 + bw - r} Q${x0 + bw},${y0} ${x0 + bw},${y0 + r} V${y1} Z" fill="${s.k.color}"/>`
        : `<rect x="${x0}" y="${y1 - h}" width="${bw}" height="${h}" fill="${s.k.color}"/>`;
      base += s.v;
    });
    svg += `<rect class="hit" data-i="${i}" x="${m.l + i * slot}" y="${m.t}" width="${slot}" height="${ih}"/>`;
  }
  el.innerHTML = svg + "</svg>";

  el.querySelectorAll(".hit").forEach((r) => {
    r.addEventListener("mousemove", (e) => {
      const i = Number(r.dataset.i), rect = el.getBoundingClientRect();
      const rows = stacks.filter((k) => (k.values[i] ?? 0) > 0)
        .map((k) => `<div>${swatch(k.color)}${esc(k.name)}<span>${fmtTokens(k.values[i])}</span></div>`).join("");
      chartTooltip(el, `<b>${hhmm(startOf(i))}–${i === 23 ? "now" : hhmm(startOf(i) + 3600)}</b>${rows || `<div class="faint">no tokens</div>`}${totals[i] ? `<div class="tot">Total<span>${fmtTokens(totals[i])}</span></div>` : ""}`,
        e.clientX - rect.left, e.clientY - rect.top);
    });
    r.addEventListener("mouseleave", () => chartTooltip(el, null));
  });
}

const legend = (items) => `<div class="legend">${items.map((i) => `<span>${swatch(i.color)}${esc(i.name)}</span>`).join("")}</div>`;
