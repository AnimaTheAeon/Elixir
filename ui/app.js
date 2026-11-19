// Elixir's window (DSM and Windows). Talks to the Elixir service through api.cgi.
const WIN = !!window.ELIXIR_WINDOWS;
if (WIN) document.documentElement.classList.add("on-windows");
"use strict";

const PAGES = [["queue", "Queue"], ["rename", "Rename"], ["auto", "Automation"], ["history", "History"], ["config", "Settings"]];
// Pages without a menu entry, and the entry they belong to.
const HIDDEN_PAGES = { log: "config" };

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));


let token = "";
let status = null;
let config = null;
let page = "rename";
let pollTimer = null;
const modalOpen = () => !!document.querySelector(".modal.open");
const closeModals = () => $$(".modal.open").forEach((m) => m.classList.remove("open"));

// ---------- API ----------

async function getToken() {
  if (WIN) return;
  try {
    const r = await fetch("/webman/login.cgi", { credentials: "same-origin" });
    const j = await r.json();
    token = j.SynoToken || "";
  } catch (e) { token = ""; }
}

async function api(route, body) {
  const opts = {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    headers: WIN ? { "X-Elixir-Key": window.ELIXIR_KEY || "" } : { "X-SYNO-TOKEN": token },
  };
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const [r0, q0] = route.split("?");
  const url = "api.cgi?r=" + encodeURIComponent(r0) + (q0 ? "&" + q0 : "");
  const res = await fetch(url, opts);
  let data = {};
  try { data = await res.json(); } catch (e) { /* empty */ }
  if (res.status === 401) {
    showOverlay("Sign in needed", esc(data.error || (WIN ? "Close this window and open Elixir again." : "Your DSM session has expired. Sign in to DSM again.")));
    throw new Error("auth");
  }
  if (res.status === 503) {
    showOverlay("Elixir isn't running", esc(data.error || (WIN ? "Start Elixir again from the Start menu." : "Start Elixir in Package Center.")));
    throw new Error("down");
  }
  hideOverlay();
  if (!res.ok) throw new Error(data.error || "Request failed");
  return data;
}

function showOverlay(title, html) {
  $("#overlay-title").textContent = title;
  $("#overlay-text").innerHTML = html;
  $("#overlay").classList.add("open");
}
function hideOverlay() { $("#overlay").classList.remove("open"); }

let toastTimer = null;
function toast(text, bad) {
  const t = $("#toast");
  t.textContent = text;
  t.className = "toast show" + (bad ? " bad" : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.className = "toast"; }, 4200);
}
/** Run an action; failures show as a toast (never silently). */
async function act(fn) {
  try { return await fn(); } catch (e) {
    if (e.message !== "auth" && e.message !== "down") toast(e.message || String(e), true);
    return null;
  }
}

// ---------- menu ----------

/** Dark Glass: move the menu's selection pill under the open page. The
 * pill is the menu's ::before, so it glides there with a CSS transition. */
function glassPill() {
  const nav = $("#menu");
  if (!nav) return;
  const cur = nav.querySelector(".menu-item.selected");
  nav.classList.toggle("has-sel", !!cur);
  if (!cur) return;
  nav.style.setProperty("--sel-top", cur.offsetTop + "px");
  nav.style.setProperty("--sel-left", cur.offsetLeft + "px");
  nav.style.setProperty("--sel-w", cur.offsetWidth + "px");
  nav.style.setProperty("--sel-h", cur.offsetHeight + "px");
}
addEventListener("resize", glassPill);


function buildMenu() {
  const nav = $("#menu");
  nav.innerHTML = PAGES.map(([id, label]) =>
    `<button class="menu-item" data-go="${id}"><svg class="hand"><use href="#hand"/></svg>${label}</button>`).join("");
  nav.addEventListener("click", (e) => {
    const b = e.target.closest("[data-go]");
    if (b) {
      nav.classList.add("menu-used");
      go(b.dataset.go);
    }
  });
  go(location.hash.slice(1) || "rename");
}

// ---------- Blades look ----------
// Glossy silver blades; the open one wears its page's color. The page itself
// (#main) moves into the open blade, so every page works as before.
// Same look as Turks and Lifestream's NAS pages.

const isBlades = () => document.body.classList.contains("blades");
const calm = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
let bstage = null, bcur = 0, bdrag = null, bcontour = null;
const BARROW = (d) => `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 16 16"><path d="${d}" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

function bladesBuild() {
  if (bstage) return;
  bstage = document.createElement("div");
  bstage.id = "bstage";
  bstage.setAttribute("aria-label", "Blades");
  document.body.insertBefore(bstage, $(".layout"));
  const make = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
  const nav = (dir, label, d) => { const b = make("button", "bnav", BARROW(d)); b.dataset.dir = dir; b.setAttribute("aria-label", label); b.onclick = () => bladesStep(dir); return b; };
  PAGES.forEach(([key, label]) => {
    const sec = make("section", "bsec k-" + key);
    sec.setAttribute("aria-label", label);
    const tab = make("button", "btab");
    tab.setAttribute("aria-label", label);
    tab.onclick = () => { if ((HIDDEN_PAGES[page] || page) !== key) go(key); };
    tab.append(make("span", "blabel", esc(label.toLowerCase())));
    const band = make("div", "bband", `<h1>${esc(label)}</h1><span class="bchip" title="Open ${esc(PAGES[0][1])}"><span class="bdot"></span><span class="btxt"></span></span><span class="bspeed"></span>`);
    $(".bchip", band).onclick = () => { if ((HIDDEN_PAGES[page] || page) !== PAGES[0][0]) go(PAGES[0][0]); };
    const foot = make("div", "bfoot", '<span class="bkey bhint">← →</span><span class="bhint">Blades</span><span class="bkey bhint">ENTER</span><span class="bhint">Select</span><span class="bkey bhint">ESC</span><span class="bhint">Back</span>');
    const navs = make("span", "bnavs");
    navs.append(nav(-1, "Previous blade", "M10 3 L5 8 L10 13"), nav(1, "Next blade", "M6 3 L11 8 L6 13"));
    foot.append(navs);
    const panel = make("div", "bpanel");
    panel.append(band, make("div", "bbody"), foot);
    const wrap = make("div", "bwrap");
    wrap.append(panel);
    sec.append(tab, wrap);
    bstage.append(sec);
  });
  const edge = make("div", "bedge");
  edge.setAttribute("aria-hidden", "true");
  bstage.append(edge);
  bcontour = bladesContour(bstage, ".bsec.open .bbody", ".bpanel");
  // Swipe sideways: anywhere with a finger, or from a tab or the top band with the mouse.
  bstage.addEventListener("pointerdown", (e) => { bdrag = { x: e.clientX, y: e.clientY, ok: e.pointerType === "touch" || !!e.target.closest(".btab, .bband") }; });
  bstage.addEventListener("pointerup", (e) => {
    if (!bdrag || !bdrag.ok) { bdrag = null; return; }
    const dx = e.clientX - bdrag.x, dy = e.clientY - bdrag.y; bdrag = null;
    if (Math.abs(dx) > 70 && Math.abs(dy) < 60) bladesStep(dx < 0 ? 1 : -1);
  });
}

// Blades: rows follow the open blade"s curved left edge. Each row shifts
// right by how far the curve comes in at its height (the most it comes in
// over the row"s span), so nothing runs into the curve near the top and
// bottom. Re-run on scroll, resize and page changes.
function bladesContour(stage, bodySel, panelSel) {
  let queued = false, shifted = new Set();
  const visible = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  // Children, looking through display: contents wrappers (they have no box).
  const kidsOf = (el) => [...el.children].flatMap((c) => (getComputedStyle(c).display === 'contents' ? kidsOf(c) : visible(c) ? [c] : []));
  const pick = (el, out) => {
    if (!visible(el)) return;
    const h = el.getBoundingClientRect().height;
    const kids = kidsOf(el);
    const t0 = kids.length ? kids[0].getBoundingClientRect().top : 0;
    const stacked = kids.length > 1 && kids.some((k) => Math.abs(k.getBoundingClientRect().top - t0) > 4);
    if (h > 90 && (stacked || kids.length === 1)) kids.forEach((k) => pick(k, out));
    else if (h > 90 && kids.length > 1) {
      // Side-by-side columns: only the leftmost one meets the curve.
      const left = kids.reduce((a, b) => (b.getBoundingClientRect().left < a.getBoundingClientRect().left ? b : a));
      pick(left, out);
    } else out.push(el);
  };
  const run = () => {
    queued = false;
    const open = stage.querySelector(bodySel);
    const panel = open && open.closest(panelSel);
    const next = new Set();
    if (open && panel) {
      const pr = panel.getBoundingClientRect(), br = open.getBoundingClientRect();
      const r = parseFloat(getComputedStyle(stage).getPropertyValue("--r")) || 44;
      const mid = pr.top + pr.height / 2, half = pr.height / 2;
      const arc = (y) => { const d = Math.max(-1, Math.min(1, (y - mid) / half)); return r * (1 - Math.sqrt(1 - d * d)); };
      const rows = [];
      [...open.children].forEach((c) => pick(c, rows));
      for (const el of rows) {
        shiftRow(el, 0);
        const er = el.getBoundingClientRect();
        const top = Math.max(er.top, br.top), bot = Math.min(er.bottom, br.bottom);
        if (bot <= top) continue;
        const dx = Math.round(Math.max(arc(top), arc(bot)));
        if (dx > 0) { shiftRow(el, dx); next.add(el); }
      }
    }
    shifted.forEach((el) => { if (!next.has(el)) shiftRow(el, 0); });
    shifted = next;
  };
  const queue = () => { if (!queued) { queued = true; requestAnimationFrame(run); } };
  stage.addEventListener("scroll", queue, true);
  addEventListener("resize", queue);
  stage.addEventListener("transitionend", queue);
  const mo = new MutationObserver(queue);
  mo.observe(stage, { childList: true, subtree: true, attributes: true, attributeFilter: ["class", "hidden", "open"] });
  queue();
  return {
    refresh: queue,
    stop: () => {
      mo.disconnect();
      stage.removeEventListener("scroll", queue, true);
      removeEventListener("resize", queue);
      stage.removeEventListener("transitionend", queue);
      shifted.forEach((el) => { shiftRow(el, 0); });
      shifted.clear();
    },
  };
}

// Moves a row right by dx and narrows it by the same amount, so its right
// end stays where it was instead of running past the blade's edge.
function shiftRow(el, dx) {
  el.style.translate = dx ? `${dx}px 0` : "";
  el.style.maxWidth = dx ? `calc(100% - ${dx}px)` : "";
}

function bladesPlace(instant) {
  if (!bstage) return;
  const W = innerWidth, narrow = W < 700, p = narrow ? 30 : 52, r = narrow ? 20 : 44, edge = narrow ? 14 : 30, n = PAGES.length;
  const ms = instant || calm() ? 0 : 700;
  bstage.style.cssText = `--p:${p}px;--r:${r}px;--edge:${edge}px;--bw:${W - edge - (n - 1) * p + r}px;--ms:${ms}ms`;
  $$(".bsec", bstage).forEach((s, i) => {
    s.style.transform = `translateX(${i <= bcur ? i * p : W - edge - (n - i) * p}px)`;
    s.style.zIndex = 10 + i;
    s.classList.toggle("open", i === bcur);
    $(".btab", s).tabIndex = i === bcur ? -1 : 0;
    $(".bwrap", s).inert = i !== bcur;
  });
  $$(".bnav", bstage).forEach((b) => { b.disabled = (+b.dataset.dir < 0 && bcur === 0) || (+b.dataset.dir > 0 && bcur === n - 1); });
}

/** Open the blade for `id`: #main moves into it and the blades slide. */
function bladesShow(id, instant) {
  bladesBuild();
  const i = Math.max(0, PAGES.findIndex(([p]) => p === (HIDDEN_PAGES[id] || id)));
  const slot = $$(".bbody", bstage)[i], main = $("#main");
  if (main.parentNode !== slot) slot.append(main);
  bcur = i;
  const title = (PAGES.find(([p]) => p === id) || [id, "Log"])[1];
  $(".bband h1", $$(".bsec", bstage)[i]).textContent = title;
  bladesPlace(instant);
  if (bcontour) bcontour.refresh();
}

function bladesOff() {
  if (bcontour) bcontour.stop();
  bcontour = null;
  const main = $("#main"), layout = $(".layout");
  if (main && layout && main.parentNode !== layout) layout.insertBefore(main, $(".side"));
  if (bstage) { bstage.remove(); bstage = null; }
}

function bladesStep(d) {
  const j = Math.max(0, Math.min(PAGES.length - 1, bcur + d));
  if (j !== bcur) go(PAGES[j][0]);
}

/** Chip and status line on every blade's top band. */
function drawBladesStatus(text, level) {
  if (!isBlades() || !bstage) return;
  const chip = level === "good" ? "All good" : level === "bad" ? "Problem" : "Needs a look";
  $$(".bband", bstage).forEach((b) => {
    $(".btxt", b).textContent = chip;
    $(".bdot", b).className = "bdot " + (level === "good" ? "" : level);
    $(".bspeed", b).textContent = text;
    $(".bspeed", b).title = text;
  });
}

/** Keys in Blades: arrows switch, Esc closes a dialog, then the log, then goes to the first blade. */
function bladesKey(e) {
  if (e.altKey || e.ctrlKey || e.metaKey) return;
  const tag = (document.activeElement || {}).tagName || "";
  if (/^(INPUT|TEXTAREA|SELECT)$/.test(tag)) return;
  if (modalOpen()) { if (e.key === "Escape") { e.preventDefault(); closeModals(); } return; }
  if (e.key === "ArrowRight") { e.preventDefault(); bladesStep(1); }
  else if (e.key === "ArrowLeft") { e.preventDefault(); bladesStep(-1); }
  else if (e.key === "Escape") {
    if (page === "log") { e.preventDefault(); go("config"); }
    else if (bcur > 0) { e.preventDefault(); go(PAGES[0][0]); }
  } else if (e.key === "Enter" && (!tag || tag === "BODY")) {
    const first = $(".bsec.open .bbody button, .bsec.open .bbody input, .bsec.open .bbody select", bstage);
    if (first) { e.preventDefault(); first.focus(); }
  }
}
document.addEventListener("keydown", (e) => { if (isBlades()) bladesKey(e); });
for (const type of ["mousedown", "mouseup"]) {
  window.addEventListener(type, (e) => {
    if (e.button !== 3 || !isBlades()) return;
    e.preventDefault();
    if (type !== "mouseup") return;
    if (modalOpen()) { if (e.key === "Escape") { e.preventDefault(); closeModals(); } return; }
    if (page === "log") return go("config");
    bladesStep(-1);
  });
}
addEventListener("resize", () => { if (isBlades()) bladesPlace(true); });

// ---------- Xbox look ----------

let greenHome = true;     // on the orb's home screen
let greenCursor = 0;      // highlighted entry there
const isGreen = () => document.body.classList.contains("green");

function applyTheme(theme) {
  const green = theme === "green", blades = theme === "blades", glass = theme === "glass";
  const key = glass ? "glass" : blades ? "blades" : green ? "green" : "ff";
  $$("#theme-pick [data-theme]").forEach((b) => b.classList.toggle("picked", b.dataset.theme === key));
  if (document.body.dataset.themed === key) return;
  document.body.dataset.themed = key;
  document.body.classList.toggle("green", green);
  document.body.classList.toggle("blades", blades);
  document.body.classList.toggle("glass", glass);
  requestAnimationFrame(glassPill);
  if (blades) {
    greenHome = false;
    document.body.classList.remove("g-at-home");
    bladesShow(page, true);
    return;
  }
  bladesOff();
  if (green) greenShowHome(false);
  else document.body.classList.remove("g-at-home");
}

function buildGreenMenu() {
  $("#g-menu").innerHTML = PAGES.map(([id, label], i) =>
    `<button class="g-tab" data-g="${i}"><span class="g-node"></span><span class="g-plate">${esc(label.toUpperCase())}</span></button>`).join("");
  $("#g-menu").addEventListener("click", (e) => {
    const b = e.target.closest("[data-g]");
    if (b) greenOpen(Number(b.dataset.g));
  });
  $("#g-menu").addEventListener("mouseover", (e) => {
    const b = e.target.closest("[data-g]");
    if (b) { greenCursor = Number(b.dataset.g); greenMarkCursor(); }
  });
  $("#g-select").onclick = () => greenOpen(greenCursor);
  $("#g-back").onclick = greenBack;
  document.addEventListener("keydown", (e) => {
    if (!isGreen() || modalOpen()) return;
    const typing = /INPUT|TEXTAREA|SELECT/.test(document.activeElement?.tagName || "");
    if (greenHome) {
      if (e.key === "ArrowDown") { greenCursor = (greenCursor + 1) % PAGES.length; greenMarkCursor(); e.preventDefault(); }
      else if (e.key === "ArrowUp") { greenCursor = (greenCursor + PAGES.length - 1) % PAGES.length; greenMarkCursor(); e.preventDefault(); }
      else if (e.key === "Enter") { greenOpen(greenCursor); e.preventDefault(); }
    } else if (e.key === "Escape" && !typing) {
      greenBack();
    } else if (e.key === "Escape" && typing) {
      document.activeElement.blur();
    }
  });
  greenMarkCursor();
}

function greenMarkCursor() {
  $$("#g-menu .g-tab").forEach((b, i) => b.classList.toggle("lit", i === greenCursor));
}

function greenOpen(i) {
  greenCursor = i;
  greenMarkCursor();
  if (calm()) {
    greenHome = false;
    document.body.classList.remove("g-at-home");
    go(PAGES[i][0]);
    return;
  }
  document.body.classList.add("g-zoom");
  setTimeout(() => {
    greenHome = false;
    document.body.classList.remove("g-at-home", "g-zoom");
    go(PAGES[i][0]);
    document.body.classList.add("g-reveal");
    setTimeout(() => document.body.classList.remove("g-reveal"), 500);
  }, 600);
}

/** Xbox page bar: a tab per page along the bottom, to switch pages without the home screen. */
function renderPageBar() {
  const cur = HIDDEN_PAGES[page] || page;
  $("#g-tabs").replaceChildren(...PAGES.map(([id, label], i) => {
    const b = document.createElement("button");
    b.className = "g-nav";
    b.textContent = label;
    b.setAttribute("aria-current", id === cur ? "page" : "false");
    b.onclick = () => {
      if (id === page) return;
      greenCursor = i;
      go(id);
      if (!calm()) {
        document.body.classList.remove("g-switch");
        void document.body.offsetWidth;
        document.body.classList.add("g-switch");
        setTimeout(() => document.body.classList.remove("g-switch"), 300);
      }
    };
    return b;
  }));
}

function greenBack() {
  if (page === "log") { go("config"); return; }
  greenShowHome(true);
}

function greenShowHome(animate) {
  greenHome = true;
  greenCursor = Math.max(0, PAGES.findIndex(([p]) => p === (HIDDEN_PAGES[page] || page)));
  greenMarkCursor();
  document.body.classList.add("g-at-home");
  if (animate && !calm()) {
    document.body.classList.add("g-unzoom");
    setTimeout(() => document.body.classList.remove("g-unzoom"), 600);
  }
}

function renderGreenStatus(text) {
  if (!status) return;
  const top = `${Number(status.queue || 0).toLocaleString()} IN QUEUE · ${Number(status.renamed_today || 0).toLocaleString()} RENAMED TODAY`;
  $$(".g-status .g-top").forEach((el) => { el.textContent = top; });
  $$(".g-status .g-bottom").forEach((el) => { el.textContent = text; });
  $$(".g-status").forEach((el) => el.classList.toggle("busy", (status.matching || 0) > 0));
}

function go(id) {
  if (!PAGES.some(([p]) => p === id) && !HIDDEN_PAGES[id]) id = "rename";
  page = id;
  if (isBlades()) bladesShow(id);
  const menuId = HIDDEN_PAGES[id] || id;
  $$(".menu-item").forEach((b) => b.classList.toggle("selected", b.dataset.go === menuId));
  glassPill();
  $$(".page").forEach((p) => p.classList.toggle("active", p.dataset.page === id));
  history.replaceState(null, "", "#" + id);
  const title = $("#g-title");
  if (title) title.textContent = (PAGES.find(([p]) => p === id) || [id, id === "log" ? "Log" : id])[1].toUpperCase();
  if ($("#g-tabs")) renderPageBar();
  refresh();
}

// ---------- rendering: items (Rename and Queue) ----------

const STATE = {
  matching: ["MATCHING…", "dim"], ready: ["READY", "good"], pick: ["NEEDS A PICK", "warn"], midgar: ["MIDGAR", "good"],
  turks: ["TURKS", "warn"], owned: ["THEIRS", "dim"], practice: ["PRACTICE", "warn"], done: ["DONE", "good"],
  failed: ["NOT DONE", "bad"], skipped: ["SKIPPED", "dim"],
};
const CHECK_NAMES = { title: "Title", year: "Year", exists: "Episode", single: "Only one" };
const selectable = (i) => i.state === "ready" || i.state === "midgar";
let lists = { rename: [], queue: [] };
let batches = [];
let currentBatch = null;
const deselected = new Set();

function checksChip(i) {
  const c = i.checks || [];
  if (!c.length) return "";
  const ok = c.filter((x) => x.ok).length;
  const tip = c.map((x) => `${x.ok ? "✓" : "✗"} ${CHECK_NAMES[x.name] || x.name}: ${x.text}`).join("\n");
  return `<span class="chk ${ok === c.length ? "ok" : "no"}" title="${esc(tip)}">${ok === c.length ? "✓" : "!"} ${ok}/${c.length}</span>`;
}

function itemRow(i, view) {
  const [tag, cls] = STATE[i.state] || [String(i.state).toUpperCase(), ""];
  const pickable = !["done", "owned", "matching", "skipped"].includes(i.state);
  const box = view === "rename"
    ? `<label class="check sel"><input type="checkbox" data-sel="${i.id}" ${selectable(i) && !deselected.has(i.id) ? "checked" : ""} ${selectable(i) ? "" : "disabled"}><span class="box"></span></label>`
    : "";
  const newName = i.state === "midgar" ? `Midgar imports it into ${esc(i.tracked_title)}`
    : i.target_name ? esc(i.target_name) : i.state === "matching" ? "…" : "—";
  const where = i.target && i.state !== "midgar" ? `<div class="path" title="${esc(i.target)}">${esc(i.target.replace(/[\\/][^\\/]*$/, ""))}</div>` : "";
  const extra = [
    i.label ? esc(i.label) : "",
    i.sidecars ? `+ ${i.sidecars} file${i.sidecars === 1 ? "" : "s"} going along` : "",
    i.torrent && ["ready", "midgar", "pick", "practice"].includes(i.state) ? "seeding: gets a second name, the download stays put" : "",
  ].filter(Boolean).join(" · ");
  const actions = [
    pickable ? `<button class="small" data-pick="${i.id}">Pick another match…</button>` : "",
    i.state === "practice" ? `<button class="small" data-doit="${i.id}">Do it now</button>` : "",
    view === "queue" && i.state === "ready" || view === "queue" && i.state === "pick" && i.plan ? `<button class="small" data-doit="${i.id}">${i.plan === "midgar" ? "Send to Midgar" : "Rename"}</button>` : "",
    view === "queue" && pickable ? `<button class="small" data-skip="${i.id}">Skip</button>` : "",
  ].join("");
  return `<div class="item st-${i.state}" data-id="${i.id}">
    ${box}
    <div class="item-body">
      <div class="old" title="${esc(i.path)}">${esc(i.name)}</div>
      <div class="new"><span class="arrow">→</span> <span class="nn">${newName}</span> ${checksChip(i)} <span class="tag ${cls}">${tag}</span></div>
      ${where}
      ${extra ? `<div class="dim small">${extra}</div>` : ""}
      ${i.message ? `<div class="msg small ${i.state === "failed" ? "bad" : i.state === "pick" || i.state === "turks" ? "warn" : "dim"}">${esc(i.message)}</div>` : ""}
    </div>
    <div class="item-actions">${actions}</div>
  </div>`;
}

function renderRename() {
  const list = lists.rename;
  $("#rename-empty").style.display = list.length ? "none" : "";
  $("#rename-list").innerHTML = list.map((i) => itemRow(i, "rename")).join("");
  const chosen = list.filter((i) => selectable(i) && !deselected.has(i.id));
  const own = chosen.filter((i) => i.state === "ready").length, mid = chosen.filter((i) => i.state === "midgar").length;
  const go = $("#rename-go");
  go.disabled = !chosen.length;
  go.textContent = own && mid ? `Rename ${own} · send ${mid} to Midgar` : mid ? `Send ${mid} to Midgar` : `Rename ${own || ""}`.trim();
  const count = (s) => list.filter((i) => i.state === s).length;
  const parts = [];
  if (count("matching")) parts.push(`${count("matching")} matching…`);
  if (count("ready")) parts.push(`${count("ready")} ready`);
  if (count("midgar")) parts.push(`${count("midgar")} for Midgar`);
  if (count("pick")) parts.push(`${count("pick")} need a pick`);
  if (count("done")) parts.push(`${count("done")} done`);
  if (count("owned")) parts.push(`${count("owned")} belong to Midgar/Turks`);
  if (count("skipped")) parts.push(`${count("skipped")} left out`);
  $("#rename-summary").textContent = parts.join(" · ");
  $("#select-all").checked = list.filter(selectable).every((i) => !deselected.has(i.id));
  $("#batches").innerHTML = batches.length > 1
    ? `<span class="dim">Recent:</span> ` + batches.map((b) => `<button class="small ${b.batch === (currentBatch || batches[0].batch) ? "picked" : ""}" data-batch="${b.batch}">${esc(b.folder || "files")} (${b.count})</button>`).join(" ")
    : "";
}

function renderQueue() {
  const list = lists.queue;
  const offers = (status && status.old || []).filter((o) => o.count > 0);
  $("#old-offers").innerHTML = offers.map((o) => `<div class="notice offer"><b>${Number(o.count).toLocaleString()} video${o.count === 1 ? " was" : "s were"} already in ${esc(o.folder)}</b> when you started watching it. Automation leaves them alone. <button class="small" data-existing="${esc(o.folder)}">Rename these too</button></div>`).join("");
  $("#queue-empty").style.display = list.length || offers.length ? "none" : "";
  $("#queue-list").innerHTML = list.map((i) => itemRow(i, "queue")).join("");
}

// ---------- rendering: automation ----------

function renderAuto() {
  if (!status || !config) return;
  const t = $("#auto-toggle");
  t.textContent = status.automatic ? "AUTOMATIC RENAMING: ON" : "AUTOMATIC RENAMING: OFF";
  t.className = "big " + (status.automatic ? "on" : "off");
  t.setAttribute("aria-checked", status.automatic ? "true" : "false");
  const a = status.auto || {};
  const line = !status.automatic ? "Off. Nothing happens by itself; Rename works any time."
    : a.state === "paused" ? a.message : status.practice ? "On, practicing: new files are checked and written down in History, nothing is renamed." : "On: new files in the folders below are renamed when Elixir is sure.";
  $("#auto-line").innerHTML = `<span class="${!status.automatic ? "dim" : a.state === "paused" ? "bad" : status.practice ? "warn" : "good"}">${esc(line)}</span>`;
  const info = Object.fromEntries((a.folders || []).map((f) => [f.path, f]));
  const old = Object.fromEntries((status.old || []).map((o) => [o.folder, o.count]));
  const box = $("#watched");
  if (!config.watched.length) {
    box.innerHTML = `<div class="empty small" style="padding:14px 0">No folders yet. Add your downloads folder (for example ${WIN ? "D:\\Downloads" : "/volume1/data/torrents"}).</div>`;
  } else {
    box.innerHTML = config.watched.map((p) => {
      const f = info[p] || {};
      const st = !status.automatic ? ["OFF", "dim"] : f.state === "watching" ? [status.practice ? "PRACTICING" : "WATCHING", status.practice ? "warn" : "good"]
        : f.state === "refused" ? ["NOT ALLOWED", "bad"] : f.state === "paused" ? ["PAUSED", "bad"] : ["…", "dim"];
      const takeover = (f.message || "").includes("Take over") ? `<button class="small" data-takeover="${esc(p)}">Take over</button>` : "";
      const bits = [f.waiting ? `${f.waiting} waiting to finish` : "", old[p] ? `${old[p]} from before (left alone)` : ""].filter(Boolean).join(" · ");
      return `<div class="party"><div class="body">
        <div class="title"><span class="name">${esc(p.split(/[\\/]/).filter(Boolean).pop() || p)}</span><span class="tag ${st[1]}">${st[0]}</span>
          <span class="actions">${takeover}<button class="small" data-unwatch="${esc(p)}">Remove</button></span></div>
        <div class="path">${esc(p)}</div>
        ${bits ? `<div class="dim small">${esc(bits)}</div>` : ""}
        <label class="noseed" title="Automation then runs here without Lifestream, and renamed files move instead of getting a second name"><input type="checkbox" data-noseed="${esc(p)}" ${(config.no_seed || []).includes(p) ? "checked" : ""}> Nothing seeds from this folder</label>
        ${f.message && f.state !== "watching" ? `<div class="notice">${esc(f.message)}</div>` : ""}
      </div></div>`;
    }).join("");
  }
}

async function runPreflight() {
  $("#preflight").innerHTML = `<div class="dim small">Checking…</div>`;
  const r = await act(() => api("preflight"));
  if (!r) { $("#preflight").innerHTML = ""; return; }
  $("#preflight").innerHTML = r.checks.map((c) => `<div class="pf"><span class="${c.ok === true ? "good" : c.ok === false ? "bad" : "dim"}">${c.ok === true ? "✓" : c.ok === false ? "✗" : "–"}</span> <b>${esc(c.name)}</b> <span class="dim">${esc(c.text)}</span></div>`).join("")
    || `<div class="dim small">Add a watched folder to test hardlinks.</div>`;
}

// ---------- rendering: history ----------

function when(t) {
  const d = new Date(t * 1000), today = new Date();
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === today.toDateString()) return `Today ${time}`;
  const y = new Date(today); y.setDate(today.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return `Yesterday ${time}`;
  return d.toLocaleDateString([], { month: "short", day: "numeric" }) + " " + time;
}

function renderHistory(h) {
  const prac = h.practice || [];
  $("#practice-list").innerHTML = prac.length ? `<div class="label" style="margin:4px 0">PRACTICE: WHAT WOULD HAVE HAPPENED</div>` + prac.map((i) => `<div class="run practice">
      <div class="run-head"><span class="tag warn">PRACTICE</span><span class="run-title">${esc(i.name)}</span><span class="grow"></span><span class="dim small">${when(i.updated)}</span><button class="small" data-doit="${i.id}">Do it now</button></div>
      <div class="dim small">${esc(i.message)}${i.label ? " · " + esc(i.label) : ""}</div></div>`).join("") + `<div class="divider"></div>` : "";
  const runs = h.runs || [];
  $("#history-empty").style.display = runs.length || prac.length ? "none" : "";
  $("#history-list").innerHTML = runs.map((r) => {
    const ok = r.items.filter((i) => i.ok).length;
    const title = r.items.length === 1 ? r.items[0].label : `${ok} of ${r.items.length} files`;
    const tag = r.undone ? ["UNDONE", "dim"] : ok === r.items.length ? ["DONE", "good"] : ok ? ["PARTLY", "warn"] : ["NOT DONE", "bad"];
    const lines = r.items.slice(0, 50).map((i) => `<div class="run-line ${i.ok ? "" : "bad"}"><span>${esc(i.label)}</span> <span class="dim">· ${esc(i.note)}</span></div>`).join("");
    return `<div class="run">
      <div class="run-head"><span class="tag ${tag[1]}">${tag[0]}</span><span class="run-title">${esc(title)}</span>${r.automatic ? `<span class="dim small">automatic</span>` : ""}<span class="grow"></span><span class="dim small">${when(r.time)}</span>
        ${r.undoable ? `<button class="small" data-undo="${r.id}">Undo</button>` : ""}</div>
      ${r.undo_note ? `<div class="small dim">${esc(r.undo_note)}</div>` : ""}${r.interrupted ? `<div class="small warn">Interrupted by a restart; finished safely afterwards.</div>` : ""}
      <details class="small"><summary>${r.items.length} file${r.items.length === 1 ? "" : "s"}</summary>${lines}</details>
    </div>`;
  }).join("");
}

// ---------- rendering: status ----------

function ago(secs) {
  if (secs < 90) return "just now";
  if (secs < 5400) return Math.round(secs / 60) + " min ago";
  if (secs < 129600) return Math.round(secs / 3600) + " h ago";
  return Math.round(secs / 86400) + " days ago";
}

let seenVersion = "";
function renderUpdate(u) {
  // The service came back on a new version after Update now: reload the page.
  if (seenVersion && u.current && u.current !== seenVersion) { location.reload(); return; }
  if (u.current) seenVersion = u.current;
  const st = $("#update-state");
  const checked = u.checked_at ? ` <span class="dim small">· checked ${ago(u.now - u.checked_at)}</span>` : "";
  if (u.checking) st.innerHTML = `Installed ${esc(u.current)} · <span class="warn">Checking for updates…</span>`;
  else if (u.error) st.innerHTML = `Installed ${esc(u.current)} · <span class="bad">Couldn't check for updates</span> <span class="dim small">${esc(u.error)}</span>${checked}`;
  else if (u.available) st.innerHTML = `Installed ${esc(u.current)} · <span class="warn">Version ${esc(u.latest)} is out</span>${checked}`;
  else if (u.latest) st.innerHTML = `Installed ${esc(u.current)} · <span class="good">Up to date</span>${checked}`;
  else st.innerHTML = `Installed ${esc(u.current)}`;
  const notes = $("#update-notes");
  const text = u.available && u.notes ? u.notes.trim() : "";
  notes.style.display = text ? "" : "none";
  if (notes.dataset.text !== text) { notes.dataset.text = text; $("#update-notes-text").textContent = text; }
  $("#update-check").disabled = !!u.checking;
  const pr = u.progress;
  const busy = !!(pr && pr.phase !== "failed");
  $("#update-now").style.display = u.available && u.can_install_now && !busy ? "" : "none";
  const prow = $("#update-progress");
  prow.style.display = pr ? "" : "none";
  if (pr) {
    const mb = (n) => (n / 1048576).toFixed(1);
    let frac = 0, text = "";
    if (pr.phase === "download") { frac = pr.total ? 0.02 + 0.8 * Math.min(1, pr.done / pr.total) : 0.1; text = `Step 1 of 3 · Downloading ${esc(pr.to)}: ${mb(pr.done)}${pr.total ? " of " + mb(pr.total) : ""} MB`; }
    else if (pr.phase === "verify") { frac = 0.88; text = "Step 2 of 3 · Checking the download…"; }
    else if (pr.phase === "restart") { frac = 0.96; text = `Step 3 of 3 · Restarting on ${esc(pr.to)}. This page reloads by itself.`; }
    else { text = `<span class="bad">The update didn't work: ${esc(pr.message)}</span>`; }
    $("#update-bar").style.width = (frac * 100).toFixed(1) + "%";
    $("#update-step").innerHTML = text;
  }
  const last = u.last_install;
  const lastEl = $("#update-last");
  if (last && last.time) {
    const w = ago(u.now - last.time);
    lastEl.innerHTML = last.ok ? `<span class="label">TASK</span>&nbsp; Ran ${w}: ${esc(last.message)}` : `<span class="label">TASK</span>&nbsp; <span class="bad">Ran ${w}: ${esc(last.message)}</span>`;
  } else lastEl.innerHTML = "";
  $("#update-setup").style.display = !WIN && config && config.auto_update && !(last && last.time) ? "" : "none";
  $("#update-window").style.display = u.can_install_now && !u.window_updates ? "" : "none";
  const cmd = $("#update-command");
  if (cmd.value !== u.command) cmd.value = u.command || "";
}

function renderStatus() {
  if (!status) return;
  $("#info-queue").textContent = Number(status.queue || 0).toLocaleString();
  $("#info-today").textContent = Number(status.renamed_today || 0).toLocaleString();
  renderAuto();
  renderUpdate(status.update || {});
  $("#about").textContent = `Elixir for ${WIN ? "Windows" : "DSM"} v${status.version}`;
  const a = status.auto || {};
  const c = status.connected || {};
  let text, level = "good";
  if (!c.tmdb && !c.tvdb) { text = "Add a TheMovieDB or TheTVDB key in Settings to start matching."; level = "warn"; }
  else if (status.matching) text = `Matching ${status.matching} file${status.matching === 1 ? "" : "s"}…`;
  else if (!status.automatic) text = "Automatic renaming is off. Rename by hand any time.";
  else if (a.state === "paused") { text = a.message; level = "bad"; }
  else text = a.message || "Watching for new files.";
  if (status.queue) text += ` ${status.queue} in the Queue.`;
  $("#statusbar").textContent = text;
  renderGreenStatus(text);
  drawBladesStatus(text, level);
}

// ---------- settings ----------

function getKey(k) { return k.split(".").reduce((o, p) => (o == null ? o : o[p]), config); }

function applyConfigToForm() {
  $$("[data-key]").forEach((el) => {
    if (document.activeElement === el && el.type !== "checkbox") return;
    const v = getKey(el.dataset.key);
    if (el.type === "checkbox") el.checked = !!v; else el.value = v ?? "";
  });
  $$("[data-secret]").forEach((el) => {
    if (document.activeElement === el) return;
    el.value = "";
    el.placeholder = config["has_" + el.dataset.secret] ? "•••••••• (saved)" : el.dataset.secret === "tvdb_pin" ? "only with a subscriber key" : "";
  });
  applyTheme(config.theme);
  try { localStorage.setItem("elixir-theme", config.theme); } catch (e) { /* ignore */ }
  renderAuto();
  renderNaming();
}

let saveTimer = null;
// Every change is numbered, so a save's reply never undoes a newer change
// (picking a second theme while the first is still saving).
let changeNo = 0;
const newest = {};
let pending = {};
function queueSave(changes, now) {
  changeNo++;
  for (const k of Object.keys(changes)) newest[k] = [changeNo, changes[k]];
  Object.assign(pending, changes);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(flushSave, now ? 0 : 400);
}
function withNewer(reply, upTo) {
  for (const [k, [no, v]] of Object.entries(newest)) {
    if (no > upTo) reply[k] = v; else delete newest[k];
  }
  return reply;
}
async function flushSave() {
  const changes = pending; pending = {};
  if (!Object.keys(changes).length) return;
  const upTo = changeNo;
  try {
    config = withNewer(await api("config", changes), upTo);
    applyConfigToForm();
    $$(".page.active .saved").forEach((s) => { s.classList.add("show"); setTimeout(() => s.classList.remove("show"), 1200); });
    refreshStatus();
  } catch (e) {
    if (e.message !== "auth" && e.message !== "down") toast("Couldn't save: " + e.message, true);
  }
}

// ---------- names ----------

const PRESETS = {
  plex: {
    episode_naming: { standard: "{Series TitleYear} - S{season:00}E{episode:00} - {Episode Title}", daily: "{Series TitleYear} - {Air-Date} - {Episode Title}", anime: "{Series TitleYear} - S{season:00}E{episode:00} - {Episode Title}", series_folder: "{Series TitleYear}", season_folder: "Season {season:00}", specials_folder: "Season 00", multi_style: 5, colon: 4 },
    movie_naming: { file: "{Movie TitleYear}", folder: "{Movie TitleYear}", colon: 4 },
  },
  jellyfin: {
    episode_naming: { standard: "{Series Title} S{season:00}E{episode:00} - {Episode Title}", daily: "{Series Title} {Air-Date} - {Episode Title}", anime: "{Series Title} S{season:00}E{episode:00} - {Episode Title}", series_folder: "{Series TitleYear}", season_folder: "Season {season:00}", specials_folder: "Season 00", multi_style: 5, colon: 4 },
    movie_naming: { file: "{Movie TitleYear}", folder: "{Movie TitleYear}", colon: 4 },
  },
  kodi: {
    episode_naming: { standard: "{Series Title} S{season:00}E{episode:00} - {Episode Title}", daily: "{Series Title} {Air-Date} - {Episode Title}", anime: "{Series Title} S{season:00}E{episode:00} - {Episode Title}", series_folder: "{Series TitleYear}", season_folder: "Season {season}", specials_folder: "Specials", multi_style: 5, colon: 4 },
    movie_naming: { file: "{Movie TitleYear}", folder: "{Movie TitleYear}", colon: 4 },
  },
};

// What's typed right now (examples update while typing, before saving).
function namingFromForm() {
  const ep = { ...(config.episode_naming || {}) }, mv = { ...(config.movie_naming || {}) };
  $$("[data-naming]").forEach((el) => {
    const [k, sub] = el.dataset.key.split(".");
    if (k === "episode_naming") ep[sub] = el.value; else mv[sub] = el.value;
  });
  return { ep, mv };
}

let namingTimer = null;
function renderNaming(soon) {
  clearTimeout(namingTimer);
  namingTimer = setTimeout(async () => {
    if (!config) return;
    const preset = config.naming_preset || "midgar";
    if (document.activeElement !== $("#naming-preset")) $("#naming-preset").value = preset;
    const { ep, mv } = namingFromForm();
    let r;
    try { r = await api("naming/example", { preset, episode_naming: ep, movie_naming: mv }); } catch (e) { return; }
    const midgar = preset === "midgar";
    $$('[data-naming="ep"]').forEach((el) => {
      el.disabled = midgar;
      if (midgar && r.naming) { const sub = el.dataset.key.split(".")[1]; el.value = r.naming[sub] ?? ""; }
    });
    $("#naming-note").textContent = midgar ? (r.midgar ? "Read from Midgar" : "Midgar's style (Midgar isn't connected)") : preset === "custom" ? "Your own formats" : "Edit any field to make it your own";
    const sep = WIN ? "\\" : "/";
    const rows = [["Episode", r.episode], ["Two episodes", r.multi], ["Special", r.special], ["Daily show", r.daily], ["Movie", r.movie]];
    $("#naming-examples").innerHTML = rows.map(([k, v]) => `<span class="ex-k">${k}</span><span class="ex-v">${esc((v || "").split(/[\\/]/).join(sep))}</span>`).join("");
  }, soon ? 0 : 250);
}

function wireNaming() {
  $("#naming-preset").onchange = (e) => {
    const p = e.target.value;
    const changes = { naming_preset: p };
    if (PRESETS[p]) Object.assign(changes, JSON.parse(JSON.stringify(PRESETS[p])));
    Object.assign(config, changes);
    applyConfigToForm();
    queueSave(changes, true);
    renderNaming(true);
  };
  document.addEventListener("input", (e) => { if (e.target.dataset && e.target.dataset.naming) renderNaming(); });
}

// Windows: files or folders dropped on the window go to Rename.
window.elixirDrop = async (paths) => {
  if (!paths || !paths.length) return;
  if (paths.length > 1) toast(`Renaming the first of ${paths.length} dropped items. Drop their folder to do them all at once.`);
  await startScan(paths[0]);
};

function wireSettings() {
  document.addEventListener("change", (e) => {
    const el = e.target;
    if (el.dataset.key) {
      const v = el.type === "checkbox" ? el.checked : el.type === "number" ? Number(el.value) : el.value;
      const [k, sub] = el.dataset.key.split(".");
      if (sub) {
        config[k] = { ...config[k], [sub]: v };
        const changes = { [k]: config[k] };
        // Editing a preset's names makes them your own.
        if (el.dataset.naming && (k === "episode_naming" ? config.naming_preset !== "custom" && config.naming_preset !== "midgar" : PRESETS[config.naming_preset])) {
          config.naming_preset = "custom";
          changes.naming_preset = "custom";
          if (k === "movie_naming") changes.episode_naming = namingFromForm().ep;
        }
        queueSave(changes);
        if (el.dataset.naming) renderNaming();
      } else {
        config[k] = v;
        queueSave({ [k]: v });
      }
      if (k === "practice") renderAuto();
    } else if (el.dataset.secret && el.value) {
      queueSave({ [el.dataset.secret]: el.value });
      el.value = "";
    }
  });
  document.addEventListener("change", async (e) => {
    const p = e.target.dataset && e.target.dataset.noseed;
    if (p === undefined) return;
    const on = e.target.checked;
    const r = await act(() => api("watched/noseed", { path: p, on }));
    if (r) { config.no_seed = (config.no_seed || []).filter((f) => f !== p).concat(on ? [p] : []); refreshStatus(); }
  });
  document.addEventListener("click", async (e) => {
    const t = e.target.closest("[data-test]");
    if (!t) return;
    await flushSave();
    const out = $(`[data-out="${t.dataset.test}"]`);
    out.innerHTML = `<span class="dim">Testing…</span>`;
    t.disabled = true;
    try {
      const r = await api("test", { service: t.dataset.test });
      out.innerHTML = `<span class="good">✓ ${esc(r.text)}</span>`;
    } catch (err) {
      if (err.message !== "auth" && err.message !== "down") out.innerHTML = `<span class="bad">✗ ${esc(err.message)}</span>`;
    }
    t.disabled = false;
  });
}

// ---------- folder / file picker ----------

let pickerPath = "";
let pickerMode = "rename"; // "rename" (folders and videos), "watch", or a setting key
async function openPicker(path = "") {
  const withFiles = pickerMode === "rename";
  const data = await act(() => api("browse?path=" + encodeURIComponent(path) + (withFiles ? "&files=1" : "")));
  if (!data) return;
  pickerPath = data.path;
  $("#picker-title").textContent = pickerMode === "rename" ? "Choose a folder or a video" : pickerMode === "watch" ? "Choose a folder to watch" : "Choose a folder";
  $("#picker-crumb").textContent = data.path || (WIN ? "This PC" : "Shared folders");
  $("#picker-up").disabled = data.parent === null || data.parent === undefined;
  $("#picker-up").dataset.parent = data.parent ?? "";
  $("#picker-choose").disabled = !data.path;
  $("#picker-choose").textContent = pickerMode === "rename" ? "Rename what's in this folder" : pickerMode === "watch" ? "Watch this folder" : "Use this folder";
  $("#picker-list").innerHTML = data.entries.map((e) => e.dir
    ? `<div class="picker-item" data-path="${esc(e.path)}" data-dir="1"><span class="ico">▸</span><span>${esc(e.name)}</span>${e.readable ? "" : ' <span class="bad small">no access</span>'}${e.volume ? `<span class="vol">${esc(e.volume)}</span>` : ""}</div>`
    : `<div class="picker-item file" data-path="${esc(e.path)}"><span class="ico">▪</span><span>${esc(e.name)}</span><span class="vol">${(e.size / 1073741824).toFixed(1)} GB</span></div>`).join("")
    || `<div class="empty">Nothing here</div>`;
  $("#picker-note").style.display = "none";
  $("#picker").classList.add("open");
}

async function startScan(path) {
  const data = await act(() => api("scan", { path, source: $("#source").value }));
  if (!data) return false;
  currentBatch = data.batch;
  deselected.clear();
  go("rename");
  return true;
}

function wirePicker() {
  $("#choose").onclick = () => { pickerMode = "rename"; openPicker(pickerPath || ""); };
  $("#add-watched").onclick = () => { pickerMode = "watch"; openPicker(""); };
  $$("[data-browse]").forEach((b) => { b.onclick = () => { pickerMode = b.dataset.browse; openPicker(""); }; });
  $("#picker-list").onclick = async (e) => {
    const item = e.target.closest("[data-path]");
    if (!item) return;
    if (item.dataset.dir) return openPicker(item.dataset.path);
    // A single video: rename just that one.
    if (await startScan(item.dataset.path)) $("#picker").classList.remove("open");
  };
  $("#picker-up").onclick = (e) => openPicker(e.target.dataset.parent || "");
  const goTyped = () => { const v = $("#picker-path").value.trim(); if (v) openPicker(v); };
  $("#picker-go").onclick = goTyped;
  $("#picker-path").onkeydown = (e) => { if (e.key === "Enter") { e.preventDefault(); goTyped(); } };
  $("#picker-cancel").onclick = () => $("#picker").classList.remove("open");
  $("#picker-choose").onclick = async () => {
    const note = $("#picker-note");
    try {
      if (pickerMode === "rename") {
        const data = await api("scan", { path: pickerPath, source: $("#source").value });
        currentBatch = data.batch;
        deselected.clear();
        $("#picker").classList.remove("open");
        go("rename");
      } else if (pickerMode === "watch") {
        const r = await api("watched/add", { path: pickerPath });
        $("#picker").classList.remove("open");
        config = await api("config");
        applyConfigToForm();
        toast(r.existing ? `Watching it. The ${r.existing} video${r.existing === 1 ? "" : "s"} already there are left alone.` : "Watching it.");
        refreshStatus();
      } else {
        const [k, sub] = pickerMode.split(".");
        config[k] = pickerPath;
        queueSave({ [k]: pickerPath }, true);
        applyConfigToForm();
        $("#picker").classList.remove("open");
        void sub;
      }
    } catch (err) {
      if (err.message === "auth" || err.message === "down") return;
      note.textContent = err.message;
      note.style.display = "";
    }
  };
}

// ---------- pick a match ----------

let matchItem = null;
function openMatcher(item) {
  matchItem = item;
  const p = item.parsed || {};
  $("#m-file").textContent = item.name;
  $("#m-query").value = p.title || "";
  $("#m-year").value = p.year || "";
  const movie = item.chosen ? item.chosen.kind === "movie" : (p.kind === "Movie" || p.kind === "Unknown");
  $("#m-kind").value = movie ? "movie" : "show";
  $("#m-source").value = item.source || "auto";
  $("#m-note").textContent = "";
  $("#m-batch").checked = true;
  showMatches(item.candidates || []);
  $("#matcher").classList.add("open");
  setTimeout(() => $("#m-query").focus(), 50);
}

let shown = [];
function showMatches(list) {
  shown = list;
  const chosen = matchItem && matchItem.chosen;
  $("#m-list").innerHTML = list.map((c, i) => `<div class="picker-item cand ${chosen && chosen.id === c.id && chosen.provider === c.provider ? "now" : ""}" data-cand="${i}">
      <span>${esc(c.title)}${c.year ? ` <span class="dim">(${c.year})</span>` : ""}</span>
      <span class="vol">${esc(c.kind === "movie" ? "Movie" : "Show")} · ${esc({ tmdb: "TheMovieDB", tvdb: "TheTVDB", anidb: "AniDB" }[c.provider] || "")}</span></div>`).join("")
    || `<div class="empty small">No matches yet. Search above.</div>`;
}

function wireMatcher() {
  const search = async () => {
    $("#m-note").textContent = "";
    $("#m-list").innerHTML = `<div class="empty small">Searching…</div>`;
    try {
      const r = await api("search", { query: $("#m-query").value, year: Number($("#m-year").value) || null, kind: $("#m-kind").value, source: $("#m-source").value });
      showMatches(r.results);
    } catch (e) {
      if (e.message !== "auth" && e.message !== "down") { $("#m-note").textContent = e.message; showMatches([]); }
    }
  };
  $("#m-search").onclick = search;
  $("#m-query").onkeydown = (e) => { if (e.key === "Enter") search(); };
  $("#m-cancel").onclick = () => $("#matcher").classList.remove("open");
  $("#m-list").onclick = async (e) => {
    const row = e.target.closest("[data-cand]");
    if (!row || !matchItem) return;
    const c = shown[Number(row.dataset.cand)];
    const r = await act(() => api("pick", { id: matchItem.id, candidate: c, batch_too: $("#m-batch").checked }));
    if (r) {
      $("#matcher").classList.remove("open");
      refresh();
    }
  };
}

// ---------- actions ----------

function findItem(id) {
  return [...lists.rename, ...lists.queue, ...(historyCache.practice || [])].find((i) => i.id === Number(id));
}
let historyCache = {};

function wireActions() {
  $("#auto-toggle").onclick = () => {
    const on = !status.automatic;
    status.automatic = on;
    config.automatic = on;
    renderAuto();
    queueSave({ automatic: on }, true);
    toast(on ? (config.practice ? "Automatic renaming is on, in practice mode." : "Automatic renaming is on.") : "Automatic renaming is off.");
  };
  $("#preflight-run").onclick = runPreflight;
  $("#view-log").onclick = () => go("log");
  $("#log-back").onclick = () => go("config");
  $("#theme-pick").onclick = (e) => {
    const b = e.target.closest("[data-theme]");
    if (!b || b.dataset.theme === config.theme) return;
    config.theme = b.dataset.theme;
    applyTheme(config.theme);
    try { localStorage.setItem("elixir-theme", config.theme); } catch (err) { /* ignore */ }
    // Stay on Settings in the new look.
    if (isGreen()) { greenHome = false; document.body.classList.remove("g-at-home"); go("config"); }
    else if (isBlades()) go("config");
    queueSave({ theme: config.theme }, true);
  };
  $("#source").onchange = async () => {
    const b = currentBatch || (batches[0] && batches[0].batch);
    if (b && lists.rename.length) {
      await act(() => api("source", { batch: b, source: $("#source").value }));
      refresh();
    }
  };
  $("#select-all").onchange = (e) => {
    lists.rename.filter(selectable).forEach((i) => (e.target.checked ? deselected.delete(i.id) : deselected.add(i.id)));
    renderRename();
  };
  $("#rename-go").onclick = async () => {
    const ids = lists.rename.filter((i) => selectable(i) && !deselected.has(i.id)).map((i) => i.id);
    if (!ids.length) return;
    $("#rename-go").disabled = true;
    if (await act(() => api("apply", { ids }))) toast("Working on it…");
    setTimeout(refresh, 800);
  };
  $("#rename-clear").onclick = async () => { await act(() => api("clear", { view: "rename", all: "1" })); currentBatch = null; refresh(); };
  $("#queue-clear").onclick = async () => { await act(() => api("clear", { view: "queue" })); refresh(); };
  document.addEventListener("change", (e) => {
    const s = e.target.closest("[data-sel]");
    if (!s) return;
    const id = Number(s.dataset.sel);
    if (s.checked) deselected.delete(id); else deselected.add(id);
    renderRename();
  });
  document.addEventListener("click", async (e) => {
    const b = e.target.closest("[data-pick],[data-doit],[data-skip],[data-undo],[data-unwatch],[data-takeover],[data-existing],[data-batch]");
    if (!b) return;
    if (b.dataset.pick) { const it = findItem(b.dataset.pick); if (it) openMatcher(it); }
    else if (b.dataset.doit) { if (await act(() => api("apply", { ids: [Number(b.dataset.doit)] }))) toast("Working on it…"); setTimeout(refresh, 800); }
    else if (b.dataset.skip) { await act(() => api("skip", { id: Number(b.dataset.skip) })); refresh(); }
    else if (b.dataset.undo) {
      if (!confirm("Put these files back the way they were?")) return;
      const r = await act(() => api("undo", { run: Number(b.dataset.undo) }));
      if (r) toast(r.note);
      refresh();
    } else if (b.dataset.unwatch) {
      if (!confirm(`Stop watching ${b.dataset.unwatch}?`)) return;
      await act(() => api("watched/remove", { path: b.dataset.unwatch }));
      config = await api("config"); applyConfigToForm(); refreshStatus();
    } else if (b.dataset.takeover) { await act(() => api("watched/takeover", { path: b.dataset.takeover })); refreshStatus(); }
    else if (b.dataset.existing) {
      const r = await act(() => api("existing", { folder: b.dataset.existing }));
      if (r) { currentBatch = r.batch; deselected.clear(); go("rename"); }
    } else if (b.dataset.batch) { currentBatch = Number(b.dataset.batch); deselected.clear(); refresh(); }
  });
  $("#update-now").onclick = async () => { await act(() => api("update/install", {})); refreshStatus(); setTimeout(refreshStatus, 1000); };
  $("#update-check").onclick = async () => { await act(() => api("update/check", {})); refreshStatus(); setTimeout(refreshStatus, 3000); };
  $("#update-command").onclick = (e) => e.target.select();
}

// ---------- polling ----------

async function refreshStatus() {
  try { status = await api("status"); } catch (e) { return; /* shown via overlay */ }
  try { renderStatus(); } catch (e) {
    $("#statusbar").textContent = "Display error: " + e.message + " — press Ctrl+F5 to reload Elixir.";
  }
}

async function refresh() {
  await refreshStatus();
  try {
    if (page === "rename") {
      const r = await api("items?view=rename" + (currentBatch ? "&batch=" + currentBatch : ""));
      lists.rename = r.items; batches = r.batches;
      if (!currentBatch && batches[0]) currentBatch = batches[0].batch;
      renderRename();
    } else if (page === "queue") {
      lists.queue = (await api("items?view=queue")).items;
      renderQueue();
    } else if (page === "history") {
      historyCache = await api("history");
      renderHistory(historyCache);
    } else if (page === "log") {
      renderLog((await api("log")).lines);
    }
  } catch (e) { /* shown via overlay */ }
}

function renderLog(lines) {
  const box = $("#log");
  const atBottom = box.scrollTop + box.clientHeight >= box.scrollHeight - 20;
  box.innerHTML = lines.map((l) => `<div class="log-line"><span class="t">${esc(l.slice(11, 19))}</span><span>${esc(l.slice(20))}</span></div>`).join("") || `<div class="empty">No activity yet.</div>`;
  if (atBottom) box.scrollTop = box.scrollHeight;
}

function schedule() {
  clearTimeout(pollTimer);
  if (document.visibilityState !== "visible") return;
  const busy = status && status.matching;
  pollTimer = setTimeout(async () => { await refresh(); schedule(); }, busy ? 1500 : 3000);
}

document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") refresh(); schedule(); });

// ---------- start screen ----------

const splashStart = Date.now();
let splashGone = false;
function splashStep(text) { const s = $(".sp-status"); if (s && !splashGone) s.textContent = text; }
function splashDone(theme) {
  if (splashGone) return;
  splashGone = true;
  const sp = $("#splash");
  if (!sp) return;
  if (theme) document.documentElement.dataset.splash = theme;
  const wait = calm() ? 0 : Math.max(0, 1800 - (Date.now() - splashStart));
  setTimeout(() => {
    sp.classList.add("gone");
    setTimeout(() => sp.remove(), calm() ? 0 : 650);
  }, wait);
}
setTimeout(() => splashDone(), 9000);

(async function init() {
  await getToken();
  buildMenu();
  buildGreenMenu();
  wireSettings();
  wireNaming();
  wirePicker();
  wireMatcher();
  wireActions();
  splashStep("Reading settings…");
  try {
    config = await api("config");
    applyConfigToForm();
  } catch (e) {
    if (e.message !== "auth" && e.message !== "down") $("#statusbar").textContent = "Couldn't load settings: " + e.message;
  }
  splashStep("Checking in with Lifestream, Midgar and Turks…");
  await refresh();
  splashStep("Ready");
  splashDone(config && config.theme);
  schedule();
})();
