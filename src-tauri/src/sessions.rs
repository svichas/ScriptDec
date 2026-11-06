use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SCROLLBACK: usize = 1 << 20;
/// A second of restart backoff, shortened so a test doesn't wait 31 s for a script to give up.
const BACKOFF_STEP: Duration = if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(1) };

/// Everything needed to (re)start a script.
#[derive(Clone)]
pub struct Spec {
    pub project_id: String,
    pub project: String,
    pub label: String,
    pub command: String,
    /// One script runs once per folder, so each worktree of a project can run it at the same time.
    pub cwd: String,
    pub node_bin: Option<PathBuf>,
    /// Restart it when it crashes.
    pub keep_alive: bool,
    /// The branch of the worktree it runs in, when that isn't the project's own folder.
    pub worktree: Option<String>,
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
    /// When the current run started and ended, in milliseconds since the epoch.
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub url: Option<String>,
    pub busy_port: Option<u16>,
    /// How long a crashed keep-alive script waits before it restarts, in seconds.
    pub restart_in: Option<u32>,
    pub cwd: String,
    pub worktree: Option<String>,
}

/// What listens on a port: a stray process, or one of our scripts.
#[derive(Serialize)]
pub struct PortOwner {
    pub pid: u32,
    pub command: String,
    pub session: Option<SessionView>,
}

/// What a script uses, summed over its process tree.
#[derive(Serialize, PartialEq, Debug)]
pub struct Usage {
    /// Percent of the whole machine, so 100 means every core is busy.
    pub cpu: f32,
    /// Resident memory in bytes.
    pub memory: u64,
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
    /// The port the script said was taken.
    busy_port: Option<u16>,
    pid: u32,
    /// Bumped on restart so the old process's threads can't touch the new run.
    run: u32,
    started_at: u64,
    ended_at: Option<u64>,
    scrollback: Vec<u8>,
    pty: Option<Pty>,
    /// Ctrl-C was typed in the terminal, so ending now is the user's doing, not a crash.
    interrupted: bool,
    /// Backoff steps until this crashed run restarts; taking it away cancels the restart.
    restart_in: Option<u32>,
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
            started_at: self.started_at,
            ended_at: self.ended_at,
            url: self.url.clone(),
            busy_port: self.busy_port,
            restart_in: self.restart_in,
            cwd: self.spec.cwd.clone(),
            worktree: self.spec.worktree.clone(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

type Sink = Box<dyn Fn(&[u8]) + Send>;
type OnExit = Box<dyn Fn(&SessionView, bool) + Send + Sync>;

#[derive(Default)]
struct Inner {
    /// By id, which is the order they were first started in.
    sessions: BTreeMap<u32, Session>,
    next_id: u32,
    /// The one session currently shown in the terminal window.
    sink: Option<(u32, Sink)>,
    /// Set by `shutdown()`, so a restart still reaping can't start a run that outlives the app.
    /// shortcut: never cleared, so if Windows' installer fails to launch after `on_before_exit`, no script starts
    /// until ScriptDeck is reopened; clear it on that error if it comes up.
    quitting: bool,
}

pub struct Sessions {
    inner: Mutex<Inner>,
    /// Called with no lock held, whenever anything in `list()` changes.
    on_change: Box<dyn Fn() + Send + Sync>,
    /// Called with no lock held, when a run ends on its own rather than being stopped, with whether it failed.
    on_exit: OnExit,
}

impl Sessions {
    pub fn new(on_change: Box<dyn Fn() + Send + Sync>, on_exit: OnExit) -> Arc<Self> {
        Arc::new(Self { inner: Mutex::default(), on_change, on_exit })
    }

    pub fn list(&self) -> Vec<SessionView> {
        self.inner.lock().unwrap().sessions.iter().map(|(id, s)| s.view(*id)).collect()
    }

    /// Each live session's usage, by id. Runs `ps`, so windows only ask while they're shown.
    pub fn usage(&self) -> HashMap<u32, Usage> {
        let roots: Vec<_> = {
            let inner = self.inner.lock().unwrap();
            inner.sessions.iter().filter(|(_, s)| s.pty.is_some()).map(|(id, s)| (*id, s.pid)).collect()
        };
        // shortcut: Unix only, through ps; Windows would need its own process APIs if it's wanted there.
        if roots.is_empty() || cfg!(windows) {
            return HashMap::new();
        }
        let procs = processes().unwrap_or_default();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        roots.into_iter().filter_map(|(id, pid)| Some((id, tree_usage(&procs, pid, cores)?))).collect()
    }

    /// Starts a script, or returns the session already running it in that folder.
    pub fn start(self: &Arc<Self>, spec: Spec) -> Result<u32, String> {
        if !Path::new(&spec.cwd).is_dir() {
            return Err(format!("Folder not found: {}", spec.cwd));
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.quitting {
            return Err("ScriptDeck is quitting".into());
        }
        let same = |s: &Spec| s.project_id == spec.project_id && s.command == spec.command && s.cwd == spec.cwd;
        let existing = inner
            .sessions
            .iter()
            .find(|(_, s)| same(&s.spec))
            .map(|(id, s)| (*id, s.status, s.run, s.restart_in));
        let (id, run, waited) = match existing {
            Some((id, Status::Running | Status::Stopping, ..)) => return Ok(id),
            // The wait of a pending restart carries over, so another quick crash waits longer.
            Some((id, _, run, waited)) => (id, run + 1, waited),
            None => {
                inner.next_id += 1;
                (inner.next_id, 0, None)
            }
        };

        let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;
        let child = pair.slave.spawn_command(shell_command(&spec)).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let pid = child.process_id().unwrap_or(0);

        let pty = Some(Pty { master: pair.master, writer });
        // A restart starts from a clean screen, except after a crash: its output is why you'd look.
        let scrollback = match waited {
            Some(_) => inner.sessions.get_mut(&id).map(|s| std::mem::take(&mut s.scrollback)).unwrap_or_default(),
            None => vec![],
        };
        let session = Session {
            spec,
            status: Status::Running,
            code: None,
            url: None,
            busy_port: None,
            pid,
            run,
            started_at: now_ms(),
            ended_at: None,
            scrollback,
            pty,
            interrupted: false,
            restart_in: None,
        };
        inner.sessions.insert(id, session);
        if let Some((_, sink)) = inner.sink.as_ref().filter(|(sink_id, _)| *sink_id == id && waited.is_none()) {
            sink(b"\x1bc");
        }
        drop(inner);

        let this = self.clone();
        thread::spawn(move || this.pump(id, run, reader));
        let this = self.clone();
        thread::spawn(move || this.watch(id, run, waited, child));
        (self.on_change)();
        Ok(id)
    }

    /// Waits for a run to end and records how. A crashed keep-alive script then restarts, after its backoff.
    fn watch(self: Arc<Self>, id: u32, run: u32, waited: Option<u32>, mut child: Box<dyn Child + Send + Sync>) {
        let started = Instant::now();
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let mut inner = self.inner.lock().unwrap();
        let (mut exited, mut restart) = (None, None);
        if let Some(s) = inner.sessions.get_mut(&id).filter(|s| s.run == run) {
            let stopped = s.status == Status::Stopping || s.interrupted;
            s.status = if stopped { Status::Stopped } else { Status::Exited };
            s.code = code;
            s.ended_at = Some(now_ms());
            s.pty = None; // hangs up anything left behind in the terminal
            let crashed = !stopped && code.is_some_and(|c| c != 0);
            if crashed && s.spec.keep_alive {
                s.restart_in = backoff(waited, started.elapsed());
            }
            // A keep-alive script says once that it's restarting, and once more if it gives up.
            let quiet = crashed && s.restart_in.is_some_and(|wait| wait != 1);
            exited = (!stopped && !quiet).then(|| (s.view(id), crashed));
            restart = s.restart_in.map(|wait| (wait, s.spec.clone()));
        }
        drop(inner);
        (self.on_change)();
        if let Some((session, failed)) = exited {
            (self.on_exit)(&session, failed);
        }
        let Some((wait, spec)) = restart else { return };
        thread::sleep(BACKOFF_STEP * wait);
        // Stop, Ctrl-C, Dismiss and Quit take the restart back; a start by hand meanwhile made a newer run.
        // shortcut: one landing between this check and start() is missed; check inside start() if that ever bites.
        let inner = self.inner.lock().unwrap();
        let due = inner.sessions.get(&id).is_some_and(|s| s.run == run && s.restart_in.is_some());
        drop(inner);
        if due && self.start(spec).is_err() {
            self.stop(id); // e.g. its folder is gone: show it failed, not restarting forever
        }
    }

    fn pump(&self, id: u32, run: u32, mut reader: Box<dyn Read + Send>) {
        let mut buf = [0u8; 16 * 1024];
        // What this run's output hasn't shown yet. Unix only for the port: naming its owner needs lsof.
        let (mut want_url, mut want_port) = (true, cfg!(unix));
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            // Read before taking the lock, which every script's output and every window waits on.
            let url = if want_url { find_url(chunk) } else { None };
            let busy_port = if want_port { find_busy_port(chunk) } else { None };
            want_url &= url.is_none();
            want_port &= busy_port.is_none();
            let mut inner = self.inner.lock().unwrap();
            let Inner { sessions, sink, .. } = &mut *inner;
            let Some(s) = sessions.get_mut(&id).filter(|s| s.run == run) else { break };
            s.scrollback.extend_from_slice(chunk);
            if s.scrollback.len() > 2 * SCROLLBACK {
                // From the start of a line, so a replay doesn't open on half an escape code.
                let cut = s.scrollback.len() - SCROLLBACK;
                let cut = s.scrollback[cut..].iter().position(|&b| b == b'\n').map_or(cut, |at| cut + at + 1);
                s.scrollback.drain(..cut);
            }
            if let Some((_, send)) = sink.as_ref().filter(|(sink_id, _)| *sink_id == id) {
                send(chunk);
            }
            let found = url.is_some() || busy_port.is_some();
            s.url = s.url.take().or(url);
            s.busy_port = s.busy_port.or(busy_port);
            drop(inner);
            if found {
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
        // A crashed script waiting to restart stays down.
        let waiting = self.inner.lock().unwrap().sessions.get_mut(&id).and_then(|s| s.restart_in.take());
        if waiting.is_some() {
            (self.on_change)();
        }
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

    /// What listens on `port`, and the live script whose process tree it's in, if any.
    pub fn port_owner(&self, port: u16) -> Option<PortOwner> {
        let (pid, command) = listener(port)?;
        let table = process_table();
        let inner = self.inner.lock().unwrap();
        let session = inner
            .sessions
            .iter()
            .filter(|(_, s)| matches!(s.status, Status::Running | Status::Stopping))
            .find(|(_, s)| descendants(&table, s.pid).contains(&pid))
            .map(|(id, s)| s.view(*id));
        Some(PortOwner { pid, command, session })
    }

    /// Stops what listens on `port`, if that's still `pid`: one of our scripts the normal way, anything
    /// else with SIGTERM to its tree, then SIGKILL after 2 s. Returns once the port is free.
    pub fn free_port(&self, port: u16, pid: u32) -> Result<(), String> {
        let Some(owner) = self.port_owner(port) else { return Ok(()) };
        if owner.pid != pid {
            (self.on_change)(); // the windows name the old owner, so have them look again
            return Err(format!("Port {port} is now in use by {} (pid {})", owner.command, owner.pid));
        }
        if let Some(session) = owner.session {
            if let Some((root, run)) = self.interrupt(session.id) {
                self.reap(session.id, run, root);
            }
            return Ok(());
        }
        kill_tree(pid, Signal::Term);
        if !wait_free(port, pid, Duration::from_secs(2)) {
            kill_tree(pid, Signal::Kill);
            wait_free(port, pid, Duration::from_secs(1));
        }
        Ok(())
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
        let mut cancelled = false;
        if let Some(s) = inner.sessions.get_mut(&id) {
            s.interrupted |= data.contains(&3);
            // Ctrl-C while a crashed script waits to restart keeps it down, like Stop.
            cancelled = data.contains(&3) && s.restart_in.take().is_some();
            if let Some(pty) = s.pty.as_mut() {
                let _ = pty.writer.write_all(data);
                let _ = pty.writer.flush();
            }
        }
        drop(inner);
        if cancelled {
            (self.on_change)();
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
            inner.quitting = true;
            inner.sessions.values_mut().for_each(|s| s.restart_in = None); // a restart now would outlive the app
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

/// Steps to wait before restarting a crashed keep-alive script: 1, 2, 4, 8, then 16.
/// A run of a minute or more starts over; None gives up, after five restarts that crashed quickly.
fn backoff(waited: Option<u32>, ran: Duration) -> Option<u32> {
    let wait = match waited {
        Some(wait) if ran < Duration::from_secs(60) => wait * 2,
        _ => 1,
    };
    (wait <= 16).then_some(wait)
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

/// One row of `ps -A -o pid=,ppid=,%cpu=,rss=`.
struct Proc {
    pid: u32,
    ppid: u32,
    cpu: f32,
    /// KiB, as ps gives it.
    rss: u64,
}

/// Every process, or None if ps can't run.
fn processes() -> Option<Vec<Proc>> {
    // The C locale, or a German one prints the CPU as 1,5.
    let out = Command::new("ps").args(["-A", "-o", "pid=,ppid=,%cpu=,rss="]).env("LC_ALL", "C").output().ok()?;
    Some(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

/// Only the pid and ppid must parse: kill_tree needs nothing else.
fn parse_ps(text: &str) -> Vec<Proc> {
    let row = |line: &str| {
        let mut parts = line.split_whitespace();
        Some(Proc {
            pid: parts.next()?.parse().ok()?,
            ppid: parts.next()?.parse().ok()?,
            cpu: parts.next().and_then(|p| p.parse().ok()).unwrap_or_default(),
            rss: parts.next().and_then(|p| p.parse().ok()).unwrap_or_default(),
        })
    };
    text.lines().filter_map(row).collect()
}

/// Every process as (pid, ppid); empty if ps can't run.
fn process_table() -> Vec<(u32, u32)> {
    processes().unwrap_or_default().iter().map(|p| (p.pid, p.ppid)).collect()
}

/// A process tree's usage, or None once its root has exited.
fn tree_usage(procs: &[Proc], root: u32, cores: usize) -> Option<Usage> {
    if !procs.iter().any(|p| p.pid == root) {
        return None;
    }
    let table: Vec<_> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    let tree = descendants(&table, root);
    let members = procs.iter().filter(|p| tree.contains(&p.pid));
    // ps gives percent of one core; dividing by the cores makes 100 the whole machine.
    let cpu = members.clone().map(|p| p.cpu).sum::<f32>() / cores as f32;
    Some(Usage { cpu, memory: members.map(|p| p.rss * 1024).sum() })
}

#[cfg(unix)]
fn kill_tree(root: u32, signal: Signal) {
    let signal = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    for pid in descendants(&process_table(), root) {
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

/// The process listening on a TCP port, as (pid, command). Unix only: lsof.
fn listener(port: u16) -> Option<(u32, String)> {
    let filter = format!("-iTCP:{port}");
    let out = Command::new("lsof").args(["-nP", &filter, "-sTCP:LISTEN", "-Fpc"]).output().ok()?;
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// `lsof -F` prints a field per line: `p` and the pid, then `c` and its command. The first process wins.
fn parse_lsof(out: &str) -> Option<(u32, String)> {
    let mut lines = out.lines();
    let pid = lines.next()?.strip_prefix('p')?.parse().ok()?;
    let command = lines.next()?.strip_prefix('c')?;
    Some((pid, command.to_string()))
}

/// Waits for `pid` to let go of `port`.
fn wait_free(port: u16, pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while listener(port).is_some_and(|(holder, _)| holder == pid) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
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
    // One pass, as it reads every chunk until an address turns up, which for most scripts is never.
    text.match_indices("://").find_map(|(at, _)| {
        let scheme = ["https", "http"].into_iter().find(|scheme| text[..at].ends_with(scheme))?;
        let rest = &text[at + 3..];
        let hosts = ["localhost:", "127.0.0.1:", "0.0.0.0:", "[::1]:"];
        let rest = &rest[hosts.into_iter().find(|host| rest.starts_with(host))?.len()..];
        let port: String = rest.chars().take_while(char::is_ascii_digit).collect();
        port.parse::<u16>().ok()?;
        let path: String = rest[port.len()..]
            .chars()
            .take_while(|c| c.is_ascii_graphic() && !"\"'<>`)".contains(*c))
            .collect();
        let path = path.trim_end_matches(['.', ',', ';', ':']);
        Some(format!("{scheme}://localhost:{port}{path}"))
    })
}

/// The port a script couldn't listen on. Servers that move to the next port say "is in use"
/// (Vite, Next.js), so only "already" counts.
/// shortcut: scans one read at a time, like find_url.
fn find_busy_port(chunk: &[u8]) -> Option<u16> {
    let text = String::from_utf8_lossy(&strip_ansi(chunk)).to_lowercase();
    let mut lines = text.lines().filter(|line| line.contains("already in use") || line.contains("already running on port"));
    lines.find_map(|line| {
        // `port 3000` (Vite, Create React App, Puma), else the end of Node's `address already in use :::3000`
        let rest = match line.find("port ") {
            Some(at) => &line[at + 5..],
            None => line.split("in use ").nth(1)?.split_whitespace().next()?.rsplit(':').next()?,
        };
        rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
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
            keep_alive: false,
            worktree: None,
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
        Sessions::new(Box::new(|| {}), Box::new(|_, _| {}))
    }

    #[test]
    fn reports_a_crash_but_not_a_clean_exit_or_a_stop_the_user_asked_for() {
        let failures = Arc::new(Mutex::new(Vec::new()));
        let seen = failures.clone();
        let report = move |s: &SessionView, failed: bool| {
            if failed {
                seen.lock().unwrap().push((s.label.clone(), s.code));
            }
        };
        let sessions = Sessions::new(Box::new(|| {}), Box::new(report));

        let clean = sessions.start(spec("exit 0")).unwrap();
        wait_for("clean exit", || view(&sessions, clean).status == Status::Exited);

        let stopped = sessions.start(spec("sleep 30")).unwrap();
        sessions.stop(stopped);
        wait_for("stop button", || view(&sessions, stopped).status == Status::Stopped);

        let typed = sessions.start(spec("echo ready; sleep 30")).unwrap();
        wait_for("ready", || output(&sessions, typed).contains("ready"));
        // The shell can drop a Ctrl-C that lands before `sleep` has started, so type it until one lands, as a person would.
        wait_for("typed Ctrl-C", || {
            sessions.send_input(typed, b"\x03");
            view(&sessions, typed).status == Status::Stopped
        });

        let quit = sessions.start(spec("echo ready; sleep 60")).unwrap();
        wait_for("ready", || output(&sessions, quit).contains("ready"));

        assert!(failures.lock().unwrap().is_empty());
        sessions.start(Spec { label: "boom".into(), ..spec("exit 7") }).unwrap();
        wait_for("failure", || !failures.lock().unwrap().is_empty());
        sessions.shutdown(); // last, since nothing starts once quitting
        wait_for("quit", || view(&sessions, quit).status == Status::Stopped);
        assert_eq!(*failures.lock().unwrap(), [("boom".to_string(), Some(7))]);
    }

    #[test]
    fn reports_a_clean_exit_with_its_start_and_end_but_not_a_stop() {
        let exits = Arc::new(Mutex::new(Vec::new()));
        let seen = exits.clone();
        let report = move |s: &SessionView, failed: bool| seen.lock().unwrap().push((s.clone(), failed));
        let sessions = Sessions::new(Box::new(|| {}), Box::new(report));

        let stopped = sessions.start(spec("sleep 30")).unwrap();
        sessions.stop(stopped);
        wait_for("stop", || view(&sessions, stopped).status == Status::Stopped);

        let id = sessions.start(spec("sleep 1")).unwrap();
        assert_eq!(view(&sessions, id).ended_at, None);
        wait_for("exit", || !exits.lock().unwrap().is_empty());
        let exits = exits.lock().unwrap();
        let (session, failed) = &exits[0];
        assert_eq!((exits.len(), session.id, *failed), (1, id, false));
        assert!(session.ended_at.unwrap() >= session.started_at + 1000);
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
    fn the_same_script_only_runs_once_per_folder() {
        let sessions = new_sessions();
        let s = spec("sleep 30");
        let first = sessions.start(s.clone()).unwrap();
        assert_eq!(sessions.start(s.clone()).unwrap(), first);
        assert_eq!(sessions.list().len(), 1);
        // The same script in another worktree is a run of its own.
        let elsewhere = std::env::temp_dir().join(format!("scriptdeck-worktree-{}", std::process::id()));
        std::fs::create_dir_all(&elsewhere).unwrap();
        let worktree = Spec { cwd: elsewhere.to_string_lossy().into(), worktree: Some("feature".into()), ..s };
        let second = sessions.start(worktree).unwrap();
        assert_ne!(second, first);
        assert_eq!(view(&sessions, second).worktree.as_deref(), Some("feature"));
        for id in [first, second] {
            sessions.stop(id);
            wait_for("stop", || view(&sessions, id).status == Status::Stopped);
        }
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
        let started = view(&sessions, id).started_at;
        sessions.restart(id).unwrap();
        wait_for("second run", || {
            view(&sessions, id).status == Status::Running
                && output(&sessions, id).contains("run=")
                && printed_pid(&sessions, id, "run=") != first
        });
        assert_eq!(output(&sessions, id).matches("run=").count(), 1);
        let second = view(&sessions, id);
        assert!(second.started_at > started && second.ended_at.is_none());
        sessions.stop(id);
        wait_for("stop", || view(&sessions, id).status == Status::Stopped);
    }

    /// Sessions that run `then` on each failure notice; a keep-alive script's first comes while its restart waits.
    fn with_notice(then: impl Fn(&Arc<Sessions>, &SessionView) + Send + Sync + 'static) -> Arc<Sessions> {
        let slot = Arc::new(std::sync::OnceLock::<Arc<Sessions>>::new());
        let handle = slot.clone();
        let notice = move |s: &SessionView, failed: bool| {
            if failed {
                then(handle.get().unwrap(), s);
            }
        };
        let sessions = Sessions::new(Box::new(|| {}), Box::new(notice));
        let _ = slot.set(sessions.clone());
        sessions
    }

    #[test]
    fn a_keep_alive_crash_restarts_in_the_same_session_until_it_gives_up() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let seen = notices.clone();
        let sessions = with_notice(move |sessions, s| {
            seen.lock().unwrap().push(s.restart_in);
            if s.restart_in.is_some() {
                wait_for("first run's output", || output(sessions, s.id).contains("boom"));
                // Skip to the last wait, so the next quick crash gives up without four more login shells.
                sessions.inner.lock().unwrap().sessions.get_mut(&s.id).unwrap().restart_in = Some(16);
            }
        });
        let id = sessions.start(Spec { keep_alive: true, ..spec("echo boom; exit 1") }).unwrap();
        // One wait per run: a login shell can take 15 s on a busy machine.
        wait_for("first crash", || !notices.lock().unwrap().is_empty());
        wait_for("giving up", || notices.lock().unwrap().len() == 2);
        assert_eq!(*notices.lock().unwrap(), [Some(1), None]);
        let run = sessions.inner.lock().unwrap().sessions[&id].run;
        assert_eq!((run, sessions.list().len()), (1, 1));
        wait_for("both runs' output", || output(&sessions, id).matches("boom").count() == 2);
    }

    #[test]
    fn stop_ctrl_c_dismiss_or_quit_during_the_wait_cancels_the_restart() {
        let cancels: [fn(&Arc<Sessions>, u32); 4] =
            [|s, id| s.stop(id), |s, id| s.send_input(id, b"\x03"), |s, id| s.clear(id), |s, _| s.shutdown()];
        let all: Vec<_> = cancels
            .into_iter()
            .map(|cancel| {
                let sessions = with_notice(move |sessions, s| cancel(sessions, s.id));
                let id = sessions.start(Spec { keep_alive: true, ..spec("exit 1") }).unwrap();
                (sessions, id)
            })
            .collect();
        for (sessions, _) in &all {
            wait_for("cancel", || sessions.list().iter().all(|s| s.status == Status::Exited && s.restart_in.is_none()));
        }
        thread::sleep(BACKOFF_STEP * 20); // long past the first wait
        for (sessions, id) in &all {
            assert!(sessions.inner.lock().unwrap().sessions.iter().all(|(other, s)| other == id && s.run == 0));
        }
    }

    #[test]
    fn backs_off_up_to_16_steps_and_starts_over_after_a_long_run() {
        let quick = Duration::from_secs(5);
        let waits: Vec<_> = std::iter::successors(backoff(None, quick), |&wait| backoff(Some(wait), quick)).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16]);
        assert_eq!(backoff(Some(16), Duration::from_secs(60)), Some(1));
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
        let dir = std::env::temp_dir().join(format!("scriptdeck-bin-{}", std::process::id()));
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
    fn trimmed_scrollback_starts_at_a_line() {
        let sessions = new_sessions();
        let id = sessions.start(spec("yes 0123456789 | head -c 3000000; echo tail-marker")).unwrap();
        wait_for("exit", || view(&sessions, id).status == Status::Exited);
        wait_for("tail", || output(&sessions, id).contains("tail-marker"));
        assert!(output(&sessions, id).starts_with("0123456789"));
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
    fn quitting_during_a_restart_starts_no_new_run() {
        let sessions = new_sessions();
        let id = sessions.start(spec("trap '' INT; echo child=$$; while true; do sleep 1; done")).unwrap();
        let shell = printed_pid(&sessions, id, "child=");
        sessions.restart(id).unwrap(); // Ctrl-C is ignored, so the restart waits on its reap
        sessions.shutdown();
        wait_for("shell gone", || !pid_alive(shell));
        thread::sleep(Duration::from_secs(1)); // the reap polls every 50 ms, then would start the new run
        assert_eq!(view(&sessions, id).status, Status::Stopped);
    }

    #[test]
    fn finds_dev_server_urls() {
        let url = |s: &str| find_url(s.as_bytes());
        assert_eq!(url("ready on http://localhost:3000"), Some("http://localhost:3000".into()));
        assert_eq!(url("at https://127.0.0.1:8443/app/.\n"), Some("https://localhost:8443/app/".into()));
        assert_eq!(url("Local: http://0.0.0.0:5173/"), Some("http://localhost:5173/".into()));
        assert_eq!(url("nothing here, http://example.com:80"), None);
        assert_eq!(url("http://localhost:99999"), None);
        assert_eq!(url("http://localhost:99999 failed, http://localhost:3001 then"), Some("http://localhost:3001".into()));
    }

    #[test]
    fn finds_urls_split_by_colour_codes() {
        let vite = "Local:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r\n";
        assert_eq!(find_url(vite.as_bytes()), Some("http://localhost:5173/".into()));
    }

    #[test]
    fn finds_the_port_a_script_could_not_take() {
        let port = |s: &str| find_busy_port(s.as_bytes());
        assert_eq!(port("Error: listen EADDRINUSE: address already in use :::3000"), Some(3000));
        assert_eq!(port("Port 5173 is already in use"), Some(5173));
        assert_eq!(port("\x1b[33mSomething is already running on port 3000.\x1b[39m"), Some(3000));
        assert_eq!(port("Address already in use - bind(2) for \"127.0.0.1\" port 3000"), Some(3000));
        // These move to the next free port on their own.
        assert_eq!(port("Port 5173 is in use, trying another one..."), None);
        assert_eq!(port("Port 3000 is in use by process 123, using available port 3001 instead."), None);
    }

    #[test]
    fn notes_the_address_a_script_serves_and_the_port_it_could_not_take() {
        let sessions = new_sessions();
        let id = sessions.start(spec("echo 'Port 5173 is already in use'; echo 'Local: http://localhost:3000/'; exit 1")).unwrap();
        wait_for("busy port and address", || {
            let s = view(&sessions, id);
            s.busy_port == Some(5173) && s.url.as_deref() == Some("http://localhost:3000/")
        });
    }

    #[test]
    fn walks_the_whole_process_tree() {
        let table = [(2, 1), (3, 2), (4, 3), (9, 8)];
        assert_eq!(descendants(&table, 1), [1, 2, 3, 4]);
    }

    #[test]
    fn reads_ps_output() {
        let text = "    1     0   0.4  23216\n  312     1  12.5   4096\nnot a process\n  313   312   1,5      -\n";
        let rows: Vec<_> = parse_ps(text).iter().map(|p| (p.pid, p.ppid, p.cpu, p.rss)).collect();
        assert_eq!(rows, [(1, 0, 0.4, 23216), (312, 1, 12.5, 4096), (313, 312, 0.0, 0)]);
    }

    #[test]
    fn sums_usage_over_the_whole_process_tree() {
        let procs = parse_ps("1 0 9.0 9000\n10 1 0.5 1024\n11 10 40.0 2048\n12 11 1.5 1024\n20 1 50.0 4096\n");
        // 42% of one core is 21% of a two-core machine.
        assert_eq!(tree_usage(&procs, 10, 2), Some(Usage { cpu: 21.0, memory: 4096 * 1024 }));
        assert_eq!(tree_usage(&procs, 99, 2), None);
    }

    #[test]
    fn reports_the_memory_a_running_script_holds() {
        let sessions = new_sessions();
        let id = sessions.start(spec("perl -e '$x = q(a) x 1e8; print qq(ready\\n); sleep 60'")).unwrap();
        wait_for("ready", || output(&sessions, id).contains("ready"));
        let memory = sessions.usage()[&id].memory;
        assert!(memory > 50_000_000, "{memory}");
        sessions.stop(id);
        wait_for("stop", || view(&sessions, id).status == Status::Stopped);
        assert!(!sessions.usage().contains_key(&id));
    }

    #[test]
    fn reads_the_listener_from_lsof() {
        assert_eq!(parse_lsof("p39855\ncnode\nf12\n"), Some((39855, "node".into())));
        assert_eq!(parse_lsof("p100\ncruby\nf9\np101\ncruby\nf9\n"), Some((100, "ruby".into())));
        assert_eq!(parse_lsof(""), None);
    }

    /// Listens on a port the OS picks, since other tests run at the same time, and gives up after a minute.
    const LISTENER: &str = r#"node -e 'require("net").createServer().listen(0, "127.0.0.1", function () { console.log("port=" + this.address().port) }); setTimeout(process.exit, 60000)'"#;

    /// Runs a listener outside ScriptDeck, and returns it with its port.
    fn listen_outside(listener: &str) -> (std::process::Child, u16) {
        let shell = Command::new("sh").args(["-c", &format!("exec {listener}")]).stdout(Stdio::piped()).spawn();
        let mut child = shell.unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        (child, line.trim().trim_start_matches("port=").parse().unwrap())
    }

    #[test]
    fn frees_a_port_held_outside_scriptdeck() {
        let (mut child, port) = listen_outside(LISTENER);
        let sessions = new_sessions();
        let owner = sessions.port_owner(port).unwrap();
        assert_eq!((owner.pid, owner.command.as_str()), (child.id(), "node"));
        assert!(owner.session.is_none());
        assert!(sessions.free_port(port, owner.pid + 1).is_err(), "only the pid the user saw");
        sessions.free_port(port, owner.pid).unwrap();
        assert!(sessions.port_owner(port).is_none());
        child.wait().unwrap();
    }

    #[test]
    fn a_holder_that_ignores_sigterm_gets_sigkill_after_2_s() {
        use std::os::unix::process::ExitStatusExt;
        let stubborn = LISTENER.replace("setTimeout", r#"process.on("SIGTERM", () => {}); setTimeout"#);
        let (mut child, port) = listen_outside(&stubborn);
        let started = Instant::now();
        new_sessions().free_port(port, child.id()).unwrap();
        assert!(started.elapsed() >= Duration::from_secs(2), "SIGTERM gets its 2 s first");
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn a_port_held_by_a_script_is_freed_with_a_normal_stop() {
        let sessions = new_sessions();
        let id = sessions.start(spec(LISTENER)).unwrap();
        let port = printed_pid(&sessions, id, "port=") as u16;
        let owner = sessions.port_owner(port).unwrap();
        assert_eq!(owner.session.map(|s| s.id), Some(id));
        sessions.free_port(port, owner.pid).unwrap();
        assert_eq!(view(&sessions, id).status, Status::Stopped);
    }
}
