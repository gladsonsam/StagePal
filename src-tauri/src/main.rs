// Hides console window on Windows release builds. DO NOT REMOVE.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    stagepal_lib::run()
}
