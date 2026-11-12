//! `ics-mcp` — Model Context Protocol server (stdio, newline-delimited JSON-RPC 2.0) exposing an
//! IC Studio project to AI agents. stdout carries protocol messages only; logs go to stderr.
//!
//! Live mode: while the IC Studio app runs, every tool call is forwarded to it (see
//! `ics_mcp::bridge`), so the agent works on the project open in the window. Without a running
//! app, or with `ICS_MCP_STANDALONE=1`, tools run on this server's own project.

use std::io::{BufRead, Write};

use ics_mcp::bridge::Client;
use ics_mcp::{ToolResult, call_tool, has_tool, tools};
use serde_json::{Map, Value, json};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "IC Studio: full-custom IC design (schematic, SPICE simulation, layout, DRC/LVS). Start with open_project or new_project, then project_info. Layout coordinates are in µm with y pointing up, snapped to the technology grid. Edits stay in memory until save_project. Live mode: while the IC Studio desktop app is running, every tool runs on the project open in its window and the user sees edits, DRC/LVS markers and waveforms there immediately; otherwise tools run on this server's own project. Call gui_status to check; in live mode a project is usually open already, so call project_info before opening another one.";

struct Rpc {
    state: tools::State,
    standalone: bool,
    /// Pid of the app calls were last forwarded to (`None`: ran locally), for logging mode switches.
    live: Option<Option<u32>>,
}

impl Default for Rpc {
    fn default() -> Self {
        Self {
            state: tools::State::default(),
            standalone: std::env::var_os("ICS_MCP_STANDALONE").is_some_and(|v| v == "1"),
            live: None,
        }
    }
}

type RpcError = (i64, String);

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
    let version = PROTOCOL_VERSIONS.iter().find(|v| **v == requested).unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "ic-studio", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

impl Rpc {
    fn handle_line(&mut self, line: &str) -> Option<Value> {
        match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        }
    }

    fn handle(&mut self, msg: Value) -> Option<Value> {
        if !msg.is_object() {
            return Some(error(Value::Null, -32600, "invalid request: expected a JSON object (batches are not supported)"));
        }
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            let is_response = msg.get("result").is_some() || msg.get("error").is_some();
            return id.filter(|_| !is_response).map(|id| error(id, -32600, "invalid request: missing method"));
        };
        let Some(id) = id else {
            eprintln!("ics-mcp: notification {method}");
            return None;
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools::definitions()})),
            "tools/call" => self.call_tool(&params),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err((code, m)) => error(id, code, &m),
        })
    }

    fn call_tool(&mut self, params: &Value) -> Result<Value, RpcError> {
        let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "tools/call: missing tool name".to_string()))?;
        let empty = Map::new();
        let args = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(m)) => m,
            Some(_) => return Err((-32602, "tools/call: arguments must be an object".into())),
        };
        if !has_tool(name) {
            return Err((-32602, format!("unknown tool: {name}")));
        }
        eprintln!("ics-mcp: tool {name}");
        let ToolResult { text, is_error } = self.run(name, args);
        if is_error {
            eprintln!("ics-mcp: {name} failed: {text}");
        }
        Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
    }

    /// Forward to a running IC Studio window if there is one, else run on the local state.
    fn run(&mut self, name: &str, args: &Map<String, Value>) -> ToolResult {
        let client = if self.standalone { None } else { Client::discover() };
        let mode = client.as_ref().map(Client::pid);
        if self.live != Some(mode) {
            match mode {
                Some(pid) => eprintln!("ics-mcp: live mode, forwarding tool calls to IC Studio (pid {pid})"),
                None if self.live.is_some() => eprintln!("ics-mcp: IC Studio is gone; running tools locally"),
                None => eprintln!("ics-mcp: no IC Studio window; running tools locally"),
            }
            self.live = Some(mode);
        }
        match client {
            Some(mut c) => {
                c.call(name, args).unwrap_or_else(|e| ToolResult::error(format!("lost the connection to IC Studio: {e}")))
            }
            None => call_tool(&mut self.state, name, args),
        }
    }
}

fn main() {
    eprintln!("ics-mcp {} ready on stdio", env!("CARGO_PKG_VERSION"));
    let mut server = Rpc::default();
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ics-mcp: stdin: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_line(&line)
            && writeln!(out, "{resp}").and_then(|_| out.flush()).is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standalone() -> Rpc {
        Rpc { standalone: true, ..Rpc::default() }
    }

    fn request(s: &mut Rpc, method: &str, params: Value) -> Value {
        s.handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).expect("response")
    }

    fn call(s: &mut Rpc, name: &str, args: Value) -> Result<Value, String> {
        let r = request(s, "tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        let text = res["content"][0]["text"].as_str().expect("text content").to_string();
        if res["isError"].as_bool().expect("isError") { Err(text) } else { Ok(serde_json::from_str(&text).expect("json text")) }
    }

    #[test]
    fn initialize_negotiates_version() {
        let mut s = standalone();
        for (asked, got) in [("2025-06-18", "2025-06-18"), ("2024-11-05", "2024-11-05"), ("1999-01-01", "2025-06-18")] {
            let r = request(&mut s, "initialize", json!({"protocolVersion": asked, "capabilities": {}}));
            assert_eq!(r["result"]["protocolVersion"], got);
            assert_eq!(r["result"]["serverInfo"]["name"], "ic-studio");
            assert!(r["result"]["capabilities"]["tools"].is_object());
        }
        assert_eq!(request(&mut s, "ping", json!({}))["result"], json!({}));
    }

    #[test]
    fn protocol_errors() {
        let mut s = standalone();
        assert_eq!(request(&mut s, "resources/list", json!({}))["error"]["code"], -32601);
        assert_eq!(request(&mut s, "tools/call", json!({"name": "nope"}))["error"]["code"], -32602);
        assert_eq!(s.handle_line("{not json").unwrap()["error"]["code"], -32700);
        assert!(s.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        assert!(s.handle(json!({"jsonrpc": "2.0", "id": 7, "result": {}})).is_none());
        assert_eq!(s.handle(json!({"jsonrpc": "2.0", "id": 3})).unwrap()["error"]["code"], -32600);
        let e = call(&mut s, "project_info", json!({})).unwrap_err();
        assert!(e.contains("no project is open"), "{e}");
        let st = call(&mut s, "gui_status", json!({})).unwrap();
        assert_eq!(st["live"], false);
        assert!(st["project"].is_null());
    }

    #[test]
    fn tools_list_has_valid_schemas() {
        let mut s = standalone();
        let r = request(&mut s, "tools/list", json!({}));
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        let expected = [
            "open_project",
            "new_project",
            "save_project",
            "project_info",
            "get_netlist",
            "simulate",
            "run_spice",
            "layout_summary",
            "add_rect",
            "add_path",
            "add_label",
            "place_instance",
            "place_pcell",
            "delete_items",
            "generate_layout",
            "flylines",
            "route",
            "run_drc",
            "extract",
            "run_lvs",
            "post_layout_parasitics",
            "export_gds",
            "import_gds",
            "gui_status",
        ];
        assert_eq!(names, expected);
        for t in tools {
            assert!(t["description"].as_str().is_some_and(|d| d.len() > 20), "{t}");
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object");
            let props = schema["properties"].as_object().unwrap();
            for req in schema["required"].as_array().unwrap() {
                assert!(props.contains_key(req.as_str().unwrap()), "{} requires unknown {req}", t["name"]);
            }
            for (k, p) in props {
                assert!(p["type"].is_string(), "{}.{k} has no type", t["name"]);
            }
        }
    }

    #[test]
    fn design_flow() {
        let dir = std::env::temp_dir().join(format!("ics-mcp-flow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = standalone();
        let proj = dir.join("proj");
        let info = call(&mut s, "new_project", json!({"path": proj, "template": "demo"})).unwrap();
        assert!(proj.join("ic-studio.json").exists());
        assert!(call(&mut s, "new_project", json!({"path": proj})).is_err());

        let info2 = call(&mut s, "project_info", json!({})).unwrap();
        assert_eq!(info, info2);
        assert_eq!(info["tech"]["name"], "generic180");
        assert!(info["tech"]["layers"].as_array().unwrap().iter().any(|l| l == "metal1"));
        assert!(info["tech"]["vias"].as_array().unwrap().iter().any(|l| l == "via1"));
        let work = info["libraries"].as_array().unwrap().iter().find(|l| l["name"] == "work").unwrap();
        let inv = work["cells"].as_array().unwrap().iter().find(|c| c["name"] == "cmos_inverter").unwrap();
        assert_eq!(inv["views"], json!(["schematic", "layout"]));

        let net = call(&mut s, "get_netlist", json!({"cell": "cmos_inverter"})).unwrap();
        assert!(net["deck"].as_str().unwrap().contains("pmos"));
        let sim = call(&mut s, "simulate", json!({"cell": "cmos_inverter"})).unwrap();
        assert!(!sim["datasets"].as_array().unwrap().is_empty(), "{sim}");
        let spice = call(&mut s, "run_spice", json!({"deck": "* rc\nV1 a 0 1\nR1 a b 1k\nR2 b 0 1k\n.op\n.end\n"})).unwrap();
        assert_eq!(spice["datasets"][0]["values"]["v(b)"], 0.5, "{spice}");

        let drc = call(&mut s, "run_drc", json!({"cell": "cmos_inverter"})).unwrap();
        assert!(drc["rules_run"].as_u64().unwrap() > 0, "{drc}");
        assert_eq!(drc["clean"], drc["violations"] == 0);

        let lvs = call(&mut s, "run_lvs", json!({"cell": "cmos_inverter"})).unwrap();
        assert_eq!(lvs["matched"], false);
        assert_eq!(lvs["differences"], 2, "{lvs}");
        let msgs = lvs["errors"].to_string();
        assert!(msgs.contains("C1") && msgs.contains("MissingDevice") && msgs.contains("Parameter"), "{msgs}");

        let x = call(&mut s, "extract", json!({"cell": "cmos_inverter"})).unwrap();
        assert_eq!(x["device_count"], 2);
        let pex = call(&mut s, "post_layout_parasitics", json!({"cell": "cmos_inverter"})).unwrap();
        assert!(pex["summary"].as_str().unwrap().starts_with("post-layout"), "{pex}");

        let m1 = call(
            &mut s,
            "place_pcell",
            json!({"cell": "amp", "kind": "nmos", "params": {"w": 1, "l": 0.18, "fingers": 2}, "x": 0, "y": 0, "name": "M1"}),
        )
        .unwrap();
        assert_eq!(m1["created_cell"], true);
        assert_eq!(m1["child"], "pcells/nmos_w1000_l180_nf2");
        let dup = call(&mut s, "place_pcell", json!({"cell": "amp", "kind": "pmos", "x": 0, "y": 8, "name": "M1"}));
        assert!(dup.unwrap_err().contains("already exists"));
        call(&mut s, "place_pcell", json!({"cell": "amp", "kind": "pmos", "x": 0, "y": 8, "mirror": true, "name": "M2"}))
            .unwrap();
        let rect = call(&mut s, "add_rect", json!({"cell": "amp", "layer": "metal1", "x0": 10, "y0": 0, "x1": 10.006, "y1": 2}))
            .unwrap();
        assert_eq!(rect["bbox_um"], json!([10.0, 0.0, 10.01, 2.0]));
        let path = call(&mut s, "add_path", json!({"cell": "amp", "layer": "metal2", "points": [[0, 5], [6, 5]], "width": 0.28}))
            .unwrap();
        call(&mut s, "add_label", json!({"cell": "amp", "layer": "metal1", "text": "out", "x": 10, "y": 1})).unwrap();
        assert!(call(&mut s, "add_rect", json!({"cell": "amp", "layer": "nope", "x0": 0, "y0": 0, "x1": 1, "y1": 1})).is_err());
        assert!(call(&mut s, "place_instance", json!({"cell": "amp", "child": "amp", "x": 0, "y": 0})).is_err());

        let sum = call(&mut s, "layout_summary", json!({"cell": "amp", "list_shapes": true})).unwrap();
        assert_eq!(sum["instance_count"], 2);
        assert_eq!(sum["shape_count"], 3);
        assert_eq!(sum["labels"][0]["text"], "out");
        assert!(sum["bbox_um"].is_array());
        let del = call(&mut s, "delete_items", json!({"cell": "amp", "ids": [path["id"], 99999]})).unwrap();
        assert_eq!(del["deleted"], json!([path["id"]]));
        assert_eq!(del["not_found"], json!([99999]));

        let generated = call(&mut s, "generate_layout", json!({"cell": "cmos_inverter"})).unwrap();
        assert_eq!(generated["placed"].as_array().unwrap().len(), 4, "two devices and two taps: {generated}");
        assert!(call(&mut s, "flylines", json!({"cell": "cmos_inverter"})).unwrap()["count"].as_u64().unwrap() > 0);
        let routed = call(&mut s, "route", json!({"cell": "cmos_inverter"})).unwrap();
        assert_eq!(routed["unrouted_flylines"], 0, "{routed}");
        assert!(routed["failed"].as_array().unwrap().is_empty() && routed["vias"].as_u64().unwrap() > 0, "{routed}");
        assert!(call(&mut s, "route", json!({"cell": "amp"})).unwrap_err().contains("no schematic"));
        assert!(call(&mut s, "generate_layout", json!({"cell": "amp"})).unwrap_err().contains("no schematic"));

        let top = call(&mut s, "place_instance", json!({"cell": "top", "child": "amp", "x": 20, "y": 0, "rot": 90})).unwrap();
        assert_eq!(top["created_cell"], true);
        let gds = dir.join("top.gds");
        let ex = call(&mut s, "export_gds", json!({"cell": "top", "path": gds})).unwrap();
        assert_eq!(ex["cells"], 4, "{ex}");
        let imp = call(&mut s, "import_gds", json!({"path": gds, "lib": "imported"})).unwrap();
        assert_eq!(imp["library"], "imported");
        assert_eq!(imp["top_cells"], json!(["imported/top"]));

        assert_eq!(call(&mut s, "project_info", json!({})).unwrap()["unsaved_changes"], true);
        call(&mut s, "save_project", json!({})).unwrap();
        let reopened = call(&mut s, "open_project", json!({"path": proj})).unwrap();
        assert_eq!(reopened["unsaved_changes"], false);
        assert!(reopened["libraries"].to_string().contains("\"amp\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
