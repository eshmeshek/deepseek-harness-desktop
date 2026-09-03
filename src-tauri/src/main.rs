// Windows release builds must not open a console behind the tray icon.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dshd::run();
}
