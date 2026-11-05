//! The frontend is embedded from `web/dist` (ADR-0002). `rust-embed` needs that
//! folder to exist at compile time, so if the frontend has not been built yet
//! this builds it with pnpm, or, when pnpm isn't available (a Rust-only CI job,
//! clippy in an editor), leaves a placeholder page that says how to build it.
use std::path::Path;
use std::process::Command;

fn main() {
    // The release's version is compiled in (see `VERSION`).
    println!("cargo:rerun-if-env-changed=GANTRY_VERSION");
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );

    if dist.join("index.html").exists() {
        return;
    }

    if std::env::var_os("GANTRY_SKIP_WEB_BUILD").is_none() && build_web() {
        return;
    }

    println!(
        "cargo:warning=web/dist is missing, so a placeholder page is embedded. \
         Run `pnpm install && pnpm --filter @gantry/web build`, then rebuild."
    );
    std::fs::create_dir_all(&dist).expect("create web/dist");
    std::fs::write(
        dist.join("index.html"),
        "<!doctype html><title>Gantry</title><p>The Gantry frontend has not been built. \
         Run <code>pnpm --filter @gantry/web build</code> and rebuild the binary.</p>",
    )
    .expect("write placeholder index.html");
}

fn build_web() -> bool {
    let pnpm = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ok = |args: &[&str]| {
        Command::new(pnpm)
            .args(args)
            .current_dir(&root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    ok(&["install", "--frozen-lockfile"]) && ok(&["--filter", "@gantry/web", "build"])
}
