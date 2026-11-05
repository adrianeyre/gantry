//! The core crate knows no protocol (ADR-0003): Drivers depend on it, never the
//! reverse. This fails if code in `src/` names a particular Driver.
use std::path::Path;

const FORBIDDEN: [&str; 6] = [
    "simulat",
    "bambu",
    "moonraker",
    "octoprint",
    "prusa",
    "mqtt",
];

fn code_without_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_core_crate_names_no_driver() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offences = Vec::new();
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let code = code_without_comments(&std::fs::read_to_string(&path).unwrap()).to_lowercase();
        for word in FORBIDDEN {
            if code.contains(word) {
                offences.push(format!("{} mentions `{word}`", path.display()));
            }
        }
    }
    assert!(offences.is_empty(), "{offences:#?}");
}

#[test]
fn the_core_crate_depends_on_no_driver() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("gantry-driver"), "{manifest}");
}
