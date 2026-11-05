// Starts the real gantry binary for Playwright: a fresh data directory, a
// throwaway secret key and one Admin account. It runs only on a developer's
// machine, never in GitHub Actions (PRD §11).
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { ADMIN, PORT } from "./config.mjs";

const root = resolve(import.meta.dirname, "../..");
const exe = join(root, "target", "debug", process.platform === "win32" ? "gantry.exe" : "gantry");
const dataDir = mkdtempSync(join(tmpdir(), "gantry-e2e-"));
const env = {
  ...process.env,
  GANTRY_DATA_DIR: dataDir,
  GANTRY_BIND: `127.0.0.1:${PORT}`,
  GANTRY_SECRET_KEY: "an end-to-end test key, never a real one",
  RUST_LOG: process.env.RUST_LOG ?? "info",
};

function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, { cwd: root, env, stdio: ["pipe", "inherit", "inherit"], ...options });
  if (result.status !== 0) {
    console.error(`${cmd} ${args.join(" ")} failed`);
    process.exit(result.status ?? 1);
  }
}

// The binary embeds web/dist, so the frontend under test is the one just built.
if (!process.env.GANTRY_E2E_SKIP_BUILD) {
  // Under `pnpm e2e`, npm_execpath is pnpm itself, so no shell is needed.
  const pnpm = process.env.npm_execpath;
  const build = ["--filter", "@gantry/web", "build"];
  if (pnpm && /\.[cm]?js$/.test(pnpm)) run(process.execPath, [pnpm, ...build]);
  else if (pnpm) run(pnpm, build);
  else run("pnpm", build);
  run("cargo", ["build", "-p", "gantry"]);
}
run(exe, ["user", "create", "--admin", "--username", ADMIN.username, "--password-stdin"], {
  input: `${ADMIN.password}\n`,
});

const server = spawn(exe, ["serve"], { cwd: root, env, stdio: "inherit" });
const stop = () => {
  server.kill();
  try {
    rmSync(dataDir, { recursive: true, force: true });
  } catch {
    // The database may still be open on Windows; the OS cleans tmp eventually.
  }
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
process.on("exit", stop);
server.on("exit", (code) => process.exit(code ?? 0));

// Every test runs against a Fleet that already has Simulated Printers in it.
async function seed() {
  const base = `http://127.0.0.1:${PORT}`;
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`${base}/healthz`)).ok) break;
    } catch {
      // not listening yet
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  const login = await fetch(`${base}/api/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(ADMIN),
  });
  const cookie = (login.headers.get("set-cookie") ?? "").split(";")[0];
  for (const name of ["Seed Simulated A", "Seed Simulated B"]) {
    const response = await fetch(`${base}/api/printers`, {
      method: "POST",
      headers: { "content-type": "application/json", cookie },
      body: JSON.stringify({ name, driver: "simulated", settings: {} }),
    });
    if (response.status !== 201) console.error(`seeding ${name} failed: ${response.status}`);
  }
  await fetch(`${base}/api/logout`, { method: "POST", headers: { cookie } });
}
seed().catch((e) => console.error("seeding failed", e));
