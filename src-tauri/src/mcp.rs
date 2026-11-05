//! Lets AI agents read and restart scripts over MCP, while the switch in Settings > General is on.
//! An agent launches `ScriptDec --mcp`, which pipes its stdio to a Unix socket the running app opens
//! in its config folder. The socket's 0600 permissions are the access control: no port is opened.

use crate::{
    config::Project,
    project::{self, Script},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::{AppHandle, Manager};

/// The revisions with an `initialize` handshake, newest first. Claude Code speaks 2025-11-25.
/// shortcut: no 2026-07-28, which swaps `initialize` for `server/discover` and per-request `_meta`;
/// clients that speak both fall back to `initialize` here. Add it when a client speaks only that.
const VERSIONS: [&str; 2] = ["2025-11-25", "2025-06-18"];
/// get_output's default, and the most matches search_output returns.
const LINES: usize = 100;

/// Runs a tool by name: text for the agent, or an error it can correct itself from.
type Tools = dyn Fn(&str, &Value) -> Result<String, String> + Send + Sync;

/// The socket, while the switch is on.
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// Opens or closes the socket to match the switch.
pub fn enable(app: &AppHandle, on: bool) -> Result<(), String> {
    let mut listener = LISTENER.lock().unwrap();
    if !on {
        *listener = None; // dropping it closes the socket
    } else if listener.is_none() {
        let path = socket_path(&app.config().identifier).ok_or("Can't find your home folder")?;
        let app = app.clone();
        *listener = Some(listen(&path, Arc::new(move |name: &str, args: &Value| call_tool(&app, name, args)))?);
    }
    Ok(())
}

/// `ScriptDec --mcp`, which an agent launches: a pipe to the running app. Returns the exit code.
pub fn bridge(identifier: &str) -> i32 {
    let Some(socket) = socket_path(identifier).and_then(|path| UnixStream::connect(path).ok()) else {
        eprintln!(
            "ScriptDec isn't letting AI agents in. Open ScriptDec and turn on \
             \"Let AI agents read and restart scripts (MCP)\" in Settings > General."
        );
        return 1;
    };
    pipe(socket, io::stdin(), &mut io::stdout()).map_or(1, |()| 0)
}

/// In the app's config folder, which Tauri derives from the bundle identifier: the bridge has no app
/// to ask. The short name keeps the path under macOS's 104-byte limit.
fn socket_path(identifier: &str) -> Option<PathBuf> {
    // shortcut: ignores XDG_CONFIG_HOME, harmless while both ends agree; read it if Linux ever ships.
    let config = if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" };
    Some(std::env::home_dir()?.join(config).join(identifier).join("mcp.sock"))
}

/// Carries the agent's messages to the app and its replies back, until either end hangs up.
fn pipe(socket: UnixStream, mut agent: impl Read + Send + 'static, out: &mut impl Write) -> io::Result<()> {
    let mut to_app = socket.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut agent, &mut to_app);
        let _ = to_app.shutdown(Shutdown::Write); // the agent is done: the app answers the rest, then hangs up
    });
    io::copy(&mut &socket, out)?;
    out.flush()
}

/// The open socket, answering each connection on its own thread. Dropping it removes the socket, and
/// connections already open get no more answers.
struct Listener {
    path: PathBuf,
    stopped: Arc<AtomicBool>,
}

fn listen(path: &Path, tools: Arc<Tools>) -> Result<Listener, String> {
    let fail = |e: io::Error| format!("Can't open {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    let _ = fs::remove_file(path); // left behind if the app crashed
    let listener = UnixListener::bind(path).map_err(fail)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(fail)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(stream) = stream {
                let (tools, flag) = (tools.clone(), flag.clone());
                thread::spawn(move || serve(stream, &*tools, &flag));
            }
        }
    });
    Ok(Listener { path: path.to_path_buf(), stopped })
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path); // wakes the accept loop, so it sees the flag
        let _ = fs::remove_file(&self.path);
    }
}

/// MCP's stdio framing: one JSON-RPC message per line, both ways.
fn serve(stream: UnixStream, tools: &Tools, stopped: &AtomicBool) {
    let Ok(mut out) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        // Checked per message, so turning the switch off also cuts off agents already connected.
        if stopped.load(Ordering::SeqCst) {
            break;
        }
        if let Some(reply) = handle(&line, tools) {
            if out.write_all(format!("{reply}\n").as_bytes()).is_err() {
                break;
            }
        }
    }
}

/// Answers one JSON-RPC message. Notifications get no reply.
fn handle(line: &str, tools: &Tools) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }));
    };
    let id = message.get("id")?;
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            Ok(json!({
                "protocolVersion": VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "scriptdec", "version": env!("CARGO_PKG_VERSION") }
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            if tool_list().iter().any(|tool| tool["name"] == name) {
                let (text, failed) = match tools(name, &params["arguments"]) {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
            } else {
                Err((-32602, format!("Unknown tool: {name}")))
            }
        }
        method => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required }
    })
}

/// Reading output, and the Run, Restart and Stop buttons. Nothing types into a terminal.
fn tool_list() -> Vec<Value> {
    let session = json!({ "type": "integer", "description": "A session id from list_scripts" });
    vec![
        tool(
            "list_scripts",
            "Lists ScriptDec's projects with their scripts, and every session (a script run) with its status, \
             dev server URL and exit code.",
            json!({}),
            &[],
        ),
        tool(
            "get_output",
            "The last lines a session printed, without colours.",
            json!({
                "session": session,
                "lines": { "type": "integer", "description": "How many lines, from the end. Default 100." }
            }),
            &["session"],
        ),
        tool(
            "search_output",
            "The lines in a session's output that contain some text, ignoring case.",
            json!({ "session": session, "query": { "type": "string", "description": "The text to look for" } }),
            &["session", "query"],
        ),
        tool(
            "start_script",
            "Runs a project's script in the background, as ScriptDec's Run button does, and returns its session. \
             A script that is already running keeps its session.",
            json!({
                "project": { "type": "string", "description": "A project id from list_scripts" },
                "script": { "type": "string", "description": "A script key from list_scripts" }
            }),
            &["project", "script"],
        ),
        tool(
            "restart_script",
            "Restarts a session's script, as its Restart button does: stops it if it's running, then runs it again.",
            json!({ "session": session }),
            &["session"],
        ),
        tool(
            "stop_script",
            "Stops a session's script, as its Stop button does: Ctrl-C, then SIGTERM, then SIGKILL.",
            json!({ "session": session }),
            &["session"],
        ),
    ]
}

/// The tools, over the same state and Sessions methods as the windows.
fn call_tool(app: &AppHandle, name: &str, args: &Value) -> Result<String, String> {
    let state = app.state::<crate::App>();
    let sessions = &state.sessions;
    let session = || {
        let id = args["session"].as_u64().and_then(|id| u32::try_from(id).ok());
        id.ok_or_else(|| "Pass a session id from list_scripts".to_string())
    };
    let output = |id: u32| sessions.output(id).ok_or_else(|| format!("No session {id}"));
    // What an action left the session as, the way list_scripts shows it.
    let view = |id: u32| {
        let found = sessions.list().into_iter().find(|s| s.id == id);
        found.map(|s| json!(s).to_string()).ok_or_else(|| format!("No session {id}"))
    };
    match name {
        "list_scripts" => {
            let projects = state.config.lock().unwrap().projects.clone();
            let projects: Vec<Value> = projects
                .iter()
                .map(|p| {
                    let scan = project::scan(p);
                    let custom = p.custom.iter().map(|c| {
                        json!({ "key": format!("custom:{}", c.id), "name": c.name, "command": c.command })
                    });
                    let scripts: Vec<Value> = custom.chain(scan.scripts.iter().map(|s| json!(s))).collect();
                    json!({ "id": p.id, "name": p.name, "path": p.path, "scripts": scripts, "error": scan.error })
                })
                .collect();
            Ok(json!({ "projects": projects, "sessions": sessions.list() }).to_string())
        }
        "get_output" => {
            let lines = args["lines"].as_u64().map_or(LINES, |n| n as usize);
            Ok(tail(&output(session()?)?, lines))
        }
        "search_output" => {
            let query = args["query"].as_str().filter(|q| !q.is_empty()).ok_or("Pass the text to look for")?;
            Ok(search(&output(session()?)?, query))
        }
        "start_script" => {
            let project = state.project(args["project"].as_str().unwrap_or_default())?;
            let key = args["script"].as_str().unwrap_or_default();
            let (label, command) = find_script(&project, project::scan(&project).scripts, key)
                .ok_or_else(|| format!("{} has no script {key}; list_scripts has their keys", project.name))?;
            view(crate::run(app.state(), project.id, label, command)?)
        }
        "restart_script" => {
            let id = session()?;
            sessions.restart(id)?;
            view(id)
        }
        "stop_script" => {
            let id = session()?;
            sessions.stop(id);
            view(id)
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

/// The label and command the menu bar runs a script with. Only listed scripts: an agent can't send
/// a command of its own.
fn find_script(project: &Project, scripts: Vec<Script>, key: &str) -> Option<(String, String)> {
    if let Some(custom) = project.custom.iter().find(|c| format!("custom:{}", c.id) == key) {
        return Some((custom.name.clone(), custom.command.clone()));
    }
    let script = scripts.into_iter().find(|s| s.key == key)?;
    let label = match &script.package {
        // scriptLabel in ui/shared.js, so the session reads the same as one started from the menu bar
        Some(package) => format!("{} {}", package.rsplit('/').next().unwrap_or(package), script.name),
        None => script.name,
    };
    Some((label, script.command))
}

/// The output's lines as the terminal shows them: a carriage return without a newline redraws its
/// line, as progress bars do, so only the last drawing is kept.
fn screen_lines(text: &str) -> Vec<&str> {
    text.lines().map(|line| line.rsplit('\r').find(|part| !part.is_empty()).unwrap_or_default()).collect()
}

fn tail(text: &str, count: usize) -> String {
    let lines = screen_lines(text);
    lines[lines.len().saturating_sub(count)..].join("\n")
}

fn search(text: &str, query: &str) -> String {
    let query = query.to_lowercase();
    let found: Vec<&str> = screen_lines(text).into_iter().filter(|line| line.to_lowercase().contains(&query)).collect();
    match found.len() {
        0 => "No matching lines".to_string(),
        n if n > LINES => format!("{n} matching lines, the last {LINES}:\n{}", found[n - LINES..].join("\n")),
        _ => found.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomScript;
    use std::io::Cursor;

    /// Stands in for the app's tools.
    fn echo(name: &str, args: &Value) -> Result<String, String> {
        if args["fail"] == true {
            Err(format!("{name} failed"))
        } else {
            Ok(format!("{name} {args}"))
        }
    }

    fn ask(message: Value) -> Value {
        handle(&message.to_string(), &echo).unwrap()
    }

    fn initialize(version: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": "test", "version": "1" }
        } })
    }

    fn call(name: &str, arguments: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
    }

    #[test]
    fn negotiates_a_protocol_version_and_offers_tools() {
        let reply = ask(initialize("2025-06-18"));
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(reply["result"]["capabilities"], json!({ "tools": {} }));
        assert_eq!(ask(initialize("2099-01-01"))["result"]["protocolVersion"], VERSIONS[0]);
    }

    #[test]
    fn lists_the_six_tools_and_calls_only_those() {
        let list = ask(json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/list" }));
        let names: Vec<_> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].clone()).collect();
        let expected = ["list_scripts", "get_output", "search_output", "start_script", "restart_script", "stop_script"];
        assert_eq!(names, expected);
        assert_eq!(list["id"], "a");

        let ok = ask(call("get_output", json!({ "session": 3 })));
        assert_eq!(ok["result"], json!({ "content": [{ "type": "text", "text": "get_output {\"session\":3}" }], "isError": false }));
        let failed = ask(call("stop_script", json!({ "fail": true })));
        assert_eq!(failed["result"]["isError"], true);
        assert_eq!(failed["result"]["content"][0]["text"], "stop_script failed");
        assert_eq!(ask(call("send_input", json!({})))["error"]["code"], -32602);
    }

    #[test]
    fn answers_pings_and_mistakes_but_not_notifications() {
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, &echo).is_none());
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }))["result"], json!({}));
        assert_eq!(handle("{ nope", &echo).unwrap()["error"]["code"], -32700);
        // Clients that also speak 2026-07-28 probe with this, and fall back to initialize on an error.
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "id": 8, "method": "server/discover" }))["error"]["code"], -32601);
    }

    #[test]
    fn reads_the_last_lines_as_the_terminal_shows_them() {
        let text = "one\r\ntwo\r\n10%\r50%\r100%\r\nlast";
        assert_eq!(tail(text, 2), "100%\nlast");
        assert_eq!(tail(text, 10), "one\ntwo\n100%\nlast");
        assert_eq!(tail("", 5), "");
    }

    #[test]
    fn searches_ignoring_case_and_keeps_the_newest_matches() {
        assert_eq!(search("ok\r\nError: boom\r\nfine\r\nerror again\r\n", "ERROR"), "Error: boom\nerror again");
        assert_eq!(search("ok", "error"), "No matching lines");
        let many = (0..150).map(|i| format!("warn {i}")).collect::<Vec<_>>().join("\n");
        let found = search(&many, "warn");
        assert!(found.starts_with("150 matching lines, the last 100:\nwarn 50\n"));
        assert!(found.ends_with("warn 149"));
    }

    #[test]
    fn starts_only_listed_scripts_with_the_menu_bar_labels() {
        let custom = CustomScript { id: "c1".into(), name: "Deploy".into(), command: "make deploy".into() };
        let project = Project { custom: vec![custom], ..Default::default() };
        let script = |key: &str, name: &str, package: Option<&str>, command: &str| Script {
            key: key.into(),
            name: name.into(),
            package: package.map(String::from),
            command: command.into(),
            body: String::new(),
        };
        let scripts = vec![
            script(":dev", "dev", None, "pnpm run dev"),
            script("@acme/app:start", "start", Some("@acme/app"), "pnpm -F @acme/app run start"),
        ];
        let find = |key| find_script(&project, scripts.clone(), key);
        assert_eq!(find(":dev"), Some(("dev".into(), "pnpm run dev".into())));
        assert_eq!(find("@acme/app:start"), Some(("app start".into(), "pnpm -F @acme/app run start".into())));
        assert_eq!(find("custom:c1"), Some(("Deploy".into(), "make deploy".into())));
        assert_eq!(find("rm -rf ~"), None);
    }

    #[test]
    fn the_socket_sits_in_the_config_folder_within_the_path_limit() {
        let path = socket_path("com.scriptdec.desktop").unwrap();
        #[cfg(target_os = "macos")]
        assert!(path.starts_with(std::env::home_dir().unwrap().join("Library/Application Support")));
        assert!(path.ends_with("com.scriptdec.desktop/mcp.sock"));
        assert!(path.as_os_str().len() < 104, "{} is too long for a socket", path.display());
    }

    #[test]
    fn the_bridge_carries_an_agent_to_the_app_and_back_until_the_switch_goes_off() {
        let dir = std::env::temp_dir().join(format!("scriptdec-mcp-{}", std::process::id()));
        let path = dir.join("mcp.sock");
        let listener = listen(&path, Arc::new(echo)).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let agent = [initialize("2025-11-25"), notification, call("list_scripts", json!({}))]
            .map(|message| format!("{message}\n"))
            .concat();
        let mut out = vec![];
        pipe(UnixStream::connect(&path).unwrap(), Cursor::new(agent), &mut out).unwrap();
        let replies: Vec<Value> =
            String::from_utf8(out).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(replies[1]["result"]["content"][0]["text"], "list_scripts {}");

        // An agent that's already connected gets no more answers once the switch is off.
        let open = UnixStream::connect(&path).unwrap();
        let mut replies = BufReader::new(open.try_clone().unwrap());
        let ping = format!("{}\n", json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }));
        (&open).write_all(ping.as_bytes()).unwrap();
        let mut reply = String::new();
        replies.read_line(&mut reply).unwrap();
        assert!(reply.contains("\"id\":9"));
        drop(listener);
        (&open).write_all(ping.as_bytes()).unwrap();
        assert_eq!(replies.read_line(&mut String::new()).unwrap(), 0);
        assert!(!path.exists());
        assert!(UnixStream::connect(&path).is_err());
    }
}
