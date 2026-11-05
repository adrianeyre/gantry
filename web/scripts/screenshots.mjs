// Takes the screenshots in docs/images/ (the README and the website use
// them) from a real Gantry with a demo Fleet of Simulated Printers. Local
// only, like Playwright's tests.
//
//   pnpm --filter @gantry/web build && cargo build -p gantry
//   pnpm --filter @gantry/web screenshots
//
// GANTRY_EXE names another binary; by default it is target/debug/gantry.
// The first run needs a browser: pnpm --filter @gantry/web exec playwright install chromium
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "@playwright/test";

const root = resolve(import.meta.dirname, "../..");
const out = join(root, "docs/images");
const exe =
  process.env.GANTRY_EXE ?? join(root, "target", "debug", process.platform === "win32" ? "gantry.exe" : "gantry");
const port = Number(process.env.GANTRY_SCREENSHOT_PORT ?? 18282);
const base = `http://127.0.0.1:${port}`;
const admin = { username: "admin", password: "a screenshot password, never a real one" };
const dataDir = mkdtempSync(join(tmpdir(), "gantry-screenshots-"));
const env = {
  ...process.env,
  GANTRY_DATA_DIR: dataDir,
  GANTRY_BIND: `127.0.0.1:${port}`,
  GANTRY_SECRET_KEY: "a screenshot key, never a real one",
  RUST_LOG: "warn",
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const created = spawnSync(exe, ["user", "create", "--admin", "--username", admin.username, "--password-stdin"], {
  env,
  input: `${admin.password}\n`,
  stdio: ["pipe", "inherit", "inherit"],
});
if (created.status !== 0) throw new Error(`could not create the Admin with ${exe}`);
const server = spawn(exe, ["serve"], { env, stdio: "inherit" });

let cookie = "";
async function api(method, path, body, headers = {}) {
  const raw = body instanceof Uint8Array;
  const response = await fetch(`${base}/api${path}`, {
    method,
    headers: { cookie, ...(raw ? { "content-type": "application/octet-stream" } : body ? { "content-type": "application/json" } : {}), ...headers },
    body: raw ? body : body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

async function seed() {
  for (let i = 0; i < 200; i++) {
    try {
      if ((await fetch(`${base}/healthz`)).ok) break;
    } catch {
      // not listening yet
    }
    await sleep(100);
  }
  const login = await fetch(`${base}/api/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(admin),
  });
  cookie = (login.headers.get("set-cookie") ?? "").split(";")[0];

  const fixtures = join(root, "web/e2e/fixtures");
  const plates = await api("POST", "/print-files?name=three-plates.gcode.3mf", readFileSync(join(fixtures, "three-plates.gcode.3mf")));
  const benchy = await api("POST", "/print-files?name=benchy-prusaslicer.gcode", readFileSync(join(fixtures, "benchy-prusaslicer.gcode")));
  const files = await api("GET", "/print-files");
  const modelOf = (id) => {
    const file = files.find((f) => f.id === id);
    return file?.printer_model ?? file?.plates?.[0]?.printer_model ?? "Simulated";
  };

  const printers = {};
  const add = async (name, model, settings = {}) => {
    const { id } = await api("POST", "/printers", { name, driver: "simulated", settings: { model, ...settings } });
    printers[name] = id;
    return id;
  };
  await add("Bambu X1C", modelOf(plates.id));
  await add("P1S Garage", modelOf(plates.id));
  await add("Prusa MK4", modelOf(benchy.id), { filament_slots: 1 });
  await add("Voron 2.4", "Voron 2.4 350", { filament_slots: 1 });
  await add("Ender 3 V3", "Ender-3 V3", { filament_slots: 1, camera: false });
  await add("A1 Workshop", modelOf(plates.id));
  await sleep(1500);

  const simulate = (name, action) => api("POST", `/printers/${printers[name]}/simulation`, action);
  const colours = [
    ["PLA", "#1d4ed8", 82],
    ["PETG", "#e11d48", 46],
    ["PLA", "#f5f5f4", 64],
    ["TPU", "#16a34a", 12],
  ];
  for (const name of ["Bambu X1C", "P1S Garage", "A1 Workshop"]) {
    for (const [i, [kind, colour, remaining_percent]] of colours.entries()) {
      await simulate(name, { action: "set_filament_slot", slot: String(i), filament: { kind, colour, remaining_percent } }).catch(() => {});
    }
  }

  // Some History: short Jobs that finish, one that fails, each then cleared.
  for (const name of ["Bambu X1C", "Prusa MK4", "Voron 2.4", "P1S Garage"]) {
    for (let i = 0; i < 2; i++) {
      await simulate(name, { action: "start_print", print_file: `bracket-v${i + 2}.gcode.3mf`, duration_secs: 2, layers: 40 });
      await sleep(2600);
      await api("POST", `/printers/${printers[name]}/mark-clear`, {}).catch(() => {});
    }
  }
  await simulate("Ender 3 V3", { action: "start_print", print_file: "cable-clip.gcode", duration_secs: 60, layers: 20 });
  await sleep(800);
  await simulate("Ender 3 V3", { action: "fail_print" });
  await sleep(800);
  await api("POST", `/printers/${printers["Ender 3 V3"]}/mark-clear`, {}).catch(() => {});

  // Now the Fleet as it should look: printing, paused, awaiting clear, idle.
  await api("POST", `/printers/${printers["Bambu X1C"]}/start`, { print_file_id: plates.id, plate: 1 });
  await simulate("Voron 2.4", { action: "start_print", print_file: "voron-skirt.gcode", duration_secs: 7200, layers: 260 });
  await simulate("Prusa MK4", { action: "start_print", print_file: "benchy-prusaslicer.gcode", duration_secs: 4, layers: 48 });
  await simulate("A1 Workshop", { action: "start_print", print_file: "planter.gcode.3mf", duration_secs: 5400, layers: 180 });
  // Run the long Jobs on to part-way through, then back to real time.
  const speeds = { "Bambu X1C": 600, "Voron 2.4": 2400, "A1 Workshop": 1400 };
  await sleep(1000);
  for (const [name, speed] of Object.entries(speeds)) await simulate(name, { action: "set_speed", speed: Math.min(speed, 1000) });
  await sleep(1500);
  for (const name of Object.keys(speeds)) await simulate(name, { action: "set_speed", speed: 1 });
  await api("POST", `/printers/${printers["Voron 2.4"]}/commands`, { command: "pause" }).catch((e) => console.warn(e.message));
  await simulate("Ender 3 V3", {
    action: "raise_fault",
    code: "nozzle-clog",
    severity: "warning",
    message: "Extruder skipping: the nozzle may be clogged",
  }).catch(() => {});
  await api("POST", "/queue", { print_file_id: plates.id }).catch((e) => console.warn(e.message));
  await api("POST", "/queue", { printer_id: printers["Prusa MK4"], print_file_id: benchy.id }).catch((e) => console.warn(e.message));
  await sleep(5000); // the Prusa finishes and waits for Mark Clear
  return { printers, plates, benchy };
}

async function capture({ printers, plates }) {
  mkdirSync(out, { recursive: true });
  const browser = await chromium.launch();
  const shoot = async (name, path, { scheme = "light", wait, full = false, before, width = 1600, height = 1000 } = {}) => {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      colorScheme: scheme,
      reducedMotion: "reduce",
    });
    const [nameValue] = cookie.split("=");
    await context.addCookies([{ name: nameValue, value: cookie.slice(nameValue.length + 1), url: base }]);
    const page = await context.newPage();
    await page.goto(`${base}${path}`);
    await page.waitForLoadState("networkidle").catch(() => {});
    if (wait) await page.locator(wait).first().waitFor({ timeout: 10_000 }).catch(() => console.warn(`${name}: ${wait} not found`));
    if (before) await before(page);
    // The cookie notice is dismissed, as anyone would after their first visit.
    const notice = page.getByRole("button", { name: "OK, got it" });
    if (await notice.isVisible().catch(() => false)) await notice.click();
    await sleep(1500);
    await page.screenshot({ path: join(out, `${name}.png`), fullPage: full });
    console.log(`docs/images/${name}.png`);
    await context.close();
  };

  await shoot("fleet-light", "/", { wait: "article", width: 1920, height: 1200 });
  await shoot("fleet-dark", "/", { scheme: "dark", wait: "article", width: 1920, height: 1200 });
  await shoot("printer-detail", `/printers/${printers["Bambu X1C"]}`, { wait: "h1" });
  await shoot("library", "/library", { wait: "img" });
  await shoot("model-viewer", `/library/${plates.id}/model`, { wait: "canvas" });
  await shoot("queue", "/queue", { wait: "h1" });
  await shoot("cameras", "/cameras", { wait: "img" });
  await shoot("wall", "/wall", { scheme: "dark", wait: "article", width: 1920, height: 1200 });
  await browser.close();
}

try {
  const seeded = await seed();
  await capture(seeded);
} finally {
  server.kill();
  await sleep(500);
  try {
    rmSync(dataDir, { recursive: true, force: true });
  } catch {
    // The database may still be open on Windows; the OS cleans tmp eventually.
  }
}
