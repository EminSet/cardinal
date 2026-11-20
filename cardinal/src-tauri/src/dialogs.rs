//! Native confirmation dialog.
//!
//! wry's WKWebView does not implement JavaScript `confirm()`: it returns
//! `false` without showing anything, so confirmations made with
//! `window.confirm` silently cancelled every action. This shows an `NSAlert`
//! on the main thread instead.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle};
use objc2_foundation::NSString;
use tauri::AppHandle;

#[tauri::command(async)]
pub async fn confirm_dialog(
    app: AppHandle,
    message: String,
    detail: Option<String>,
    confirm_label: String,
    cancel_label: String,
    destructive: Option<bool>,
) -> Result<bool, String> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(false);
            return;
        };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&message));
        if let Some(detail) = detail.filter(|d| !d.is_empty()) {
            alert.setInformativeText(&NSString::from_str(&detail));
        }
        alert.setAlertStyle(NSAlertStyle::Warning);
        let confirm = alert.addButtonWithTitle(&NSString::from_str(&confirm_label));
        if destructive.unwrap_or(false) {
            confirm.setHasDestructiveAction(true);
        }
        let cancel = alert.addButtonWithTitle(&NSString::from_str(&cancel_label));
        cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        let response = alert.runModal();
        let _ = tx.send(response == NSAlertFirstButtonReturn);
    })
    .map_err(|e| format!("Could not show dialog: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or(false))
        .await
        .map_err(|e| format!("Dialog failed: {e}"))
}
