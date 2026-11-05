fn main() {
    // The bundled page's commands get permissions of their own, so only the
    // capability for the app's own page (capabilities/app-page.json) can call
    // them. A server's pages never can.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "resume",
            "choices",
            "connect_remote",
            "start_embedded",
            "create_first_admin",
        ]),
    ))
    .expect("tauri-build");
}
