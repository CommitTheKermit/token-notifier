#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    token_notifier_lib::run();
}
