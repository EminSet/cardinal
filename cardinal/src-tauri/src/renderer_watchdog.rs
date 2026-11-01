use crate::{lifecycle::EXIT_REQUESTED, renderer_health::Health};
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::Manager;

fn record(app: &tauri::AppHandle, message: &str) {
    let Ok(dir) = app.path().app_log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("renderer-health.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 1_048_576) {
        let _ = std::fs::rename(&path, dir.join("renderer-health.previous.log"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{} {message}", now());
    }
}

static LAST_RESPONSE: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[tauri::command]
pub(crate) fn renderer_heartbeat(app: tauri::AppHandle) {
    let time = now();
    let previous = LAST_RESPONSE.swap(time, Ordering::Relaxed);
    if previous == 0 || time.saturating_sub(previous) >= 60 {
        record(&app, "Renderer DOM and IPC responsive");
    }
}

pub(crate) fn start(app: tauri::AppHandle) {
    record(&app, "Renderer watchdog started");
    std::thread::spawn(move || {
        let mut health = Health::default();
        while !EXIT_REQUESTED.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(15));
            let Some(window) = app.get_webview_window("main") else {
                continue;
            };
            let visible =
                window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
            if health.check(now(), visible, LAST_RESPONSE.load(Ordering::Relaxed)) {
                tracing::warn!(
                    "Renderer unresponsive for 60s; reloading main webview (maximum 3 per 10 minutes)"
                );
                record(
                    &app,
                    "Renderer unresponsive for 60s; reloading main webview",
                );
                if let Err(error) = window.reload() {
                    tracing::error!("Renderer recovery failed: {error}");
                }
            } else if visible {
                // Native-driven probe also detects a terminated WebContent process.
                let _ = window.eval("if (document.getElementById('root')?.childElementCount > 0) { window.__TAURI_INTERNALS__.invoke('renderer_heartbeat'); }");
            }
        }
    });
}
