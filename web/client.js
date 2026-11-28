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

function fitTab(tab, forceSync = false, allowKeyboardSync = false) {
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
  // Automatic following stays frozen while the soft keyboard is up or after
  // Sync size locked the grid. An explicit Sync size click is the only path
  // allowed to synchronize the keyboard-reduced viewport.
  const canSync = allowKeyboardSync ||
    (!state.sizeLocked && keyboardInset === 0 && keyboardTimer === null);
  if (state.sizeMode === "browser" && canSync) {
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
  syncConnection();
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
    tab.wsFailures = 0;
    clearError();
    syncConnection();
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
    if (state.active !== tab.id || state.clientBlocked) {
      syncConnection();
      return;
    }
    tab.wsFailures += 1;
    // A couple of quick retries cover a server restart. After that, fall back
    // to snapshots so the view is never permanently frozen.
    if (tab.wsFailures >= 3) {
      poll(tab);
      return;
    }
    tab.reconnectTimer = setTimeout(() => startStream(tab), 300 * tab.wsFailures);
    syncConnection();
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
  syncConnection();
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
    // Release screen-keyboard modifiers while currentTab() still points at
    // the old tab; otherwise they leak into the new tab as ctrl/alt/meta flags.
    releaseAllScreenModifiers();
    // A tab switched away mid-IME may never receive compositionend. Clear all
    // three gate inputs — leaving `lastInputType` set swallowed the first key
    // typed after coming back (xterm's listener runs before ours).
    previous.composing = false;
    previous.lastInputIsComposing = false;
    previous.lastInputType = "";
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
  // An open scrollback dump belongs to the tab it was taken from; follow the
  // newly active tab (or close when there is none).
  syncScrollback();
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

function touchWheelCell(tab, event) {
  const screen = tab.term.element && tab.term.element.querySelector(".xterm-screen");
  const metrics = cellMetrics(tab);
  if (!screen || !metrics) return { col: 1, row: 1 };
  const rect = screen.getBoundingClientRect();
  return {
    col: Math.max(1, Math.min(tab.term.cols, Math.floor((event.clientX - rect.left) / metrics.cw) + 1)),
    row: Math.max(1, Math.min(tab.term.rows, Math.floor((event.clientY - rect.top) / metrics.ch) + 1))
  };
}

function emitTouchWheel(tab, button, event) {
  const mouseMode = tab.term.modes && tab.term.modes.mouseTracking !== "none";
  if (mouseMode) {
    const { col, row } = touchWheelCell(tab, event);
    sendInput(tab, `\x1b[<${button};${col};${row}M`, "key");
  } else if (typeof tab.term.scrollLines === "function") {
    tab.followCursor = false;
    armScrollGuard(tab);
    tab.term.scrollLines(button === 65 ? 1 : -1);
  }
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
    // Retry budget belongs to the tab: a global counter let failures in one
    // tab consume another tab's chances to reconnect after a server restart.
    wsFailures: 0,
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
    beforeInputIsKey: false,
    handledBeforeInputKey: false,
    handledBeforeInputText: "",
    lastPasteAt: 0,
    lastCompositionEndAt: 0,
    // Utterance synchronisation state (see reconcileTo).
    sent: [],
    pendingText: null,
    reconcileTimer: null,
    keySentAt: 0,
    keySentText: "",
    pasteSentAt: 0,
    pendingPaint: null,
    debugPaintLine: null,
    debugPaintAlt: null,
    touchWheel: null
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
    // Ask Chrome not to offer stored passwords, cards or addresses for the
    // terminal. This is only a hint: mobile Chrome may still show its native
    // keyboard accessory bar, including the Done button.
    textarea.setAttribute("autocomplete", "off");
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
    // Capture phase runs before xterm's own beforeinput handler, which is
    // what emits onData. Android delivers `input`/beforeinput before the late
    // `keydown`, so keydown timing cannot classify the keystroke; the
    // beforeinput metadata can.
    textarea.addEventListener("beforeinput", (event) => {
      tab.handledBeforeInputKey = false;
      tab.beforeInputIsKey =
        (event.inputType === "insertText" || event.inputType === "insertCompositionText") &&
        typeof event.data === "string" &&
        event.data.length === 1 &&
        !event.isComposing;
    }, true);
    textarea.addEventListener("beforeinput", (event) => {
      tab.lastInputType = event.inputType || "";
      tab.lastInputIsComposing = !!event.isComposing;
    }, true);
    textarea.addEventListener("input", () => {
      const raw = textareaText(tab);
      // A paste is not a single key, so there is no key marker to compare: an
      // `insertFromPaste` input that follows an onData already forwarded as a
      // paste is the browser's DOM insertion of those same bytes (Safari on
      // iPad with a hardware keyboard always performs it). Drop it.
      if (
        tab.lastInputType === "insertFromPaste" &&
        Date.now() - (tab.pasteSentAt || 0) <= DEDUPE_WINDOW_MS
      ) {
        dbg("dedupe-paste", tab, { text: clip(raw) });
        endUtterance(tab);
        return;
      }
      // xterm already transmitted this key from its own handler; this input
      // event is just the browser's DOM insertion of the same text. Drop the
      // field contents and stay out of the way. Matching the text (not just the
      // time) keeps a genuinely new character typed in quick succession, and
      // dictation that follows a keystroke, out of this branch.
      if (
        raw !== null &&
        Date.now() - (tab.keySentAt || 0) <= DEDUPE_WINDOW_MS &&
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

  // The right third is a touch-wheel zone, excluding the browser's outer
  // gesture edge. The gesture is claimed only after a clear vertical drag,
  // so taps and the left two-thirds keep xterm's normal selection behavior.
  wrapper.addEventListener("pointerdown", (event) => {
    if (event.pointerType !== "touch" || !event.isPrimary || event.touches > 1) return;
    const rect = wrapper.getBoundingClientRect();
    if (event.clientX < rect.left + rect.width * 2 / 3 || event.clientX > rect.right - 24) return;
    tab.touchWheel = {
      pointer: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      lastY: event.clientY,
      remaining: 0,
      active: false
    };
  });
  wrapper.addEventListener("pointermove", (event) => {
    const gesture = tab.touchWheel;
    if (!gesture || gesture.pointer !== event.pointerId) return;
    const totalX = event.clientX - gesture.x;
    const totalY = event.clientY - gesture.y;
    if (!gesture.active) {
      if (Math.abs(totalY) < 10) return;
      if (Math.abs(totalX) > Math.abs(totalY)) {
        tab.touchWheel = null;
        return;
      }
      gesture.active = true;
      if (typeof tab.term.clearSelection === "function") tab.term.clearSelection();
      if (event.isTrusted) {
        try { wrapper.setPointerCapture(event.pointerId); } catch {}
      }
    }
    event.preventDefault();
    event.stopPropagation();
    gesture.remaining += event.clientY - gesture.lastY;
    gesture.lastY = event.clientY;
    const metrics = cellMetrics(tab);
    const step = metrics ? metrics.ch : 16;
    while (Math.abs(gesture.remaining) >= step) {
      // Natural touch direction: swipe up moves forward, swipe down moves back.
      const button = gesture.remaining < 0 ? 65 : 64;
      gesture.remaining += gesture.remaining < 0 ? step : -step;
      emitTouchWheel(tab, button, event);
    }
  });
  const finishTouchWheel = (event) => {
    const gesture = tab.touchWheel;
    if (!gesture || gesture.pointer !== event.pointerId) return;
    if (gesture.active) {
      event.preventDefault();
      event.stopPropagation();
    }
    tab.touchWheel = null;
  };
  wrapper.addEventListener("pointerup", finishTouchWheel);
  wrapper.addEventListener("pointercancel", finishTouchWheel);

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
      // onBinary strings are byte buckets (one char per PTY byte), not UTF-8.
      // The "key" route would expand bytes >= 0x80 into two UTF-8 bytes and
      // corrupt legacy mouse reports.
      sendInput(tab, text, "binary");
      endUtterance(tab);
    });
  }
  term.onData((data) => {
    const text = textOf(data);
    // Android may invoke onData twice for one keystroke (before, then after
    // the late keydown). The first pass already handled it; drop the replay.
    if (tab.handledBeforeInputKey && text === tab.handledBeforeInputText) {
      tab.handledBeforeInputKey = false;
      dbg("dedupe-late", tab, { text: clip(text) });
      return;
    }
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
    // xterm forwards a clipboard paste once, from its paste event. It does not
    // cancel the browser's default insertion into the hidden textarea, though:
    // Safari on iPad with a hardware keyboard writes the same clipboard text
    // there and fires `insertFromPaste` right afterwards. Remember that the
    // paste has already been transmitted, so the input listener can drop the
    // DOM copy instead of reconciling it into a second copy of the text.
    if (Date.now() - (tab.lastPasteAt || 0) <= KEY_WINDOW_MS) {
      tab.pasteSentAt = Date.now();
    }
    if (!isPrintable(text)) {
      // Control key: CR, DEL, an arrow/Home/End sequence, a Ctrl chord. These
      // never enter the textarea, so there is nothing to reconcile and the
      // mirror can no longer describe the line — send and end the utterance.
      sendInput(tab, text, "key");
      endUtterance(tab);
      return;
    }
    if (keyDriven(tab) || tab.beforeInputIsKey) {
      // Android Chrome can fire `input` (and therefore the reconcile route)
      // before the late `keydown` that makes this onData key-driven. In that
      // case the character has already reached the PTY through reconcile, so
      // sending it again here would duplicate it. Recognise that by the mirror
      // state instead of a timing heuristic: if `sent` already holds this text,
      // it has been transmitted and there is nothing more to do.
      const alreadySent = (tab.sent || []).length > 0 &&
        (tab.sent || []).slice(-(Array.from(text).length)).join("") === text;
      if (alreadySent) {
        dbg("dedupe-key", tab, { text: clip(text) });
        endUtterance(tab);
        return;
      }
      // xterm encoded this key correctly, so `data` IS the transmission. The
      // field merely mirrors it (and WebKit may have inserted a copy anyway);
      // clearing it keeps an accumulating field from replaying old characters.
      if (tab.beforeInputIsKey) {
        tab.handledBeforeInputKey = true;
        tab.handledBeforeInputText = text;
      }
      tab.beforeInputIsKey = false;
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
      // No tab is left to describe; never keep an old dump on screen.
      syncScrollback();
      emptyState.hidden = false;
      statusEl.textContent = "no tabs";
      clearError();
      return;
    }
    emptyState.hidden = true;
    if (active !== state.active) activate(active, false);
    else updateTabButtons();
    // Re-sync the visible tab with the latest native dimensions, or re-fit it
    // to this viewport when browser-size mode is enabled.
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
    syncConnection();
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
  state.sizeLocked = false;
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

syncSizeButton.addEventListener("click", () => {
  const tab = currentTab();
  if (!tab || state.sizeMode !== "browser") return;
  state.sizeLocked = true;
  applySizeModeUi();
  // This explicit action intentionally supports capturing the reduced height
  // while the keyboard is open. Later viewport changes only adjust padding.
  fitTab(tab, true, true);
  applyPadding(tab);
});

const scrollbackText = document.getElementById("scrollback-text");
// The overlay is a snapshot of one tab, so it follows the active tab: switching
// tabs reloads it instead of leaving another tab's text under this tab's
// chrome. Each load is tagged, so a slow response for the previous tab cannot
// overwrite the newer one.
let scrollbackTabId = null;
let scrollbackLoad = 0;

function closeScrollback() {
  scrollbackView.hidden = true;
  scrollbackTabId = null;
  scrollbackLoad += 1;
}

// The dump ends at the last non-empty row of the newest screen, so the line
// the terminal is currently showing can be counted back from that end. Opening
// the view lands on what the user was looking at instead of the oldest
// history. Blank rows below the last content are absent from the dump, hence
// the blankBelow correction.
function scrollToTerminalViewport(tab) {
  const term = tab && tab.term;
  const buffer = term && term.buffer && term.buffer.active;
  if (!buffer || !term.rows) return;
  const rows = term.rows;
  let blankBelow = 0;
  for (let i = buffer.baseY + rows - 1; i >= buffer.baseY; i--) {
    const line = buffer.getLine(i);
    if (line && line.translateToString(true) !== "") break;
    blankBelow++;
  }
  const lines = scrollbackText.textContent.split("\n");
  // Every row is terminated by "\n"; the empty tail is not a content line.
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  const scrolledUp = Math.max(0, buffer.baseY - buffer.viewportY);
  const top = Math.min(
    Math.max(0, lines.length + blankBelow - rows - scrolledUp),
    Math.max(0, lines.length - 1)
  );
  const lineHeight = parseFloat(getComputedStyle(scrollbackText).lineHeight);
  if (lineHeight > 0) scrollbackText.scrollTop = top * lineHeight;
}

async function loadScrollback(tab) {
  const seq = ++scrollbackLoad;
  scrollbackTabId = tab.id;
  scrollbackText.textContent = "";
  try {
    const text = await api(`/tabs/${tab.id}/scrollback`);
    if (seq !== scrollbackLoad) return;
    scrollbackText.textContent = text;
    scrollToTerminalViewport(tab);
  } catch (error) {
    if (seq !== scrollbackLoad) return;
    scrollbackText.textContent =
      `⚠ ${error instanceof Error ? error.message : String(error)}`;
  }
}

function syncScrollback() {
  if (scrollbackView.hidden) return;
  const tab = currentTab();
  if (!tab) {
    closeScrollback();
    return;
  }
  if (tab.id !== scrollbackTabId) loadScrollback(tab);
}

document.getElementById("scrollback-close").addEventListener("click", closeScrollback);

document.getElementById("scrollback").addEventListener("click", () => {
  if (!scrollbackView.hidden) {
    closeScrollback();
    return;
  }
  const tab = currentTab();
  if (!tab) return;
  scrollbackView.hidden = false;
  loadScrollback(tab);
});

