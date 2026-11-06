//! Whether macOS shows ScriptDeck's notifications. The notification plugin answers "granted" on every desktop, so
//! this asks UNUserNotificationCenter. Elsewhere there's nothing to ask.

#[cfg(not(target_os = "macos"))]
use tauri::plugin::PermissionState;

#[cfg(not(target_os = "macos"))]
pub fn state() -> PermissionState {
    PermissionState::Granted
}

#[cfg(not(target_os = "macos"))]
pub fn ask() -> Result<PermissionState, String> {
    Ok(PermissionState::Granted)
}

#[cfg(target_os = "macos")]
pub use mac::{ask, state};

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::{rc::Retained, runtime::Bool};
    use objc2_foundation::NSError;
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
    };
    use std::{ptr::NonNull, sync::mpsc, time::Duration};
    use tauri::plugin::PermissionState;

    /// None outside an app bundle, where asking throws, and in dev builds, which post as Terminal.
    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        let bundled = std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
        (bundled && !tauri::is_dev()).then(UNUserNotificationCenter::currentNotificationCenter)
    }

    /// Waits for the answer, which comes on another thread. Settings waits on it at every focus, so a late answer
    /// counts as granted rather than holding the window blank.
    pub fn state() -> PermissionState {
        let Some(center) = center() else { return PermissionState::Granted };
        let (tx, rx) = mpsc::channel();
        let reply = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the system passes a live settings object.
            let _ = tx.send(unsafe { settings.as_ref() }.authorizationStatus());
        });
        center.getNotificationSettingsWithCompletionHandler(&reply);
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(UNAuthorizationStatus::NotDetermined) => PermissionState::Prompt,
            Ok(UNAuthorizationStatus::Denied) => PermissionState::Denied,
            _ => PermissionState::Granted,
        }
    }

    /// Shows macOS's Allow prompt and waits for the click. Once it's been answered, macOS answers alone.
    pub fn ask() -> Result<PermissionState, String> {
        let Some(center) = center() else { return Ok(PermissionState::Granted) };
        let (tx, rx) = mpsc::channel();
        let reply = RcBlock::new(move |_granted: Bool, error: *mut NSError| {
            // SAFETY: null, or an error that lives through this call.
            let _ = tx.send(unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string()));
        });
        let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
        center.requestAuthorizationWithOptions_completionHandler(options, &reply);
        match rx.recv() {
            Ok(Some(error)) => Err(error),
            _ => Ok(state()),
        }
    }
}
