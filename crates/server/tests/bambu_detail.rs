//! The Printer detail a Bambu Printer gives (#46): Faults with readable
//! messages and severities, and Filament Slots marked low at the threshold,
//! exactly as for a Simulated Printer.
use std::sync::Arc;
use std::time::Duration;

use gantry_core::{
    DriverRegistry, FaultSeverity, Fleet, JobState, MemoryJobLog, PrinterConfig, Secrets,
};
use gantry_mock_bambu::MockBambu;

const SERIAL: &str = "00MDETAIL000777";
const CODE: &str = "13572468";

#[tokio::test]
async fn bambu_faults_and_low_filament_slots_show_in_the_detail() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("ams_multiple");
    mock.report_fixture("hms_faults");

    let mut drivers = DriverRegistry::new();
    drivers.register(gantry_driver_bambu::BambuDriver);
    let fleet = Fleet::new(drivers, Arc::new(MemoryJobLog::new()));
    let mut settings = serde_json::Map::new();
    settings.insert("host".into(), mock.address().to_string().into());
    fleet
        .start(
            "bambu",
            PrinterConfig {
                id: "x1".into(),
                name: "X1".into(),
                settings,
                secrets: Secrets::new(
                    [
                        ("serial".to_owned(), SERIAL.to_owned()),
                        ("access_code".to_owned(), CODE.to_owned()),
                    ]
                    .into(),
                ),
            },
            JobState::default(),
        )
        .unwrap();
    let view = fleet
        .wait_for("x1", Duration::from_secs(10), |v| {
            !v.status.faults.is_empty() && v.status.filament_slots.len() > 4
        })
        .await
        .unwrap();

    // Faults carry a severity and a message a person can read.
    for fault in &view.status.faults {
        assert!(
            fault.message.chars().any(char::is_alphabetic) && fault.message.len() > 8,
            "{fault:?}"
        );
    }
    assert!(
        view.status
            .faults
            .iter()
            .any(|f| f.severity == FaultSeverity::Error || f.severity == FaultSeverity::Warning)
    );

    // At the default 15%, the slots with 12%, 5% and 0% left are low.
    let low = |view: &gantry_core::PrinterView| -> Vec<(String, Option<u8>)> {
        view.status
            .filament_slots
            .iter()
            .filter(|s| s.low)
            .map(|s| {
                (
                    s.id.clone(),
                    s.filament.as_ref().and_then(|f| f.remaining_percent),
                )
            })
            .collect()
    };
    let marked = low(&view);
    assert!(!marked.is_empty(), "{:?}", view.status.filament_slots);
    assert!(
        marked.iter().all(|(_, left)| left.is_some_and(|l| l <= 15)),
        "{marked:?}"
    );
    for slot in &view.status.filament_slots {
        if let Some(left) = slot.filament.as_ref().and_then(|f| f.remaining_percent) {
            assert_eq!(slot.low, left <= 15, "{slot:?}");
        }
    }

    // Raising the threshold marks more of them.
    fleet.set_low_filament_threshold(65);
    let view = fleet
        .wait_for("x1", Duration::from_secs(5), |v| {
            low(v).len() > marked.len()
        })
        .await
        .unwrap();
    assert!(
        low(&view)
            .iter()
            .all(|(_, left)| left.is_some_and(|l| l <= 65))
    );
    fleet.stop_all().await;
}
