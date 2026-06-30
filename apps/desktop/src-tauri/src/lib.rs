//! Tauri shell: wires the React UI directly into sdm-engine.
//! Sprint 6 scope per docs/SPRINT_PLAN.md.

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("error while running SmartDownloadManager");
}
