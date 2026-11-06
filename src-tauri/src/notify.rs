//! The notifications a run that ends on its own posts: a crash, a test run, or a long task finishing, each behind its
//! own switch in Settings > Preferences.

use crate::{config::Settings, permission, sessions::SessionView, App};
use tauri::{plugin::PermissionState, AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

/// "48s", "2m 3s", "1h 5m".
fn took(millis: u64) -> String {
    let secs = millis / 1000;
    match secs {
        ..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs / 60 % 60),
    }
}

/// How long a clean run took, when it's long enough that you've likely looked away.
fn long_task(code: Option<i32>, millis: u64) -> Option<String> {
    (code == Some(0) && millis >= 30_000).then(|| took(millis))
}

/// A test run, going by words in its name or command: test, test:unit, e2e, vitest run, jest, node --test.
fn is_test(session: &SessionView) -> bool {
    let text = format!("{} {}", session.label, session.command).to_lowercase();
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| matches!(word, "test" | "tests" | "e2e" | "vitest" | "jest" | "mocha" | "cypress"))
}

/// The title and body for a run that ended on its own: a crash, a test run, or a long task finishing, each behind
/// its own switch.
fn notification(session: &SessionView, failed: bool, settings: &Settings) -> Option<(String, String)> {
    let ran = session.ended_at.map_or(0, |end| end.saturating_sub(session.started_at));
    let test = settings.notify_on_tests && is_test(session);
    match (failed, long_task(session.code, ran)) {
        (true, _) if settings.notify_on_failure || test => {
            // A keep-alive script's first crash; if it gives up, the plain notice follows.
            let restarting = if session.restart_in.is_some() { ", restarting" } else { "" };
            Some((
                format!("{} failed", session.label),
                format!("{}: exit code {}{restarting}", session.project, session.code.unwrap_or_default()),
            ))
        }
        (false, _) if test && session.code == Some(0) => {
            Some((format!("{} passed in {}", session.label, took(ran)), session.project.clone()))
        }
        (false, Some(took)) if settings.notify_on_finish => {
            Some((format!("{} finished in {took}", session.label), session.project.clone()))
        }
        _ => None,
    }
}

/// Async: macOS answers on another thread. "prompt" until it's been asked, then "granted" or "denied".
#[tauri::command]
pub async fn notification_permission() -> PermissionState {
    permission::state()
}

/// macOS asks only once, so after a "Don't Allow" this opens System Settings, the one place left to allow them.
#[tauri::command]
pub async fn allow_notifications(app: AppHandle) -> Result<PermissionState, String> {
    if permission::state() != PermissionState::Denied {
        return permission::ask();
    }
    let pane = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";
    let url = format!("{pane}?id={}", app.config().identifier);
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())?;
    Ok(PermissionState::Denied)
}

/// A crash, a test run, or a long task finishing. Stopped runs never get here.
pub fn script_ended(app: &AppHandle, session: &SessionView, failed: bool) {
    // The output is already in front of them, and `pnpm outdated` exiting 1 isn't news.
    let watching = app.get_webview_window("terminal").is_some_and(|window| window.is_focused().unwrap_or(false));
    if watching {
        return;
    }
    let settings = app.state::<App>().config.lock().unwrap().settings.clone();
    if let Some((title, body)) = notification(session, failed, &settings) {
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::Status;

    #[test]
    fn announces_clean_runs_of_30_seconds_or_more() {
        assert_eq!(long_task(Some(0), 31_000).as_deref(), Some("31s"));
        assert_eq!(long_task(Some(0), 123_400).as_deref(), Some("2m 3s"));
        assert_eq!(long_task(Some(0), 3_725_000).as_deref(), Some("1h 2m"));
        assert_eq!(long_task(Some(0), 29_999), None);
        assert_eq!(long_task(Some(0), 5_000), None);
        assert_eq!(long_task(Some(1), 60_000), None);
        assert_eq!(long_task(None, 60_000), None);
    }

    /// `web`'s build, ended on its own with `code` after `secs`.
    fn ended(code: i32, secs: u64) -> SessionView {
        SessionView {
            id: 1,
            project_id: "p".into(),
            project: "web".into(),
            label: "build".into(),
            command: "pnpm build".into(),
            status: Status::Exited,
            code: Some(code),
            started_at: 1_000,
            ended_at: Some(1_000 + secs * 1000),
            url: None,
            busy_port: None,
            restart_in: None,
            cwd: "/src/web".into(),
            worktree: None,
        }
    }

    #[test]
    fn each_switch_silences_only_its_own_notification() {
        let (long, short, crash) = (ended(0, 123), ended(0, 5), ended(1, 3));
        let all = Settings::default();
        let no_finish = Settings { notify_on_finish: false, ..Settings::default() };
        let no_crash = Settings { notify_on_failure: false, ..Settings::default() };
        let post = |s: &SessionView, settings| notification(s, s.code != Some(0), settings);
        let title = |s: &SessionView, settings| post(s, settings).map(|(title, _)| title);

        assert_eq!(post(&long, &all), Some(("build finished in 2m 3s".into(), "web".into())));
        assert_eq!(post(&crash, &all), Some(("build failed".into(), "web: exit code 1".into())));
        assert_eq!(post(&short, &all), None);
        assert_eq!(title(&long, &no_finish), None);
        assert_eq!(title(&crash, &no_finish).as_deref(), Some("build failed"));
        assert_eq!(title(&long, &no_crash).as_deref(), Some("build finished in 2m 3s"));
        assert_eq!(title(&crash, &no_crash), None);

        let restarting = SessionView { restart_in: Some(1), ..crash.clone() };
        let body = post(&restarting, &all).map(|(_, body)| body);
        assert_eq!(body.as_deref(), Some("web: exit code 1, restarting"));
    }

    #[test]
    fn test_runs_say_how_they_went_however_long_they_took() {
        let test = |code, secs| SessionView { label: "test:unit".into(), command: "pnpm run test:unit".into(), ..ended(code, secs) };
        let all = Settings::default();
        let only_tests = Settings { notify_on_failure: false, notify_on_finish: false, ..Settings::default() };
        let no_tests = Settings { notify_on_tests: false, ..Settings::default() };
        let post = |s: &SessionView, settings| notification(s, s.code != Some(0), settings);
        let title = |s: &SessionView, settings| post(s, settings).map(|(title, _)| title);

        assert_eq!(post(&test(0, 5), &all), Some(("test:unit passed in 5s".into(), "web".into())));
        assert_eq!(title(&test(0, 123), &all).as_deref(), Some("test:unit passed in 2m 3s"));
        assert_eq!(post(&test(1, 5), &only_tests), Some(("test:unit failed".into(), "web: exit code 1".into())));
        assert_eq!(title(&test(0, 5), &only_tests).as_deref(), Some("test:unit passed in 5s"));
        assert_eq!(title(&test(0, 5), &no_tests), None);
        assert_eq!(title(&test(0, 123), &no_tests).as_deref(), Some("test:unit finished in 2m 3s"));
        assert_eq!(title(&ended(0, 5), &all), None); // a quick build is still quiet
    }

    #[test]
    fn spots_test_runs_by_their_name_or_command() {
        let run = |label: &str, command: &str| is_test(&SessionView { label: label.into(), command: command.into(), ..ended(0, 1) });
        assert!(run("test", "pnpm run test"));
        assert!(run("ui test:e2e", "pnpm --filter @acme/ui run test:e2e"));
        assert!(run("unit", "npx vitest run"));
        assert!(run("e2e", "pnpm run e2e"));
        assert!(run("Specs", "node --test"));
        assert!(!run("build", "pnpm run build"));
        assert!(!run("latest", "npm i -g pnpm@latest"));
        assert!(!run("pretest", "pnpm run pretest"));
    }
}
