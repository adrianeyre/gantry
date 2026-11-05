//! `gantry bambu-probe`: checks a real Bambu printer against what the Driver
//! expects, for validating it once one is owned (#27, #28). See
//! `docs/processes/validating-bambu-on-real-hardware.md`.
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::Args;
use gantry_driver_bambu::probe::{ProbeOptions, probe};

#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// The printer's address, optionally with `:port` (8883 by default).
    #[arg(long)]
    host: String,
    /// The model, when the serial number doesn't say (X1, X1C, X1E, P1P, P1S,
    /// A1 or A1 mini).
    #[arg(long)]
    model: Option<String>,
    /// How long to record the printer's reports, in seconds.
    #[arg(long, default_value_t = 30)]
    seconds: u64,
    /// Also read the camera, for this many seconds (0 skips it).
    #[arg(long, default_value_t = 10)]
    camera_seconds: u64,
    /// The camera's port, if not the model's own.
    #[arg(long)]
    camera_port: Option<u16>,
    /// Where to save the payloads (serial and access code removed) and the
    /// first camera frame.
    #[arg(long, default_value = "bambu-probe")]
    out: PathBuf,
}

/// A credential from the environment, or asked for without echoing it.
fn credential(variable: &str, prompt: &str) -> anyhow::Result<String> {
    if let Ok(value) = std::env::var(variable)
        && !value.trim().is_empty()
    {
        return Ok(value.trim().to_owned());
    }
    let value = rpassword::prompt_password(prompt)
        .with_context(|| format!("could not read it (or set {variable})"))?;
    anyhow::ensure!(!value.trim().is_empty(), "{prompt} is required");
    Ok(value.trim().to_owned())
}

pub async fn run(args: ProbeArgs) -> anyhow::Result<()> {
    let (host, port) = match args.host.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse().context("the port isn't a number")?),
        None => (args.host.clone(), gantry_driver_bambu::MQTT_PORT),
    };
    // Never on the command line, where they would land in shell history.
    let serial = credential("GANTRY_PROBE_SERIAL", "Serial number: ")?;
    let access_code = credential("GANTRY_PROBE_ACCESS_CODE", "Access code: ")?;
    let options = ProbeOptions {
        host,
        port,
        serial,
        access_code,
        model: args.model,
        listen: Duration::from_secs(args.seconds.max(1)),
        camera: (args.camera_seconds > 0).then(|| Duration::from_secs(args.camera_seconds)),
        camera_port: args.camera_port,
        out: Some(args.out.clone()),
    };
    println!(
        "Probing {}:{} for {} s. Nothing on the printer is changed.",
        options.host, options.port, args.seconds
    );
    let report = probe(&options).await?;

    println!();
    println!(
        "Model:            {}",
        report.model.map_or("unknown (pass --model)", |m| m.name())
    );
    if report.firmware.is_empty() {
        println!("Firmware:         not reported (no answer to get_version)");
    }
    for (module, version) in &report.firmware {
        println!("Firmware:         {module} {version}");
    }
    println!(
        "Status reports:   {} (first after {})",
        report.reports,
        report
            .first_status_after
            .map_or("none".to_owned(), |d| format!("{:.1} s", d.as_secs_f64()))
    );
    println!("Other messages:   {}", report.other_messages);
    if let Some(status) = &report.status {
        println!("Printer State:    {:?}", status.state);
        if let Some(job) = &status.job {
            println!(
                "Job:              {} ({:.0}%, layer {:?} of {:?}, key {})",
                job.print_file.as_deref().unwrap_or("unknown"),
                job.progress,
                job.layer,
                job.total_layers,
                job.key
            );
        }
        println!("Temperatures:     {:?}", status.temperatures);
        for fan in &status.fans {
            println!("Fan:              {} {}%", fan.name, fan.percent);
        }
        for slot in &status.filament_slots {
            println!("Filament Slot:    {} {:?}", slot.label, slot.filament);
        }
        for fault in &status.faults {
            println!(
                "Fault:            {:?} {} ({})",
                fault.severity, fault.message, fault.code
            );
        }
        println!("Speed profile:    {:?}", status.speed_profile);
        println!("Lights:           {:?}", status.lights);
        println!("Loaded slot:      {:?}", status.loaded_slot);
        println!("Capabilities:     {:?}", status.capabilities);
    }
    if let Some(camera) = &report.camera {
        println!(
            "Camera:           {} — {} frame(s) in {:.1} s{}",
            camera.kind.unwrap_or("none"),
            camera.frames,
            camera.seconds,
            camera
                .error
                .as_ref()
                .map(|e| format!(", error: {e}"))
                .unwrap_or_default()
        );
    }
    println!();
    println!(
        "Saved {} file(s) in {}. The serial number and access code are replaced, but check \
         by eye before committing any as a fixture.",
        report.saved.len(),
        args.out.display()
    );
    Ok(())
}
