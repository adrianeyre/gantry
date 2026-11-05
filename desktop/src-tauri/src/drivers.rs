//! The Drivers the embedded server runs with: the same ones the `gantry`
//! binary has by default (crates/gantry/src/drivers.rs, ADR-0003).
use gantry_core::DriverRegistry;

pub fn registry() -> DriverRegistry {
    let mut drivers = DriverRegistry::new();
    drivers
        .register(gantry_driver_simulated::SimulatedDriver)
        .register(gantry_driver_bambu::BambuDriver)
        .register(gantry_driver_moonraker::MoonrakerDriver)
        .register(gantry_driver_octoprint::OctoPrintDriver)
        .register(gantry_driver_prusalink::PrusaLinkDriver)
        .register(gantry_driver_duet::DuetDriver)
        .register(gantry_driver_elegoo_sdcp::ElegooSdcpDriver);
    drivers
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_default_driver_is_registered() {
        let drivers = super::registry();
        for id in [
            "simulated",
            "bambu",
            "moonraker",
            "octoprint",
            "prusalink",
            "duet",
            "elegoo-sdcp",
        ] {
            assert!(drivers.get(id).is_some(), "{id}");
        }
    }
}
