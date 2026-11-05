use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const SCROLLBACK: usize = 1 << 20;

/// Everything needed to (re)start a script.
#[derive(Clone)]
pub struct Spec {
    pub project_id: String,
    pub project: String,
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub node_bin: Option<PathBuf>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Stopping,
    /// Ended because the user stopped it.
    Stopped,
    /// Ended on its own.
    Exited,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: u32,
    pub project_id: String,
    pub project: String,
    pub label: String,
    pub command: String,
    pub status: Status,
    pub code: Option<i32>,
    pub url: Option<String>,
}

struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

struct Session {
    spec: Spec,
    status: Status,
    code: Option<i32>,
    url: Option<String>,
    pid: u32,
    /// Bumped on restart so the old process's threads can't touch the new run.
    run: u32,
    scrollback: Vec<u8>,
    pty: Option<Pty>,
    /// Ctrl-C was typed in the terminal, so ending now is the user's doing, not a crash.
    interrupted: bool,
}

impl Session {
    fn view(&self, id: u32) -> SessionView {
        SessionView {
            id,
            project_id: self.spec.project_id.clone(),
            project: self.spec.project.clone(),
            label: self.spec.label.clone(),
            command: self.spec.command.clone(),
            status: self.status,
            code: self.code,
            url: self.url.clone(),
        }
    }
}

type Sink = Box<dyn Fn(&[u8]) + Send>;

#[derive(Default)]
struct Inner {
    sessions: HashMap<u32, Session>,
    next_id: u32,
    /// The one session currently shown in the terminal window.
    sink: Option<(u32, Sink)>,
}

pub struct Sessions {
    inner: Mutex<Inner>,
    /// Called with no lock held, whenever anything in `list()` changes.
    on_change: Box<dyn Fn() + Send + Sync>,
    /// Called with no lock held, when a script ends with an error nobody asked for.
    on_failure: Box<dyn Fn(&SessionView) + Send + Sync>,
}

impl Sessions {
    pub fn new(
        on_change: Box<dyn Fn() + Send + Sync>,
        on_failure: Box<dyn Fn(&SessionView) + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self { inner: Mutex::default(), on_change, on_failure })
    }

    pub fn list(&self) -> Vec<SessionView> {
        let inner = self.inner.lock().unwrap();
        let mut list: Vec<_> = inner.sessions.iter().map(|(id, s)| s.view(*id)).collect();
        list.sort_by_key(|s| s.id);
        list
    }

    /// Starts a script, or returns the session already running it.
    pub fn start(self: &Arc<Self>, spec: Spec) -> Result<u32, String> {
        if !Path::new(&spec.cwd).is_dir() {
            return Err(format!("Folder not found: {}", spec.cwd));
        }
        let mut inner = self.inner.lock().unwrap();
        let existing = inner
            .sessions
            .iter()
            .find(|(_, s)| s.spec.project_id == spec.project_id && s.spec.command == spec.command)
            .map(|(id, s)| (*id, s.status, s.run));
        let (id, run) = match existing {
            Some((id, Status::Running | Status::Stopping, _)) => return Ok(id),
            Some((id, _, run)) => (id, run + 1),
            None => {
                inner.next_id += 1;
                (inner.next_id, 0)
            }
        };

        let err = |e: Box<dyn std::fmt::Display>| e.to_string();
        let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).map_err(|e| err(Box::new(e)))?;
        let mut child = pair.slave.spawn_command(shell_command(&spec)).map_err(|e| err(Box::new(e)))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| err(Box::new(e)))?;
        let writer = pair.master.take_writer().map_err(|e| err(Box::new(e)))?;
        let pid = child.process_id().unwrap_or(0);

        let pty = Some(Pty { master: pair.master, writer });
        let session = Session {
            spec,
            status: Status::Running,
            code: None,
            url: None,
            pid,
            run,
            scrollback: vec![],
            pty,
            interrupted: false,
        };
        inner.sessions.insert(id, session);
        if let Some((_, sink)) = inner.sink.as_ref().filter(|(sink_id, _)| *sink_id == id) {
            sink(b"\x1bc"); // a restart starts from a clean screen
        }
        drop(inner);

        let this = self.clone();
        thread::spawn(move || this.pump(id, run, reader));
        let this = self.clone();
        thread::spawn(move || {
            let code = child.wait().ok().map(|s| s.exit_code() as i32);
            let mut inner = this.inner.lock().unwrap();
            let mut failed = None;
            if let Some(s) = inner.sessions.get_mut(&id).filter(|s| s.run == run) {
                let stopped = s.status == Status::Stopping || s.interrupted;
                s.status = if stopped { Status::Stopped } else { Status::Exited };
                s.code = code;
                s.pty = None; // hangs up anything left behind in the terminal
                failed = (!stopped && code.is_some_and(|c| c != 0)).then(|| s.view(id));
            }
            drop(inner);
            (this.on_change)();
            if let Some(session) = failed {
                (this.on_failure)(&session);
            }
        });
        (self.on_change)();
        Ok(id)
    }

    fn pump(&self, id: u32, run: u32, mut reader: Box<dyn Read + Send>) {
        let mut buf = [0u8; 16 * 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            let mut inner = self.inner.lock().unwrap();
            let Inner { sessions, sink, .. } = &mut *inner;
            let Some(s) = sessions.get_mut(&id).filter(|s| s.run == run) else { break };
            s.scrollback.extend_from_slice(chunk);
            if s.scrollback.len() > 2 * SCROLLBACK {
                s.scrollback.drain(..s.scrollback.len() - SCROLLBACK);
            }
            if let Some((_, send)) = sink.as_ref().filter(|(sink_id, _)| *sink_id == id) {
                send(chunk);
            }
            let found_url = s.url.is_none() && {
                s.url = find_url(chunk);
                s.url.is_some()
            };
            drop(inner);
            if found_url {
                (self.on_change)();
            }
        }
    }

    /// Ctrl-C, like a terminal would: zsh gives the script its own process group,
    /// so signalling the shell alone would leave the dev server running.
    fn interrupt(&self, id: u32) -> Option<(u32, u32)> {
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.get_mut(&id).filter(|s| s.status == Status::Running)?;
        s.status = Status::Stopping;
        if let Some(pty) = &mut s.pty {
            let _ = pty.writer.write_all(b"\x03");
            let _ = pty.writer.flush();
        }
        let target = (s.pid, s.run);
        drop(inner);
        (self.on_change)();
        Some(target)
    }

    fn wait_exit(&self, id: u32, run: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let inner = self.inner.lock().unwrap();
            let alive = inner.sessions.get(&id).is_some_and(|s| {
                s.run == run && matches!(s.status, Status::Running | Status::Stopping)
            });
            drop(inner);
            if !alive {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Escalates for scripts that trap or ignore Ctrl-C.
    fn reap(&self, id: u32, run: u32, pid: u32) {
        for (grace, signal) in [(Duration::from_secs(3), Signal::Term), (Duration::from_secs(2), Signal::Kill)] {
            if self.wait_exit(id, run, grace) {
                return;
            }
            kill_tree(pid, signal);
        }
        self.wait_exit(id, run, Duration::from_secs(1));
    }

    pub fn stop(self: &Arc<Self>, id: u32) {
        if let Some((pid, run)) = self.interrupt(id) {
            let this = self.clone();
            thread::spawn(move || this.reap(id, run, pid));
        }
    }

    pub fn stop_all(self: &Arc<Self>) {
        for session in self.list() {
            self.stop(session.id);
        }
    }

    pub fn restart(self: &Arc<Self>, id: u32) -> Result<(), String> {
        let spec = {
            let inner = self.inner.lock().unwrap();
            inner.sessions.get(&id).ok_or("Session is gone")?.spec.clone()
        };
        match self.interrupt(id) {
            Some((pid, run)) => {
                let this = self.clone();
                thread::spawn(move || {
                    this.reap(id, run, pid);
                    let _ = this.start(spec);
                });
                Ok(())
            }
            None => self.start(spec).map(|_| ()),
        }
    }

    /// Forgets a finished session.
    pub fn clear(&self, id: u32) {
        let mut inner = self.inner.lock().unwrap();
        if inner.sessions.get(&id).is_some_and(|s| s.pty.is_none()) {
            inner.sessions.remove(&id);
        }
        drop(inner);
        (self.on_change)();
    }

    /// Replays the scrollback, then streams; atomic so nothing is lost or doubled.
    pub fn attach(&self, id: u32, sink: Sink) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(s) = inner.sessions.get(&id) {
            sink(&s.scrollback);
        }
        inner.sink = Some((id, sink));
    }

    pub fn detach(&self) {
        self.inner.lock().unwrap().sink = None;
    }

    /// The scrollback as plain text, for AI agents.
    pub fn output(&self, id: u32) -> Option<String> {
        let scrollback = self.inner.lock().unwrap().sessions.get(&id)?.scrollback.clone();
        Some(String::from_utf8_lossy(&strip_ansi(&scrollback)).into_owned())
    }

    pub fn send_input(&self, id: u32, data: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(s) = inner.sessions.get_mut(&id) {
            s.interrupted |= data.contains(&3);
            if let Some(pty) = s.pty.as_mut() {
                let _ = pty.writer.write_all(data);
                let _ = pty.writer.flush();
            }
        }
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) {
        let inner = self.inner.lock().unwrap();
        if let Some(pty) = inner.sessions.get(&id).and_then(|s| s.pty.as_ref()) {
            let _ = pty.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    }

    /// Blocking: runs on quit, so nothing outlives the app.
    pub fn shutdown(&self) {
        let targets: Vec<_> = {
            let mut inner = self.inner.lock().unwrap();
            let alive = inner.sessions.iter_mut().filter(|(_, s)| s.pty.is_some());
            alive
                .map(|(id, s)| {
                    s.status = Status::Stopping; // quitting shouldn't read as every script crashing
                    (*id, s.run, s.pid)
                })
                .collect()
        };
        for (_, _, pid) in &targets {
            kill_tree(*pid, Signal::Term);
        }
        for (id, run, pid) in targets {
            if !self.wait_exit(id, run, Duration::from_millis(1500)) {
                kill_tree(pid, Signal::Kill);
            }
        }
    }
}

fn shell_command(spec: &Spec) -> CommandBuilder {
    #[cfg(unix)]
    let mut cmd = {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        // Set after the rc files have run, or an nvm default in .zshrc would win over the project's version.
        let script = match &spec.node_bin {
            Some(bin) => format!("export PATH={}:\"$PATH\"; {}", crate::project::sh_quote(&bin.to_string_lossy()), spec.command),
            None => spec.command.clone(),
        };
        // Login + interactive: a GUI app gets a bare PATH, this gives the one the user's terminal has.
        let mut cmd = CommandBuilder::new(shell);
        cmd.args(["-ilc", &script]);
        cmd
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = CommandBuilder::new("cmd");
        cmd.args(["/C", &spec.command]);
        if let Some(bin) = &spec.node_bin {
            let path = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{};{path}", bin.display()));
        }
        cmd
    };
    cmd.cwd(&spec.cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if std::env::var_os("LANG").is_none() {
        cmd.env("LANG", "en_US.UTF-8");
    }
    cmd
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

fn descendants(table: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut tree = vec![root];
    let mut i = 0;
    while i < tree.len() {
        let parent = tree[i];
        tree.extend(table.iter().filter(|(_, ppid)| *ppid == parent).map(|(pid, _)| *pid));
        i += 1;
    }
    tree
}

#[cfg(unix)]
fn kill_tree(root: u32, signal: Signal) {
    let Ok(out) = Command::new("ps").args(["-A", "-o", "pid=,ppid="]).output() else { return };
    let table: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace().map(|p| p.parse().ok());
            Some((parts.next()??, parts.next()??))
        })
        .collect();
    let signal = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    for pid in descendants(&table, root) {
        unsafe { libc::kill(pid as i32, signal) };
    }
}

#[cfg(windows)]
fn kill_tree(root: u32, _signal: Signal) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &root.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    out
}

/// The dev-server address a script printed, normalised to localhost.
/// shortcut: scans one read at a time, so a URL split across two reads is missed; buffer the tail if that shows up.
fn find_url(chunk: &[u8]) -> Option<String> {
    let text = strip_ansi(chunk); // Vite colours the port, which splits the URL
    let text = String::from_utf8_lossy(&text);
    let hosts = ["localhost:", "127.0.0.1:", "0.0.0.0:", "[::1]:"];
    let schemes = ["http://", "https://"];
    let found = schemes.iter().flat_map(|scheme| hosts.iter().map(move |host| (*scheme, *host)));
    found
        .filter_map(|(scheme, host)| {
            let at = text.find(&format!("{scheme}{host}"))?;
            let rest = &text[at + scheme.len() + host.len()..];
            let port: String = rest.chars().take_while(char::is_ascii_digit).collect();
            port.parse::<u16>().ok()?;
            let path: String = rest[port.len()..]
                .chars()
                .take_while(|c| c.is_ascii_graphic() && !"\"'<>`)".contains(*c))
                .collect();
            let path = path.trim_end_matches(['.', ',', ';', ':']);
            Some((at, format!("{scheme}localhost:{port}{path}")))
        })
        .min_by_key(|(at, _)| *at)
        .map(|(_, url)| url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn spec(command: &str) -> Spec {
        Spec {
            project_id: format!("p{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            project: "demo".into(),
            label: "demo".into(),
            command: command.into(),
            cwd: std::env::temp_dir().to_string_lossy().into(),
            node_bin: None,
        }
    }

    fn output(sessions: &Sessions, id: u32) -> String {
        let inner = sessions.inner.lock().unwrap();
        String::from_utf8_lossy(&inner.sessions[&id].scrollback).into_owned()
    }

    fn view(sessions: &Sessions, id: u32) -> SessionView {
        sessions.list().into_iter().find(|s| s.id == id).unwrap()
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(30));
        }
    }

    fn pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    /// The real pid of a child, printed by the script itself.
    fn printed_pid(sessions: &Sessions, id: u32, tag: &str) -> u32 {
        let mut pid = None;
        wait_for("pid", || {
            pid = output(sessions, id).split(tag).nth(1).and_then(|rest| {
                rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
            });
            pid.is_some()
        });
        pid.unwrap()
    }

    fn new_sessions() -> Arc<Sessions> {
        Sessions::new(Box::new(|| {}), Box::new(|_| {}))
    }

    #[test]
    fn reports_a_crash_but_not_a_clean_exit_or_a_stop_the_user_asked_for() {
        let failures = Arc::new(Mutex::new(Vec::new()));
        let seen = failures.clone();
        let report = move |s: &SessionView| seen.lock().unwrap().push((s.label.clone(), s.code));
        let sessions = Sessions::new(Box::new(|| {}), Box::new(report));

        let clean = sessions.start(spec("exit 0")).unwrap();
        wait_for("clean exit", || view(&sessions, clean).status == Status::Exited);

        let stopped = sessions.start(spec("sleep 30")).unwrap();
        sessions.stop(stopped);
        wait_for("stop button", || view(&sessions, stopped).status == Status::Stopped);

        let typed = sessions.start(spec("echo ready; sleep 30")).unwrap();
        wait_for("ready", || output(&sessions, typed).contains("ready"));
        sessions.send_input(typed, b"\x03");
        wait_for("typed Ctrl-C", || view(&sessions, typed).status == Status::Stopped);

        let quit = sessions.start(spec("echo ready; sleep 30")).unwrap();
        wait_for("ready", || output(&sessions, quit).contains("ready"));
        sessions.shutdown();
        wait_for("quit", || view(&sessions, quit).status == Status::Stopped);

        assert!(failures.lock().unwrap().is_empty());
        sessions.start(Spec { label: "boom".into(), ..spec("exit 7") }).unwrap();
        wait_for("failure", || !failures.lock().unwrap().is_empty());
        assert_eq!(*failures.lock().unwrap(), [("boom".to_string(), Some(7))]);
    }

    #[test]
    fn captures_output_and_exit_code() {
        let sessions = new_sessions();
        let id = sessions.start(spec("echo hello-pty; exit 3")).unwrap();
        wait_for("exit", || view(&sessions, id).status == Status::Exited);
        wait_for("output", || output(&sessions, id).contains("hello-pty"));
        assert_eq!(view(&sessions, id).code, Some(3));
    }

    #[test]
    fn the_same_script_only_runs_once() {
        let sessions = new_sessions();
        let s = spec("sleep 30");
        let first = sessions.start(s.clone()).unwrap();
        assert_eq!(sessions.start(s).unwrap(), first);
        assert_eq!(sessions.list().len(), 1);
        sessions.stop(first);
        wait_for("stop", || view(&sessions, first).status == Status::Stopped);
    }

    #[test]
    fn stop_reaches_the_real_child_process() {
        let sessions = new_sessions();
        let id = sessions.start(spec("sh -c 'echo child=$$; exec sleep 60'")).unwrap();
        let child = printed_pid(&sessions, id, "child=");
        assert!(pid_alive(child));
        sessions.stop(id);
        wait_for("stop", || view(&sessions, id).status == Status::Stopped);
        wait_for("child gone", || !pid_alive(child));
    }

    #[test]
    fn stop_escalates_when_ctrl_c_is_ignored() {
        let sessions = new_sessions();
        let id = sessions.start(spec("trap '' INT TERM; echo child=$$; while true; do sleep 1; done")).unwrap();
        let shell = printed_pid(&sessions, id, "child=");
        sessions.stop(id);
        wait_for("stop", || view(&sessions, id).status == Status::Stopped);
        wait_for("shell gone", || !pid_alive(shell));
    }

    #[test]
    fn restart_keeps_the_session_id_and_starts_clean() {
        let sessions = new_sessions();
        let id = sessions.start(spec("echo run=$$; sleep 60")).unwrap();
        let first = printed_pid(&sessions, id, "run=");
        sessions.restart(id).unwrap();
        wait_for("second run", || {
            view(&sessions, id).status == Status::Running
                && output(&sessions, id).contains("run=")
                && printed_pid(&sessions, id, "run=") != first
        });
        assert_eq!(output(&sessions, id).matches("run=").count(), 1);
        sessions.stop(id);
        wait_for("stop", || view(&sessions, id).status == Status::Stopped);
    }

    #[test]
    fn attach_replays_scrollback_then_streams() {
        let sessions = new_sessions();
        let id = sessions.start(spec("echo before; read x; echo after-$x; sleep 60")).unwrap();
        wait_for("before", || output(&sessions, id).contains("before"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        sessions.attach(id, Box::new(move |b| sink.lock().unwrap().extend_from_slice(b)));
        sessions.send_input(id, b"typed\n");
        let text = || String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
        wait_for("stream", || text().contains("after-typed"));
        assert!(text().contains("before"));
        sessions.stop(id);
        wait_for("stop", || view(&sessions, id).status == Status::Stopped);
    }

    #[test]
    fn output_for_agents_is_plain_text() {
        let sessions = new_sessions();
        let id = sessions.start(spec("printf '\\033[31mred\\033[0m plain\\n'")).unwrap();
        wait_for("exit", || view(&sessions, id).status == Status::Exited);
        wait_for("output", || sessions.output(id).unwrap().contains("red plain"));
        assert_eq!(sessions.output(id + 1), None);
    }

    #[test]
    fn project_node_beats_whatever_the_rc_files_put_on_path() {
        let dir = std::env::temp_dir().join(format!("scriptdec-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("node");
        std::fs::write(&bin, "#!/bin/sh\necho fake-node-v1\n").unwrap();
        Command::new("chmod").args(["+x", bin.to_str().unwrap()]).status().unwrap();
        let sessions = new_sessions();
        let id = sessions.start(Spec { node_bin: Some(dir), ..spec("node") }).unwrap();
        wait_for("exit", || view(&sessions, id).status == Status::Exited);
        wait_for("output", || output(&sessions, id).contains("fake-node-v1"));
    }

    #[test]
    fn scrollback_stays_bounded_and_keeps_the_newest_output() {
        let sessions = new_sessions();
        let id = sessions.start(spec("head -c 3000000 /dev/zero | tr '\\0' x; echo; echo tail-marker")).unwrap();
        wait_for("exit", || view(&sessions, id).status == Status::Exited);
        wait_for("tail", || output(&sessions, id).contains("tail-marker"));
        assert!(sessions.inner.lock().unwrap().sessions[&id].scrollback.len() <= 2 * SCROLLBACK);
    }

    #[test]
    fn refuses_a_missing_folder() {
        let err = new_sessions().start(Spec { cwd: "/no/such/dir".into(), ..spec("true") });
        assert!(err.unwrap_err().contains("/no/such/dir"));
    }

    #[test]
    fn shutdown_stops_everything() {
        let sessions = new_sessions();
        let id = sessions.start(spec("echo child=$$; sleep 60")).unwrap();
        let shell = printed_pid(&sessions, id, "child=");
        sessions.shutdown();
        wait_for("shell gone", || !pid_alive(shell));
    }

    #[test]
    fn finds_dev_server_urls() {
        let url = |s: &str| find_url(s.as_bytes());
        assert_eq!(url("ready on http://localhost:3000"), Some("http://localhost:3000".into()));
        assert_eq!(url("at https://127.0.0.1:8443/app/.\n"), Some("https://localhost:8443/app/".into()));
        assert_eq!(url("Local: http://0.0.0.0:5173/"), Some("http://localhost:5173/".into()));
        assert_eq!(url("nothing here, http://example.com:80"), None);
        assert_eq!(url("http://localhost:99999"), None);
    }

    #[test]
    fn finds_urls_split_by_colour_codes() {
        let vite = "Local:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r\n";
        assert_eq!(find_url(vite.as_bytes()), Some("http://localhost:5173/".into()));
    }

    #[test]
    fn walks_the_whole_process_tree() {
        let table = [(2, 1), (3, 2), (4, 3), (9, 8)];
        assert_eq!(descendants(&table, 1), [1, 2, 3, 4]);
    }
}
