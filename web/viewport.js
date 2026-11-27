
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
    if (response.status === 401) showAuthOverlay("Session expired. Unlock again.");
    throw new Error(`${response.status} ${detail}`);
  }
  const contentType = response.headers.get("content-type") || "";
  return contentType.includes("json") ? response.json() : response.text();
}

new ResizeObserver(scheduleFit).observe(panes);
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
    recordViewport("keyboard-close");
    // iPad Chrome scrolls the root element to reveal a focused bottom input.
    // Once the keyboard is gone, end that focus-driven pan and restore the
    // root immediately and again after the viewport transition settles.
    const active = document.activeElement;
    if (active && typeof active.blur === "function") active.blur();
    pinDocument();
    requestAnimationFrame(pinDocument);
    // Keyboard closed: wait for the layout to settle before allowing another
    // window -> PTY resize, otherwise closing it replays the resize we
    // deliberately skipped on open.
    if (keyboardTimer !== null) clearTimeout(keyboardTimer);
    keyboardTimer = setTimeout(() => {
      keyboardTimer = null;
      normalViewportHeight = visualViewport.height || window.innerHeight;
      pinDocument();
      scheduleFit();
    }, 200);
  }
}

// The root element and body both use this height. window.innerHeight and the
// initial layout viewport include iPad Chrome's browser-chrome-covered strip;
// visualViewport.height is the actual area available to the application.
function appViewportHeight() {
  const fallback = window.innerHeight || document.documentElement.clientHeight || 0;
  return visualViewport ? visualViewport.height || fallback : fallback;
}

// Belt and braces for engines where overflow:clip is unavailable or the
// viewport was panned anyway (iOS scrolling an off-screen focused field into
// view). A no-op once the document is genuinely unscrollable.
function pinDocument() {
  const scroller = document.scrollingElement;
  if (scroller && (scroller.scrollLeft !== 0 || scroller.scrollTop !== 0)) {
    scroller.scrollLeft = 0;
    scroller.scrollTop = 0;
  }
  if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
}

const scrollDebugTimes = new WeakMap();
function recordDomScroll(event) {
  const target = event.target;
  const element = target === document ? document.scrollingElement : target;
  if (!element) return;
  const now = Date.now();
  if (now - (scrollDebugTimes.get(element) || 0) < 250) return;
  scrollDebugTimes.set(element, now);
  const active = document.activeElement;
  dbg("dom-scroll", currentTab(), {
    target: element.id || element.className || element.tagName,
    top: Math.round(element.scrollTop || 0),
    left: Math.round(element.scrollLeft || 0),
    range: [element.clientWidth, element.clientHeight, element.scrollWidth, element.scrollHeight],
    active: active ? active.id || active.className || active.tagName : null,
    ...sessionFacts(currentTab())
  });
}
// Scroll does not bubble, but capture sees the root, pane and xterm scrolling
// separately. This identifies the actual moving container on keyboard close.
document.addEventListener("scroll", recordDomScroll, true);

// CSS overflow/touch-action alone does not stop iPad Chrome from scrolling the
// root. Arbitrate the gesture at document level: an inner scroller may consume
// a drag only while it can still move in that direction. At either boundary —
// including an exactly fitted pane — the outer document must not take over.
let touchScrollStart = null;
document.addEventListener("touchstart", (event) => {
  const touch = event.touches[0];
  const target = event.target instanceof Element ? event.target : null;
  touchScrollStart = touch && target
    ? { x: touch.clientX, y: touch.clientY, pane: target.closest(".pane") }
    : null;
}, { passive: true });

document.addEventListener("touchmove", (event) => {
  const target = event.target instanceof Element ? event.target : null;
  if (target && target.closest(".screen-keyboard, .key-scroll, .tabstrip, .help-card")) return;
  const touch = event.touches[0];
  const start = touchScrollStart;
  if (touch && start && start.pane && event.touches.length === 1) {
    const dx = start.x - touch.clientX;
    const dy = start.y - touch.clientY;
    const horizontal = Math.abs(dx) > Math.abs(dy);
    const delta = horizontal ? dx : dy;
    const current = horizontal ? start.pane.scrollLeft : start.pane.scrollTop;
    const maximum = horizontal
      ? start.pane.scrollWidth - start.pane.clientWidth
      : start.pane.scrollHeight - start.pane.clientHeight;
    const canScroll = delta > 0
      ? current < maximum - 1
      : current > 1;
    if (canScroll && Math.abs(delta) > 2) return;
  }
  event.preventDefault();
}, { passive: false });
document.addEventListener("touchend", () => { touchScrollStart = null; }, { passive: true });
document.addEventListener("touchcancel", () => { touchScrollStart = null; }, { passive: true });

let lastViewportDebugAt = 0;
function recordViewport(event) {
  const now = Date.now();
  if (now - lastViewportDebugAt < 250) return;
  lastViewportDebugAt = now;
  dbg("viewport", currentTab(), { event, ...sessionFacts(currentTab()) });
}

if (visualViewport) {
  syncViewportHeight();
  visualViewport.addEventListener("resize", () => {
    syncViewportHeight();
    revealBottom(currentTab());
    scheduleFit();
    recordViewport("resize");
  });
  visualViewport.addEventListener("scroll", () => {
    recordViewport("scroll-before-pin");
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

