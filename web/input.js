function currentTab() {
  return state.active === null ? null : state.tabs.get(state.active) || null;
}

// A separate field for speech recognition and other input methods that rewrite
// their text while producing it. Nothing reaches the PTY until submission, so
// no prefix comparison, synthetic Backspace or utterance mirror is involved.
// Direct input through xterm remains available for ordinary keyboards and IME.
// Android soft keyboards often do not turn an empty-field Enter into a form
// submit (iOS does), so handle the gesture explicitly on both keydown and
// beforeinput. Empty submission is the documented shortcut for terminal Enter.
stagedInput.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.isComposing || event.keyCode === 229) return;
  event.preventDefault();
  stagedInputForm.requestSubmit();
});
stagedInput.addEventListener("beforeinput", (event) => {
  if (event.isComposing) return;
  if (event.inputType === "insertLineBreak" || event.inputType === "insertParagraph" || event.data === "\n") {
    event.preventDefault();
    stagedInputForm.requestSubmit();
  }
});

stagedInputForm.addEventListener("submit", (event) => {
  event.preventDefault();
  const tab = currentTab();
  const text = stagedInput.value;
  if (!tab) {
    report(new Error("No active terminal"));
    return;
  }
  // Non-empty text is inserted exactly as written, without an implicit CR.
  // An empty submission is the explicit shortcut for the terminal Enter key.
  sendInput(tab, text || "\r", "staged");
  stagedInput.value = "";
});

// Backspace normally edits only this staging field. Once the field is empty,
// forward the same gesture to the terminal instead. keydown covers hardware
// and most soft keyboards; beforeinput covers mobile engines that omit it.
// Cancelling keydown prevents the corresponding beforeinput, avoiding a
// duplicate DEL on browsers that implement both events.
function forwardEmptyStagedBackspace(event) {
  const isBackspace = event.type === "keydown"
    ? event.key === "Backspace" && event.keyCode !== 229
    : event.inputType === "deleteContentBackward";
  if (!isBackspace || event.isComposing || stagedInput.value !== "") return;
  const tab = currentTab();
  if (!tab) return;
  event.preventDefault();
  sendInput(tab, "\x7f", "staged");
}
stagedInput.addEventListener("keydown", forwardEmptyStagedBackspace);
stagedInput.addEventListener("beforeinput", forwardEmptyStagedBackspace);

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
      // onBinary strings are byte buckets (one char per PTY byte), not UTF-8;
      // encoding them as text corrupts legacy mouse reports >= 0x80. Build the
      // payload once so the WebSocket and its HTTP fallback agree byte-for-byte.
      const payload = source === "binary"
        ? Uint8Array.from(data, (ch) => ch.charCodeAt(0) & 0xff)
        : new TextEncoder().encode(data);
      const socket = tab.socket;
      if (socket && socket.readyState === WebSocket.OPEN && !state.clientBlocked) {
        socket.send(payload);
        return;
      }
      // Fallback: keep the HTTP input endpoint alive for old servers and for
      // the short window while the WebSocket is reconnecting.
      return api(`/tabs/${tab.id}/input`, {
        method: "POST",
        body: payload,
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
// Mobile soft keyboards (Android Chrome in particular) can deliver the
// matching `input` event well after xterm already sent the key and cleared the
// field. A short window dropped the dedupe and the same character was sent
// twice, so keep a generous window for the dedupe check only.
const DEDUPE_WINDOW_MS = 2000;
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
// into Ctrl+Arrow etc. instead of appending to a pre-baked byte string. A
// literal chord already spells out its own modifiers and ignores latches.
function encodeToolbarKey(key) {
  if (key.literal) return key.data;
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
  const stateText = mode === "locked" ? "locked" : mode === "once" ? "next key only" : "off";
  return `${key.label} · ${stateText} | Tap: next key only; double-tap: lock; tap again: off`;
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
  { label: "^R", data: "\x12", title: "reverse search" },
  // One-tap chord: CSI 1;5 F is what xterm sends for Ctrl+End.
  { label: "^End", data: "\x1b[1;5F", literal: true, title: "Ctrl+End" }
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

