//! Live-mode bridge between `ics-mcp` and a running IC Studio window.
//!
//! The app listens on 127.0.0.1 (ephemeral port) and advertises `{port, token, pid}` in
//! `live.json` in a per-user directory (`ICS_LIVE_DIR` overrides it). The protocol is
//! newline-delimited JSON: `{"token", "tool", "arguments"}` is answered with
//! `{"text", "is_error"}`; `{"token", "ping": true}` is answered by the listener itself with
//! `{"pong": true, "pid"}`. A request with a wrong token gets an error and the connection closes.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

use crate::ToolResult;

const LIVE_FILE: &str = "live.json";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// Simulations and DRC on large layouts can take a while.
const CALL_TIMEOUT: Duration = Duration::from_secs(600);

/// Directory holding `live.json`.
pub fn live_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = var("ICS_LIVE_DIR") {
        return Some(d);
    }
    if cfg!(windows) {
        return var("LOCALAPPDATA").map(|d| d.join("ic-studio"));
    }
    var("XDG_RUNTIME_DIR").map(|d| d.join("ic-studio")).or_else(|| var("HOME").map(|h| h.join(".local/state/ic-studio")))
}

fn live_file() -> io::Result<PathBuf> {
    live_dir().map(|d| d.join(LIVE_FILE)).ok_or_else(|| io::Error::other("no per-user directory for live.json"))
}

fn random_token() -> String {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    (0..2u8)
        .map(|i| {
            let mut h = RandomState::new().build_hasher();
            let addr = &h as *const _ as usize;
            h.write_u128(nanos);
            h.write_u32(std::process::id());
            h.write_usize(addr);
            h.write_u8(i);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// A tool call waiting to be run on the app's UI thread.
pub struct Pending {
    pub tool: String,
    pub arguments: Map<String, Value>,
    pub reply: Sender<ToolResult>,
}

/// The app side: accepts connections and hands each tool call to the app as a [`Pending`].
pub struct Server {
    port: u16,
    token: String,
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Listen and write `live.json`. `wake` runs whenever a request arrives (e.g. to repaint the UI).
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> io::Result<(Server, Receiver<Pending>)> {
        let path = live_file()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let token = random_token();
        std::fs::write(&path, json!({"port": port, "token": token, "pid": std::process::id()}).to_string())?;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let (tok, halt) = (token.clone(), stop.clone());
        std::thread::Builder::new().name("ics-bridge".into()).spawn(move || {
            for conn in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                let (tx, tok, wake) = (tx.clone(), tok.clone(), wake.clone());
                let _ =
                    std::thread::Builder::new().name("ics-bridge-conn".into()).spawn(move || serve(stream, &tok, &tx, &*wake));
            }
        })?;
        Ok((Server { port, token, path, stop }, rx))
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], self.port)), CONNECT_TIMEOUT);
        let ours = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v["token"] == self.token.as_str() && v["pid"] == std::process::id());
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn serve(stream: TcpStream, token: &str, tx: &Sender<Pending>, wake: &dyn Fn()) {
    let Ok(mut out) = stream.try_clone() else { return };
    let _ = stream.set_nodelay(true);
    let mut respond = |v: Value| writeln!(out, "{v}").and_then(|_| out.flush()).is_ok();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                respond(ToolResult::error(format!("bridge: invalid request: {e}")).to_json());
                break;
            }
        };
        if req["token"] != token {
            respond(ToolResult::error("bridge: invalid token").to_json());
            break;
        }
        if req["ping"] == true {
            if !respond(json!({"pong": true, "pid": std::process::id()})) {
                break;
            }
            continue;
        }
        let Some(tool) = req["tool"].as_str() else {
            respond(ToolResult::error("bridge: missing tool name").to_json());
            break;
        };
        let arguments = req["arguments"].as_object().cloned().unwrap_or_default();
        let (reply, answer) = mpsc::channel();
        let res = match tx.send(Pending { tool: tool.to_string(), arguments, reply }) {
            Ok(()) => {
                wake();
                answer.recv().unwrap_or_else(|_| ToolResult::error("IC Studio dropped the request"))
            }
            Err(_) => ToolResult::error("IC Studio is shutting down"),
        };
        if !respond(res.to_json()) {
            break;
        }
    }
}

/// The `ics-mcp` side: a connection to a running IC Studio window.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    token: String,
    pid: u32,
}

impl Client {
    /// Connect to the app advertised in `live.json`. A file nobody listens behind is removed.
    pub fn discover() -> Option<Client> {
        let path = live_file().ok()?;
        let text = std::fs::read_to_string(&path).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let port = u16::try_from(v["port"].as_u64()?).ok()?;
        let token = v["token"].as_str()?.to_string();
        let stream = match TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), CONNECT_TIMEOUT) {
            Ok(s) => s,
            Err(_) => {
                if std::fs::read_to_string(&path).is_ok_and(|now| now == text) {
                    let _ = std::fs::remove_file(&path);
                }
                return None;
            }
        };
        stream.set_nodelay(true).ok()?;
        stream.set_read_timeout(Some(PING_TIMEOUT)).ok()?;
        let mut c = Client { writer: stream.try_clone().ok()?, reader: BufReader::new(stream), token, pid: 0 };
        let pong = c.exchange(json!({"token": c.token, "ping": true})).ok()?;
        if pong["pong"] != true {
            return None;
        }
        c.pid = u32::try_from(pong["pid"].as_u64()?).ok()?;
        Some(c)
    }

    /// Process id of the connected app.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn call(&mut self, tool: &str, arguments: &Map<String, Value>) -> io::Result<ToolResult> {
        self.reader.get_ref().set_read_timeout(Some(CALL_TIMEOUT))?;
        let v = self.exchange(json!({"token": self.token, "tool": tool, "arguments": arguments}))?;
        ToolResult::from_json(&v).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("bad bridge response: {v}")))
    }

    fn exchange(&mut self, req: Value) -> io::Result<Value> {
        writeln!(self.writer, "{req}")?;
        self.writer.flush()?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "IC Studio closed the connection"));
        }
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::State;
    use std::sync::Mutex;

    /// Tests share the process environment (`ICS_LIVE_DIR`).
    static ENV: Mutex<()> = Mutex::new(());

    fn live_dir_for(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ics-live-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: tests touching the environment hold `ENV`.
        unsafe { std::env::set_var("ICS_LIVE_DIR", &dir) };
        dir
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn round_trip_through_a_served_state() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = live_dir_for("rt");
        let woken = Arc::new(AtomicBool::new(false));
        let w = woken.clone();
        let (server, rx) = Server::start(move || w.store(true, Ordering::SeqCst)).unwrap();
        let live: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(LIVE_FILE)).unwrap()).unwrap();
        assert_eq!(live["port"], server.port());
        assert_eq!(live["pid"], std::process::id());
        assert_eq!(live["token"].as_str().unwrap().len(), 32);
        let app = std::thread::spawn(move || {
            let mut state = State::in_gui(None, false);
            for p in rx {
                let _ = p.reply.send(crate::call_tool(&mut state, &p.tool, &p.arguments));
            }
        });

        let mut c = Client::discover().expect("server is discoverable");
        assert_eq!(c.pid(), std::process::id());
        let status = c.call("gui_status", &Map::new()).unwrap().value().unwrap();
        assert_eq!(status["live"], true);
        let proj = dir.join("proj");
        let info = c.call("new_project", &args(json!({"path": proj}))).unwrap();
        assert!(!info.is_error, "{info:?}");
        let lvs = c.call("run_lvs", &args(json!({"cell": "cmos_inverter"}))).unwrap().value().unwrap();
        assert_eq!(lvs["differences"], 2, "{lvs}");
        let unknown = c.call("nope", &Map::new()).unwrap();
        assert!(unknown.is_error && unknown.text.contains("unknown tool"));
        assert!(woken.load(Ordering::SeqCst));

        let mut bad = Client::discover().unwrap();
        bad.token = "0".repeat(32);
        let r = bad.call("project_info", &Map::new()).unwrap();
        assert!(r.is_error && r.text.contains("invalid token"), "{r:?}");
        assert!(bad.call("project_info", &Map::new()).is_err(), "connection closes after a bad token");

        drop(server);
        assert!(!dir.join(LIVE_FILE).exists());
        drop(c);
        drop(bad);
        app.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_or_foreign_live_file() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = live_dir_for("stale");
        assert!(Client::discover().is_none());
        std::fs::create_dir_all(&dir).unwrap();
        let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(dir.join(LIVE_FILE), json!({"port": port, "token": "x", "pid": 1}).to_string()).unwrap();
        assert!(Client::discover().is_none());
        assert!(!dir.join(LIVE_FILE).exists(), "stale file is cleaned up");

        let (first, _rx1) = Server::start(|| {}).unwrap();
        let (second, _rx2) = Server::start(|| {}).unwrap();
        drop(first);
        assert!(dir.join(LIVE_FILE).exists(), "a newer server's file is left alone");
        assert!(Client::discover().is_some());
        drop(second);
        assert!(!dir.join(LIVE_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
