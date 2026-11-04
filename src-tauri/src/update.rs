//! In-app updates from the latest GitHub release. The state lives here, not in a window,
//! so every window shows the same banner and an update downloads once.

use serde::Serialize;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Windows ask on every focus; a few checks a day is plenty.
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

/// What the update banner shows. Every change reaches every window as `update-changed`.
#[derive(Clone, Default, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    #[default]
    Idle,
    Available { version: String },
    /// `percent` is missing while the download's size is unknown.
    Downloading { version: String, percent: Option<u8> },
    /// Downloaded and verified. Installing waits for a click, because restarting stops the scripts.
    Ready { version: String },
    Installing { version: String },
    Failed { version: String },
}

#[derive(Default)]
pub struct Updates(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    status: Status,
    update: Option<Update>,
    package: Option<Vec<u8>>,
    checked_at: Option<Instant>,
}

impl Updates {
    fn set(&self, app: &AppHandle, status: Status) {
        self.0.lock().unwrap().status = status.clone();
        let _ = app.emit("update-changed", status);
    }
}

/// Downloading, downloaded or installing: checking again would only offer the same version.
fn busy(status: &Status) -> bool {
    matches!(status, Status::Downloading { .. } | Status::Ready { .. } | Status::Installing { .. })
}

async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<Option<Update>> {
    let handle = app.clone();
    app.updater_builder()
        // Windows' installer exits the app straight away, so stop the scripts as quitting does.
        .on_before_exit(move || handle.state::<crate::App>().sessions.shutdown())
        .build()?
        .check()
        .await
}

/// The update status, after asking GitHub for a newer release when the last check is a few hours old.
/// `force` asks anyway, and reports a failed check instead of keeping the last status.
#[tauri::command]
pub async fn check_for_update(app: AppHandle, force: bool) -> Result<Status, String> {
    let updates = app.state::<Updates>();
    {
        let inner = updates.0.lock().unwrap();
        let fresh = inner.checked_at.is_some_and(|at| at.elapsed() < RECHECK);
        if busy(&inner.status) || (fresh && !force) {
            return Ok(inner.status.clone());
        }
    }
    let found = match check(&app).await {
        Ok(found) => found,
        Err(error) if force => return Err(format!("Couldn't check for updates: {error}")),
        Err(_) => return Ok(updates.0.lock().unwrap().status.clone()), // offline: the next focus tries again
    };
    let status = {
        let mut inner = updates.0.lock().unwrap();
        if busy(&inner.status) {
            return Ok(inner.status.clone()); // another window started the download meanwhile
        }
        inner.checked_at = Some(Instant::now());
        inner.status = match &found {
            Some(update) => Status::Available { version: update.version.clone() },
            None => Status::Idle,
        };
        inner.update = found;
        inner.status.clone()
    };
    let _ = app.emit("update-changed", &status);
    Ok(status)
}

/// Downloads and verifies the update, with its progress in every window.
#[tauri::command]
pub async fn download_update(app: AppHandle) -> Result<(), String> {
    let updates = app.state::<Updates>();
    let update = {
        let mut inner = updates.0.lock().unwrap();
        let startable = matches!(inner.status, Status::Available { .. } | Status::Failed { .. });
        let Some(update) = inner.update.clone().filter(|_| startable) else {
            return Ok(()); // another window started it
        };
        inner.status = Status::Downloading { version: update.version.clone(), percent: None };
        update
    };
    let version = update.version.clone();
    let _ = app.emit("update-changed", Status::Downloading { version: version.clone(), percent: None });
    let (mut received, mut shown) = (0u64, None);
    let downloaded = update
        .download(
            |chunk, size| {
                received += chunk as u64;
                let percent = size.map(|size| (received * 100 / size.max(1)).min(100) as u8);
                if percent != shown {
                    shown = percent;
                    updates.set(&app, Status::Downloading { version: version.clone(), percent });
                }
            },
            || {},
        )
        .await;
    match downloaded {
        Ok(package) => {
            updates.0.lock().unwrap().package = Some(package);
            updates.set(&app, Status::Ready { version });
            Ok(())
        }
        Err(error) => {
            updates.set(&app, Status::Failed { version });
            Err(format!("Couldn't download the update: {error}"))
        }
    }
}

/// Installs the downloaded update and restarts into it. On Windows the installer closes and reopens the app.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let updates = app.state::<Updates>();
    let (update, package) = {
        let mut inner = updates.0.lock().unwrap();
        let ready = matches!(inner.status, Status::Ready { .. });
        match (ready, inner.update.clone(), inner.package.take()) {
            (true, Some(update), Some(package)) => {
                inner.status = Status::Installing { version: update.version.clone() };
                (update, package)
            }
            _ => return Ok(()), // a second click while installing
        }
    };
    let version = update.version.clone();
    let _ = app.emit("update-changed", Status::Installing { version: version.clone() });
    if let Err(error) = update.install(package) {
        updates.set(&app, Status::Failed { version });
        return Err(format!("Couldn't install the update: {error}"));
    }
    app.request_restart(); // exits through RunEvent::Exit, which stops the scripts
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_has_the_shape_the_banner_reads() {
        let downloading = Status::Downloading { version: "0.2.0".into(), percent: Some(42) };
        assert_eq!(
            serde_json::to_value(downloading).unwrap(),
            json!({ "state": "downloading", "version": "0.2.0", "percent": 42 })
        );
        assert_eq!(serde_json::to_value(Status::Idle).unwrap(), json!({ "state": "idle" }));
    }
}
