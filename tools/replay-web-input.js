// Replays input scenarios captured from a real iPad against the live web client
// in headless Chrome, then reports the bytes that actually reached the PTY.
// Development aid only: no dependencies (Node's built-in WebSocket + CDP).
//
//   node tools/replay-web-input.js [scenario...]     (default: all)
//
// Scenarios: spaces, dictation, ime
const { spawn } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const CHROME = process.env.RTERMINAL_CHROME
  || "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const PORT = Number(process.env.RTERMINAL_CDP_PORT || 9333);
const TARGET = process.env.RTERMINAL_WEB_URL || "http://127.0.0.1:7878/web/";
const LOG_URL = new URL("/web/debug/log?lines=400", TARGET).toString();
const PROFILE = path.join(os.tmpdir(), "rterminal-replay");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// --- scenarios -------------------------------------------------------------
// `value` is what the hidden textarea holds after the step, `data` what the
// input event reports, `key` an optional keydown (iOS reports keyCode 229 for
// spaces, 32 for a hardware space).

const SCENARIOS = {
  // Five space taps through the REAL input pipeline. WebKit is what inserts a
  // NO-BREAK SPACE into the field after xterm cancels the key, so a synthesised
  // event cannot reproduce it. Expect five plain 0x20, no c2 a0, no 0x7f.
  spaces: {
    native: [
      { key: " ", code: "Space", vk: 32, text: " " },
      { key: " ", code: "Space", vk: 32, text: " " },
      { key: " ", code: "Space", vk: 32, text: " " },
      { key: " ", code: "Space", vk: 32, text: " " },
      { key: " ", code: "Space", vk: 32, text: " " }
    ]
  },
  // Dictation: two recognition passes, each one a select-all + insert of the
  // full sentence. Expect the sentence exactly once.
  dictation: {
    steps: [
      "好的",
      "好的，现",
      "好的，现在我们可以",
      "好的，现在我们可以测",
      "好的，现在我们可以测试一",
      "好的，现在我们可以测试一下",
      "好的，",
      "好的，现在",
      "好的，现在我们",
      "好的，现在我们可以",
      "好的，现在我们可以测试"
    ].map((value) => ({ value, data: value, inputType: "insertText", delay: 300 }))
  },
  // REAL keys through the browser input pipeline (CDP Input.dispatchKeyEvent),
  // not synthesised DOM events. This is the scenario that matters for ordinary
  // typing: a synthesised `input` event skips the editor's own key handling and
  // hides the fact that xterm registers its listener before ours, so a scheme
  // that asks onData to defer to the textarea silently drops every keystroke.
  // Expect: a b c d, one space, one DEL, one CR.
  keys: {
    native: [
      { key: "a", code: "KeyA", vk: 65, text: "a" },
      { key: "b", code: "KeyB", vk: 66, text: "b" },
      { key: "c", code: "KeyC", vk: 67, text: "c" },
      { key: "d", code: "KeyD", vk: 68, text: "d" },
      { key: " ", code: "Space", vk: 32, text: " " },
      { key: "Backspace", code: "Backspace", vk: 8 },
      { key: "Enter", code: "Enter", vk: 13 }
    ]
  },
  // Plain typing on a hardware-ish keyboard: xterm encodes the key itself and
  // no input event follows. Expect one byte per key.
  typing: {
    steps: ["e", "c", "h", "o"].map((key) => ({
      key: { key, keyCode: key.toUpperCase().charCodeAt(0) },
      delay: 200
    }))
  },
  // Exact sequence from a captured iPad session: dictation, then one
  // recognition pass that retracts the tail and rebuilds it
  // ("测试一下内容" → "测试" → "测试一下" → "测试一下内容"). Expect
  // 测/试/一下/内容 and NO backspaces — the retraction settles inside the hold
  // window. An earlier build froze the held value at "测试一下" and then deleted
  // the two characters the user had just re-dictated.
  rewrite: {
    steps: [
      { value: "测", data: "测", inputType: "insertText", delay: 120 },
      { value: "测试", data: "测试", inputType: "insertText", delay: 120 },
      { value: "测试一下", data: "测试一下", inputType: "insertText", delay: 120 },
      { value: "测试一下内容", data: "测试一下内容", inputType: "insertText", delay: 900 },
      { value: "测试", data: "测试", inputType: "insertText", delay: 60 },
      { value: "测试一下", data: "一下", inputType: "insertText", delay: 60 },
      { value: "测试一下内容", data: "内容", inputType: "insertText", delay: 900 }
    ]
  },
  // Typing, then dictating into the same line: the typed characters must not be
  // swallowed or deleted by the reconciler.
  mixed: {
    steps: [
      { key: { key: "l", keyCode: 76 }, delay: 80 },
      { key: { key: "s", keyCode: 83 }, delay: 80 },
      { value: "你好", data: "你好", inputType: "insertText", delay: 900 }
    ]
  },
  // Dictate, submit, dictate again: the submitted sentence must not be
  // resurrected into the next utterance. Expect 你好, CR, 再见.
  commit: {
    steps: [
      { value: "你好", data: "你好", inputType: "insertText", delay: 400 },
      { key: { key: "Enter", keyCode: 13 }, delay: 400 },
      { value: "再见", data: "再见", inputType: "insertText", delay: 900 }
    ]
  },
  // IME commit: pre-edit is gated, the commit must go out exactly once even
  // though xterm raises onData from compositionend after the input event.
  ime: {
    steps: [
      { composition: "start", value: "", delay: 30 },
      { composition: "update", data: "w", value: "w", delay: 30 },
      { value: "w", data: "w", inputType: "insertCompositionText", delay: 30 },
      { composition: "update", data: "ws", value: "ws", delay: 30 },
      { value: "ws", data: "ws", inputType: "insertCompositionText", delay: 30 },
      { composition: "update", data: "wsl", value: "wsl", delay: 30 },
      { value: "wsl", data: "wsl", inputType: "insertCompositionText", delay: 30 },
      { value: "", data: null, inputType: "deleteCompositionText", delay: 30 },
      { value: "wsl", data: "wsl", inputType: "insertFromComposition", delay: 30 },
      { composition: "end", data: "wsl", value: "wsl", delay: 250 }
    ]
  }
};

// --- driver ----------------------------------------------------------------

const PAGE_SCRIPT = (steps) => `(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const panes = [...document.querySelectorAll(".pane")];
  const pane = panes.find((p) => !p.hidden) || panes[0];
  const field = (pane || document).querySelector(".xterm-helper-textarea");
  if (!field) return "no hidden textarea";
  field.focus();
  const steps = ${JSON.stringify(steps)};
  for (const step of steps) {
    if (step.key) {
      const down = new KeyboardEvent("keydown", {
        key: step.key.key, bubbles: true, cancelable: true, composed: true
      });
      Object.defineProperty(down, "keyCode", { get: () => step.key.keyCode });
      Object.defineProperty(down, "which", { get: () => step.key.keyCode });
      field.dispatchEvent(down);
    }
    if (step.composition) {
      field.dispatchEvent(new CompositionEvent("composition" + step.composition, {
        data: step.data || "", bubbles: true, composed: true
      }));
    }
    if (step.value !== undefined || step.append !== undefined) {
      field.value = step.value !== undefined ? step.value : field.value + step.append;
      field.setSelectionRange(field.value.length, field.value.length);
      const inputType = step.inputType || "insertText";
      const isComposing = inputType.includes("Composition");
      field.dispatchEvent(new InputEvent("beforeinput", {
        inputType, data: step.data ?? null, isComposing, bubbles: true, composed: true
      }));
      field.dispatchEvent(new InputEvent("input", {
        inputType, data: step.data ?? null, isComposing, bubbles: true, composed: true
      }));
    }
    await sleep(step.delay || 60);
  }
  await sleep(900);
  return "ok";
})()`;

async function connect() {
  let page = null;
  for (let i = 0; i < 40 && !page; i++) {
    await sleep(250);
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      page = list.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
    } catch {
      // Chrome is still starting.
    }
  }
  if (!page) throw new Error("no CDP page target");
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  const pending = new Map();
  let nextId = 0;
  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    const resolve = pending.get(msg.id);
    if (resolve) {
      pending.delete(msg.id);
      resolve(msg);
    }
  });
  await new Promise((resolve) => ws.addEventListener("open", resolve));
  return {
    send: (method, params = {}) => new Promise((resolve) => {
      const id = ++nextId;
      pending.set(id, resolve);
      ws.send(JSON.stringify({ id, method, params }));
    }),
    close: () => ws.close()
  };
}

// Abandon the line so the next scenario starts clean, on BOTH sides: a real
// Ctrl-C interrupts the shell line, and clearing the field (which the page
// treats as a buffer reset, not a retraction) drops its utterance mirror.
// Skipping the second half left the mirror pointing at text from the previous
// scenario, and the next one "corrected" a line it no longer knew about.
async function abandonLine(cdp) {
  await fetch(new URL("/tabs/active/input", TARGET), { method: "POST", body: "\u0003" });
  await cdp.send("Runtime.evaluate", {
    expression: `(() => {
      const pane = [...document.querySelectorAll(".pane")].find((p) => !p.hidden);
      const field = (pane || document).querySelector(".xterm-helper-textarea");
      if (!field) return false;
      field.value = "";
      field.dispatchEvent(new InputEvent("input", {
        bubbles: true, composed: true, inputType: "deleteContentBackward"
      }));
      field.focus();
      return true;
    })()`,
    returnByValue: true
  });
  await sleep(200);
}

async function ptyLinesAfter(marker) {
  const text = await (await fetch(LOG_URL)).text();
  const at = text.lastIndexOf(marker);
  const tail = at >= 0 ? text.slice(at) : text;
  return tail.split("\n").filter((line) => line.includes(" i| "));
}

async function mark(name) {
  try {
    await fetch(new URL("/web/debug", TARGET), {
      method: "POST",
      body: JSON.stringify({ s: "harness", n: 0, k: "marker", name })
    });
  } catch {
    // The marker is only a delimiter; carry on without it.
  }
}

function describe(line) {
  const match = line.match(/ i\| (\S+) (\d+)B 0x([0-9a-f ]*) "(.*)"$/);
  if (!match) return line;
  return `${match[2].padStart(4)}B  ${match[3].padEnd(34)} ${match[4]}`;
}

async function main() {
  const wanted = process.argv.slice(2);
  const names = wanted.length === 0 || wanted.includes("all")
    ? Object.keys(SCENARIOS)
    : wanted;
  for (const name of names) {
    if (!SCENARIOS[name]) throw new Error(`unknown scenario: ${name}`);
  }

  fs.rmSync(PROFILE, { recursive: true, force: true });
  const chrome = spawn(CHROME, [
    "--headless=new",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    `--user-data-dir=${PROFILE}`,
    `--remote-debugging-port=${PORT}`,
    "about:blank"
  ], { stdio: "ignore" });

  try {
    const cdp = await connect();
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Page.navigate", { url: TARGET });
    await sleep(3000);

    for (const name of names) {
      const marker = `marker-${name}-${Date.now()}`;
      await mark(marker);
      const scenario = SCENARIOS[name];
      if (scenario.native) {
        // Drive the real input pipeline. textarea focus is what the user's tap
        // does; everything after it is the browser's own key handling.
        await cdp.send("Runtime.evaluate", {
          expression: `(() => {
            const pane = [...document.querySelectorAll(".pane")].find((p) => !p.hidden);
            const field = (pane || document).querySelector(".xterm-helper-textarea");
            if (field) field.focus();
            return !!field;
          })()`,
          returnByValue: true
        });
        for (const step of scenario.native) {
          const base = {
            key: step.key,
            code: step.code,
            windowsVirtualKeyCode: step.vk,
            nativeVirtualKeyCode: step.vk,
            modifiers: 0
          };
          // Only a printable key carries text; Backspace/Enter must stay
          // rawKeyDown or the browser would insert a character instead.
          await cdp.send("Input.dispatchKeyEvent", step.text
            ? { ...base, type: "keyDown", text: step.text, unmodifiedText: step.text }
            : { ...base, type: "rawKeyDown" });
          await cdp.send("Input.dispatchKeyEvent", { ...base, type: "keyUp" });
          await sleep(160);
        }
      } else {
        const reply = await cdp.send("Runtime.evaluate", {
          expression: PAGE_SCRIPT(scenario.steps),
          awaitPromise: true,
          returnByValue: true
        });
        const state = reply.result && reply.result.result && reply.result.result.value;
        if (state !== "ok") console.log(`\n=== ${name} (${state}) ===`);
      }
      await sleep(900);
      const lines = await ptyLinesAfter(marker);
      console.log(`\n=== ${name} ===`);
      for (const line of lines) console.log("  " + describe(line));
      if (lines.length === 0) console.log("  (nothing reached the PTY)");
      // Abandon the line so the next scenario starts clean. This goes through a
      // real Ctrl-C key event, not a direct POST: posting to the API would leave
      // the page's own utterance mirror untouched and the next scenario would
      // "correct" a line it no longer knows about.
      await abandonLine(cdp);
      await sleep(400);
    }

    cdp.close();
  } finally {
    chrome.kill();
    await sleep(300);
    try {
      fs.rmSync(PROFILE, { recursive: true, force: true });
    } catch {
      // Chrome may still hold a handle; a temp dir is harmless.
    }
  }
}

main().catch((error) => {
  console.error("replay failed:", error);
  process.exit(1);
});
