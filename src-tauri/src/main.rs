// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The child-reaper subprocess (unix): a re-exec of this binary that
    // outlives the app just long enough to kill orphaned children. It
    // must never boot Tauri.
    if std::env::args().nth(1).as_deref() == Some("--child-reaper") {
        embral_lib::run_child_reaper();
        return;
    }
    embral_lib::run()
}
