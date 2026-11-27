const toolbarPressTimers = new WeakMap();
function flashToolbarKey(button) {
  const previous = toolbarPressTimers.get(button);
  if (previous) clearTimeout(previous);
  button.classList.add("key-down");
  toolbarPressTimers.set(button, setTimeout(() => {
    toolbarPressTimers.delete(button);
    button.classList.remove("key-down");
  }, 120));
}

function makeKeyButton(key, onPress) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = key.label;
  button.title = key.title || key.label;
  if (key.mod) button.classList.add("key-mod");
  button.addEventListener("pointerdown", () => flashToolbarKey(button));
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
// with modifier flags on the event, and xterm does the encoding. A one-second
// modifier latch is available; textarea reconciliation and IME are deliberately
// absent. The system keyboard is one button away for those.

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
    wide: true, caps: true, title: "Caps Lock (toggle letter case)"
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
    skMod("⌃", "ctrl", "ControlLeft", 17, { key: "Control" }),
    skMod("⌥", "alt", "AltLeft", 18, { key: "Alt" }),
    skMod("⌘", "meta", "MetaLeft", 91, { key: "Meta" }),
    sk("Space", " ", "Space", 32, null, { space: true }),
    skMod("⌘", "meta", "MetaRight", 92, { key: "Meta" }),
    skMod("⌥", "alt", "AltRight", 18, { key: "Alt" }),
    skMod("⌃", "ctrl", "ControlRight", 17, { key: "Control" })
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

// Effective modifiers on the on-screen keyboard. Most are held only while a
// finger is down; a modifier held for one second moves into screenLatchedMods
// instead and survives its pointerup for exactly one subsequent non-modifier.
const SCREEN_MOD_LATCH_MS = 1000;
const screenHeldMods = new Set();
const screenHeldDefs = new Map();
const screenLatchedMods = new Set();
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
  screenLatchedMods.clear();
  refreshScreenModButtons();
}

function releaseLatchedScreenModifiers() {
  if (!screenLatchedMods.size) return;
  // The physical modifier pointer already sent its keyup when the latch was
  // established. Only the effective flags remain to be cleared here.
  for (const mod of screenLatchedMods) {
    screenHeldMods.delete(mod);
    screenHeldDefs.delete(mod);
  }
  screenLatchedMods.clear();
  refreshScreenModButtons();
  dbg("screen-mod-consume", null, {});
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
    const latched = screenLatchedMods.has(mod);
    for (const button of buttons) {
      button.classList.toggle("sk-held", held);
      button.classList.toggle("sk-latched", latched);
      button.setAttribute("aria-pressed", String(latched));
    }
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
  button.title = def.title || (def.mod
    ? `${def.label} (hold; hold for 1 second to latch for the next key)`
    : def.label);
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
    button.setAttribute("aria-pressed", "false");
  }
  if (def.caps) {
    screenCapsButton = button;
    button.setAttribute("aria-pressed", "false");
  }

  let pointer = null;
  let activeDef = null;
  let delayTimer = null;
  let repeatTimer = null;
  let pressTimer = null;
  const stop = () => {
    // Share the global stop registry with the compact toolbar: switching tabs
    // or keyboard modes must be able to cancel an in-flight key repeat.
    repeatStops.delete(stop);
    if (delayTimer !== null) clearTimeout(delayTimer);
    if (repeatTimer !== null) clearInterval(repeatTimer);
    if (pressTimer !== null) clearTimeout(pressTimer);
    delayTimer = null;
    repeatTimer = null;
    pressTimer = null;
    if (pointer !== null && button.hasPointerCapture?.(pointer)) {
      try { button.releasePointerCapture(pointer); } catch {}
    }
    pointer = null;
    button.classList.remove("sk-down");
    if (!activeDef) return;
    const released = activeDef;
    if (released.mod && !screenLatchedMods.has(released.mod)) {
      screenHeldMods.delete(released.mod);
      screenHeldDefs.delete(released.mod);
      refreshScreenModButtons();
    }
    dispatchScreenKey("keyup", released);
    activeDef = null;
    // A latched modifier applies to one complete non-modifier keystroke. Its
    // keyup is dispatched before the latch is cleared, so xterm sees matching
    // modifier flags on both halves of that keystroke.
    if (!released.mod) releaseLatchedScreenModifiers();
  };

  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    button.blur();
    stop();
    repeatStops.add(stop);
    pointer = event.pointerId;
    // A synthetic PointerEvent (the replay harness) has no live pointer to
    // capture; attempting it would emit lostpointercapture and release the
    // modifier before the next key is pressed.
    if (event.isTrusted) {
      try { button.setPointerCapture?.(event.pointerId); } catch {}
    }
    button.classList.add("sk-down");
    // A key press is a short visual acknowledgement, not a persistent mode.
    // Held modifiers/Caps retain their separate state colours after this.
    pressTimer = setTimeout(() => {
      pressTimer = null;
      button.classList.remove("sk-down");
    }, 120);
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
      // Tapping an already latched modifier is the explicit way to cancel it
      // without spending it on a key.
      if (screenLatchedMods.has(def.mod)) {
        screenLatchedMods.delete(def.mod);
        screenHeldMods.delete(def.mod);
        screenHeldDefs.delete(def.mod);
        activeDef = null;
        refreshScreenModButtons();
        return;
      }
      if (screenHeldMods.has(def.mod)) { activeDef = null; return; }
      screenHeldMods.add(def.mod);
      screenHeldDefs.set(def.mod, def);
      refreshScreenModButtons();
      delayTimer = setTimeout(() => {
        delayTimer = null;
        if (activeDef !== def) return;
        screenLatchedMods.add(def.mod);
        refreshScreenModButtons();
        dbg("screen-mod-latch", null, { mod: def.mod });
      }, SCREEN_MOD_LATCH_MS);
    }
    dispatchScreenKey("keydown", activeDef);
    // xterm deliberately ignores a bare keydown whose key is an upper-case
    // letter: it waits for the browser's keypress(charCode) that normally
    // follows. The same applies to Space: evaluateKeyboardEvent only encodes
    // printable keys with keyCode >= 48 on keydown, so keyCode 32 falls through
    // with no key at all. A synthetic event has no default action to supply
    // that keypress, so do it here. Ctrl+Space is already encoded by keydown as
    // a control byte and Alt/Meta+Space get their own escape, so never
    // double-send.
    const needsKeypress = /^[A-Z]$/.test(activeDef.key)
      || (activeDef.key === " " && !screenHeldMods.has("ctrl")
        && !screenHeldMods.has("alt") && !screenHeldMods.has("meta"));
    if (needsKeypress) {
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

// A quick horizontal movement is usually a finger slipping off a key, not a
// request to move the whole keyboard. Require a half-second touch before a
// horizontal drag can take over. CSS touch-action:none prevents the browser's
// native panner from bypassing this gate; wheel/trackpad scrolling is unchanged.
const SCREEN_SCROLL_HOLD_MS = 500;
let screenScrollGesture = null;

screenKeyboard.addEventListener("pointerdown", (event) => {
  if (event.pointerType !== "touch" || !event.isPrimary) return;
  screenScrollGesture = {
    pointer: event.pointerId,
    x: event.clientX,
    lastX: event.clientX,
    at: performance.now(),
    scrolling: false
  };
});

screenKeyboard.addEventListener("pointermove", (event) => {
  const gesture = screenScrollGesture;
  if (!gesture || event.pointerId !== gesture.pointer) return;
  if (!gesture.scrolling) {
    if (performance.now() - gesture.at < SCREEN_SCROLL_HOLD_MS ||
        Math.abs(event.clientX - gesture.x) < 8) return;
    gesture.scrolling = true;
    gesture.lastX = event.clientX;
    // Cancel the key/repeat that the finger originally landed on before the
    // gesture becomes a scroll. Latched modifiers have no active pointer here.
    stopAllRepeats();
  }
  event.preventDefault();
  screenKeyboard.scrollLeft -= event.clientX - gesture.lastX;
  gesture.lastX = event.clientX;
});

function endScreenScrollGesture(event) {
  if (screenScrollGesture && event.pointerId === screenScrollGesture.pointer) {
    screenScrollGesture = null;
  }
}
screenKeyboard.addEventListener("pointerup", endScreenScrollGesture);
screenKeyboard.addEventListener("pointercancel", endScreenScrollGesture);

function buildScreenKeyboard() {
  // Rebuilding is only used by the Win/Mac layout switch. Drop the old button
  // tree (and its modifiers) instead of leaving stale handlers on detached
  // nodes that would still be reachable through screenModButtons.
  stopAllRepeats();
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
  // Keep Win/Mac switching in the right-hand key area, not a separate header.
  side.prepend(skLayoutToggle);
  layout.appendChild(side);
  skLayoutHolder.replaceChildren(layout);
  updateScreenShiftLabels();
  const mac = state.screenLayout === "mac";
  skLayoutToggle.textContent = mac ? "Win layout" : "Mac layout";
  skLayoutToggle.title = mac
    ? "Mac layout active (⌘/⌥/⌃). Switch to Win layout (Win/Alt/Ctrl)."
    : "Win layout active (Win/Alt/Ctrl). Switch to Mac layout (⌘/⌥/⌃).";
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
  keyboardModeButton.setAttribute("aria-label", screen
    ? "Screen keyboard active; switch to system keyboard"
    : "System keyboard active; switch to screen keyboard");
  screenKeyboard.hidden = !screen;
  document.body.classList.toggle("with-screen-keyboard", screen);
}

function setKeyboardMode(mode) {
  if (state.keyboardMode === mode) return;
  // Caps Lock is a virtual lock: never let it survive a mode switch and
  // silently change what the next typed letter means.
  screenCapsLock = false;
  stopAllRepeats();
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

