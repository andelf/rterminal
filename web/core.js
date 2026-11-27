"use strict";

// Built-in xterm.js client for rterminal's HTTP control API.
// Tabs here are a mirror of the native tab list: switching/creating/closing
// all go through the same stable tab ids the native window uses.

const tabStrip = document.getElementById("tabs");
const actionsMenu = document.getElementById("actions");
const toolsToggle = document.getElementById("tools-toggle");
const panes = document.getElementById("panes");
const statusEl = document.getElementById("status");
const statusBar = document.querySelector(".statusbar");
const connectionBadge = document.getElementById("connection-badge");
const connectionLabel = document.getElementById("connection-label");
const stealClientButton = document.getElementById("steal-client");
const stagedInputForm = document.getElementById("staged-input-form");
const stagedInput = document.getElementById("staged-input");
const scrollbackView = document.getElementById("scrollback-view");
const emptyState = document.getElementById("empty-state");
const sizeModeButton = document.getElementById("size-mode");
const syncSizeButton = document.getElementById("sync-size");
const keyboardModeButton = document.getElementById("keyboard-mode");
const screenKeyboard = document.getElementById("screen-keyboard");
const skLayoutHolder = document.getElementById("sk-layout");
const skLayoutToggle = document.getElementById("sk-layout-toggle");
const toolbarToggle = document.getElementById("toggle-toolbar");
const keyToolbar = document.getElementById("key-toolbar");
const keyScroll = document.getElementById("key-scroll");
const keyNav = document.getElementById("key-nav");
const debugButton = document.getElementById("debug");
const authOverlay = document.getElementById("auth-overlay");
const authForm = document.getElementById("auth-form");
const authToken = document.getElementById("auth-token");
const authError = document.getElementById("auth-error");
const helpButton = document.getElementById("help-button");
const helpOverlay = document.getElementById("help-overlay");
const helpCloseButton = document.getElementById("help-close");

function savedSizeMode() {
  // "pc" (default): the native window decides the grid; this view mirrors it
  // and pads/scrolls around it. "browser": this viewport decides the grid and
  // the native window is resized to match, so neither side scrolls.
  try {
    return localStorage.getItem("rterminal.sizeMode") === "browser"
      ? "browser"
      : "pc";
  } catch {
    return "pc";
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

function savedKeyboardMode() {
  // "system" (default) keeps the browser's own keyboard, IME and dictation.
  // "screen" uses the built-in keyboard and deliberately does not focus
  // xterm's helper textarea, so the OS soft keyboard stays out of the way.
  try {
    return localStorage.getItem("rterminal.keyboardMode") === "screen"
      ? "screen"
      : "system";
  } catch {
    return "system";
  }
}

function savedScreenLayout() {
  // The built-in keyboard can mimic either a PC or an Apple key cap layout.
  // Start from the host platform, but remember an explicit choice forever.
  try {
    const saved = localStorage.getItem("rterminal.screenLayout");
    if (saved === "win" || saved === "mac") return saved;
  } catch {}
  const platform = navigator.platform || "";
  return /Mac|iPhone|iPad|iPod/i.test(platform) ? "mac" : "win";
}

const state = {
  tabs: new Map(),
  active: null,
  refreshing: false,
  sizeMode: savedSizeMode(),
  // Browser-size mode starts in auto-follow. Sync size takes one explicit
  // snapshot and then holds that grid until the next click or PC-size mode.
  sizeLocked: false,
  keyboardMode: savedKeyboardMode(),
  screenLayout: savedScreenLayout(),
  showToolbar: savedToolbar(),
  // Toolbar modifiers have three states, matching a hardware keyboard and
  // the platform on-screen keyboards: off / one-shot (applies to the next
  // input only) / locked (double tap, applies until explicitly released).
  mods: { shift: "off", ctrl: "off", alt: "off", meta: "off" },
  webConnected: false,
  clientBlocked: false,
  // WebSocket data plane. HTTP remains the control plane.
  wsPort: 7879
};
function applySizeModeUi() {
  const browser = state.sizeMode === "browser";
  sizeModeButton.setAttribute("aria-label", browser
    ? `Browser size ${state.sizeLocked ? "locked" : "auto-follow"}; switch to PC size`
    : "PC size active; switch to browser size");
  sizeModeButton.setAttribute("aria-pressed", String(browser));
  syncSizeButton.hidden = !browser;
  syncSizeButton.setAttribute("aria-label", state.sizeLocked
    ? "Browser size locked; sync latest size and lock again"
    : "Sync current browser size and lock it");
  syncSizeButton.title = state.sizeLocked
    ? "Size is locked. Click to sync the latest browser grid and keep it locked again."
    : "Sync the current browser grid once and keep it fixed until the next click.";
}
applySizeModeUi();
toolbarToggle.setAttribute("aria-pressed", String(state.showToolbar));

// Command buttons act without stealing the editor's focus or dismissing the
// OS keyboard. Keep native click and keyboard activation intact; modal Help
// and keyboard-mode focus handling deliberately use their own rules.
for (const id of [
  "new-tab",
  "empty-new-tab",
  "scrollback",
  "refresh",
  "toggle-toolbar",
  "debug",
  "size-mode",
  "sync-size",
  "staged-input-send"
]) {
  const button = document.getElementById(id);
  let suppressTrustedClickUntil = 0;
  button.addEventListener("click", (event) => {
    if (event.isTrusted && performance.now() < suppressTrustedClickUntil) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, true);
  button.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    if (event.pointerType === "touch") {
      suppressTrustedClickUntil = performance.now() + 500;
    }
    event.preventDefault();
  });
  button.addEventListener("pointerup", (event) => {
    if (event.button !== 0 || event.pointerType !== "touch") return;
    // iOS/Chrome may suppress the compatibility click after pointerdown was
    // cancelled. Complete the touch explicitly without allowing a duplicate.
    event.preventDefault();
    button.click();
  });
}

function closeHelp() {
  helpOverlay.hidden = true;
  helpButton.focus({ preventScroll: true });
}
helpButton.addEventListener("click", () => {
  helpOverlay.hidden = false;
  helpCloseButton.focus({ preventScroll: true });
});
helpCloseButton.addEventListener("click", closeHelp);
helpOverlay.addEventListener("click", (event) => {
  if (event.target === helpOverlay) closeHelp();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !helpOverlay.hidden) {
    event.preventDefault();
    closeHelp();
  }
});

// Phones keep a stable layout viewport while the on-screen keyboard shrinks
// only the visual viewport. Treat that as a viewport pan, not a terminal
// resize: the PTY grid and the native window stay put, and the live bottom is
// scrolled into the visible strip instead.
const visualViewport = window.visualViewport;
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

async function notifyClient(connected, force = false) {
  const tab = currentTab();
  const body = JSON.stringify({
    id: clientId,
    connected,
    cols: tab ? tab.term.cols : 80,
    rows: tab ? tab.term.rows : 24,
    browser_size: state.sizeMode === "browser",
    force
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
    if (connected) state.clientBlocked = false;
    stealClientButton.hidden = true;
    syncConnection();
    return true;
  } catch (error) {
    if (connected && /409|already connected/.test(String(error))) {
      statusEl.textContent = "Another web client is connected";
      stealClientButton.hidden = false;
      const active = currentTab();
      if (active) stopPolling(active);
      if (!state.clientBlocked) {
        // A transient duplicate registration must not lock this page out
        // forever: retry once the server's 5 s stale-client window has passed.
        state.clientBlocked = true;
        syncConnection();
        setTimeout(() => {
          state.clientBlocked = false;
          refresh();
        }, 6000);
      }
    } else if (connected) {
      report(error);
    }
  }
  return false;
}

stealClientButton.addEventListener("click", async () => {
  stealClientButton.disabled = true;
  try {
    if (await notifyClient(true, true)) {
      // Invalidate every old socket/reconnect before attaching the new owner.
      for (const tab of state.tabs.values()) stopPolling(tab);
      await refresh();
      const active = currentTab();
      if (active) startStream(active);
    }
  } finally {
    stealClientButton.disabled = false;
  }
});

window.addEventListener("beforeunload", () => {
  const body = JSON.stringify({
    id: clientId,
    connected: false,
    browser_size: state.sizeMode === "browser"
  });
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

// Transport indicator. WS is the live PTY data plane; HTTP is the degraded
// snapshot-poll path where every keystroke is a separate POST.
const CONNECTION_MODES = {
  ws: { label: "WS", title: "WebSocket PTY stream active; keystrokes go over the socket" },
  reconnect: { label: "Retry", title: "WebSocket dropped; retrying" },
  http: { label: "HTTP", title: "HTTP polling fallback: full snapshot every 100ms, one POST per keystroke" },
  blocked: { label: "Blocked", title: "Another web client owns this terminal" },
  connecting: { label: "Connecting", title: "Establishing the live stream" },
  idle: { label: "Idle", title: "No terminal tabs" },
  offline: { label: "Offline", title: "Cannot reach the rterminal server" }
};

function setConnection(mode) {
  const info = CONNECTION_MODES[mode] || CONNECTION_MODES.connecting;
  if (connectionBadge.dataset.mode !== mode) connectionBadge.dataset.mode = mode;
  if (connectionLabel.textContent !== info.label) connectionLabel.textContent = info.label;
  connectionBadge.title = info.title;
  connectionBadge.setAttribute("aria-label", `Connection: ${info.label}. ${info.title}`);
}

function syncConnection() {
  if (state.clientBlocked) return setConnection("blocked");
  const tab = currentTab();
  if (!tab) return setConnection("idle");
  if (tab.socket && tab.socket.readyState === WebSocket.OPEN) return setConnection("ws");
  if (tab.reconnectTimer !== null || tab.wsFailures > 0) return setConnection("reconnect");
  if (tab.timer !== null) return setConnection("http");
  setConnection("connecting");
}

// On narrow viewports the tool row becomes a popup so the tabs keep the width.
function closeToolsMenu() {
  actionsMenu.classList.remove("open");
  toolsToggle.setAttribute("aria-expanded", "false");
}

toolsToggle.addEventListener("click", () => {
  const open = !actionsMenu.classList.contains("open");
  actionsMenu.classList.toggle("open", open);
  toolsToggle.setAttribute("aria-expanded", String(open));
});

actionsMenu.addEventListener("click", (event) => {
  if (event.target instanceof Element && event.target.closest("button")) closeToolsMenu();
});

document.addEventListener("pointerdown", (event) => {
  if (!actionsMenu.classList.contains("open")) return;
  const target = event.target instanceof Element ? event.target : null;
  if (target && target.closest("#actions, #tools-toggle")) return;
  closeToolsMenu();
});

window.addEventListener("resize", () => {
  if (window.innerWidth > 560) closeToolsMenu();
});

// ---------------------------------------------------------------------------
// Input debug recorder
// ---------------------------------------------------------------------------
// IME and dictation problems only reproduce on a real phone, where there is no
// devtools console. While this recorder is on, every relevant DOM event and
// every byte we hand to the PTY stream to `POST /web/debug`, which appends it
// to `~/.rterminal/web-debug/web-debug.log` (also readable through
// `GET /web/debug/log`). Nothing is summarised locally: the log is the ground
// truth for what the browser actually reported.

let webAuthorized = false;
