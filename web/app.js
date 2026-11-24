"use strict";

// Built-in xterm.js client for rterminal's HTTP control API.
// Tabs here are a mirror of the native tab list: switching/creating/closing
// all go through the same stable tab ids the native window uses.

const tabStrip = document.getElementById("tabs");
const panes = document.getElementById("panes");
const statusEl = document.getElementById("status");
const statusBar = document.querySelector(".statusbar");
const scrollbackView = document.getElementById("scrollback-view");
const emptyState = document.getElementById("empty-state");
const webFitToggle = document.getElementById("web-fit");
const toolbarToggle = document.getElementById("toggle-toolbar");
const keyToolbar = document.getElementById("key-toolbar");
const keyScroll = document.getElementById("key-scroll");
const keyNav = document.getElementById("key-nav");

function savedWebFit() {
  // The web view always sizes itself to its own window. The checkbox only
  // decides whether that size is also pushed to the native PC grid.
  try {
    return localStorage.getItem("rterminal.webFit") === "1";
  } catch {
    return false;
  }
}

function savedToolbar() {
  try {
    return localStorage.getItem("rterminal.toolbar") === "1";
  } catch {
    // Off by default: the top bar stays minimal until the user asks for it.
    return false;
  }
}

const state = {
  tabs: new Map(),
  active: null,
  refreshing: false,
  webFit: savedWebFit(),
  showToolbar: savedToolbar(),
  // Sticky modifiers latched by the toolbar. They modify both toolbar keys
  // and text typed on the soft keyboard, then stay on until toggled off.
  mods: { shift: false, ctrl: false, alt: false, meta: false },
  webConnected: false,
  clientBlocked: false
};
webFitToggle.checked = state.webFit;
panes.classList.toggle("host-size", !state.webFit);
toolbarToggle.setAttribute("aria-pressed", String(state.showToolbar));

// Phones keep a stable layout viewport while the on-screen keyboard shrinks
// only the visual viewport. Treat that as a viewport pan, not a terminal
// resize: the PTY grid and the native window stay put, and the live bottom is
// scrolled into the visible strip instead.
const visualViewport = window.visualViewport;
const coarsePointer =
  window.matchMedia && window.matchMedia("(pointer: coarse)").matches;
let normalViewportHeight = visualViewport
  ? visualViewport.height || window.innerHeight
  : window.innerHeight;
let keyboardInset = 0;
let keyboardTimer = null;

const clientId = (() => {
  try {
    let id = sessionStorage.getItem("rterminal.webClientId");
    if (!id) {
      id =
        typeof crypto.randomUUID === "function"
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random()}`;
      sessionStorage.setItem("rterminal.webClientId", id);
    }
    return id;
  } catch {
    return `${Date.now()}-${Math.random()}`;
  }
})();

async function notifyClient(connected) {
  const tab = currentTab();
  const body = JSON.stringify({
    id: clientId,
    connected,
    cols: tab ? tab.term.cols : 80,
    rows: tab ? tab.term.rows : 24,
    web_fit: state.webFit
  });
  try {
    const response = await fetch("/web/client", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
      keepalive: connected
    });
    if (!response.ok) {
      const detail = (await response.text()).slice(0, 200);
      throw new Error(`${response.status} ${detail}`);
    }
    state.webConnected = connected;
  } catch (error) {
    if (connected && /409|already connected/.test(String(error))) {
      state.clientBlocked = true;
      statusEl.textContent = "另一个 Web 客户端已连接";
      const active = currentTab();
      if (active) stopPolling(active);
    } else if (connected) {
      report(error);
    }
  }
}

window.addEventListener("beforeunload", () => {
  const body = JSON.stringify({ id: clientId, connected: false, web_fit: state.webFit });
  navigator.sendBeacon("/web/client", new Blob([body], { type: "application/json" }));
});

function report(error) {
  const message = error instanceof Error ? error.message : String(error);
  statusEl.textContent = `⚠ ${message}`;
  statusBar.classList.add("error");
  console.error(error);
}

function clearError() {
  statusBar.classList.remove("error");
}

function keyboardInsetPx() {
  if (!visualViewport) return 0;
  // A real pinch zoom shrinks visualViewport.height too, but it is not a
  // keyboard and must not drive the layout or a PTY resize.
  if (visualViewport.scale > 1.01) return 0;
  const height = visualViewport.height;
  if (!height) return 0;
  if (height > normalViewportHeight) normalViewportHeight = height;
  const inset = normalViewportHeight - height;
  // Browser chrome collapsing is only a few pixels; the soft keyboard takes
  // a large, stable bite out of the visual viewport. Smaller changes remain
  // ordinary window resizes.
  return inset >= 120 ? Math.round(inset) : 0;
}

function armScrollGuard(tab) {
  tab.scrollGuard = true;
  if (tab.scrollGuardTimer !== null) clearTimeout(tab.scrollGuardTimer);
  tab.scrollGuardTimer = setTimeout(() => {
    tab.scrollGuardTimer = null;
    tab.scrollGuard = false;
  }, 100);
}

// Bring the live cursor into view on BOTH axes. xterm only owns the inner
// viewport; the outer .pane is what scrolls when the PTY grid is bigger than
// the phone screen, so it must be adjusted here too.
function scrollToCursor(tab) {
  if (!tab) return;
  armScrollGuard(tab);
  const term = tab.term;
  if (typeof term.scrollToBottom === "function") term.scrollToBottom();

  const active = term.buffer && term.buffer.active;
  const col = active ? active.cursorX : 0;
  const row = active ? active.cursorY : Math.max(0, term.rows - 1);
  const { cw, ch } = cellMetrics(tab);
  const x = 4 + col * cw;
  const y = 4 + row * ch;
  const viewW = tab.wrapper.clientWidth;
  const viewH = tab.wrapper.clientHeight;

  let left = tab.wrapper.scrollLeft;
  let top = tab.wrapper.scrollTop;
  if (x < left) left = x;
  else if (x + cw > left + viewW) left = x + cw - viewW;
  if (y < top) top = y;
  else if (y + ch > top + viewH) top = y + ch - viewH;
  tab.wrapper.scrollLeft = Math.max(0, left);
  tab.wrapper.scrollTop = Math.max(0, top);
}

function followCursor(tab) {
  if (!tab) return;
  tab.followCursor = true;
  scrollToCursor(tab);
}

function revealBottom(tab) {
  // Viewport/keyboard changes only re-take control if the user was already
  // following the cursor. A manual scroll stays put until the next input.
  if (tab && tab.followCursor) scrollToCursor(tab);
}

async function api(path, options = {}) {
  const response = await fetch(path, options);
  if (!response.ok) {
    const detail = (await response.text()).slice(0, 300);
    throw new Error(`${response.status} ${detail}`);
  }
  const contentType = response.headers.get("content-type") || "";
  return contentType.includes("json") ? response.json() : response.text();
}

function currentTab() {
  return state.active === null ? null : state.tabs.get(state.active) || null;
}

// Inject keystrokes through the same serialised chain as xterm's onData, so a
// toolbar Enter cannot overtake characters still queued from the soft keyboard.
function sendInput(tab, data) {
  if (!tab || !data) return;
  followCursor(tab);
  tab.inputChain = tab.inputChain
    .then(() => api(`/tabs/${tab.id}/input`, { method: "POST", body: data }))
    .catch(report);
}

// Same modifier numbering as the native keyboard encoder: 1 + shift + 2*alt
// + 4*ctrl (plus 8*meta for the platform key, which native reserves for app
// shortcuts). CSI 1;n{key} is how terminals encode modified navigation keys.
function modifierCode() {
  const m = state.mods;
  return 1 + (m.shift ? 1 : 0) + (m.alt ? 2 : 0) + (m.ctrl ? 4 : 0) + (m.meta ? 8 : 0);
}

function anyModifier() {
  const m = state.mods;
  return m.shift || m.ctrl || m.alt || m.meta;
}

// Alt/Meta prefix printable and control bytes with ESC. Ctrl on a plain letter
// folds it to the matching control byte; shift uppercases. IME/paste text is
// never rewritten, only a single ASCII keystroke is.
function encodeTyped(text) {
  if (!anyModifier() || typeof text !== "string" || text.length !== 1) return text;
  const ch = text[0];
  const code = ch.charCodeAt(0);
  if (code < 0x20 || code > 0x7e) return text;
  const m = state.mods;
  let out = ch;
  if (m.ctrl) out = String.fromCharCode(ch.toLowerCase().charCodeAt(0) & 0x1f);
  else if (m.shift) out = ch.toUpperCase();
  if (m.alt || m.meta) out = "\x1b" + out;
  return out;
}

// A toolbar key is a semantic key, so latched modifiers turn a plain arrow
// into Ctrl+Arrow etc. instead of appending to a pre-baked byte string.
function encodeToolbarKey(key) {
  const m = state.mods;
  if (!anyModifier()) return key.data;
  if (key.special === "tab") {
    if (m.shift && !m.ctrl && !m.alt && !m.meta) return "\x1b[Z";
    return `\x1b[1;${modifierCode()}I`;
  }
  if (key.csiFinal) return `\x1b[1;${modifierCode()}${key.csiFinal}`;
  // Control / Esc bytes: Alt or Meta adds an ESC prefix; Shift alone leaves
  // the control byte unchanged (there is no shifted form of ^C).
  return (m.alt || m.meta ? "\x1b" : "") + key.data;
}

const MODIFIER_KEYS = [
  { id: "shift", label: "Shift", title: "Shift (sticky)" },
  { id: "ctrl", label: "Ctrl", title: "Ctrl (sticky)" },
  { id: "alt", label: "Alt", title: "Alt (sticky)" },
  {
    id: "meta",
    label: /Mac|iPhone|iPad|iPod/i.test(navigator.platform || "") ? "Cmd" : "Win",
    title: "Windows / Command (sticky)"
  }
];

// One-shot keys scroll horizontally. Arrows live in the separate pinned pad.
const ACTION_KEYS = [
  { label: "Esc", data: "\x1b", special: "escape" },
  { label: "Tab", data: "\t", special: "tab" },
  { separator: true },
  { label: "^C", data: "\x03", title: "interrupt" },
  { label: "^D", data: "\x04", title: "EOF / exit" },
  { label: "^Z", data: "\x1a", title: "suspend" },
  { label: "^A", data: "\x01", title: "line start" },
  { label: "^E", data: "\x05", title: "line end" },
  { label: "^U", data: "\x15", title: "clear line" },
  { label: "^K", data: "\x0b", title: "clear to end" },
  { label: "^L", data: "\x0c", title: "clear screen" },
  { label: "^R", data: "\x12", title: "reverse search" }
];

const NAV_KEYS = [
  { label: "↑", data: "\x1b[A", csiFinal: "A", title: "up" },
  { label: "↓", data: "\x1b[B", csiFinal: "B", title: "down" },
  { label: "←", data: "\x1b[D", csiFinal: "D", title: "left" },
  { label: "→", data: "\x1b[C", csiFinal: "C", title: "right" }
];

function makeKeyButton(key, onPress) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = key.label;
  button.title = key.title || key.label;
  if (key.mod) button.classList.add("key-mod");
  // preventDefault on pointerdown keeps xterm's hidden textarea focused, so
  // the soft keyboard stays open; blur() drops the transient pressed state.
  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    button.blur();
    onPress();
  });
  return button;
}

function buildToolbar() {
  for (const key of MODIFIER_KEYS) {
    const button = makeKeyButton(key, () => {
      state.mods[key.id] = !state.mods[key.id];
      button.classList.toggle("active", state.mods[key.id]);
      button.setAttribute("aria-pressed", String(state.mods[key.id]));
    });
    button.classList.add("key-toggle");
    button.setAttribute("aria-pressed", "false");
    keyScroll.appendChild(button);
  }
  for (const key of ACTION_KEYS) {
    if (key.separator) {
      const sep = document.createElement("span");
      sep.className = "key-sep";
      keyScroll.appendChild(sep);
      continue;
    }
    keyScroll.appendChild(
      makeKeyButton(key, () => sendInput(currentTab(), encodeToolbarKey(key)))
    );
  }
  for (const key of NAV_KEYS) {
    keyNav.appendChild(
      makeKeyButton(key, () => sendInput(currentTab(), encodeToolbarKey(key)))
    );
  }
}

function clearModifiers() {
  for (const key of MODIFIER_KEYS) {
    state.mods[key.id] = false;
  }
  for (const button of keyScroll.querySelectorAll(".key-toggle")) {
    button.classList.remove("active");
    button.setAttribute("aria-pressed", "false");
  }
}

function setToolbar(visible) {
  state.showToolbar = visible;
  keyToolbar.hidden = !visible;
  document.body.classList.toggle("with-toolbar", visible);
  toolbarToggle.setAttribute("aria-pressed", String(visible));
  // A latched Ctrl/Alt must never survive into plain typing after hiding.
  if (!visible) clearModifiers();
  try {
    localStorage.setItem("rterminal.toolbar", visible ? "1" : "0");
  } catch {
    // Storage may be disabled; the toggle still works for this session.
  }
  scheduleFit();
}

function makeTerm(cols, rows) {
  return new Terminal({
    cols,
    rows,
    fontFamily: '"Cascadia Mono", Consolas, "rterm Powerline", "Segoe UI Symbol", monospace',
    fontSize: 14,
    lineHeight: 1.2,
    cursorBlink: false,
    scrollback: 2000,
    allowProposedApi: false,
    theme: {
      background: "#0f1115",
      foreground: "#d7dae0",
      cursor: "#3b82f6",
      cursorAccent: "#0f1115",
      selectionBackground: "#264f78",
      black: "#1d1f21",
      red: "#cc6666",
      green: "#b5bd68",
      yellow: "#f0c674",
      blue: "#81a2be",
      magenta: "#b294bb",
      cyan: "#8abeb7",
      white: "#c5c8c6",
      brightBlack: "#666666",
      brightRed: "#d54e53",
      brightGreen: "#b9ca4a",
      brightYellow: "#e7c547",
      brightBlue: "#7aa6da",
      brightMagenta: "#c397d8",
      brightCyan: "#70c0ba",
      brightWhite: "#eaeaea"
    }
  });
}

function cellMetrics(tab) {
  const element = tab.term.element;
  // The rendered screen is the ground truth for cell size. Deriving it from
  // CSS font metrics is unreliable: xterm.js rounds cell height to whole
  // pixels and its hidden measure element holds 32 '~' glyphs, not one cell.
  const screen = element && element.querySelector(".xterm-screen");
  const box = screen ? screen.getBoundingClientRect() : null;
  if (box && box.width > 0 && box.height > 0 && tab.term.cols && tab.term.rows) {
    return { cw: box.width / tab.term.cols, ch: box.height / tab.term.rows };
  }

  const measure = element && element.querySelector(".xterm-char-measure-element");
  const style = element ? getComputedStyle(element) : null;
  const fontPx = parseFloat((style && style.fontSize) || "14") || 14;
  const rawLine = style && style.lineHeight;
  const linePx = rawLine && rawLine !== "normal" ? parseFloat(rawLine) : fontPx * 1.2;
  const sample = measure ? (measure.textContent || "").length : 0;
  const width = measure ? measure.getBoundingClientRect().width / Math.max(1, sample) : 0;
  return { cw: width || fontPx * 0.6, ch: linePx || fontPx * 1.2 };
}

// The dot pads are text, so their advance width is not necessarily the same
// as xterm's cell width (especially on iOS, where the font fallback differs).
// Copy xterm's font and measure a real run of dots, otherwise a bottom strip
// sized in cells ends several columns short in pixels.
function padStyle(tab, fontPx, ch) {
  const termStyle = getComputedStyle(tab.term.element);
  return `font-family:${termStyle.fontFamily};font-size:${fontPx}px;line-height:${ch}px;`;
}

function measureDotWidth(tab, style) {
  const probe = document.createElement("pre");
  probe.textContent = ".".repeat(32);
  probe.style.cssText = `position:absolute;visibility:hidden;white-space:pre;margin:0;${style}`;
  document.body.appendChild(probe);
  const width = probe.getBoundingClientRect().width / 32;
  probe.remove();
  return width > 0 ? width : cellMetrics(tab).cw;
}

function applyPadding(tab) {
  const padRight = tab.padRight;
  const padBottom = tab.padBottom;
  if (!state.webConnected || state.clientBlocked) {
    padRight.hidden = true;
    padBottom.hidden = true;
    return;
  }

  const rect = panes.getBoundingClientRect();
  const { cw, ch } = cellMetrics(tab);
  const visibleCols = Math.max(0, Math.floor((rect.width - 8) / cw));
  const visibleRows = Math.max(0, Math.floor((rect.height - 8) / ch));
  const extraCols = visibleCols - tab.term.cols;
  const extraRows = visibleRows - tab.term.rows;
  if (extraCols <= 0 && extraRows <= 0) {
    padRight.hidden = true;
    padBottom.hidden = true;
    return;
  }

  const termStyle = getComputedStyle(tab.term.element);
  const fontPx = parseFloat(termStyle.fontSize) || 14;
  const style = padStyle(tab, fontPx, ch);
  const dotWidth = measureDotWidth(tab, style);
  if (extraCols > 0) {
    const rightWidth = extraCols * cw;
    const rightCols = Math.max(1, Math.ceil(rightWidth / dotWidth));
    padRight.style.cssText =
      `left:${4 + tab.term.cols * cw}px;top:4px;` +
      `width:${rightWidth}px;overflow:hidden;${style}`;
    padRight.textContent = Array.from(
      { length: tab.term.rows },
      () => ".".repeat(rightCols)
    ).join("\n");
    padRight.hidden = false;
  } else {
    padRight.hidden = true;
  }
  if (extraRows > 0) {
    // Cover the WHOLE scrollable surface, not just the currently visible
    // columns, so horizontally scrolled-to areas still have dots. Size by
    // pixels and overflow:hidden so a partial cell cannot create a scrollbar.
    const stripWidth = Math.max(tab.term.cols * cw, rect.width - 8);
    const stripCols = Math.max(1, Math.ceil(stripWidth / dotWidth));
    padBottom.style.cssText =
      `left:4px;top:${4 + tab.term.rows * ch}px;` +
      `width:${stripWidth}px;overflow:hidden;${style}`;
    padBottom.textContent = Array.from(
      { length: extraRows },
      () => ".".repeat(stripCols)
    ).join("\n");
    padBottom.hidden = false;
  } else {
    padBottom.hidden = true;
  }
}

function fitTab(tab) {
  // The web grid is ALWAYS the native PTY grid. The screen.ansi repaint is
  // rendered for exactly dto.cols x dto.rows, so mirroring that grid 1:1 keeps
  // the cursor and every cell aligned. Letting the browser pick its own grid
  // made fast typing look like duplicated/stuck characters (lsl) because the
  // full repaint landed on a differently sized buffer.
  const cols = Math.max(1, Number(tab.dto.cols) || 80);
  const rows = Math.max(1, Number(tab.dto.rows) || 24);

  if (cols !== tab.term.cols || rows !== tab.term.rows) {
    armScrollGuard(tab);
    tab.term.resize(cols, rows);
    // resize() pushes the old rows into xterm's scrollback. The next poll's
    // ED 3 repaint clears it, but pin the viewport now so the resize itself
    // never leaves a stale-history view on screen.
    if (typeof tab.term.scrollToBottom === "function") tab.term.scrollToBottom();
  }

  // "窗口同步 PC" only decides whether this window's size is pushed to the
  // native PTY. The web view itself still follows whatever grid the PTY has;
  // a size change reaches it through dto on the next refresh. Extra space is
  // handled by dot padding / scrolling, never by term.resize.
  //
  // The soft keyboard only changes what is visible; resizing the PTY then
  // would redraw running programs on every keystroke. On touch viewports the
  // native column count is also authoritative, so only height may follow the
  // browser when the user explicitly enabled syncing.
  if (state.webFit && keyboardInset === 0 && keyboardTimer === null) {
    const rect = panes.getBoundingClientRect();
    if (rect.width && rect.height) {
      const { cw, ch } = cellMetrics(tab);
      const wantCols = coarsePointer
        ? cols
        : Math.max(20, Math.min(300, Math.floor((rect.width - 8) / cw)));
      const wantRows = Math.max(5, Math.min(200, Math.floor((rect.height - 8) / ch)));
      if (wantCols !== cols || wantRows !== rows) {
        api(`/tabs/${tab.id}/resize`, {
          method: "POST",
          body: JSON.stringify({ cols: wantCols, rows: wantRows })
        }).catch(report);
      }
    }
  }
  applyPadding(tab);
}

function stopPolling(tab) {
  if (tab.timer !== null) {
    clearTimeout(tab.timer);
    tab.timer = null;
  }
}

async function poll(tab) {
  if (state.clientBlocked || state.active !== tab.id) return;
  try {
    const raw = await api(`/tabs/${tab.id}/screen.ansi`);
    if (state.active !== tab.id || !raw) return;
    const alt = raw[0] === "1";
    const ansi = raw.slice(1);
    if (tab.alt !== alt) {
      tab.term.write(alt ? "\x1b[?1049h" : "\x1b[?1049l");
      tab.alt = alt;
    }
    // Arm the guard before write: xterm may scroll its own viewport while
    // repainting, and that must not look like a user scroll. Only re-centre
    // on the cursor while we are still following it.
    if (tab.followCursor) scrollToCursor(tab);
    tab.term.write(ansi);
    if (tab.followCursor) scrollToCursor(tab);
    clearError();
    statusEl.textContent = `tab ${tab.id} · ${tab.term.cols}×${tab.term.rows}`;
  } catch (error) {
    report(error);
  }
  if (state.active === tab.id) {
    tab.timer = setTimeout(() => poll(tab), 100);
  }
}

function updateTabButtons() {
  for (const [id, tab] of state.tabs) {
    tab.button.classList.toggle("active", id === state.active);
    tab.title.textContent = tab.dto.title || `tab ${id}`;
  }
}

function activate(id, fromUser) {
  if (state.active === id) return;
  const previous = currentTab();
  if (previous) stopPolling(previous);

  state.active = id;
  for (const [tabId, tab] of state.tabs) {
    tab.wrapper.hidden = tabId !== id;
  }
  updateTabButtons();

  const next = state.tabs.get(id);
  if (next) {
    fitTab(next);
    applyPadding(next);
    if (next.followCursor) scrollToCursor(next);
    poll(next);
  }
  if (fromUser) {
    api(`/tabs/${id}/activate`, { method: "POST" })
      .then(refresh)
      .catch(report);
  }
}

function destroyTab(tab) {
  stopPolling(tab);
  tab.button.remove();
  tab.wrapper.remove();
  state.tabs.delete(tab.id);
}

function createTab(dto) {
  const id = String(dto.id);
  const wrapper = document.createElement("div");
  wrapper.className = "pane";
  wrapper.hidden = true;
  panes.appendChild(wrapper);

  const term = makeTerm(dto.cols || 80, dto.rows || 24);
  term.open(wrapper);

  const padRight = document.createElement("pre");
  const padBottom = document.createElement("pre");
  padRight.className = "dot-pad";
  padBottom.className = "dot-pad";
  padRight.hidden = true;
  padBottom.hidden = true;
  wrapper.append(padRight, padBottom);

  const button = document.createElement("div");
  button.className = "tab";
  const title = document.createElement("span");
  title.className = "title";
  const close = document.createElement("button");
  close.className = "close";
  close.type = "button";
  close.title = "Close tab";
  close.textContent = "×";
  button.append(title, close);
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    if (event.target.closest(".close")) {
      api(`/tabs/${id}`, { method: "DELETE" })
        .then(refresh)
        .catch(report);
    } else {
      activate(id, true);
    }
  });
  tabStrip.appendChild(button);

  const tab = {
    id,
    dto,
    term,
    wrapper,
    button,
    title,
    padRight,
    padBottom,
    timer: null,
    alt: null,
    inputChain: Promise.resolve(),
    followCursor: true,
    scrollGuard: false,
    scrollGuardTimer: null
  };
  state.tabs.set(id, tab);

  wrapper.addEventListener("scroll", () => {
    if (tab.scrollGuard) return;
    tab.followCursor = false;
  });
  if (typeof term.onScroll === "function") {
    term.onScroll(() => {
      if (tab.scrollGuard) return;
      tab.followCursor = false;
    });
  }
  term.onData((data) => sendInput(tab, encodeTyped(data)));
  return tab;
}

async function refresh() {
  if (state.refreshing) return;
  state.refreshing = true;
  try {
    const info = await api("/tabs");
    const ids = new Set(info.tabs.map((dto) => String(dto.id)));
    for (const [id, tab] of [...state.tabs]) {
      if (!ids.has(id)) destroyTab(tab);
    }
    for (const dto of info.tabs) {
      const id = String(dto.id);
      const existing = state.tabs.get(id);
      if (existing) existing.dto = dto;
      else createTab(dto);
    }
    const active = info.active === null || info.active === undefined ? null : String(info.active);
    if (info.tabs.length === 0) {
      state.active = null;
      emptyState.hidden = false;
      statusEl.textContent = "no tabs";
      clearError();
      return;
    }
    emptyState.hidden = true;
    if (active !== state.active) activate(active, false);
    else updateTabButtons();
    // Re-sync the visible tab with the latest native dimensions, or re-fit it
    // to this viewport when "Web 尺寸优先" is enabled.
    const activeTab = currentTab();
    if (activeTab) {
      fitTab(activeTab);
      applyPadding(activeTab);
    }
    if (!state.clientBlocked) notifyClient(true);
    clearError();
  } catch (error) {
    report(error);
  } finally {
    state.refreshing = false;
  }
}

function createTabViaApi() {
  api("/tabs", { method: "POST" })
    .then(refresh)
    .catch(report);
}

document.getElementById("new-tab").addEventListener("click", createTabViaApi);
document.getElementById("empty-new-tab").addEventListener("click", createTabViaApi);

document.getElementById("refresh").addEventListener("click", () => refresh());

toolbarToggle.addEventListener("click", () => setToolbar(!state.showToolbar));

webFitToggle.addEventListener("change", () => {
  state.webFit = webFitToggle.checked;
  panes.classList.toggle("host-size", !state.webFit);
  try {
    localStorage.setItem("rterminal.webFit", state.webFit ? "1" : "0");
  } catch {
    // Storage may be disabled; the toggle still works for this session.
  }
  const tab = currentTab();
  if (tab) {
    fitTab(tab);
    applyPadding(tab);
  }
});

document.getElementById("scrollback").addEventListener("click", async () => {
  if (!scrollbackView.hidden) {
    scrollbackView.hidden = true;
    return;
  }
  const tab = currentTab();
  if (!tab) return;
  try {
    scrollbackView.textContent = await api(`/tabs/${tab.id}/scrollback`);
    scrollbackView.hidden = false;
  } catch (error) {
    report(error);
  }
});

let resizeTimer = null;
function scheduleFit() {
  if (resizeTimer !== null) clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    resizeTimer = null;
    syncViewportHeight();
    const tab = currentTab();
    if (tab) {
      fitTab(tab);
      applyPadding(tab);
      // When the keyboard covers the bottom, pushing the live rows above it
      // is all we do: no PTY resize, no native window resize.
      revealBottom(tab);
    }
  }, 80);
}

new ResizeObserver(scheduleFit).observe(panes);
window.addEventListener("resize", scheduleFit);
window.addEventListener("orientationchange", scheduleFit);

// Mobile browsers keep the layout viewport fixed and shrink only the visual
// viewport when the soft keyboard opens. Keep --app-height in sync so the
// terminal surface can scroll, but never let that height change resize the
// PTY. A "normal" height baseline keeps browser-chrome jitter (a few pixels)
// in the ordinary window-resize path.
function syncViewportHeight() {
  if (!visualViewport) return;
  if (visualViewport.scale > 1.01) {
    // Leave the layout alone while zoomed; iOS may still allow pinch zoom
    // despite the viewport meta tag. Reset the keyboard state so zooming
    // cannot leave a stale "keyboard open" flag behind.
    keyboardInset = 0;
    return;
  }
  const inset = keyboardInsetPx();
  const wasActive = keyboardInset > 0;
  keyboardInset = inset;

  document.documentElement.style.setProperty(
    "--app-height",
    `${Math.max(1, Math.floor(visualViewport.height))}px`
  );

  if (inset > 0) {
    if (keyboardTimer !== null) {
      clearTimeout(keyboardTimer);
      keyboardTimer = null;
    }
    revealBottom(currentTab());
    return;
  }

  if (wasActive) {
    // Keyboard closed: wait for the layout to settle before allowing another
    // window -> PTY resize, otherwise closing it replays the resize we
    // deliberately skipped on open.
    if (keyboardTimer !== null) clearTimeout(keyboardTimer);
    keyboardTimer = setTimeout(() => {
      keyboardTimer = null;
      normalViewportHeight = visualViewport.height || window.innerHeight;
      scheduleFit();
    }, 200);
  }
}

if (visualViewport) {
  syncViewportHeight();
  visualViewport.addEventListener("resize", () => {
    syncViewportHeight();
    revealBottom(currentTab());
    scheduleFit();
  });
  visualViewport.addEventListener("scroll", () => {
    syncViewportHeight();
    scheduleFit();
  });
} else {
  window.addEventListener("resize", () => {
    normalViewportHeight = window.innerHeight;
  });
}
if (document.fonts && document.fonts.ready) document.fonts.ready.then(scheduleFit);
if (document.fonts && document.fonts.load) {
  document.fonts
    .load('14px "rterm Powerline"', "\ue0b0\ue0b2\ue0b4\ue0b6")
    .then(scheduleFit)
    .catch(() => {});
}

// Materialise the toolbar once and apply the persisted visibility. body's
// grid template switches to the 5-row layout only when it is shown.
buildToolbar();
keyToolbar.hidden = !state.showToolbar;
document.body.classList.toggle("with-toolbar", state.showToolbar);

refresh();
setInterval(refresh, 1000);
