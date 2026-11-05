// No console window behind the app in a release build on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    gantry_desktop::run();
}
