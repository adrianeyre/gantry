//! The Drivers compiled into this binary (ADR-0003). Each is one line here,
//! behind its Cargo feature; nothing else changes to add one.
use gantry_core::DriverRegistry;

pub fn registry() -> DriverRegistry {
    // `mut` is unused only when no Driver feature is on.
    #[allow(unused_mut)]
    let mut drivers = DriverRegistry::new();
    #[cfg(feature = "simulated")]
    drivers.register(gantry_driver_simulated::SimulatedDriver);
    #[cfg(feature = "bambu")]
    drivers.register(gantry_driver_bambu::BambuDriver);
    #[cfg(feature = "moonraker")]
    drivers.register(gantry_driver_moonraker::MoonrakerDriver);
    #[cfg(feature = "octoprint")]
    drivers.register(gantry_driver_octoprint::OctoPrintDriver);
    #[cfg(feature = "prusalink")]
    drivers.register(gantry_driver_prusalink::PrusaLinkDriver);
    #[cfg(feature = "duet")]
    drivers.register(gantry_driver_duet::DuetDriver);
    #[cfg(feature = "elegoo-sdcp")]
    drivers.register(gantry_driver_elegoo_sdcp::ElegooSdcpDriver);
    drivers
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(feature = "simulated")]
    fn the_simulated_driver_is_registered() {
        assert!(super::registry().get("simulated").is_some());
    }

    #[test]
    #[cfg(feature = "bambu")]
    fn the_bambu_driver_is_registered() {
        assert!(super::registry().get("bambu").is_some());
    }

    #[test]
    #[cfg(feature = "moonraker")]
    fn the_moonraker_driver_is_registered() {
        assert!(super::registry().get("moonraker").is_some());
    }

    #[test]
    #[cfg(feature = "octoprint")]
    fn the_octoprint_driver_is_registered() {
        assert!(super::registry().get("octoprint").is_some());
    }

    #[test]
    #[cfg(feature = "prusalink")]
    fn the_prusalink_driver_is_registered() {
        assert!(super::registry().get("prusalink").is_some());
    }

    #[test]
    #[cfg(feature = "duet")]
    fn the_duet_driver_is_registered() {
        assert!(super::registry().get("duet").is_some());
    }

    #[test]
    #[cfg(feature = "elegoo-sdcp")]
    fn the_elegoo_sdcp_driver_is_registered() {
        assert!(super::registry().get("elegoo-sdcp").is_some());
    }
}
