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
applyDebugUi();
keyToolbar.hidden = !state.showToolbar;
document.body.classList.toggle("with-toolbar", state.showToolbar);

let appStarted = false;
function startWebApp(config) {
  if (appStarted) return;
  appStarted = true;
  webAuthorized = true;
  authOverlay.hidden = true;
  if (config && config.ws_port) state.wsPort = Number(config.ws_port);
  flushDebug(false);
  // Register immediately instead of waiting for tab construction and a
  // WebSocket attempt. A conflicting client must see Steal as soon as the
  // server answers; clientBlocked prevents later stream attempts in refresh().
  notifyClient(true);
  refresh();
  setInterval(refresh, 1000);
}

async function bootstrap() {
  try {
    const response = await fetch("/web/config");
    if (response.status === 401) {
      showAuthOverlay();
      return;
    }
    if (!response.ok) throw new Error(`${response.status} ${await response.text()}`);
    startWebApp(await response.json());
  } catch (error) {
    report(error);
    setConnection("offline");
  }
}

authForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  authError.textContent = "";
  const submit = document.getElementById("auth-submit");
  submit.disabled = true;
  try {
    const response = await fetch("/web/auth", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token: authToken.value })
    });
    if (!response.ok) {
      showAuthOverlay(response.status === 401 ? "Invalid token" : `Unlock failed: ${response.status}`);
      return;
    }
    authToken.value = "";
    await bootstrap();
  } finally {
    submit.disabled = false;
  }
});

bootstrap();
