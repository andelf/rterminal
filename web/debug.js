
function showAuthOverlay(message = "") {
  authError.textContent = message;
  authOverlay.hidden = false;
  authToken.focus({ preventScroll: true });
}

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
  return Array.from(new TextEncoder().encode(text), (byte) =>
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
  if (debug.queue.length === 0 || !webAuthorized) return;
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
  return {
    "X-Rterminal-Client": clientId,
    ...(debug.on ? { "X-Rterminal-Debug": "1" } : {})
  };
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
    offset: viewport ? [Math.round(viewport.offsetLeft), Math.round(viewport.offsetTop)] : null,
    scroll: [Math.round(window.scrollX), Math.round(window.scrollY)],
    scrollSize: document.scrollingElement
      ? [document.scrollingElement.scrollWidth, document.scrollingElement.scrollHeight]
      : null,
    appHeight: appViewportHeight(),
    rootHeight: Math.round(document.documentElement.getBoundingClientRect().height),
    bodyHeight: Math.round(document.body.getBoundingClientRect().height),
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
    statusEl.textContent = "Input debug recording enabled (saved to the server log)";
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
    statusEl.textContent = "Input debug recording stopped";
  }
  applyDebugUi();
  // The byte trace flag is part of the WebSocket URL, so reconnect to apply it.
  const active = currentTab();
  if (active && active.socket) startStream(active);
});

