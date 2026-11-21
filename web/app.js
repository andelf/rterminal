"use strict";

// Built-in xterm.js client for rterminal's HTTP control API.
// Tabs here are a mirror of the native tab list: switching/creating/closing
// all go through the same stable tab ids the native window uses.

const tabStrip = document.getElementById("tabs");
const panes = document.getElementById("panes");
const statusEl = document.getElementById("status");
const statusBar = document.querySelector(".statusbar");
const scrollbackView = document.getElementById("scrollback-view");

const state = { tabs: new Map(), active: null, refreshing: false };

function report(error) {
  const message = error instanceof Error ? error.message : String(error);
  statusEl.textContent = `⚠ ${message}`;
  statusBar.classList.add("error");
  console.error(error);
}

function clearError() {
  statusBar.classList.remove("error");
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

function makeTerm(cols, rows) {
  return new Terminal({
    cols,
    rows,
    fontFamily: '"Cascadia Mono", Consolas, "Segoe UI Symbol", monospace',
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
  const measure = element && element.querySelector(".xterm-char-measure-element");
  const style = element ? getComputedStyle(element) : null;
  const fontPx = parseFloat((style && style.fontSize) || "14") || 14;
  const rawLine = style && style.lineHeight;
  const linePx = rawLine && rawLine !== "normal" ? parseFloat(rawLine) : fontPx * 1.2;
  const width = measure ? measure.getBoundingClientRect().width : 0;
  return { cw: width || fontPx * 0.6, ch: linePx || fontPx * 1.2 };
}

function fitTab(tab, syncNative) {
  const rect = panes.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const { cw, ch } = cellMetrics(tab);
  const cols = Math.max(20, Math.min(300, Math.floor((rect.width - 8) / cw)));
  const rows = Math.max(5, Math.min(200, Math.floor((rect.height - 8) / ch)));
  if (cols === tab.term.cols && rows === tab.term.rows) return;

  tab.term.resize(cols, rows);
  tab.term.write("\x1b[2J\x1b[H");
  if (syncNative) {
    api(`/tabs/${tab.id}/resize`, {
      method: "POST",
      body: JSON.stringify({ cols, rows })
    }).catch(report);
  }
}

function stopPolling(tab) {
  if (tab.timer !== null) {
    clearTimeout(tab.timer);
    tab.timer = null;
  }
}

async function poll(tab) {
  if (state.active !== tab.id) return;
  try {
    const raw = await api(`/tabs/${tab.id}/screen.ansi`);
    if (state.active !== tab.id || !raw) return;
    const alt = raw[0] === "1";
    const ansi = raw.slice(1);
    if (tab.alt !== alt) {
      tab.term.write(alt ? "\x1b[?1049h" : "\x1b[?1049l");
      tab.alt = alt;
    }
    tab.term.write(ansi);
    clearError();
    statusEl.textContent = `tab ${tab.id} · ${tab.term.cols}×${tab.term.rows} · synced`;
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
    fitTab(next, true);
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
    timer: null,
    alt: null
  };
  state.tabs.set(id, tab);
  term.onData((data) => {
    api(`/tabs/${id}/input`, { method: "POST", body: data }).catch(report);
  });
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
    if (active !== state.active) activate(active, false);
    else updateTabButtons();
    clearError();
  } catch (error) {
    report(error);
  } finally {
    state.refreshing = false;
  }
}

document.getElementById("new-tab").addEventListener("click", () => {
  api("/tabs", { method: "POST" })
    .then(refresh)
    .catch(report);
});

document.getElementById("refresh").addEventListener("click", () => refresh());

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
new ResizeObserver(() => {
  if (resizeTimer !== null) clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    resizeTimer = null;
    const tab = currentTab();
    if (tab) fitTab(tab, true);
  }, 150);
}).observe(panes);

refresh();
setInterval(refresh, 1000);
