// The desktop app's own page: choose a Gantry server, or run one here. It is
// deliberately not the React app: once a server is chosen, the window goes to
// that server and runs the dashboard it serves (src-tauri/src/shell.rs).

/** @param {string} command @param {Record<string, unknown>} [args] */
function invoke(command, args) {
  return window.__TAURI_INTERNALS__.invoke(command, args);
}

const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));

function busy(message) {
  $("status").textContent = message;
  $("error").hidden = true;
  for (const control of document.querySelectorAll("button, input")) control.disabled = true;
}

function idle() {
  for (const control of document.querySelectorAll("button, input")) control.disabled = false;
}

function fail(message) {
  idle();
  $("status").textContent = "";
  $("error").textContent = message;
  $("error").hidden = false;
}

/** Shows what the app said comes next. */
function show(step) {
  idle();
  $("choose").hidden = step.step !== "choose";
  $("first-admin").hidden = step.step !== "first_admin";
  switch (step.step) {
    case "choose":
      $("status").textContent = "";
      if (step.server) /** @type {HTMLInputElement} */ ($("server")).value = step.server;
      if (step.error) fail(step.error);
      $(step.embedded ? "embedded" : "server").focus();
      break;
    case "first_admin":
      $("status").textContent = "";
      $("username").focus();
      break;
    case "opened":
      $("status").textContent = "Opening the dashboard…";
      break;
  }
}

/** Runs a command, showing what happens next or why it failed. */
async function run(message, command, args) {
  busy(message);
  try {
    show(await invoke(command, args));
  } catch (error) {
    fail(String(error));
  }
}

$("remote").addEventListener("submit", (event) => {
  event.preventDefault();
  const server = /** @type {HTMLInputElement} */ ($("server")).value;
  run("Checking the server…", "connect_remote", { server });
});

$("embedded").addEventListener("click", () => {
  run("Starting Gantry on this computer…", "start_embedded");
});

$("admin").addEventListener("submit", (event) => {
  event.preventDefault();
  const username = /** @type {HTMLInputElement} */ ($("username")).value;
  const password = /** @type {HTMLInputElement} */ ($("password")).value;
  const repeat = /** @type {HTMLInputElement} */ ($("repeat")).value;
  if (password !== repeat) {
    fail("The passwords do not match.");
    $("repeat").focus();
    return;
  }
  run("Creating the account…", "create_first_admin", { username, password });
});

// "Change Server…" in the menu opens this page with ?choose: show the choice
// instead of going straight back to the last server.
const choosing = new URLSearchParams(window.location.search).has("choose");
run(choosing ? "" : "Opening Gantry…", choosing ? "choices" : "resume");
