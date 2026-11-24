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
const sizeModeButton = document.getElementById("size-mode");
const sizeModeLabel = document.getElementById("size-mode-label");
const keyboardModeButton = document.getElementById("keyboard-mode");
const keyboardModeLabel = document.getElementById("keyboard-mode-label");
const screenKeyboard = document.getElementById("screen-keyboard");
const skLayoutHolder = document.getElementById("sk-layout");
const skLayoutToggle = document.getElementById("sk-layout-toggle");
const toolbarToggle = document.getElementById("toggle-toolbar");
const keyToolbar = document.getElementById("key-toolbar");
const keyScroll = document.getElementById("key-scroll");
const keyNav = document.getElementById("key-nav");
const debugButton = document.getElementById("debug");

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
  wsPort: 7879,
  wsFailures: 0
};
function applySizeModeUi() {
  const browser = state.sizeMode === "browser";
  sizeModeLabel.textContent = browser ? "浏览器尺寸" : "PC 尺寸";
  sizeModeButton.setAttribute("aria-pressed", String(browser));
}
applySizeModeUi();
toolbarToggle.setAttribute("aria-pressed", String(state.showToolbar));

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

async function notifyClient(connected) {
  const tab = currentTab();
  const body = JSON.stringify({
    id: clientId,
    connected,
    cols: tab ? tab.term.cols : 80,
    rows: tab ? tab.term.rows : 24,
    browser_size: state.sizeMode === "browser"
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

// ---------------------------------------------------------------------------
// Input debug recorder
// ---------------------------------------------------------------------------
// IME and dictation problems only reproduce on a real phone, where there is no
// devtools console. While this recorder is on, every relevant DOM event and
// every byte we hand to the PTY stream to `POST /web/debug`, which appends it
// to `~/.rterminal/web-debug/web-debug.log` (also readable through
// `GET /web/debug/log`). Nothing is summarised locally: the log is the ground
// truth for what the browser actually reported.
const textEncoder = new TextEncoder();

const debug = {
  on: false,
  announced: false,
  seq: 0,
  queue: [],
  timer: null,
  session: newDebugSession()
};

try {
  // On by default while the client is under active development: a forgotten
  // tap would mean redoing the whole phone test. The button turns red so it is
  // never a secret, and one tap stops it for the session and beyond.
  debug.on = localStorage.getItem("rterminal.debug") !== "0";
} catch {
  // Private mode: recording still works, it just is not remembered.
  debug.on = true;
}

function newDebugSession() {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
}

// Clipping counts code points so a CJK / emoji log line is never cut in half.
function clip(value, max = 160) {
  if (typeof value !== "string") {
    return value === undefined || value === null ? null : String(value);
  }
  const chars = Array.from(value);
  if (chars.length <= max) return value;
  return `${chars.slice(0, max).join("")}…(${chars.length})`;
}

function hexOf(text) {
  if (typeof text !== "string") return "";
  return Array.from(textEncoder.encode(text), (byte) =>
    byte.toString(16).padStart(2, "0")
  ).join(" ");
}

function modString(event) {
  let out = "";
  if (event.shiftKey) out += "S";
  if (event.ctrlKey) out += "C";
  if (event.altKey) out += "A";
  if (event.metaKey) out += "M";
  return out;
}

function dbg(kind, tab, fields) {
  if (!debug.on) return;
  const entry = {
    s: debug.session,
    n: ++debug.seq,
    k: kind,
    t: Date.now(),
    up: Math.round(performance.now())
  };
  if (tab) entry.tab = tab.id;
  if (fields) {
    for (const key of Object.keys(fields)) entry[key] = fields[key];
  }
  let line;
  try {
    line = JSON.stringify(entry);
  } catch {
    line = JSON.stringify({ s: debug.session, n: entry.n, k: kind, err: "unserialisable" });
  }
  debug.queue.push(line);
  if (debug.queue.length >= 32) flushDebug(false);
  else if (debug.timer === null) debug.timer = setTimeout(() => flushDebug(false), 400);
}

function flushDebug(beacon) {
  if (debug.timer !== null) {
    clearTimeout(debug.timer);
    debug.timer = null;
  }
  if (debug.queue.length === 0) return;
  const body = `${debug.queue.join("\n")}\n`;
  debug.queue = [];
  if (beacon) {
    // sendBeacon survives the page going away, which is exactly when a
    // dictation bug tends to happen (keyboard dismissal, tab switch).
    navigator.sendBeacon("/web/debug", new Blob([body], { type: "application/x-ndjson" }));
    return;
  }
  fetch("/web/debug", {
    method: "POST",
    headers: { "Content-Type": "application/x-ndjson" },
    body,
    keepalive: body.length < 60000
  }).catch(() => {
    debug.queue.push(...body.trimEnd().split("\n"));
  });
}

function debugHeaders() {
  return debug.on ? { "X-Rterminal-Debug": "1" } : undefined;
}

function sessionFacts(tab) {
  const viewport = window.visualViewport;
  return {
    ua: navigator.userAgent,
    plt: navigator.platform || "",
    touch: navigator.maxTouchPoints || 0,
    coarse: window.matchMedia ? window.matchMedia("(pointer: coarse)").matches : null,
    iw: window.innerWidth,
    ih: window.innerHeight,
    dpr: window.devicePixelRatio,
    vw: viewport ? Math.round(viewport.width) : null,
    vh: viewport ? Math.round(viewport.height) : null,
    scale: viewport ? viewport.scale : null,
    // Document geometry, so a "the page scrolls for no reason" report can be
    // checked from the log alone: the two numbers must stay equal.
    doc: (() => {
      const el = document.scrollingElement || document.documentElement;
      return [el.scrollHeight, el.clientHeight];
    })(),
    kb: keyboardInset,
    mode: state.sizeMode,
    cols: tab ? tab.term.cols : null,
    rows: tab ? tab.term.rows : null
  };
}

function announceDebug() {
  if (!debug.on || debug.announced) return;
  debug.announced = true;
  dbg("session", currentTab(), sessionFacts(currentTab()));
  flushDebug(false);
}

// Text the hidden textarea currently holds. On iOS dictation this keeps the
// whole accumulated sentence, which is why the same prefix can be committed
// again; recording it next to each input event makes that visible.
function textareaEl(tab) {
  if (!tab.term.element) return null;
  return tab.term.element.querySelector(".xterm-helper-textarea");
}

function textareaText(tab) {
  const el = textareaEl(tab);
  return el ? el.value : null;
}

function textareaValue(tab) {
  const text = textareaText(tab);
  return text === null ? null : clip(text, 200);
}

function cursorLine(tab) {
  try {
    const buffer = tab.term.buffer.active;
    const line = buffer.getLine(buffer.baseY + buffer.cursorY);
    return line ? clip(line.translateToString(true), 120) : null;
  } catch {
    return null;
  }
}

// Independent capture listeners: the recorder observes the platform, it does
// not participate in gating, so it can never change what the app does.
function bindDebug(tab, textarea) {
  const target = textarea || tab.wrapper;
  target.addEventListener("keydown", (event) => {
    dbg("keydown", tab, {
      key: event.key,
      code: event.code,
      kc: event.keyCode,
      rep: event.repeat,
      ime: event.isComposing,
      loc: event.location,
      mod: modString(event)
    });
  }, true);
  target.addEventListener("keypress", (event) => {
    dbg("keypress", tab, { key: event.key, kc: event.keyCode, ime: event.isComposing });
  }, true);
  target.addEventListener("compositionstart", (event) => {
    dbg("comp", tab, { p: "start", data: clip(event.data), value: textareaValue(tab) });
  }, true);
  target.addEventListener("compositionupdate", (event) => {
    dbg("comp", tab, { p: "update", data: clip(event.data), value: textareaValue(tab) });
  }, true);
  target.addEventListener("compositionend", (event) => {
    dbg("comp", tab, { p: "end", data: clip(event.data), value: textareaValue(tab) });
  }, true);
  target.addEventListener("beforeinput", (event) => {
    dbg("beforeinput", tab, {
      it: event.inputType || "",
      data: clip(event.data),
      ime: !!event.isComposing,
      value: textareaValue(tab)
    });
  }, true);
  target.addEventListener("input", (event) => {
    dbg("input", tab, {
      it: event.inputType || "",
      data: clip(event.data),
      ime: !!event.isComposing,
      hex: hexOf(event.data),
      value: textareaValue(tab)
    });
  }, true);
  target.addEventListener("paste", (event) => {
    const text = event.clipboardData ? event.clipboardData.getData("text") : "";
    dbg("paste", tab, { text: clip(text), hex: hexOf(text) });
  }, true);
  target.addEventListener("focus", () => dbg("focus", tab, { value: textareaValue(tab) }), true);
  target.addEventListener("blur", () => dbg("blur", tab, { value: textareaValue(tab) }), true);
}

function applyDebugUi() {
  debugButton.setAttribute("aria-pressed", String(debug.on));
  debugButton.classList.toggle("on", debug.on);
  if (debug.on) {
    announceDebug();
    statusEl.textContent = "调试记录已开启（回传服务端日志）";
  }
}

debugButton.addEventListener("click", () => {
  debug.on = !debug.on;
  try {
    localStorage.setItem("rterminal.debug", debug.on ? "1" : "0");
  } catch {
    // Nothing to persist to; the session still records.
  }
  if (debug.on) {
    // New session per enable so one test run is one contiguous block in the log.
    debug.session = newDebugSession();
    debug.seq = 0;
    debug.announced = false;
    announceDebug();
  } else {
    flushDebug(true);
    debug.announced = false;
    statusEl.textContent = "调试记录已停止";
  }
  applyDebugUi();
  // The byte trace flag is part of the WebSocket URL, so reconnect to apply it.
  const active = currentTab();
  if (active && active.socket) startStream(active);
});

applyDebugUi();

window.addEventListener("pagehide", () => flushDebug(true));

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

  const metrics = cellMetrics(tab);
  if (!metrics) return;
  const active = term.buffer && term.buffer.active;
  const col = active ? active.cursorX : 0;
  const row = active ? active.cursorY : Math.max(0, term.rows - 1);
  const { cw, ch } = metrics;
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
function sendInput(tab, data, source = "key") {
  if (!tab || !data) return;
  followCursor(tab);
  // A queued retraction must reach the PTY before anything typed after it.
  if (source !== "reconcile") flushHold(tab);
  dbg("send", tab, { src: source, text: clip(data), hex: hexOf(data), len: data.length });
  // Every byte leaves through this one function, after the caller has encoded
  // it. That makes it the safe place to spend a one-shot modifier: the key
  // that had to be modified has already been built, and the next key will be
  // plain. Locked modifiers are untouched.
  consumeOneShotModifiers(source);
  tab.inputChain = tab.inputChain
    .then(() => {
      const socket = tab.socket;
      if (socket && socket.readyState === WebSocket.OPEN && !state.clientBlocked) {
        if (source === "binary") {
          // onBinary strings are byte buckets (one char per PTY byte), not UTF-8.
          const bytes = new Uint8Array(data.length);
          for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i) & 0xff;
          socket.send(bytes);
        } else {
          socket.send(new TextEncoder().encode(data));
        }
        return;
      }
      // Fallback: keep the HTTP input endpoint alive for old servers and for
      // the short window while the WebSocket is reconnecting.
      return api(`/tabs/${tab.id}/input`, {
        method: "POST",
        body: data,
        headers: debugHeaders()
      });
    })
    .catch(report);
}

// Input synchronisation.
//
// One invariant keeps this stable: the hidden textarea's `value` is the only
// document, and the PTY is maintained as an exact projection of it. Every route
// that can carry text — xterm's onData, the raw input event, compositionend —
// calls the same idempotent reconciler with that same value, so listener order
// cannot change the outcome and no route can double-send.
//
// `data` is not trustworthy. A captured iPad session shows both shapes:
// `data="测试一下"` (the whole sentence, re-inserted after select-all) and
// `data="内容"` (only the new fragment), while `value` was always the
// post-insert text. xterm forwards `data` verbatim and never reads or clears
// `value` (verified in the vendored 5.5.0 source), so anything that trusts
// `data` either duplicates text or loses it.
//
// Ordering is now known and in our favour: xterm registers its `input` listener
// inside term.open() and ours are added later. On the same target element
// capture does NOT win — listeners run in registration order — so onData always
// fires first, i.e. after the browser has already written `value`. That is why
// onData can reconcile from `value` too, and why the earlier "beforeinput flag
// tells onData to stand down" handshake swallowed every single keystroke.
//
// Append (a typed character, dictation extending the sentence) is sent at once
// so input stays live. A retraction or rewrite is held for RECONCILE_HOLD_MS and
// re-evaluated against the newest value on every event, because a recognition
// pass usually catches back up. The hold must always track the latest value:
// freezing it is exactly what deleted characters the user had re-dictated.
const RECONCILE_HOLD_MS = 600;
// How close a keydown/paste must be to count as the origin of an input event.
// A key's DOM update lands within a frame; dictation never comes anywhere near
// this window after the last physical key.
const KEY_WINDOW_MS = 200;
// DEL is what the toolbar's Backspace sends and what PSReadLine expects.
const BACKSPACE = "\x7f";

// A payload the browser wrote into the textarea: text that must be reconciled
// from `value`. CR, DEL, ESC sequences and Ctrl chords never enter the document,
// they belong to the PTY alone.
function isPrintable(data) {
  if (!data) return false;
  for (const ch of data) {
    const cp = ch.codePointAt(0);
    if (cp < 0x20 || cp === 0x7f) return false;
  }
  return true;
}

function isCompositionInput(tab) {
  return tab.composing || tab.lastInputIsComposing ||
    tab.lastInputType === "insertCompositionText";
}

function codePoints(text) {
  return Array.from(typeof text === "string" ? text : "");
}

// Did a key or a paste produce the event currently being handled? The marker is
// written by a CAPTURE listener on the pane, i.e. before anything on the
// textarea — including xterm's own keydown handler, which forwards some keys
// straight into onData. Recording it on the textarea was too late: those
// keystrokes looked like dictation and were routed to the wrong path.
function keyDriven(tab) {
  if (tab.composing || tab.lastInputIsComposing) return true;
  const now = Date.now();
  return now - (tab.lastKeyDownAt || 0) <= KEY_WINDOW_MS ||
    now - (tab.lastPasteAt || 0) <= KEY_WINDOW_MS;
}

function commonPrefix(a, b) {
  const limit = Math.min(a.length, b.length);
  let i = 0;
  while (i < limit && a[i] === b[i]) i++;
  return i;
}

// WebKit hands spaces back as NO-BREAK SPACE (U+00A0) in some paths, and a
// NO-BREAK SPACE is not a word separator in a shell, so every command typed
// with one silently fails. Fold the invisible variants back to a plain space.
function normalizeField(text) {
  return typeof text === "string" ? text.replace(/[\u00a0\u202f]/g, " ") : "";
}

function cancelHold(tab) {
  tab.pendingText = null;
  if (tab.reconcileTimer !== null) {
    clearTimeout(tab.reconcileTimer);
    tab.reconcileTimer = null;
  }
}

// Make the PTY match `target` for the current utterance.
//
// Idempotent by construction: it compares `target` against `sent` (what the PTY
// already has for this utterance), so calling it twice with the same value — as
// onData and the input event do — sends nothing the second time. `force` is for
// the settle timer, which must emit a retraction instead of re-holding it.
function reconcileTo(tab, target, force) {
  const next = codePoints(target);
  const sent = tab.sent || [];
  const prefix = commonPrefix(sent, next);
  const deletes = sent.length - prefix;
  const insert = next.slice(prefix).join("");

  if (deletes === 0) {
    if (!insert) {
      // Already in sync — but a hold may be pending from an earlier retraction
      // and this newer value proves recognition caught back up. Drop it.
      cancelHold(tab);
      return;
    }
    tab.sent = next;
    cancelHold(tab);
    dbg("reconcile", tab, { del: 0, ins: clip(insert), held: 0 });
    sendInput(tab, insert, "reconcile");
    return;
  }

  if (!force) {
    // Shorter than what the PTY holds: retraction or re-recognition. Hold it,
    // always against the latest value.
    tab.pendingText = target;
    dbg("reconcile", tab, { del: deletes, ins: clip(insert), held: 1 });
    if (tab.reconcileTimer !== null) clearTimeout(tab.reconcileTimer);
    tab.reconcileTimer = setTimeout(() => {
      tab.reconcileTimer = null;
      const latest = tab.pendingText;
      tab.pendingText = null;
      if (latest !== null && latest !== undefined) reconcileTo(tab, latest, true);
    }, RECONCILE_HOLD_MS);
    return;
  }

  tab.pendingText = null;
  tab.sent = next;
  dbg("reconcile", tab, { del: deletes, ins: clip(insert), held: 0 });
  sendInput(tab, BACKSPACE.repeat(deletes) + insert, "reconcile");
}

// Reconcile against the textarea's current value. This is the only entry point
// that reads the document, so every event ends up in the same computation.
function syncField(tab) {
  if (isCompositionInput(tab)) {
    dbg("gate", tab, { why: "composing", text: clip(textareaText(tab)) });
    return;
  }
  const raw = textareaText(tab);
  if (raw === null) return;
  const text = normalizeField(raw);
  // An empty field is a buffer reset, not a retraction. Platforms clear the
  // textarea after committing (and xterm does it around compositionend), so
  // treating "" as "delete everything that was sent" is what silently ate text
  // the user had already produced. Deleting the line itself comes through
  // Backspace/Ctrl-U on the control path, which ends the utterance explicitly.
  if (text === "") {
    cancelHold(tab);
    tab.sent = [];
    return;
  }
  if (text !== raw) dbg("nbsp", tab, { hex: hexOf(raw) });
  reconcileTo(tab, text, false);
}

// The utterance ends: Enter submitted it, a cursor-moving key invalidated it, or
// the field is being abandoned. Forget both the mirror and the document so the
// platform cannot replay the old text into the next one.
function endUtterance(tab) {
  cancelHold(tab);
  tab.sent = [];
  const el = textareaEl(tab);
  if (el && el.value) {
    el.value = "";
    dbg("field-clear", tab, {});
  }
}

// Settle a held retraction now (blur, tab switch).
function flushHold(tab) {
  if (!tab) return;
  const latest = tab.pendingText;
  cancelHold(tab);
  if (latest !== null && latest !== undefined) reconcileTo(tab, latest, true);
}

function modOn(id) {
  return state.mods[id] !== "off";
}

// Same modifier numbering as the native keyboard encoder: 1 + shift + 2*alt
// + 4*ctrl (plus 8*meta for the platform key, which native reserves for app
// shortcuts). CSI 1;n{key} is how terminals encode modified navigation keys.
function modifierCode() {
  return 1
    + (modOn("shift") ? 1 : 0)
    + (modOn("alt") ? 2 : 0)
    + (modOn("ctrl") ? 4 : 0)
    + (modOn("meta") ? 8 : 0);
}

function anyModifier() {
  // In screen-keyboard mode the modifier flags live in the synthetic key
  // events themselves; xterm encodes them. Applying the toolbar modifiers a
  // second time here would produce doubled Ctrl/Alt prefixes.
  if (state.keyboardMode === "screen") return false;
  return MODIFIER_KEYS.some((key) => modOn(key.id));
}

// Alt/Meta prefix printable and control bytes with ESC. Ctrl on a plain letter
// folds it to the matching control byte; shift uppercases. IME/paste text is
// never rewritten, only a single ASCII keystroke is.
function encodeTyped(text) {
  if (!anyModifier() || typeof text !== "string" || text.length !== 1) return text;
  const ch = text[0];
  const code = ch.charCodeAt(0);
  if (code < 0x20 || code > 0x7e) return text;
  let out = ch;
  if (modOn("ctrl")) out = String.fromCharCode(ch.toLowerCase().charCodeAt(0) & 0x1f);
  else if (modOn("shift")) out = ch.toUpperCase();
  if (modOn("alt") || modOn("meta")) out = "\x1b" + out;
  return out;
}

// A toolbar key is a semantic key, so latched modifiers turn a plain arrow
// into Ctrl+Arrow etc. instead of appending to a pre-baked byte string.
function encodeToolbarKey(key) {
  if (!anyModifier()) return key.data;
  // Backspace must match the native encoder: plain DEL, Ctrl-BS, ESC prefix
  // for Alt/Meta. Treating it as a generic control byte would swallow Ctrl.
  if (key.special === "backspace") {
    const data = modOn("ctrl") ? "\x08" : key.data;
    return modOn("alt") || modOn("meta") ? "\x1b" + data : data;
  }
  if (key.special === "tab") {
    if (modOn("shift") && !modOn("ctrl") && !modOn("alt") && !modOn("meta")) {
      return "\x1b[Z";
    }
    return `\x1b[1;${modifierCode()}I`;
  }
  if (key.csiFinal) return `\x1b[1;${modifierCode()}${key.csiFinal}`;
  // Control / Esc bytes: Alt or Meta adds an ESC prefix; Shift alone leaves
  // the control byte unchanged (there is no shifted form of ^C).
  return (modOn("alt") || modOn("meta") ? "\x1b" : "") + key.data;
}

const MODIFIER_KEYS = [
  { id: "shift", label: "Shift" },
  { id: "ctrl", label: "Ctrl" },
  { id: "alt", label: "Alt" },
  {
    id: "meta",
    label: /Mac|iPhone|iPad|iPod/i.test(navigator.platform || "") ? "Cmd" : "Win"
  }
];

// Modifier buttons behave like the modifiers on a real keyboard and on the
// platform on-screen keyboards: one tap arms the NEXT key, a quick double tap
// locks it on, and one more tap releases it. The locked state is visually
// different, because an invisible difference would make every shortcut a gamble.
const DOUBLE_TAP_MS = 320;
const modifierButtons = new Map();
const lastModifierTap = new Map();

function modifierTitle(key) {
  const mode = state.mods[key.id];
  const stateText = mode === "locked" ? "已锁定" : mode === "once" ? "下一个按键生效" : "关闭";
  return `${key.label} · ${stateText} ｜ 单击=仅作用于下一个按键；快速双击=持续锁定；再单击=关闭`;
}

function applyModifierUi() {
  for (const key of MODIFIER_KEYS) {
    const button = modifierButtons.get(key.id);
    if (!button) continue;
    const mode = state.mods[key.id];
    button.classList.toggle("active", mode !== "off");
    button.classList.toggle("locked", mode === "locked");
    button.setAttribute("aria-pressed", String(mode !== "off"));
    button.title = modifierTitle(key);
  }
}

function setModifier(id, mode) {
  if (state.mods[id] === mode) return;
  state.mods[id] = mode;
  applyModifierUi();
  dbg("mod", null, { key: id, mode });
}

function tapModifier(key) {
  const now = Date.now();
  const mode = state.mods[key.id];
  const quick = mode === "once" && now - (lastModifierTap.get(key.id) || 0) <= DOUBLE_TAP_MS;
  // Tapping an armed modifier again before the double-tap window closes means
  // "lock it"; after the window it means "cancel". A locked modifier always
  // releases on the next tap, so there is no way to get stuck in a chord.
  const next = mode === "locked" ? "off" : quick ? "locked" : mode === "once" ? "off" : "once";
  lastModifierTap.set(key.id, now);
  setModifier(key.id, next);
}

// Called from sendInput after the caller has encoded its bytes: one-shot
// modifiers are spent by the transmission they were armed for, locked ones
// survive so a modifier can be held across several keys.
function consumeOneShotModifiers(source) {
  const spent = [];
  for (const key of MODIFIER_KEYS) {
    if (state.mods[key.id] !== "once") continue;
    state.mods[key.id] = "off";
    spent.push(key.id);
  }
  if (!spent.length) return;
  applyModifierUi();
  dbg("mod", null, { spent, src: source });
}

// Far left: the two keys with no printable soft-keyboard equivalent.
const LEADING_KEYS = [
  { label: "Esc", data: "\x1b", special: "escape" },
  { label: "Tab", data: "\t", special: "tab" }
];

// Editing/control bytes. Backspace repeats because deleting one character at
// a time by tapping is unusable on a phone; ^C and friends stay one-shot.
const EDIT_KEYS = [
  {
    label: "⌫",
    data: "\x7f",
    special: "backspace",
    title: "Backspace (hold to repeat)",
    repeat: true
  },
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

// Arrows are pinned in their own pad on the right and repeat while held.
const NAV_KEYS = [
  { label: "↑", data: "\x1b[A", csiFinal: "A", title: "up (hold to repeat)", repeat: true },
  { label: "↓", data: "\x1b[B", csiFinal: "B", title: "down (hold to repeat)", repeat: true },
  { label: "←", data: "\x1b[D", csiFinal: "D", title: "left (hold to repeat)", repeat: true },
  { label: "→", data: "\x1b[C", csiFinal: "C", title: "right (hold to repeat)", repeat: true }
];

// xterm only serialises input into bytes: auto-repeat comes from the OS for
// physical keyboards, but a button tap never repeats and mobile soft
// keyboards usually do not either. Emulate it here for the few keys where
// holding is meaningful. The initial delay keeps a normal tap at one stroke.
const REPEAT_DELAY_MS = 400;
const REPEAT_INTERVAL_MS = 45;
const repeatStops = new Set();

function stopAllRepeats() {
  for (const stop of [...repeatStops]) stop();
}

function bindHoldRepeat(button, press) {
  let delayTimer = null;
  let repeatTimer = null;
  let pointer = null;

  const stop = () => {
    if (delayTimer !== null) clearTimeout(delayTimer);
    if (repeatTimer !== null) clearInterval(repeatTimer);
    delayTimer = null;
    repeatTimer = null;
    if (pointer !== null && button.hasPointerCapture?.(pointer)) {
      try {
        button.releasePointerCapture(pointer);
      } catch {
        // Pointer may already be gone (e.g. lostpointercapture fired first).
      }
    }
    pointer = null;
    button.classList.remove("repeating");
    repeatStops.delete(stop);
  };

  button.addEventListener("pointerdown", (event) => {
    // A second finger on the same key must not leak the first key's timers.
    stop();
    event.preventDefault();
    button.blur();
    pointer = event.pointerId;
    // Keep receiving pointerup when the finger drifts off the small button.
    // A synthetic pointer has no live capture target: attempting it fires
    // lostpointercapture immediately and would release a held key at once.
    if (event.isTrusted) {
      try { button.setPointerCapture?.(event.pointerId); } catch {}
    }
    repeatStops.add(stop);
    button.classList.add("repeating");
    press();
    delayTimer = setTimeout(() => {
      delayTimer = null;
      repeatTimer = setInterval(press, REPEAT_INTERVAL_MS);
    }, REPEAT_DELAY_MS);
  });
  button.addEventListener("pointerup", stop);
  button.addEventListener("pointercancel", stop);
  button.addEventListener("lostpointercapture", stop);
  // iOS otherwise pops a selection/zoom menu on a long press.
  button.addEventListener("contextmenu", (event) => event.preventDefault());
}

function makeKeyButton(key, onPress) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = key.label;
  button.title = key.title || key.label;
  if (key.mod) button.classList.add("key-mod");
  const press = () => onPress();
  if (key.repeat) {
    bindHoldRepeat(button, press);
    return button;
  }
  // preventDefault on pointerdown keeps xterm's hidden textarea focused, so
  // the soft keyboard stays open; blur() drops the transient pressed state.
  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    button.blur();
    press();
  });
  return button;
}

function appendToolbarSeparator() {
  const sep = document.createElement("span");
  sep.className = "key-sep";
  keyScroll.appendChild(sep);
}

function buildToolbar() {
  const sendKey = (key) => () => sendInput(currentTab(), encodeToolbarKey(key));
  // Esc/Tab first, then the sticky modifiers: both stay ahead of the control
  // bytes so the keys with no soft-keyboard equivalent need the least scroll.
  for (const key of LEADING_KEYS) {
    keyScroll.appendChild(makeKeyButton(key, sendKey(key)));
  }
  appendToolbarSeparator();
  for (const key of MODIFIER_KEYS) {
    const button = makeKeyButton(key, () => tapModifier(key));
    button.classList.add("key-toggle");
    button.setAttribute("aria-pressed", "false");
    modifierButtons.set(key.id, button);
    keyScroll.appendChild(button);
  }
  applyModifierUi();
  appendToolbarSeparator();
  for (const key of EDIT_KEYS) {
    keyScroll.appendChild(makeKeyButton(key, sendKey(key)));
  }
  for (const key of NAV_KEYS) {
    keyNav.appendChild(makeKeyButton(key, sendKey(key)));
  }
}

function clearModifiers() {
  for (const key of MODIFIER_KEYS) state.mods[key.id] = "off";
  applyModifierUi();
}

function setToolbar(visible) {
  state.showToolbar = visible;
  keyToolbar.hidden = !visible;
  document.body.classList.toggle("with-toolbar", visible);
  toolbarToggle.setAttribute("aria-pressed", String(visible));
  // A latched Ctrl/Alt must never survive into plain typing after hiding,
  // and a held key must not keep sending input into a hidden toolbar.
  if (!visible) {
    stopAllRepeats();
    clearModifiers();
  }
  try {
    localStorage.setItem("rterminal.toolbar", visible ? "1" : "0");
  } catch {
    // Storage may be disabled; the toggle still works for this session.
  }
  scheduleFit();
}

// --- Built-in screen keyboard ------------------------------------------------
//
// This is a different input model from the compact toolbar above. The toolbar
// edits text through the browser's hidden textarea; here we act *as* the
// keyboard: each tap dispatches a real-shaped keydown/keyup at xterm's textarea
// with modifier flags on the event, and xterm does the encoding. There is no
// one-shot/lock state, no textarea reconciliation and no IME — by design. The
// system keyboard is one button away for those.

function sk(label, key, code, vk, shifted, extra) {
  return { label, key, code, vk, shifted, ...(extra || {}) };
}
function skMod(label, mod, code, vk, extra) {
  // `key` must stay a real KeyboardEvent.key even when the cap shows a glyph
  // (⌘/⌥/⌃), otherwise xterm would try to encode the glyph as text.
  return { label, key: label, code, vk, mod, ...(extra || {}) };
}
function skLetters(letters) {
  // Caps stay lowercase by default and switch to the upper-case variant while
  // Shift is held, matching the on-screen keyboards people are used to.
  return [...letters].map((c) => sk(c.toLowerCase(), c.toLowerCase(), `Key${c}`, c.charCodeAt(0), c));
}
function skCaps() {
  return sk("Caps", "CapsLock", "CapsLock", 20, null, {
    wide: true, caps: true, title: "Caps Lock（切换字母大小写）"
  });
}
const SK_DIGITS = "1234567890";
const SK_SHIFTED_DIGITS = "!@#$%^&*()";

const SCREEN_KEY_ROWS_WIN = [
  [
    sk("Esc", "Escape", "Escape", 27),
    ...Array.from({ length: 12 }, (_, i) => sk(`F${i + 1}`, `F${i + 1}`, `F${i + 1}`, 112 + i))
  ],
  [
    sk("`", "`", "Backquote", 192, "~"),
    ...[...SK_DIGITS].map((d, i) => sk(d, d, `Digit${d}`, 49 + i, SK_SHIFTED_DIGITS[i])),
    sk("-", "-", "Minus", 189, "_"),
    sk("=", "=", "Equal", 187, "+"),
    sk("⌫", "Backspace", "Backspace", 8, null, { wide: true, repeat: true })
  ],
  [
    sk("Tab", "Tab", "Tab", 9, null, { wide: true }),
    ...skLetters("QWERTYUIOP"),
    sk("[", "[", "BracketLeft", 219, "{"),
    sk("]", "]", "BracketRight", 221, "}"),
    sk("\\", "\\", "Backslash", 220, "|")
  ],
  [
    skCaps(),
    ...skLetters("ASDFGHJKL"),
    sk(";", ";", "Semicolon", 186, ":"),
    sk("'", "'", "Quote", 222, '"'),
    sk("Enter", "Enter", "Enter", 13, null, { wide: true })
  ],
  [
    skMod("Shift", "shift", "ShiftLeft", 16),
    ...skLetters("ZXCVBNM"),
    sk(",", ",", "Comma", 188, "<"),
    sk(".", ".", "Period", 190, ">"),
    sk("/", "/", "Slash", 191, "?"),
    skMod("Shift", "shift", "ShiftRight", 16)
  ],
  [
    skMod("Ctrl", "ctrl", "ControlLeft", 17),
    skMod("Win", "meta", "MetaLeft", 91),
    skMod("Alt", "alt", "AltLeft", 18),
    sk("Space", " ", "Space", 32, null, { space: true }),
    skMod("Alt", "alt", "AltRight", 18),
    skMod("Win", "meta", "MetaRight", 92),
    skMod("Ctrl", "ctrl", "ControlRight", 17)
  ]
];

// Apple key caps: the modifier *order* changes (control/option/command, with
// command next to the space bar), and the labels are glyphs. The underlying
// event flags are unchanged: ⌘ is metaKey, ⌥ is altKey, ⌃ is ctrlKey. No
// extra "Mac mode" translation layer — xterm still does all the encoding.
const SCREEN_KEY_ROWS_MAC = [
  SCREEN_KEY_ROWS_WIN[0],
  SCREEN_KEY_ROWS_WIN[1],
  SCREEN_KEY_ROWS_WIN[2],
  [
    skCaps(),
    ...skLetters("ASDFGHJKL"),
    sk(";", ";", "Semicolon", 186, ":"),
    sk("'", "'", "Quote", 222, '"'),
    sk("return", "Enter", "Enter", 13, null, { wide: true })
  ],
  SCREEN_KEY_ROWS_WIN[4],
  [
    skMod("⌃", "ctrl", "ControlLeft", 17, { key: "Control", title: "Control（按住）" }),
    skMod("⌥", "alt", "AltLeft", 18, { key: "Alt", title: "Option（按住）" }),
    skMod("⌘", "meta", "MetaLeft", 91, { key: "Meta", title: "Command（按住）" }),
    sk("Space", " ", "Space", 32, null, { space: true }),
    skMod("⌘", "meta", "MetaRight", 92, { key: "Meta", title: "Command（按住）" }),
    skMod("⌥", "alt", "AltRight", 18, { key: "Alt", title: "Option（按住）" }),
    skMod("⌃", "ctrl", "ControlRight", 17, { key: "Control", title: "Control（按住）" })
  ]
];

function screenKeyRows() {
  return state.screenLayout === "mac" ? SCREEN_KEY_ROWS_MAC : SCREEN_KEY_ROWS_WIN;
}

const SCREEN_NAV_KEYS = [
  [sk("Ins", "Insert", "Insert", 45), sk("Home", "Home", "Home", 36), sk("PgUp", "PageUp", "PageUp", 33)],
  [sk("Del", "Delete", "Delete", 46), sk("End", "End", "End", 35), sk("PgDn", "PageDown", "PageDown", 34)]
];
// Standard inverted-T: ↑ sits above ↓, not above ←. `null` cells keep the
// three columns aligned without being focusable or matchable as keys.
const SCREEN_ARROW_KEYS = [
  [null, sk("↑", "ArrowUp", "ArrowUp", 38, null, { repeat: true }), null],
  [
    sk("←", "ArrowLeft", "ArrowLeft", 37, null, { repeat: true }),
    sk("↓", "ArrowDown", "ArrowDown", 40, null, { repeat: true }),
    sk("→", "ArrowRight", "ArrowRight", 39, null, { repeat: true })
  ]
];

// Which modifiers are currently DOWN on the on-screen keyboard. This is held
// key state, not a latched mode: it can only be true while a finger is on a
// modifier button, exactly like a physical keyboard.
const screenHeldMods = new Set();
const screenHeldDefs = new Map();
const screenModButtons = new Map();
// Keys with an upper-case / shifted variant. Their caps follow the held Shift
// state instead of hard-coding one symbol: digits and punctuation show both
// variants at once, letters swap between lower and upper case.
const screenShiftLabels = [];
// Caps Lock is a real lock, unlike the held modifiers above: it survives
// subsequent keys and only affects letters (Shift inverts it, as on a PC).
let screenCapsLock = false;
let screenCapsButton = null;

function isLetterShift(def) {
  return !!def.shifted && def.shifted.toLowerCase() === def.key.toLowerCase();
}

function screenLetterUpper(def) {
  if (!isLetterShift(def)) return false;
  const shift = screenHeldMods.has("shift");
  return screenCapsLock ? !shift : shift;
}

function screenKeyEvent(type, def, charCode) {
  const event = new KeyboardEvent(type, {
    key: def.key,
    code: def.code,
    bubbles: true,
    cancelable: true,
    composed: true,
    shiftKey: screenHeldMods.has("shift"),
    ctrlKey: screenHeldMods.has("ctrl"),
    altKey: screenHeldMods.has("alt"),
    metaKey: screenHeldMods.has("meta")
  });
  // Chrome's KeyboardEvent constructor does not accept keyCode/which. xterm
  // still reads them, so define them the same way the replay harness does.
  Object.defineProperty(event, "keyCode", { get: () => def.vk });
  Object.defineProperty(event, "which", { get: () => charCode ?? def.vk });
  if (charCode !== undefined) {
    Object.defineProperty(event, "charCode", { get: () => charCode });
  }
  return event;
}

function dispatchScreenKey(type, def, charCode) {
  const tab = currentTab();
  const field = tab && textareaEl(tab);
  if (!field) return false;
  field.dispatchEvent(screenKeyEvent(type, def, charCode));
  return true;
}

function effectiveScreenKey(def) {
  if (!def.shifted) return def;
  if (isLetterShift(def)) {
    return screenLetterUpper(def) ? { ...def, key: def.shifted } : def;
  }
  return screenHeldMods.has("shift") ? { ...def, key: def.shifted } : def;
}

function releaseAllScreenModifiers() {
  // Send the keyups while the flag is still set, so xterm sees the same state
  // that was on the matching keydown.
  for (const [mod, def] of screenHeldDefs) dispatchScreenKey("keyup", def);
  screenHeldMods.clear();
  screenHeldDefs.clear();
  refreshScreenModButtons();
}

function updateScreenShiftLabels() {
  const shifted = screenHeldMods.has("shift");
  for (const entry of screenShiftLabels) {
    const { def, button } = entry;
    const main = button.querySelector(".sk-key-main");
    const secondary = button.querySelector(".sk-key-shift");
    const upper = secondary ? shifted : screenLetterUpper(def);
    main.textContent = upper ? def.shifted : def.label;
    if (secondary) secondary.textContent = upper ? def.label : def.shifted;
    button.classList.toggle("sk-shifted", upper);
  }
}

function refreshScreenModButtons() {
  for (const [mod, buttons] of screenModButtons) {
    const held = screenHeldMods.has(mod);
    for (const button of buttons) button.classList.toggle("sk-held", held);
  }
  if (screenCapsButton) {
    screenCapsButton.classList.toggle("sk-caps", screenCapsLock);
    screenCapsButton.setAttribute("aria-pressed", String(screenCapsLock));
  }
  updateScreenShiftLabels();
}

function screenKeySpacer() {
  const spacer = document.createElement("span");
  spacer.className = "sk-spacer";
  spacer.setAttribute("aria-hidden", "true");
  return spacer;
}

function screenKeyButton(def) {
  const button = document.createElement("button");
  button.type = "button";
  // Keep the base label available to the replay harness even while Shift has
  // swapped the visible cap to its upper variant.
  button.dataset.label = def.label;
  button.dataset.key = def.key;
  button.title = def.title || (def.mod ? `${def.label}（按住）` : def.label);
  if (def.shifted) {
    const main = document.createElement("span");
    main.className = "sk-key-main";
    button.appendChild(main);
    if (!isLetterShift(def)) {
      const secondary = document.createElement("span");
      secondary.className = "sk-key-shift";
      button.appendChild(secondary);
    }
    screenShiftLabels.push({ def, button });
  } else {
    button.textContent = def.label;
  }
  button.classList.add("sk-key");
  if (def.wide) button.classList.add("sk-wide");
  if (def.space) button.classList.add("sk-space");
  if (def.mod) {
    if (!screenModButtons.has(def.mod)) screenModButtons.set(def.mod, []);
    screenModButtons.get(def.mod).push(button);
  }
  if (def.caps) {
    screenCapsButton = button;
    button.setAttribute("aria-pressed", "false");
  }

  let pointer = null;
  let activeDef = null;
  let delayTimer = null;
  let repeatTimer = null;
  const stop = () => {
    if (delayTimer !== null) clearTimeout(delayTimer);
    if (repeatTimer !== null) clearInterval(repeatTimer);
    delayTimer = null;
    repeatTimer = null;
    if (pointer !== null && button.hasPointerCapture?.(pointer)) {
      try { button.releasePointerCapture(pointer); } catch {}
    }
    pointer = null;
    button.classList.remove("sk-down");
    if (!activeDef) return;
    if (activeDef.mod) {
      screenHeldMods.delete(activeDef.mod);
      screenHeldDefs.delete(activeDef.mod);
      refreshScreenModButtons();
    }
    dispatchScreenKey("keyup", activeDef);
    activeDef = null;
  };

  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    button.blur();
    stop();
    pointer = event.pointerId;
    // A synthetic PointerEvent (the replay harness) has no live pointer to
    // capture; attempting it would emit lostpointercapture and release the
    // modifier before the next key is pressed.
    if (event.isTrusted) {
      try { button.setPointerCapture?.(event.pointerId); } catch {}
    }
    button.classList.add("sk-down");
    if (def.caps) {
      // A lock, not a key event: nothing is sent to the PTY, only the letter
      // caps and the next keystrokes change.
      screenCapsLock = !screenCapsLock;
      refreshScreenModButtons();
      dbg("screen-caps", null, { on: screenCapsLock });
      return;
    }
    activeDef = def.mod ? def : effectiveScreenKey(def);
    dbg("skdown", null, { key: def.label, mod: def.mod || null, held: [...screenHeldMods] });
    if (def.mod) {
      if (screenHeldMods.has(def.mod)) { activeDef = null; return; }
      screenHeldMods.add(def.mod);
      screenHeldDefs.set(def.mod, def);
      refreshScreenModButtons();
    }
    dispatchScreenKey("keydown", activeDef);
    // xterm deliberately ignores a bare keydown whose key is an upper-case
    // letter: it waits for the browser's keypress(charCode) that normally
    // follows. A synthetic event has no such default action, so supply it.
    // Lower-case letters and symbols are already emitted by the keydown.
    if (/^[A-Z]$/.test(activeDef.key)) {
      dispatchScreenKey("keypress", activeDef, activeDef.key.charCodeAt(0));
    }
    if (def.repeat) {
      // A real key repeats by sending another keydown; xterm turns each one
      // into bytes. Pairing it with a keyup is what the probe verified.
      delayTimer = setTimeout(() => {
        delayTimer = null;
        repeatTimer = setInterval(() => {
          dispatchScreenKey("keydown", activeDef);
          dispatchScreenKey("keyup", activeDef);
        }, 70);
      }, 420);
    }
  });
  button.addEventListener("pointerup", stop);
  button.addEventListener("pointercancel", stop);
  button.addEventListener("lostpointercapture", stop);
  button.addEventListener("contextmenu", (event) => event.preventDefault());
  return button;
}

function buildScreenKeyboard() {
  // Rebuilding is only used by the Win/Mac layout switch. Drop the old button
  // tree (and its modifiers) instead of leaving stale handlers on detached
  // nodes that would still be reachable through screenModButtons.
  releaseAllScreenModifiers();
  screenModButtons.clear();
  screenShiftLabels.length = 0;
  screenCapsLock = false;
  screenCapsButton = null;
  const layout = document.createElement("div");
  layout.className = "sk-layout";
  const main = document.createElement("div");
  main.className = "sk-main";
  for (const row of screenKeyRows()) {
    const el = document.createElement("div");
    el.className = "sk-row";
    for (const def of row) el.appendChild(screenKeyButton(def));
    main.appendChild(el);
  }
  layout.appendChild(main);

  const side = document.createElement("div");
  side.className = "sk-side";
  for (const group of [SCREEN_NAV_KEYS, SCREEN_ARROW_KEYS]) {
    const pad = document.createElement("div");
    pad.className = "sk-pad";
    for (const row of group) {
      const el = document.createElement("div");
      el.className = "sk-row";
      for (const def of row) {
        el.appendChild(def ? screenKeyButton(def) : screenKeySpacer());
      }
      pad.appendChild(el);
    }
    side.appendChild(pad);
  }
  layout.appendChild(side);
  skLayoutHolder.replaceChildren(layout);
  updateScreenShiftLabels();
  const mac = state.screenLayout === "mac";
  skLayoutToggle.textContent = mac ? "Win 样式" : "Mac 样式";
  skLayoutToggle.title = mac
    ? "当前是 Mac 样式键盘（⌘/⌥/⌃）。点此切回 Win 样式（Win/Alt/Ctrl）。"
    : "当前是 Win 样式键盘（Win/Alt/Ctrl）。点此切换为 Mac 样式（⌘/⌥/⌃）。";
  skLayoutToggle.setAttribute("aria-pressed", String(mac));
}

function setScreenLayout(layout) {
  if (state.screenLayout === layout) return;
  state.screenLayout = layout;
  buildScreenKeyboard();
  try {
    localStorage.setItem("rterminal.screenLayout", layout);
  } catch {}
  dbg("screen-layout", null, { layout });
}

skLayoutToggle.addEventListener("click", () => {
  setScreenLayout(state.screenLayout === "mac" ? "win" : "mac");
});

function applyKeyboardModeUi() {
  const screen = state.keyboardMode === "screen";
  keyboardModeButton.setAttribute("aria-pressed", String(screen));
  keyboardModeLabel.textContent = screen ? "屏幕键盘" : "系统键盘";
  screenKeyboard.hidden = !screen;
  document.body.classList.toggle("with-screen-keyboard", screen);
}

function setKeyboardMode(mode) {
  if (state.keyboardMode === mode) return;
  // Caps Lock is a virtual lock: never let it survive a mode switch and
  // silently change what the next typed letter means.
  screenCapsLock = false;
  releaseAllScreenModifiers();
  state.keyboardMode = mode;
  applyKeyboardModeUi();
  if (mode === "screen") {
    // Toolbar modifiers are a text-editing concept; they must not leak into
    // a keyboard that carries its own modifier flags on every key event.
    clearModifiers();
    if (state.showToolbar) setToolbar(false);
    // Keep the OS soft keyboard from opening over the built-in one.
    for (const tab of state.tabs.values()) {
      const field = textareaEl(tab);
      if (field && document.activeElement === field) field.blur();
    }
  }
  try {
    localStorage.setItem("rterminal.keyboardMode", mode);
  } catch {}
  dbg("keyboard-mode", null, { mode });
  scheduleFit();
}

keyboardModeButton.addEventListener("click", () => {
  setKeyboardMode(state.keyboardMode === "screen" ? "system" : "screen");
});

// A physical key arriving while an on-screen modifier is down must release
// it, or the terminal would keep receiving carrying modifier flags.
document.addEventListener("keydown", (event) => {
  if (!event.isTrusted || state.keyboardMode !== "screen") return;
  if (screenHeldMods.size) releaseAllScreenModifiers();
}, true);

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

// Return null when the cell size cannot be measured. FitAddon does the same:
// guessing from font metrics produces NaN/Infinity downstream (a bogus resize
// sent to the PTY, or an unbounded dot pad), which is worse than skipping one
// layout pass. The caller simply waits for the next refresh.
function cellMetrics(tab) {
  const element = tab.term.element;
  // The rendered screen is the ground truth for cell size. Deriving it from
  // CSS font metrics is unreliable: xterm.js rounds cell height to whole
  // pixels and its hidden measure element holds 32 '~' glyphs, not one cell.
  const screen = element && element.querySelector(".xterm-screen");
  const box = screen ? screen.getBoundingClientRect() : null;
  if (box && box.width > 0 && box.height > 0 && tab.term.cols && tab.term.rows) {
    const cw = box.width / tab.term.cols;
    const ch = box.height / tab.term.rows;
    if (Number.isFinite(cw) && Number.isFinite(ch) && cw > 0 && ch > 0) {
      return { cw, ch };
    }
  }

  const measure = element && element.querySelector(".xterm-char-measure-element");
  const sample = measure ? (measure.textContent || "").length : 0;
  if (sample > 0) {
    const width = measure.getBoundingClientRect().width / sample;
    const style = element ? getComputedStyle(element) : null;
    const rawLine = style && style.lineHeight;
    const ch = rawLine && rawLine !== "normal" ? parseFloat(rawLine) : NaN;
    if (Number.isFinite(width) && width > 0 && Number.isFinite(ch) && ch > 0) {
      return { cw: width, ch };
    }
  }
  return null;
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
  if (width > 0) return width;
  const metrics = cellMetrics(tab);
  return metrics ? metrics.cw : 0;
}

function applyPadding(tab) {
  const padRight = tab.padRight;
  const padBottom = tab.padBottom;
  if (!state.webConnected || state.clientBlocked) {
    padRight.hidden = true;
    padBottom.hidden = true;
    return;
  }

  const metrics = cellMetrics(tab);
  if (!metrics) {
    padRight.hidden = true;
    padBottom.hidden = true;
    return;
  }
  const { cw, ch } = metrics;
  const rect = panes.getBoundingClientRect();
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
  // Both measurements failed this pass. Skip rather than generating an
  // unbounded run of dots (dotWidth === 0 -> Infinity columns).
  if (!(dotWidth > 0)) {
    padRight.hidden = true;
    padBottom.hidden = true;
    return;
  }
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

function fitTab(tab, forceSync = false) {
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

  // Two explicit modes, decided by who is authoritative for the grid:
  //
  //   "pc"      (default): the native window is authoritative. We only mirror
  //             its grid; if this viewport is smaller we scroll, if larger we
  //             pad with dots. Never resize the PTY.
  //   "browser": this viewport is authoritative. We compute the grid it can
  //             show and ask the native side to adopt it AND resize the PC
  //             window to match (sync_window), so neither side scrolls.
  //
  // The soft keyboard only changes what is visible; resizing the PTY or the PC
  // window then would redraw running programs on every keystroke, so both
  // modes stay frozen while it is up.
  if (state.sizeMode === "browser" && keyboardInset === 0 && keyboardTimer === null) {
    const rect = panes.getBoundingClientRect();
    const metrics = cellMetrics(tab);
    if (rect.width && rect.height && metrics) {
      const { cw, ch } = metrics;
      // Browser is authoritative on both axes, touch devices included.
      const wantCols = Math.max(20, Math.min(300, Math.floor((rect.width - 8) / cw)));
      const wantRows = Math.max(5, Math.min(200, Math.floor((rect.height - 8) / ch)));
      if (
        Number.isFinite(wantCols) && Number.isFinite(wantRows) &&
        (forceSync || wantCols !== cols || wantRows !== rows)
      ) {
        api(`/tabs/${tab.id}/resize`, {
          method: "POST",
          body: JSON.stringify({
            cols: wantCols,
            rows: wantRows,
            sync_window: true
          })
        }).catch(report);
      }
    }
  }
  applyPadding(tab);
}

function stopPolling(tab) {
  // Invalidate callbacks from the previous connection before closing it.
  tab.streamGen = (tab.streamGen || 0) + 1;
  if (tab.timer !== null) {
    clearTimeout(tab.timer);
    tab.timer = null;
  }
  if (tab.reconnectTimer !== null) {
    clearTimeout(tab.reconnectTimer);
    tab.reconnectTimer = null;
  }
  if (tab.socket) {
    const socket = tab.socket;
    tab.socket = null;
    try {
      socket.close();
    } catch {
      // Already closing.
    }
  }
}

function streamUrl(tab) {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const host = location.hostname || "127.0.0.1";
  const port = state.wsPort || Number(location.port) + 1 || 7879;
  const debugFlag = debug.on ? "&debug=1" : "";
  return `${scheme}://${host}:${port}/ws/tabs/${tab.id}?client=${encodeURIComponent(clientId)}${debugFlag}`;
}

// Live PTY stream. The server sends one ANSI snapshot on attach, followed by
// raw output bytes. HTTP polling stays as a fallback for connection failures.
function startStream(tab) {
  stopPolling(tab);
  if (state.clientBlocked || state.active !== tab.id) return;
  // Snapshot tabs have no PTY, so they cannot be streamed. Their static
  // screen is served by the snapshot branch of /screen.ansi instead.
  if (tab.dto && tab.dto.kind === "snapshot") {
    poll(tab);
    return;
  }
  const gen = tab.streamGen;
  let socket;
  try {
    socket = new WebSocket(streamUrl(tab));
  } catch (error) {
    report(error);
    poll(tab);
    return;
  }
  socket.binaryType = "arraybuffer";
  tab.socket = socket;

  socket.addEventListener("open", () => {
    if (gen !== tab.streamGen) return;
    state.wsFailures = 0;
    clearError();
  });

  socket.addEventListener("message", (event) => {
    if (gen !== tab.streamGen || state.active !== tab.id) return;
    if (event.data instanceof ArrayBuffer) {
      const bytes = new Uint8Array(event.data);
      tab.term.write(bytes);
      if (tab.followCursor) scrollToCursor(tab);
      dbg("wsout", tab, { len: bytes.length });
      return;
    }
    let message;
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    if (message.type === "snapshot") {
      const alt = message.alt === true;
      const ansi = typeof message.ansi === "string" ? message.ansi : "";
      if (isCompositionInput(tab)) {
        tab.pendingPaint = { alt, ansi };
      } else {
        paintSnapshot(tab, alt, ansi);
      }
      statusEl.textContent = `tab ${tab.id} · ${tab.term.cols}×${tab.term.rows}`;
      clearError();
    }
  });

  socket.addEventListener("close", () => {
    if (gen !== tab.streamGen) return;
    tab.socket = null;
    if (state.active !== tab.id || state.clientBlocked) return;
    state.wsFailures += 1;
    // A couple of quick retries cover a server restart. After that, fall back
    // to snapshots so the view is never permanently frozen.
    if (state.wsFailures >= 3) {
      poll(tab);
      return;
    }
    tab.reconnectTimer = setTimeout(() => startStream(tab), 300 * state.wsFailures);
  });
}

function textOf(data) {
  return typeof data === "string" ? data : String(data || "");
}

// A full ANSI repaint moves xterm's composition view and its hidden textarea
// out from under an active IME, which shows up as a half-composed CJK glyph
// being cut off. While composing, keep only the newest snapshot and paint it
// once compositionend fires.
function paintSnapshot(tab, alt, ansi) {
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
  if (debug.on) {
    // Only log a repaint when the cursor line actually changed; otherwise a
    // 10 Hz poll would bury the keystrokes in noise.
    const line = cursorLine(tab);
    if (line !== tab.debugPaintLine || (alt ? 1 : 0) !== tab.debugPaintAlt) {
      tab.debugPaintLine = line;
      tab.debugPaintAlt = alt ? 1 : 0;
      dbg("paint", tab, { alt: alt ? 1 : 0, len: ansi.length, line });
    }
  }
}

function flushPendingPaint(tab) {
  const pending = tab.pendingPaint;
  tab.pendingPaint = null;
  if (pending) paintSnapshot(tab, pending.alt, pending.ansi);
}

async function poll(tab) {
  if (state.clientBlocked || state.active !== tab.id) return;
  try {
    const raw = await api(`/tabs/${tab.id}/screen.ansi`);
    if (state.active !== tab.id || !raw) return;
    const alt = raw[0] === "1";
    const ansi = raw.slice(1);
    if (isCompositionInput(tab)) {
      // Defer, do not drop: the PTY keeps running while the user composes.
      tab.pendingPaint = { alt, ansi };
    } else {
      paintSnapshot(tab, alt, ansi);
      clearError();
      statusEl.textContent = `tab ${tab.id} · ${tab.term.cols}×${tab.term.rows}`;
    }
  } catch (error) {
    report(error);
  }
  if (state.active === tab.id && !tab.socket && tab.reconnectTimer === null) {
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
  // A key held while switching tabs must not keep deleting in the new tab.
  stopAllRepeats();
  const previous = currentTab();
  if (previous) {
    stopPolling(previous);
    // A tab switched away mid-IME may never receive compositionend. Clear the
    // stale gate so it cannot swallow input when it becomes active again.
    previous.composing = false;
    previous.lastInputIsComposing = false;
    // A held retraction would otherwise be lost when the tab is reactivated.
    flushHold(previous);
    flushPendingPaint(previous);
  }

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
    startStream(next);
  }
  if (fromUser) {
    api(`/tabs/${id}/activate`, { method: "POST" })
      .then(refresh)
      .catch(report);
  }
}

function destroyTab(tab) {
  stopPolling(tab);
  // Drop any queued retraction: sending it would hit a tab that no longer
  // exists.
  tab.pendingText = null;
  if (tab.reconcileTimer !== null) {
    clearTimeout(tab.reconcileTimer);
    tab.reconcileTimer = null;
  }
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
    socket: null,
    reconnectTimer: null,
    streamGen: 0,
    alt: null,
    inputChain: Promise.resolve(),
    followCursor: true,
    scrollGuard: false,
    scrollGuardTimer: null,
    // IME / dictation state, driven by real DOM flags (see gating helpers).
    composing: false,
    lastInputType: "",
    lastInputIsComposing: false,
    lastKeyDownAt: 0,
    lastPasteAt: 0,
    lastCompositionEndAt: 0,
    // Utterance synchronisation state (see reconcileTo).
    sent: [],
    pendingText: null,
    reconcileTimer: null,
    keySentAt: 0,
    keySentText: "",
    pendingPaint: null,
    debugPaintLine: null,
    debugPaintAlt: null
  };
  state.tabs.set(id, tab);

  // The hidden textarea is the only place the browser reports IME state.
  // composition* is authoritative for pre-edit text; beforeinput supplies
  // inputType/isComposing so we can also guard platforms where those events are
  // unreliable. These listeners run AFTER xterm's own — same target, later
  // registration — but that no longer matters: every one of them feeds the same
  // idempotent reconciler, so an extra or missing call cannot duplicate text.
  const textarea = term.element && term.element.querySelector(".xterm-helper-textarea");
  if (textarea) {
    // The built-in keyboard dispatches events directly at this field; it does
    // not need focus. Blurring it here stops the OS soft keyboard from popping
    // up over the built-in one when a tap lands on the terminal.
    textarea.addEventListener("focus", () => {
      if (state.keyboardMode === "screen") textarea.blur();
    }, true);
    textarea.addEventListener("compositionstart", () => { tab.composing = true; }, true);
    textarea.addEventListener("compositionupdate", () => { tab.composing = true; }, true);
    textarea.addEventListener("compositionend", () => {
      tab.composing = false;
      tab.lastInputIsComposing = false;
      tab.lastInputType = "";
      tab.lastCompositionEndAt = Date.now();
      flushPendingPaint(tab);
      // The committed text is already in `value`. Reconcile from there instead
      // of from `event.data`: a commit that xterm also forwarded is then
      // impossible to send twice, and a commit xterm dropped is still sent.
      syncField(tab);
    }, true);
    textarea.addEventListener("beforeinput", (event) => {
      tab.lastInputType = event.inputType || "";
      tab.lastInputIsComposing = !!event.isComposing;
    }, true);
    textarea.addEventListener("input", () => {
      // xterm already transmitted this key from its own handler; this input
      // event is just the browser's DOM insertion of the same text. Drop the
      // field contents and stay out of the way. Matching the text (not just the
      // time) keeps a genuinely new character typed in quick succession, and
      // dictation that follows a keystroke, out of this branch.
      const raw = textareaText(tab);
      if (
        raw !== null &&
        Date.now() - (tab.keySentAt || 0) <= KEY_WINDOW_MS &&
        normalizeField(raw) === (tab.keySentText || "")
      ) {
        endUtterance(tab);
        return;
      }
      // Every other source ends up here with `value` already updated: typing
      // that xterm forwarded via input, dictation, paste, IME. Reconciling is
      // idempotent, so a second call for the same event sends nothing extra.
      syncField(tab);
    }, true);
    // A genuine key — not an IME placeholder (keyCode 229 / isComposing) —
    // proves no composition is in flight. Without this the flags below can
    // latch forever when compositionend never arrives (switching IME off,
    // losing focus mid-composition), and then every later character is treated
    // as pre-edit text and silently dropped. The key marker itself is recorded
    // on the pane's capture listener (see below); this one only clears flags.
    textarea.addEventListener("keydown", (event) => {
      if (!event.isComposing && event.keyCode !== 229) {
        tab.composing = false;
        tab.lastInputIsComposing = false;
        tab.lastInputType = "";
      }
    }, true);
    // Dismissing the keyboard ends the utterance: settle any held retraction
    // right away instead of waiting for the timer.
    textarea.addEventListener("blur", () => flushHold(tab), true);
  }

  // Capture on the pane, so the marker exists before ANY textarea listener —
  // including xterm's own keydown handler, which forwards some keys into onData
  // synchronously.
  wrapper.addEventListener("keydown", () => {
    tab.lastKeyDownAt = Date.now();
  }, true);
  wrapper.addEventListener("paste", () => {
    tab.lastPasteAt = Date.now();
  }, true);

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
  // xterm routes mouse reports through onBinary when the host requested the
  // legacy (X10/VT200/DRAG) encoding, and through onData for SGR. We forward
  // both: otherwise clicking in a mouse-mode app only moves xterm's cursor.
  if (typeof term.onBinary === "function") {
    term.onBinary((data) => {
      const text = textOf(data);
      dbg("ondata", tab, {
        text: clip(text), hex: hexOf(text), len: text.length,
        it: tab.lastInputType, ime: tab.lastInputIsComposing,
        composing: tab.composing, printable: false, binary: true
      });
      sendInput(tab, text, "key");
      endUtterance(tab);
    });
  }
  term.onData((data) => {
    const text = textOf(data);
    // Snapshot the decision inputs at the moment xterm handed us the bytes.
    dbg("ondata", tab, {
      text: clip(text),
      hex: hexOf(text),
      len: text.length,
      it: tab.lastInputType,
      ime: tab.lastInputIsComposing,
      composing: tab.composing,
      printable: isPrintable(text),
      kdMs: tab.lastKeyDownAt ? Date.now() - tab.lastKeyDownAt : null,
      ceMs: tab.lastCompositionEndAt ? Date.now() - tab.lastCompositionEndAt : null
    });
    // Pre-edit / half-formed characters must never reach the PTY.
    if (isCompositionInput(tab)) {
      dbg("gate", tab, { text: clip(text), why: "composing" });
      return;
    }
    if (!isPrintable(text)) {
      // Control key: CR, DEL, an arrow/Home/End sequence, a Ctrl chord. These
      // never enter the textarea, so there is nothing to reconcile and the
      // mirror can no longer describe the line — send and end the utterance.
      sendInput(tab, text, "key");
      endUtterance(tab);
      return;
    }
    if (keyDriven(tab)) {
      // xterm encoded this key correctly, so `data` IS the transmission. The
      // field merely mirrors it (and WebKit may have inserted a copy anyway);
      // clearing it keeps an accumulating field from replaying old characters.
      sendInput(tab, encodeTyped(text), "key");
      // The browser has usually queued its own DOM insertion for this same key,
      // so an input event is still coming. Remember which text was sent, so that
      // event can be recognised as a duplicate instead of being sent twice (the
      // space key used to arrive as two spaces for exactly this reason).
      tab.keySentAt = Date.now();
      tab.keySentText = text;
      endUtterance(tab);
      return;
    }
    // No key and no paste: dictation. Its `data` may be the whole re-recognised
    // sentence, so only `value` is trusted; the input event that carries this
    // same text reconciles it as well, which is harmless because reconciling is
    // idempotent.
    if (textareaText(tab) === null) {
      // No field to consult (should not happen): fall back to the event data.
      sendInput(tab, encodeTyped(text), "key");
      return;
    }
    syncField(tab);
  });
  bindDebug(tab, textarea);
  if (debug.on) {
    dbg("bind", tab, {
      cols: term.cols,
      rows: term.rows,
      dtoCols: dto.cols,
      dtoRows: dto.rows,
      textarea: !!textarea
    });
  }
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

sizeModeButton.addEventListener("click", () => {
  state.sizeMode = state.sizeMode === "pc" ? "browser" : "pc";
  applySizeModeUi();
  try {
    localStorage.setItem("rterminal.sizeMode", state.sizeMode);
  } catch {
    // Storage may be disabled; the toggle still works for this session.
  }
  // Tell the native side which side is authoritative. In browser mode force
  // one resize so the PC window aligns even if the grid number did not change.
  notifyClient(true);
  const tab = currentTab();
  if (tab) {
    fitTab(tab, state.sizeMode === "browser");
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

  // Never taller than the layout viewport. iPad Safari reports a visual
  // viewport LARGER than the layout viewport once its chrome collapses, and
  // sizing the app from that number overflowed the document: the page became
  // scrollable for no reason and dragging it only hid the top bar.
  document.documentElement.style.setProperty(
    "--app-height",
    `${Math.max(1, Math.floor(appViewportHeight()))}px`
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

// Height the app may occupy: the visual viewport (so it ends above the soft
// keyboard) clamped to the layout viewport (so it can never overflow).
function appViewportHeight() {
  const layout = document.documentElement.clientHeight || window.innerHeight || 0;
  const visual = visualViewport ? visualViewport.height || 0 : layout;
  if (!layout) return visual;
  if (!visual) return layout;
  return Math.min(visual, layout);
}

// Belt and braces for engines where overflow:clip is unavailable or the
// viewport was panned anyway (iOS scrolling an off-screen focused field into
// view). A no-op once the document is genuinely unscrollable.
function pinDocument() {
  if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
}

if (visualViewport) {
  syncViewportHeight();
  visualViewport.addEventListener("resize", () => {
    syncViewportHeight();
    revealBottom(currentTab());
    scheduleFit();
  });
  visualViewport.addEventListener("scroll", () => {
    pinDocument();
    syncViewportHeight();
    scheduleFit();
  });
} else {
  window.addEventListener("resize", () => {
    normalViewportHeight = window.innerHeight;
    syncViewportHeight();
  });
}
window.addEventListener("scroll", pinDocument);

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
buildScreenKeyboard();
applyKeyboardModeUi();
keyToolbar.hidden = !state.showToolbar;
document.body.classList.toggle("with-toolbar", state.showToolbar);

// The data-plane port comes from the server so a LAN client (iPad) uses the
// right port even when the CLI overrides the default.
api("/web/config")
  .then((config) => {
    if (config && config.ws_port) state.wsPort = Number(config.ws_port);
  })
  .catch(() => {})
  .finally(refresh);
setInterval(refresh, 1000);
