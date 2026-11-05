//! Writes the Print Files the Playwright tests upload into `web/e2e/fixtures`.
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../web/e2e/fixtures");
    std::fs::create_dir_all(&dir).expect("create web/e2e/fixtures");
    for (name, bytes) in gantry_print_file_fixtures::e2e_files() {
        std::fs::write(dir.join(name), bytes).expect("write a fixture");
        println!("wrote web/e2e/fixtures/{name}");
    }
}
