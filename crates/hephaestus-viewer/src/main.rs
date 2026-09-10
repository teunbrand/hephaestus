// A release build must not open a console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    hephaestus_viewer::run();
}
