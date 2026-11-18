//! `extension.toml`: what an extension is and what it adds to IC Studio.
//!
//! ```toml
//! [extension]
//! id = "design-report"              # lowercase letters, digits and '-'
//! name = "Design report"
//! version = "0.1.0"
//! description = "Writes a Markdown report of a cell: devices, DRC and LVS."
//! authors = ["Jane Doe"]
//! license = "MIT"
//! repository = "https://github.com/jane/ics-design-report"
//! ics = ">=0.2.9"                   # IC Studio versions it works with
//!
//! [run]                             # a program speaking the extension protocol on stdio
//! command = ["python3", "main.py"]  # relative to the extension folder
//! windows = ["python", "main.py"]   # optional, on Windows instead of `command`
//!
//! [[tools]]                         # becomes the tool `design-report__report`
//! name = "report"
//! description = "Write reports/<cell>.md for a cell."
//! input_schema = { type = "object", properties = { cell = { type = "string" } }, required = ["cell"] }
//!
//! [[commands]]                      # Extensions menu and command palette
//! id = "report"
//! title = "Design report of the active cell"
//! tool = "report"                   # its own tool, or any built-in tool
//! arguments = { cell = "${cell}" }  # ${cell}, ${project}, ${extension} are filled in
//!
//! [[commands]]
//! id = "frame"
//! title = "Draw a metal frame"
//! skill = "scripts/frame.il"        # a SKILL script run on the active cell
//!
//! [[pdks]]                          # PDK files offered by Load PDK
//! name = "My 130 nm PDK"
//! file = "pdk/my130.toml"
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// Separates an extension's id from its tool name in the tool names agents see.
pub const SEP: &str = "__";

/// Longest tool name some model providers accept.
const MAX_TOOL_NAME: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub extension: Info,
    #[serde(default)]
    pub run: Option<Run>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub commands: Vec<CommandDef>,
    #[serde(default)]
    pub pdks: Vec<PdkDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Info {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Version requirement on IC Studio, e.g. `>=0.2.9` or `>=0.3, <0.5`.
    #[serde(default)]
    pub ics: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    pub command: Vec<String>,
    /// Replaces `command` on Windows.
    #[serde(default)]
    pub windows: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Run {
    /// The command line for this platform.
    pub fn argv(&self) -> &[String] {
        match &self.windows {
            Some(w) if cfg!(windows) => w,
            _ => &self.command,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (an object schema); none: no arguments.
    #[serde(default)]
    pub input_schema: Option<Value>,
    /// The tool changes neither schematics nor layouts (it still may call tools that do).
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandDef {
    pub id: String,
    pub title: String,
    /// A tool of this extension, or a built-in tool.
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub arguments: Option<serde_json::Map<String, Value>>,
    /// A SKILL file (relative to the extension folder) run on the active cell.
    #[serde(default)]
    pub skill: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PdkDef {
    pub name: String,
    pub file: String,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| format!("extension.toml: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    pub fn id(&self) -> &str {
        &self.extension.id
    }

    /// The name agents see for one of this extension's tools.
    pub fn tool_name(&self, tool: &str) -> String {
        format!("{}{SEP}{tool}", self.extension.id)
    }

    fn validate(&self) -> Result<(), String> {
        let id = &self.extension.id;
        let ok_id = id.len() <= 40
            && id.starts_with(|c: char| c.is_ascii_lowercase())
            && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !id.ends_with('-');
        if !ok_id {
            return Err(format!("extension id '{id}': use 1-40 lowercase letters, digits and '-', starting with a letter"));
        }
        if self.extension.name.trim().is_empty() {
            return Err("extension name is empty".into());
        }
        parse_version(&self.extension.version).ok_or_else(|| format!("version '{}' is not x.y.z", self.extension.version))?;
        if let Some(req) = &self.extension.ics {
            parse_requirement(req)?;
        }
        if let Some(run) = &self.run
            && run.argv().is_empty()
        {
            return Err("[run] command is empty".into());
        }
        if !self.tools.is_empty() && self.run.is_none() {
            return Err("tools need a [run] command that serves them".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for t in &self.tools {
            let ok = !t.name.is_empty()
                && t.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && !t.name.contains(SEP);
            if !ok {
                return Err(format!("tool name '{}': use letters, digits, '_' and '-' (not '{SEP}')", t.name));
            }
            if self.tool_name(&t.name).len() > MAX_TOOL_NAME {
                return Err(format!("tool name '{}' is too long: id{SEP}name must fit {MAX_TOOL_NAME} characters", t.name));
            }
            if !names.insert(&t.name) {
                return Err(format!("tool '{}' is declared twice", t.name));
            }
            if let Some(s) = &t.input_schema
                && s.get("type").and_then(Value::as_str) != Some("object")
            {
                return Err(format!("tool '{}': input_schema must have type = \"object\"", t.name));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for c in &self.commands {
            if !ids.insert(&c.id) {
                return Err(format!("command '{}' is declared twice", c.id));
            }
            if c.tool.is_some() == c.skill.is_some() {
                return Err(format!("command '{}': give exactly one of tool or skill", c.id));
            }
        }
        Ok(())
    }
}

/// `x.y.z` (a missing part is 0; a pre-release suffix is ignored).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.');
    let mut part = || it.next().map_or(Some(0), |p| p.parse().ok());
    let v = (part()?, part()?, part()?);
    it.next().is_none().then_some(v)
}

type Comparator = (&'static str, (u64, u64, u64));

/// Comma-separated comparators (`>=`, `>`, `<=`, `<`, `=`); a bare version means `>=`.
pub fn parse_requirement(req: &str) -> Result<Vec<Comparator>, String> {
    req.split(',')
        .map(|c| {
            let c = c.trim();
            let (op, rest) =
                ["<=", ">=", "<", ">", "="].iter().find_map(|op| c.strip_prefix(op).map(|r| (*op, r))).unwrap_or((">=", c));
            parse_version(rest).map(|v| (op, v)).ok_or_else(|| format!("version requirement '{req}' is not understood"))
        })
        .collect()
}

/// Whether `version` meets `req`.
pub fn satisfies(req: &str, version: &str) -> bool {
    let (Ok(cs), Some(v)) = (parse_requirement(req), parse_version(version)) else { return false };
    cs.iter().all(|(op, c)| match *op {
        ">=" => v >= *c,
        ">" => v > *c,
        "<=" => v <= *c,
        "<" => v < *c,
        _ => v == *c,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "[extension]\nid = \"demo\"\nname = \"Demo\"\nversion = \"0.1.0\"\n";

    #[test]
    fn parses_the_documented_example() {
        let doc = include_str!("manifest.rs");
        let toml: String = doc
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//!").strip_prefix(' ').unwrap_or(""))
            .skip_while(|l| !l.starts_with("```toml"))
            .skip(1)
            .take_while(|l| !l.starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n");
        let m = Manifest::parse(&toml).unwrap();
        assert_eq!(m.id(), "design-report");
        assert_eq!(m.tool_name("report"), "design-report__report");
        assert_eq!(m.commands.len(), 2);
        assert_eq!(m.pdks[0].file, "pdk/my130.toml");
        assert_eq!(m.tools[0].input_schema.as_ref().unwrap()["required"][0], "cell");
    }

    #[test]
    fn rejects_bad_manifests() {
        let bad =
            |extra: &str, id: &str| Manifest::parse(&MINIMAL.replace("demo", id).replace("\nname", &format!("{extra}\nname")));
        assert!(bad("", "demo").is_ok());
        for id in ["Demo", "1x", "a_b", "x-"] {
            assert!(bad("", id).is_err(), "{id}");
        }
        let tool = format!("{MINIMAL}[[tools]]\nname = \"t\"\ndescription = \"d\"\n");
        assert!(Manifest::parse(&tool).unwrap_err().contains("[run]"));
        let run = format!("{tool}[run]\ncommand = [\"x\"]\n");
        assert!(Manifest::parse(&run).is_ok());
        assert!(Manifest::parse(&run.replace("name = \"t\"", "name = \"a__b\"")).is_err());
        let cmd = format!("{MINIMAL}[[commands]]\nid = \"c\"\ntitle = \"C\"\n");
        assert!(Manifest::parse(&cmd).unwrap_err().contains("exactly one"));
    }

    #[test]
    fn version_requirements() {
        assert!(satisfies(">=0.2.9", "0.2.9"));
        assert!(satisfies("0.2", "0.3.0"));
        assert!(!satisfies(">=0.3", "0.2.9"));
        assert!(satisfies(">=0.2, <0.4", "0.3.1"));
        assert!(!satisfies(">=0.2, <0.4", "0.4.0"));
        assert!(parse_requirement("~1").is_err());
        assert_eq!(parse_version("v1.2.3-beta"), Some((1, 2, 3)));
    }
}
