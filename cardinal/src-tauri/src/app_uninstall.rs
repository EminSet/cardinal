//! Uninstall apps together with their leftovers.
//!
//! Every removal moves items to the user's Trash (never deletes). Items the
//! user can't move (e.g. under `/Library` or root-owned bundles) are moved in a
//! single privileged step that macOS authorises with its own password dialog.
//! Only paths produced by the latest `analyze_app` for the same app are accepted.

use crate::{commands::SearchState, folder_sizes};
use folder_size::apps::{
    self, AppPlan, AppSummary, Locations, analyze, is_running, needs_admin, read_bundle,
    running_executables,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::State;
use tracing::{info, warn};

static HOME: Lazy<PathBuf> = Lazy::new(|| {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/".into())
});
static LOCATIONS: Lazy<Locations> = Lazy::new(|| Locations::system(&HOME));
/// Latest plan per app path; removal is limited to these items.
static PLANS: Lazy<Mutex<HashMap<String, AppPlan>>> = Lazy::new(|| Mutex::new(HashMap::new()));

const INDEX_HIT_LIMIT: usize = 300;
/// Paths that are never touched, whatever a plan says.
const PROTECTED_PREFIXES: &[&str] = &[
    "/System",
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/libexec",
    "/private/etc",
    "/etc",
    "/dev",
    "/Volumes",
    "/cores",
];

fn size_hint(path: &Path) -> Option<u64> {
    folder_sizes::size_of(path)
}

#[tauri::command(async)]
pub async fn list_installed_apps() -> Result<Vec<AppSummary>, String> {
    tauri::async_runtime::spawn_blocking(|| apps::list_apps(&LOCATIONS, &size_hint))
        .await
        .map_err(|e| format!("Listing apps failed: {e}"))
}

#[tauri::command(async)]
pub async fn analyze_app(path: String, state: State<'_, SearchState>) -> Result<AppPlan, String> {
    let app_path = PathBuf::from(&path);
    let bundle = read_bundle(&app_path).ok_or("Not an application bundle")?;
    // Anything on disk named after the bundle id, via Cardinal's index.
    let extra: Vec<PathBuf> = state
        .find_paths(format!("\"{}\"", bundle.bundle_id), INDEX_HIT_LIMIT)
        .into_iter()
        .filter(|p| !p.starts_with(&app_path) && p.exists())
        .collect();
    let plan = tauri::async_runtime::spawn_blocking(move || {
        analyze(&LOCATIONS, &app_path, &extra, &size_hint)
    })
    .await
    .map_err(|e| format!("Analysis failed: {e}"))?
    .ok_or("This app can't be removed")?;
    PLANS.lock().insert(path, plan.clone());
    Ok(plan)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedItem {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallReport {
    pub moved: Vec<String>,
    pub failed: Vec<FailedItem>,
    pub receipts_forgotten: Vec<String>,
    /// The macOS administrator dialog was shown and cancelled.
    pub admin_cancelled: bool,
}

fn is_protected(path: &Path) -> bool {
    path.components().count() < 3
        || PROTECTED_PREFIXES.iter().any(|p| path.starts_with(p))
        || HOME.starts_with(path)
        || path == HOME.join("Library")
}

#[tauri::command(async)]
pub async fn uninstall_app(
    path: String,
    items: Vec<String>,
    receipts: Vec<String>,
) -> Result<UninstallReport, String> {
    let plan = PLANS
        .lock()
        .get(&path)
        .cloned()
        .ok_or("Analyze the app again before removing it")?;
    let allowed: HashSet<&str> = plan.items.iter().map(|i| i.path.as_str()).collect();
    let mut selected = Vec::new();
    for item in &items {
        if !allowed.contains(item.as_str()) {
            return Err(format!("{item} is not part of this app's removal plan"));
        }
        let p = PathBuf::from(item);
        if is_protected(&p) {
            return Err(format!("{item} is a protected location"));
        }
        let leftover = plan
            .items
            .iter()
            .find(|i| i.path == *item)
            .cloned()
            .unwrap();
        selected.push(leftover);
    }
    let receipts: Vec<String> = receipts
        .into_iter()
        .filter(|r| plan.receipts.contains(r))
        .collect();
    if is_running(Path::new(&plan.app.path), &running_executables()) {
        return Err("app_running".into());
    }
    let bundle_id = plan.app.bundle_id.clone();
    let report =
        tauri::async_runtime::spawn_blocking(move || perform(&selected, &receipts, &bundle_id))
            .await
            .map_err(|e| format!("Removal failed: {e}"))?;
    folder_sizes::forget_paths(&report.moved);
    PLANS.lock().remove(&path);
    info!(app = %plan.app.bundle_id, moved = report.moved.len(), failed = report.failed.len(), "app removed");
    Ok(report)
}

fn perform(items: &[apps::Leftover], receipts: &[String], bundle_id: &str) -> UninstallReport {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let mut report = UninstallReport::default();
    let mut admin: Vec<&apps::Leftover> = Vec::new();

    for item in items {
        let path = Path::new(&item.path);
        if !path.exists() && std::fs::symlink_metadata(path).is_err() {
            report.moved.push(item.path.clone()); // already gone
            continue;
        }
        if item.needs_admin || needs_admin(path) {
            admin.push(item);
            continue;
        }
        if item.launch_item {
            let _ = Command::new("/bin/launchctl")
                .args(["bootout", &format!("gui/{uid}")])
                .arg(path)
                .output();
        }
        match trash::delete(path) {
            Ok(()) => report.moved.push(item.path.clone()),
            Err(error) => {
                warn!(?error, path = %item.path, "trash failed; retrying with administrator rights");
                admin.push(item);
            }
        }
    }

    if !admin.is_empty() || !receipts.is_empty() {
        run_privileged(&admin, receipts, uid, gid, &mut report);
    }

    // Drop the app's cached preferences so they are not written back.
    let _ = Command::new("/usr/bin/defaults")
        .args(["delete", bundle_id])
        .output();
    report
}

/// `Name (tag).ext`, keeping the extension so Finder still sees an app or plist.
fn trash_name(path: &Path, tag: &str) -> String {
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match path.extension() {
        Some(ext) => format!("{stem} ({tag}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({tag})"),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Moves `items` into the user's Trash and forgets `receipts` as root, after
/// macOS asks the user for an administrator password.
fn run_privileged(
    items: &[&apps::Leftover],
    receipts: &[String],
    uid: u32,
    gid: u32,
    report: &mut UninstallReport,
) {
    let trash = HOME.join(".Trash");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut script = String::from("set +e; ");
    for (index, item) in items.iter().enumerate() {
        let path = Path::new(&item.path);
        let dest = trash.join(trash_name(path, &format!("Cardinal {stamp}-{index}")));
        let (src, dst) = (
            shell_quote(&item.path),
            shell_quote(&dest.to_string_lossy()),
        );
        if item.launch_item {
            script.push_str(&format!(
                "/bin/launchctl bootout system {src} >/dev/null 2>&1; "
            ));
        }
        script.push_str(&format!(
            "if /bin/mv {src} {dst}; then /usr/sbin/chown -R {uid}:{gid} {dst}; echo OK:{index}; else echo FAIL:{index}; fi; "
        ));
    }
    for (index, pkg) in receipts.iter().enumerate() {
        script.push_str(&format!(
            "if /usr/sbin/pkgutil --forget {} >/dev/null 2>&1; then echo PKG:{index}; fi; ",
            shell_quote(pkg)
        ));
    }
    let apple = format!(
        "do shell script {} with administrator privileges",
        applescript_string(&script)
    );
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple])
        .output();
    let Ok(output) = output else {
        for item in items {
            report.failed.push(FailedItem {
                path: item.path.clone(),
                reason: "osascript unavailable".into(),
            });
        }
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let cancelled = stderr.contains("-128");
        report.admin_cancelled = cancelled;
        for item in items {
            report.failed.push(FailedItem {
                path: item.path.clone(),
                reason: if cancelled {
                    "admin_cancelled".into()
                } else {
                    stderr.trim().to_owned()
                },
            });
        }
        return;
    }
    // osascript joins `echo` lines with carriage returns.
    let lines: Vec<&str> = stdout.split(['\r', '\n']).map(str::trim).collect();
    for (index, item) in items.iter().enumerate() {
        if lines.contains(&format!("OK:{index}").as_str()) {
            report.moved.push(item.path.clone());
        } else {
            report.failed.push(FailedItem {
                path: item.path.clone(),
                reason: "move_failed".into(),
            });
        }
    }
    for (index, pkg) in receipts.iter().enumerate() {
        if lines.contains(&format!("PKG:{index}").as_str()) {
            report.receipts_forgotten.push(pkg.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_hostile_names() {
        let name = r#"/tmp/a 'b' "c" \d $(rm -rf ~)"#;
        assert_eq!(
            shell_quote(name),
            r#"'/tmp/a '\''b'\'' "c" \d $(rm -rf ~)'"#
        );
        let script = format!("echo {}", shell_quote(name));
        let apple = applescript_string(&script);
        assert!(apple.starts_with('"') && apple.ends_with('"'));
        assert!(
            !apple[1..apple.len() - 1]
                .replace("\\\\", "")
                .replace("\\\"", "")
                .contains('"')
        );
    }

    #[test]
    fn trash_names_keep_the_extension() {
        assert_eq!(
            trash_name(Path::new("/Applications/A B.app"), "Cardinal 1-0"),
            "A B (Cardinal 1-0).app"
        );
        assert_eq!(
            trash_name(Path::new("/Library/LaunchDaemons/com.x.y.plist"), "t"),
            "com.x.y (t).plist"
        );
        assert_eq!(
            trash_name(Path::new("/Library/Application Support/Foo"), "t"),
            "Foo (t)"
        );
    }

    #[test]
    fn protected_paths_are_refused() {
        assert!(is_protected(Path::new("/")));
        assert!(is_protected(Path::new("/Library")));
        assert!(is_protected(Path::new("/System/Applications/Mail.app")));
        assert!(is_protected(Path::new("/usr/bin/true")));
        assert!(is_protected(&HOME.join("Library")));
        assert!(is_protected(&HOME));
        assert!(!is_protected(Path::new("/Applications/Spotify.app")));
        assert!(!is_protected(Path::new(
            "/Library/LaunchDaemons/com.acme.helper.plist"
        )));
        assert!(!is_protected(&HOME.join("Library/Caches/com.acme.app")));
    }
}
