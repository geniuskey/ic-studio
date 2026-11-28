//! Model providers for the AI features (the copilot, later the rule designer): Gemini, OpenAI,
//! OpenAI-compatible servers and Claude, configured once in the "AI Models" window.
//!
//! Callers speak Gemini `generateContent` JSON. Each client translates the request into its
//! provider's format and the answer back, so the agent loops stay provider-neutral. A model turn
//! may carry a `{"raw": {"provider", "message"}}` part with the provider's own message; the same
//! provider replays it verbatim (Claude needs its thinking blocks back unchanged), the others
//! skip it.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::i18n::{t, tf};

/// One `generateContent` round trip: Gemini request JSON in, Gemini response JSON out.
pub trait Llm: Send {
    fn generate(&mut self, request: &Value) -> Result<Value, String>;

    /// Like [`Llm::generate`], but hands each piece of answer text to `on_text` as it arrives.
    /// `on_text` returns false to abort the request. The default streams nothing until the
    /// whole answer is in.
    fn stream(&mut self, request: &Value, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Value, String> {
        let r = self.generate(request)?;
        for p in r["candidates"][0]["content"]["parts"].as_array().into_iter().flatten() {
            if let Some(t) = p["text"].as_str().filter(|_| p["thought"] != true)
                && !on_text(t)
            {
                return Err(CANCELLED.into());
            }
        }
        Ok(r)
    }
}

/// The error a stream aborted by its `on_text` callback returns.
pub const CANCELLED: &str = "cancelled";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Provider {
    #[default]
    Gemini,
    OpenAi,
    Compatible,
    Claude,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Gemini, Provider::OpenAi, Provider::Compatible, Provider::Claude];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Gemini => "Google Gemini",
            Provider::OpenAi => "OpenAI",
            Provider::Compatible => "OpenAI-compatible",
            Provider::Claude => "Anthropic Claude",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Provider::Gemini => "gemini",
            Provider::OpenAi => "openai",
            Provider::Compatible => "compatible",
            Provider::Claude => "claude",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Gemini => "gemini-flash-latest",
            Provider::OpenAi => "gpt-5",
            Provider::Compatible => "",
            Provider::Claude => "claude-opus-5-5",
        }
    }

    pub fn default_base(self) -> &'static str {
        match self {
            Provider::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            Provider::OpenAi => "https://api.openai.com/v1",
            Provider::Compatible => "",
            Provider::Claude => "https://api.anthropic.com",
        }
    }

    /// Environment variables (or `.env` entries) the key falls back to.
    pub fn key_vars(self) -> &'static [&'static str] {
        match self {
            Provider::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Provider::OpenAi => &["OPENAI_API_KEY"],
            Provider::Compatible => &[],
            Provider::Claude => &["ANTHROPIC_API_KEY"],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
pub struct ProviderConfig {
    pub api_key: String,
    /// Empty: the provider's default model.
    pub model: String,
    /// Empty: the provider's public endpoint.
    pub base_url: String,
    /// Sent with every request (proxies, gateways, organization headers).
    pub headers: Vec<Header>,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
pub struct LlmSettings {
    pub provider: Provider,
    pub gemini: ProviderConfig,
    pub openai: ProviderConfig,
    pub compatible: ProviderConfig,
    pub claude: ProviderConfig,
}

impl LlmSettings {
    pub fn config(&self, p: Provider) -> &ProviderConfig {
        match p {
            Provider::Gemini => &self.gemini,
            Provider::OpenAi => &self.openai,
            Provider::Compatible => &self.compatible,
            Provider::Claude => &self.claude,
        }
    }

    pub fn config_mut(&mut self, p: Provider) -> &mut ProviderConfig {
        match p {
            Provider::Gemini => &mut self.gemini,
            Provider::OpenAi => &mut self.openai,
            Provider::Compatible => &mut self.compatible,
            Provider::Claude => &mut self.claude,
        }
    }

    pub fn model(&self) -> String {
        let m = self.config(self.provider).model.trim();
        if m.is_empty() { self.provider.default_model().to_string() } else { m.to_string() }
    }

    /// "OpenAI · gpt-5", for the windows that use the model.
    pub fn describe(&self) -> String {
        let m = self.model();
        if m.is_empty() { self.provider.name().to_string() } else { format!("{} · {m}", self.provider.name()) }
    }
}

/// API keys and headers (which may hold tokens) of every provider. They are kept out of the
/// app's settings file, which the system writes readable by others, in an owner-only file of
/// their own ([`secrets_path`]).
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
pub struct Secrets {
    pub keys: std::collections::BTreeMap<String, (String, Vec<Header>)>,
}

impl LlmSettings {
    const PROVIDERS: [Provider; 4] = [Provider::Gemini, Provider::OpenAi, Provider::Compatible, Provider::Claude];

    /// The settings without keys and headers, and those apart.
    pub fn split_secrets(&self) -> (LlmSettings, Secrets) {
        let mut public = self.clone();
        let mut secrets = Secrets::default();
        for p in Self::PROVIDERS {
            let c = public.config_mut(p);
            let (key, headers) = (std::mem::take(&mut c.api_key), std::mem::take(&mut c.headers));
            if !key.is_empty() || !headers.is_empty() {
                secrets.keys.insert(format!("{p:?}"), (key, headers));
            }
        }
        (public, secrets)
    }

    /// Fill in keys and headers from `secrets` where these settings have none (settings saved
    /// by an older version still hold their own, moved out on the next save).
    pub fn merge_secrets(&mut self, secrets: &Secrets) {
        for p in Self::PROVIDERS {
            if let Some((key, headers)) = secrets.keys.get(&format!("{p:?}")) {
                let c = self.config_mut(p);
                if c.api_key.is_empty() {
                    c.api_key = key.clone();
                }
                if c.headers.is_empty() {
                    c.headers = headers.clone();
                }
            }
        }
    }
}

/// The owner-only file with the API keys: `secrets.json` next to `ICS_SETTINGS_PATH` when set,
/// else in the per-user config folder (`ic-studio/secrets.json`).
pub fn secrets_path() -> Option<PathBuf> {
    match std::env::var_os("ICS_SETTINGS_PATH").filter(|p| !p.is_empty()) {
        Some(p) => Some(PathBuf::from(p).parent()?.join("secrets.json")),
        None => dirs::config_dir().map(|d| d.join("ic-studio").join("secrets.json")),
    }
}

pub fn load_secrets() -> Secrets {
    secrets_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_secrets(s: &Secrets) -> Result<(), String> {
    let path = secrets_path().ok_or("no per-user config folder for the API keys")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    ics_mcp::bridge::write_private(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// `KEY=value` lines of a `.env` file (quotes and `export ` stripped).
pub fn dotenv(path: &std::path::Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("export ").unwrap_or(l);
            if l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()))
        })
        .collect()
}

/// A key from the environment, else from a `.env` file in one of `dirs` (e.g. the project
/// folder) or the working directory, or their parents. The user's own environment wins: a
/// project from someone else could otherwise bring a `.env` that sends the conversation, and the
/// design in it, to their account.
pub fn env_key(vars: &[&str], dirs: &[PathBuf]) -> Option<String> {
    let from_env = vars.iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty())).map(|k| k.trim().to_string());
    from_env.or_else(|| {
        let cwd = std::env::current_dir().ok();
        dirs.iter().chain(&cwd).flat_map(|d| d.ancestors().take(4)).find_map(|d| {
            let found = dotenv(&d.join(".env"));
            vars.iter().find_map(|k| found.iter().find(|(n, v)| n == k && !v.is_empty()).map(|(_, v)| v.clone()))
        })
    })
}

/// The client for the selected provider; the key falls back to the environment / `.env` in `dirs`.
pub fn client(s: &LlmSettings, dirs: &[PathBuf]) -> Result<Box<dyn Llm>, String> {
    let p = s.provider;
    let c = s.config(p);
    let key = Some(c.api_key.trim().to_string()).filter(|k| !k.is_empty()).or_else(|| env_key(p.key_vars(), dirs));
    let key = match (key, p) {
        (Some(k), _) => k,
        (None, Provider::Compatible) => String::new(),
        (None, _) => {
            return Err(format!(
                "No API key for {}: set one in AI Models settings, or {} in the environment or a .env file.",
                p.name(),
                p.key_vars().join(" / ")
            ));
        }
    };
    let model = s.model();
    if model.is_empty() {
        return Err(format!("Set a model for {} in AI Models settings.", p.name()));
    }
    let base = Some(c.base_url.trim().trim_end_matches('/').to_string()).filter(|b| !b.is_empty());
    let Some(base) = base.or_else(|| Some(p.default_base().to_string()).filter(|b| !b.is_empty())) else {
        return Err(format!("Set a base URL for {} in AI Models settings.", p.name()));
    };
    let headers = c
        .headers
        .iter()
        .filter(|h| !h.name.trim().is_empty())
        .map(|h| (h.name.trim().to_string(), h.value.trim().to_string()))
        .collect();
    Ok(Box::new(Http { provider: p, key, model, base, headers }))
}

struct Http {
    provider: Provider,
    key: String,
    model: String,
    base: String,
    headers: Vec<(String, String)>,
}

/// The message of a provider's error body.
fn error_message(body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .or(body["error"].as_str())
        .or(body["message"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string().chars().take(300).collect())
}

/// What a streaming POST got back.
enum Streamed {
    /// Every server-sent event's `data` was handed to the callback.
    Events,
    /// The server answered with plain JSON instead of an event stream.
    Json(Value),
}

impl Http {
    fn send(&self, url: &str, auth: &[(&str, String)], body: &Value) -> Result<ureq::http::Response<ureq::Body>, String> {
        let mut req = ureq::post(url);
        for (k, v) in auth.iter().filter(|(_, v)| !v.is_empty()) {
            req = req.header(*k, v);
        }
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        let mut res =
            req.config().http_status_as_error(false).build().send_json(body).map_err(|e| format!("request failed: {e}"))?;
        let status = res.status();
        if !status.is_success() {
            let text = res.body_mut().read_to_string().map_err(|e| format!("bad response: {e}"))?;
            let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
            return Err(format!("{} {}: {}", self.provider.name(), status.as_u16(), error_message(&body)));
        }
        Ok(res)
    }

    fn post(&self, url: &str, auth: &[(&str, String)], body: &Value) -> Result<Value, String> {
        let mut res = self.send(url, auth, body)?;
        let text = res.body_mut().read_to_string().map_err(|e| format!("bad response: {e}"))?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// POST and read the answer as server-sent events, handing each event's JSON `data` to
    /// `on_event`, which returns false to stop reading.
    fn post_stream(
        &self,
        url: &str,
        auth: &[(&str, String)],
        body: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<bool, String>,
    ) -> Result<Streamed, String> {
        let mut res = self.send(url, auth, body)?;
        let sse =
            res.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("text/event-stream"));
        if !sse {
            let text = res.body_mut().read_to_string().map_err(|e| format!("bad response: {e}"))?;
            return Ok(Streamed::Json(serde_json::from_str(&text).unwrap_or(Value::String(text))));
        }
        let reader = std::io::BufReader::new(res.body_mut().as_reader());
        read_sse(reader, on_event)?;
        Ok(Streamed::Events)
    }
}

/// Read server-sent events, handing each event's `data` (parsed as JSON) to `on_event`.
fn read_sse(reader: impl std::io::BufRead, on_event: &mut dyn FnMut(Value) -> Result<bool, String>) -> Result<(), String> {
    let mut data = String::new();
    let mut flush = |data: &mut String| -> Result<bool, String> {
        let d = std::mem::take(data);
        let d = d.trim();
        if d.is_empty() || d == "[DONE]" {
            return Ok(true);
        }
        match serde_json::from_str::<Value>(d) {
            Ok(v) => on_event(v),
            Err(_) => Ok(true),
        }
    };
    for line in reader.lines() {
        let line = line.map_err(|e| format!("the stream broke off: {e}"))?;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !flush(&mut data)? {
                return Ok(());
            }
        } else if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.strip_prefix(' ').unwrap_or(d));
        }
    }
    flush(&mut data)?;
    Ok(())
}

/// Turn a stream abort into the cancel error, else run the event handler.
fn keep(on_text: &mut dyn FnMut(&str) -> bool, t: &str) -> Result<bool, String> {
    if t.is_empty() || on_text(t) { Ok(true) } else { Err(CANCELLED.into()) }
}

/// Gemini stream chunks into one `generateContent` answer: consecutive text pieces merge.
#[derive(Default)]
struct GeminiAcc {
    parts: Vec<Value>,
    finish: Value,
}

impl GeminiAcc {
    fn chunk(&mut self, v: &Value, on_text: &mut dyn FnMut(&str) -> bool) -> Result<bool, String> {
        if v.get("error").is_some() {
            return Err(format!("Google Gemini: {}", error_message(v)));
        }
        let c = &v["candidates"][0];
        if let Some(f) = c.get("finishReason") {
            self.finish = f.clone();
        }
        for p in c["content"]["parts"].as_array().into_iter().flatten() {
            if let Some(t) = p["text"].as_str() {
                if p["thought"] != true {
                    keep(on_text, t)?;
                }
                if let Some(last) = self.parts.last_mut()
                    && last["text"].is_string()
                    && last["thought"] == p["thought"]
                    && last.get("thoughtSignature").is_none()
                {
                    let joined = format!("{}{t}", last["text"].as_str().unwrap_or_default());
                    last["text"] = joined.into();
                    if let Some(sig) = p.get("thoughtSignature") {
                        last["thoughtSignature"] = sig.clone();
                    }
                    continue;
                }
            }
            self.parts.push(p.clone());
        }
        Ok(true)
    }

    fn finish(mut self) -> Value {
        self.parts.retain(|p| !(p["text"].as_str().is_some_and(|t| t.is_empty()) && p.get("thoughtSignature").is_none()));
        json!({"candidates": [{"content": {"role": "model", "parts": self.parts}, "finishReason": self.finish}]})
    }
}

/// OpenAI chat-completion chunks into one `chat.completion` message.
#[derive(Default)]
struct OpenAiAcc {
    content: String,
    refusal: String,
    calls: Vec<Value>,
    finish: Value,
}

impl OpenAiAcc {
    fn chunk(&mut self, v: &Value, on_text: &mut dyn FnMut(&str) -> bool) -> Result<bool, String> {
        if v.get("error").is_some() {
            return Err(error_message(v));
        }
        let c = &v["choices"][0];
        if !c["finish_reason"].is_null() {
            self.finish = c["finish_reason"].clone();
        }
        let d = &c["delta"];
        if let Some(t) = d["content"].as_str() {
            keep(on_text, t)?;
            self.content.push_str(t);
        }
        if let Some(t) = d["refusal"].as_str() {
            self.refusal.push_str(t);
        }
        for tc in d["tool_calls"].as_array().into_iter().flatten() {
            let i = tc["index"].as_u64().map(|i| i as usize).unwrap_or(self.calls.len());
            while self.calls.len() <= i {
                self.calls.push(json!({"id": "", "type": "function", "function": {"name": "", "arguments": ""}}));
            }
            let call = &mut self.calls[i];
            if let Some(id) = tc["id"].as_str().filter(|s| !s.is_empty()) {
                call["id"] = id.into();
            }
            for k in ["name", "arguments"] {
                if let Some(piece) = tc["function"][k].as_str() {
                    let joined = format!("{}{piece}", call["function"][k].as_str().unwrap_or_default());
                    call["function"][k] = joined.into();
                }
            }
        }
        Ok(true)
    }

    fn finish(self) -> Value {
        let mut msg =
            json!({"role": "assistant", "content": if self.content.is_empty() { Value::Null } else { self.content.into() }});
        if !self.calls.is_empty() {
            msg["tool_calls"] = self.calls.into();
        }
        if !self.refusal.is_empty() {
            msg["refusal"] = self.refusal.into();
        }
        json!({"choices": [{"message": msg, "finish_reason": self.finish}]})
    }
}

/// Claude message stream events into one Messages API response.
#[derive(Default)]
struct ClaudeAcc {
    content: Vec<Value>,
    /// The partial JSON of each tool_use block's input.
    inputs: Vec<String>,
    stop: Value,
    stop_details: Value,
}

impl ClaudeAcc {
    fn event(&mut self, v: &Value, on_text: &mut dyn FnMut(&str) -> bool) -> Result<bool, String> {
        let at = v["index"].as_u64().map(|i| i as usize).unwrap_or(0);
        match v["type"].as_str().unwrap_or_default() {
            "error" => return Err(format!("Anthropic Claude: {}", error_message(v))),
            "content_block_start" => {
                while self.content.len() <= at {
                    self.content.push(Value::Null);
                    self.inputs.push(String::new());
                }
                self.content[at] = v["content_block"].clone();
            }
            "content_block_delta" => {
                let Some(b) = self.content.get_mut(at) else { return Ok(true) };
                let d = &v["delta"];
                let append = |b: &mut Value, k: &str, piece: &str| {
                    let joined = format!("{}{piece}", b[k].as_str().unwrap_or_default());
                    b[k] = joined.into();
                };
                match d["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let t = d["text"].as_str().unwrap_or_default();
                        append(b, "text", t);
                        keep(on_text, t)?;
                    }
                    "thinking_delta" => append(b, "thinking", d["thinking"].as_str().unwrap_or_default()),
                    "signature_delta" => b["signature"] = d["signature"].clone(),
                    "input_json_delta" => self.inputs[at].push_str(d["partial_json"].as_str().unwrap_or_default()),
                    "citations_delta" => {
                        if !b["citations"].is_array() {
                            b["citations"] = json!([]);
                        }
                        if let Some(c) = b["citations"].as_array_mut() {
                            c.push(d["citation"].clone());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(b) = self.content.get_mut(at)
                    && b.get("input").is_some()
                {
                    let raw = self.inputs[at].trim();
                    if !raw.is_empty() {
                        b["input"] = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
                    } else if !b["input"].is_object() {
                        b["input"] = json!({});
                    }
                }
            }
            "message_delta" => {
                if let Some(s) = v["delta"].get("stop_reason") {
                    self.stop = s.clone();
                }
                if let Some(s) = v["delta"].get("stop_details") {
                    self.stop_details = s.clone();
                }
            }
            "message_stop" => return Ok(false),
            _ => {}
        }
        Ok(true)
    }

    fn finish(self) -> Value {
        let content: Vec<Value> = self.content.into_iter().filter(|b| !b.is_null()).collect();
        json!({"content": content, "stop_reason": self.stop, "stop_details": self.stop_details})
    }
}

impl Llm for Http {
    fn generate(&mut self, request: &Value) -> Result<Value, String> {
        match self.provider {
            Provider::Gemini => {
                let url = format!("{}/models/{}:generateContent", self.base, self.model);
                self.post(&url, &[("x-goog-api-key", self.key.clone())], &to_gemini(request))
            }
            Provider::OpenAi | Provider::Compatible => {
                let url = format!("{}/chat/completions", self.base);
                let body = to_openai(request, &self.model, self.provider.tag());
                let auth = if self.key.is_empty() { String::new() } else { format!("Bearer {}", self.key) };
                from_openai(&self.post(&url, &[("Authorization", auth)], &body)?, self.provider.tag())
            }
            Provider::Claude => {
                let url = format!("{}/v1/messages", self.base);
                let body = to_claude(request, &self.model);
                let auth = [("x-api-key", self.key.clone()), ("anthropic-version", "2026-10-21".to_string())];
                from_claude(&self.post(&url, &auth, &body)?)
            }
        }
    }

    fn stream(&mut self, request: &Value, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Value, String> {
        match self.provider {
            Provider::Gemini => {
                let url = format!("{}/models/{}:streamGenerateContent?alt=sse", self.base, self.model);
                let mut acc = GeminiAcc::default();
                let auth = [("x-goog-api-key", self.key.clone())];
                match self.post_stream(&url, &auth, &to_gemini(request), &mut |v| acc.chunk(&v, on_text))? {
                    Streamed::Events => Ok(acc.finish()),
                    // A plain answer is a list of chunks (or one answer) when the proxy ignores `alt=sse`.
                    Streamed::Json(v) => {
                        let mut acc = GeminiAcc::default();
                        for c in v.as_array().cloned().unwrap_or_else(|| vec![v]) {
                            acc.chunk(&c, on_text)?;
                        }
                        Ok(acc.finish())
                    }
                }
            }
            Provider::OpenAi | Provider::Compatible => {
                let url = format!("{}/chat/completions", self.base);
                let mut body = to_openai(request, &self.model, self.provider.tag());
                body["stream"] = true.into();
                let auth = if self.key.is_empty() { String::new() } else { format!("Bearer {}", self.key) };
                let mut acc = OpenAiAcc::default();
                let res = match self.post_stream(&url, &[("Authorization", auth)], &body, &mut |v| acc.chunk(&v, on_text))? {
                    Streamed::Events => acc.finish(),
                    Streamed::Json(v) => {
                        if let Some(t) = v["choices"][0]["message"]["content"].as_str() {
                            keep(on_text, t)?;
                        }
                        v
                    }
                };
                from_openai(&res, self.provider.tag())
            }
            Provider::Claude => {
                let url = format!("{}/v1/messages", self.base);
                let mut body = to_claude(request, &self.model);
                body["stream"] = true.into();
                let auth = [("x-api-key", self.key.clone()), ("anthropic-version", "2026-10-21".to_string())];
                let mut acc = ClaudeAcc::default();
                let res = match self.post_stream(&url, &auth, &body, &mut |v| acc.event(&v, on_text))? {
                    Streamed::Events => acc.finish(),
                    Streamed::Json(v) => {
                        for b in v["content"].as_array().into_iter().flatten() {
                            if let Some(t) = b["text"].as_str() {
                                keep(on_text, t)?;
                            }
                        }
                        v
                    }
                };
                from_claude(&res)
            }
        }
    }
}

fn system_text(req: &Value) -> String {
    req["systemInstruction"]["parts"]
        .as_array()
        .map(|p| p.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

fn declarations(req: &Value) -> Vec<Value> {
    req["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|t| t["functionDeclarations"].as_array().cloned().unwrap_or_default())
        .collect()
}

fn schema(d: &Value) -> Value {
    let s = d.get("parametersJsonSchema").or(d.get("parameters")).cloned().unwrap_or(json!({"type": "object"}));
    if s.is_object() { s } else { json!({"type": "object"}) }
}

/// The provider's own message saved on a model turn, if it came from `tag`.
fn raw<'a>(parts: &'a [Value], tag: &str) -> Option<&'a Value> {
    parts.iter().find_map(|p| p.get("raw").filter(|r| r["provider"] == tag).map(|r| &r["message"]))
}

fn raw_part(tag: &str, message: Value) -> Value {
    json!({"raw": {"provider": tag, "message": message}})
}

/// Gemini only knows Gemini parts: drop other providers' saved messages and call ids.
fn to_gemini(req: &Value) -> Value {
    let mut req = req.clone();
    if let Some(contents) = req["contents"].as_array_mut() {
        for c in contents {
            if let Some(parts) = c["parts"].as_array_mut() {
                parts.retain(|p| p.get("raw").is_none());
                for p in parts.iter_mut() {
                    for k in ["functionCall", "functionResponse"] {
                        if let Some(o) = p.get_mut(k).and_then(Value::as_object_mut) {
                            o.remove("id");
                        }
                    }
                }
            }
        }
    }
    req
}

/// Call ids for the history: a call keeps its own id, else gets one from its position; each
/// response takes the first unanswered call of the previous model turn with the same name.
struct CallIds {
    open: Vec<(String, String)>,
}

impl CallIds {
    fn model_turn(&mut self, turn: usize, parts: &[Value]) -> Vec<String> {
        self.open.clear();
        let mut ids = vec![];
        for (k, c) in parts.iter().filter_map(|p| p.get("functionCall")).enumerate() {
            let id = c["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call_{turn}_{k}"));
            self.open.push((id.clone(), c["name"].as_str().unwrap_or_default().to_string()));
            ids.push(id);
        }
        ids
    }

    fn response(&mut self, name: &str) -> String {
        let at = self.open.iter().position(|(_, n)| n == name).unwrap_or(0);
        if at < self.open.len() { self.open.remove(at).0 } else { format!("call_orphan_{name}") }
    }
}

fn response_text(r: &Value) -> (String, bool) {
    let res = &r["response"];
    let error = res.get("error").is_some();
    let text = match res.get("result").or(res.get("error")) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => res.to_string(),
    };
    (text, error)
}

fn to_openai(req: &Value, model: &str, tag: &str) -> Value {
    let mut messages = vec![];
    let system = system_text(req);
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    let mut ids = CallIds { open: vec![] };
    for (turn, c) in req["contents"].as_array().into_iter().flatten().enumerate() {
        let parts = c["parts"].as_array().cloned().unwrap_or_default();
        if c["role"] == "model" {
            let call_ids = ids.model_turn(turn, &parts);
            if let Some(m) = raw(&parts, tag) {
                // Servers answer tool calls with `"content": null`, but strict OpenAI-compatible
                // proxies (litellm) reject null on the way back in: replay it as "".
                let mut m = m.clone();
                if !m["content"].is_string() && !m["content"].is_array() {
                    m["content"] = "".into();
                }
                messages.push(m);
                continue;
            }
            let text: String =
                parts.iter().filter(|p| p["thought"] != true).filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n");
            let calls: Vec<Value> = parts
                .iter()
                .filter_map(|p| p.get("functionCall"))
                .zip(&call_ids)
                .map(|(f, id)| {
                    json!({"id": id, "type": "function", "function": {"name": f["name"], "arguments": f["args"].to_string()}})
                })
                .collect();
            let mut m = json!({"role": "assistant", "content": text});
            if !calls.is_empty() {
                m["tool_calls"] = calls.into();
            }
            messages.push(m);
        } else {
            for p in &parts {
                if let Some(r) = p.get("functionResponse") {
                    let id = ids.response(r["name"].as_str().unwrap_or_default());
                    messages.push(json!({"role": "tool", "tool_call_id": id, "content": response_text(r).0}));
                }
            }
            for p in &parts {
                if let Some(t) = p["text"].as_str() {
                    messages.push(json!({"role": "user", "content": t}));
                }
            }
        }
    }
    let mut body = json!({"model": model, "messages": messages});
    let tools: Vec<Value> = declarations(req)
        .iter()
        .map(|d| json!({"type": "function", "function": {"name": d["name"], "description": d["description"], "parameters": schema(d)}}))
        .collect();
    if !tools.is_empty() {
        body["tools"] = tools.into();
    }
    body
}

fn from_openai(res: &Value, tag: &str) -> Result<Value, String> {
    let choice = &res["choices"][0];
    let msg = &choice["message"];
    let mut parts = vec![];
    if let Some(t) = msg["content"].as_str().filter(|t| !t.trim().is_empty()) {
        parts.push(json!({"text": t}));
    }
    for c in msg["tool_calls"].as_array().into_iter().flatten() {
        let f = &c["function"];
        let args = match &f["arguments"] {
            Value::String(s) if s.trim().is_empty() => json!({}),
            Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
            v => v.clone(),
        };
        parts.push(json!({"functionCall": {"id": c["id"], "name": f["name"], "args": args}}));
    }
    if parts.is_empty() {
        if let Some(r) = msg["refusal"].as_str() {
            return Err(format!("The model refused: {r}"));
        }
    } else {
        parts.push(raw_part(tag, msg.clone()));
    }
    Ok(json!({"candidates": [{"content": {"role": "model", "parts": parts}, "finishReason": choice["finish_reason"]}]}))
}

fn to_claude(req: &Value, model: &str) -> Value {
    let mut messages = vec![];
    let mut ids = CallIds { open: vec![] };
    for (turn, c) in req["contents"].as_array().into_iter().flatten().enumerate() {
        let parts = c["parts"].as_array().cloned().unwrap_or_default();
        if c["role"] == "model" {
            let call_ids = ids.model_turn(turn, &parts);
            if let Some(m) = raw(&parts, "claude") {
                messages.push(json!({"role": "assistant", "content": m}));
                continue;
            }
            let mut content = vec![];
            let mut k = 0;
            for p in &parts {
                if let Some(t) = p["text"].as_str().filter(|t| p["thought"] != true && !t.trim().is_empty()) {
                    content.push(json!({"type": "text", "text": t}));
                }
                if let Some(f) = p.get("functionCall") {
                    let input = if f["args"].is_object() { f["args"].clone() } else { json!({}) };
                    content.push(json!({"type": "tool_use", "id": call_ids[k], "name": f["name"], "input": input}));
                    k += 1;
                }
            }
            if content.is_empty() {
                content.push(json!({"type": "text", "text": "(no answer)"}));
            }
            messages.push(json!({"role": "assistant", "content": content}));
        } else {
            let mut content = vec![];
            for p in &parts {
                if let Some(r) = p.get("functionResponse") {
                    let id = ids.response(r["name"].as_str().unwrap_or_default());
                    let (text, error) = response_text(r);
                    content.push(json!({"type": "tool_result", "tool_use_id": id, "content": text, "is_error": error}));
                }
            }
            for p in &parts {
                if let Some(t) = p["text"].as_str() {
                    content.push(json!({"type": "text", "text": t}));
                }
            }
            messages.push(json!({"role": "user", "content": content}));
        }
    }
    let mut body = json!({"model": model, "max_tokens": 16000, "messages": messages});
    let system = system_text(req);
    if !system.is_empty() {
        body["system"] = system.into();
    }
    let tools: Vec<Value> = declarations(req)
        .iter()
        .map(|d| json!({"name": d["name"], "description": d["description"], "input_schema": schema(d)}))
        .collect();
    if !tools.is_empty() {
        body["tools"] = tools.into();
    }
    body
}

fn from_claude(res: &Value) -> Result<Value, String> {
    let stop = res["stop_reason"].as_str().unwrap_or_default();
    if stop == "refusal" {
        let why = res["stop_details"]["explanation"].as_str().unwrap_or("the request was declined");
        return Err(format!("Claude declined: {why}"));
    }
    let content = res["content"].as_array().cloned().unwrap_or_default();
    let mut parts = vec![];
    for b in &content {
        match b["type"].as_str() {
            Some("text") => {
                if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    parts.push(json!({"text": t}));
                }
            }
            Some("tool_use") => parts.push(json!({"functionCall": {"id": b["id"], "name": b["name"], "args": b["input"]}})),
            _ => {}
        }
    }
    if !parts.is_empty() {
        parts.push(raw_part("claude", Value::Array(content)));
    }
    Ok(json!({"candidates": [{"content": {"role": "model", "parts": parts}, "finishReason": stop}]}))
}

/// The "AI Models" window: provider, key, model, endpoint and headers, with a connection test.
#[derive(Default)]
pub struct AiSettingsWindow {
    pub open: bool,
    test: Option<Receiver<Result<String, String>>>,
    status: Option<Result<String, String>>,
}

impl AiSettingsWindow {
    /// Returns true when `s` changed.
    pub fn window(&mut self, ctx: &egui::Context, s: &mut LlmSettings, dirs: &[PathBuf]) -> bool {
        if let Some(rx) = &self.test
            && let Ok(r) = rx.try_recv()
        {
            self.status = Some(r);
            self.test = None;
        }
        let mut open = self.open;
        let before = s.clone();
        crate::float::tool_window(ctx, "ai_models", "AI Models", [520.0, 420.0], &mut open, |ui| self.ui(ui, s, dirs));
        self.open = open;
        let changed = *s != before;
        if changed && self.test.is_none() {
            self.status = None;
        }
        changed
    }

    fn ui(&mut self, ui: &mut egui::Ui, s: &mut LlmSettings, dirs: &[PathBuf]) {
        let pal = crate::theme::palette(ui.ctx());
        ui.label(egui::RichText::new(t("Used by the copilot and the other AI features.")).color(pal.text_weak));
        ui.add_space(6.0);
        let p = s.provider;
        egui::Grid::new("ai_models").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            let l = ui.label(t("Provider"));
            egui::ComboBox::from_id_salt("ai_provider")
                .selected_text(p.name())
                .show_ui(ui, |ui| {
                    for q in Provider::ALL {
                        ui.selectable_value(&mut s.provider, q, q.name());
                    }
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
            let c = s.config_mut(p);
            let l = ui.label(t("API key"));
            let hint = if env_key(p.key_vars(), dirs).is_some() {
                "from environment / .env".to_string()
            } else if p.key_vars().is_empty() {
                "optional".to_string()
            } else {
                p.key_vars()[0].to_string()
            };
            ui.add(egui::TextEdit::singleline(&mut c.api_key).password(true).desired_width(300.0).hint_text(hint))
                .labelled_by(l.id);
            ui.end_row();
            let l = ui.label(t("Model"));
            let hint = if p.default_model().is_empty() { "e.g. llama3.1" } else { p.default_model() };
            ui.add(egui::TextEdit::singleline(&mut c.model).desired_width(300.0).hint_text(hint)).labelled_by(l.id);
            ui.end_row();
            let l = ui.label(t("Base URL"));
            let hint = if p.default_base().is_empty() { "e.g. http://localhost:11434/v1" } else { p.default_base() };
            ui.add(egui::TextEdit::singleline(&mut c.base_url).desired_width(300.0).hint_text(hint)).labelled_by(l.id);
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(t("Custom headers")).strong());
        let c = s.config_mut(p);
        let mut remove = None;
        for (k, h) in c.headers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut h.name).desired_width(150.0).hint_text(t("Header")))
                    .on_hover_text(tf("Header {} name", &[&(k + 1).to_string()]));
                ui.add(egui::TextEdit::singleline(&mut h.value).desired_width(240.0).hint_text(t("Value")))
                    .on_hover_text(tf("Header {} value", &[&(k + 1).to_string()]));
                if ui.small_button("x").on_hover_text(t("Remove header")).clicked() {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = remove {
            c.headers.remove(k);
        }
        if ui.button(t("+ Header")).clicked() {
            c.headers.push(Header::default());
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let testing = self.test.is_some();
            if ui.add_enabled(!testing, egui::Button::new(t("Test connection"))).clicked() {
                self.status = None;
                let (tx, rx) = mpsc::channel();
                let (s, dirs, ctx) = (s.clone(), dirs.to_vec(), ui.ctx().clone());
                std::thread::spawn(move || {
                    let req = json!({"contents": [{"role": "user", "parts": [{"text": "Reply with the single word OK."}]}]});
                    let r = client(&s, &dirs).and_then(|mut c| c.generate(&req)).map(|r| {
                        let t: Vec<&str> = r["candidates"][0]["content"]["parts"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|p| p["text"].as_str())
                            .collect();
                        format!("Connected: {}", t.join(" ").trim())
                    });
                    let _ = tx.send(r);
                    ctx.request_repaint();
                });
                self.test = Some(rx);
            }
            if testing {
                ui.add(egui::Spinner::new());
            }
        });
        match &self.status {
            Some(Ok(t)) => {
                ui.label(egui::RichText::new(t).color(pal.ok));
            }
            Some(Err(e)) => {
                ui.label(egui::RichText::new(e).color(pal.danger));
            }
            None => {}
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(t("Settings, keys included, are saved as plain text in your user profile."))
                .small()
                .color(pal.text_weak),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_kept_apart_from_the_settings() {
        let mut s = LlmSettings::default();
        s.claude.api_key = "sk-secret".into();
        s.openai.headers = vec![Header { name: "X-Token".into(), value: "t".into() }];
        s.claude.model = "m".into();
        let (public, secrets) = s.split_secrets();
        let text = serde_json::to_string(&public).unwrap();
        assert!(!text.contains("sk-secret") && !text.contains("X-Token"), "{text}");
        assert_eq!(public.claude.model, "m");
        let mut back = public.clone();
        back.merge_secrets(&serde_json::from_str(&serde_json::to_string(&secrets).unwrap()).unwrap());
        assert_eq!(back, s);
    }

    #[test]
    fn reads_dotenv_lines() {
        let d = std::env::temp_dir().join(format!("ics-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".env"), "# c\r\nexport A = \"x y\"\r\nGEMINI_API_KEY='k1'\r\n").unwrap();
        let v = dotenv(&d.join(".env"));
        assert_eq!(v, [("A".to_string(), "x y".to_string()), ("GEMINI_API_KEY".to_string(), "k1".to_string())]);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn agent_request() -> Value {
        json!({
            "systemInstruction": {"parts": [{"text": "sys"}]},
            "contents": [
                {"role": "user", "parts": [{"text": "run drc"}]},
                {"role": "model", "parts": [{"text": "Checking."}, {"functionCall": {"name": "run_drc", "args": {"cell": "a"}}},
                    {"functionCall": {"name": "flylines", "args": {}}}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "run_drc", "response": {"result": {"violations": 0}}}},
                    {"functionResponse": {"name": "flylines", "response": {"error": "no layout"}}}]},
            ],
            "tools": [{"functionDeclarations": [{"name": "run_drc", "description": "d", "parametersJsonSchema": {"type": "object"}}]}],
        })
    }

    #[test]
    fn openai_pairs_calls_and_results() {
        let b = to_openai(&agent_request(), "m", "openai");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["tool_calls"][1]["id"], "call_1_1");
        assert_eq!(m[2]["content"], "Checking.");
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{\"cell\":\"a\"}");
        assert_eq!(m[3]["tool_call_id"], "call_1_0");
        assert_eq!(m[4]["tool_call_id"], "call_1_1");
        assert_eq!(m[4]["content"], "no layout");
        assert_eq!(b["tools"][0]["function"]["name"], "run_drc");
        let r = from_openai(
            &json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null,
                "tool_calls": [{"id": "x1", "type": "function", "function": {"name": "run_drc", "arguments": "{\"cell\":\"b\"}"}}]}}]}),
            "openai",
        )
        .unwrap();
        let parts = &r["candidates"][0]["content"]["parts"];
        assert_eq!(parts[0]["functionCall"]["args"]["cell"], "b");
        assert_eq!(parts[0]["functionCall"]["id"], "x1");
        assert_eq!(parts[1]["raw"]["provider"], "openai");
        // Replaying that turn must not send `"content": null` back (litellm rejects it).
        let mut req = agent_request();
        req["contents"][1] = r["candidates"][0]["content"].clone();
        let b = to_openai(&req, "m", "openai");
        assert_eq!(b["messages"][2]["content"], "");
        assert_eq!(b["messages"][2]["tool_calls"][0]["id"], "x1");
    }

    #[test]
    fn claude_replays_raw_content_and_flags_errors() {
        let mut req = agent_request();
        let raw = json!([{"type": "thinking", "thinking": "", "signature": "s"}, {"type": "tool_use", "id": "toolu_1", "name": "run_drc", "input": {}},
            {"type": "tool_use", "id": "toolu_2", "name": "flylines", "input": {}}]);
        req["contents"][1]["parts"] = json!([
            {"functionCall": {"id": "toolu_1", "name": "run_drc", "args": {}}},
            {"functionCall": {"id": "toolu_2", "name": "flylines", "args": {}}},
            raw_part("claude", raw.clone()),
        ]);
        let b = to_claude(&req, "claude-opus-5-5");
        assert_eq!(b["system"], "sys");
        assert_eq!(b["messages"][1]["content"], raw);
        let results = &b["messages"][2]["content"];
        assert_eq!(results[0]["tool_use_id"], "toolu_1");
        assert_eq!(results[1]["tool_use_id"], "toolu_2");
        assert_eq!(results[1]["is_error"], true);
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        let g = to_gemini(&req);
        assert_eq!(g["contents"][1]["parts"].as_array().unwrap().len(), 2);
        assert!(g["contents"][1]["parts"][0]["functionCall"].get("id").is_none());
    }

    #[test]
    fn claude_refusal_is_an_error() {
        assert!(from_claude(&json!({"stop_reason": "refusal", "content": []})).is_err());
        let r = from_claude(&json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "OK"}]})).unwrap();
        assert_eq!(r["candidates"][0]["content"]["parts"][0]["text"], "OK");
    }

    /// Run `sse` through the stream reader into `acc`, collecting the streamed text.
    fn feed(sse: &str, mut handle: impl FnMut(Value, &mut dyn FnMut(&str) -> bool) -> Result<bool, String>) -> String {
        let mut text = String::new();
        read_sse(sse.as_bytes(), &mut |v| {
            handle(v, &mut |t| {
                text.push_str(t);
                true
            })
        })
        .unwrap();
        text
    }

    #[test]
    fn claude_stream_assembles_blocks() {
        let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n\
            data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hm\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n\
            data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Run\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"ning.\"}}\n\n\
            data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"run_drc\",\"input\":{}}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cell\\\":\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a\\\"}\"}}\n\n\
            data: {\"type\":\"content_block_stop\",\"index\":2}\n\n\
            data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n\
            data: {\"type\":\"message_stop\"}\n\n";
        let mut acc = ClaudeAcc::default();
        let text = feed(sse, |v, f| acc.event(&v, f));
        assert_eq!(text, "Running.");
        let r = from_claude(&acc.finish()).unwrap();
        let parts = &r["candidates"][0]["content"]["parts"];
        assert_eq!(parts[0]["text"], "Running.");
        assert_eq!(parts[1]["functionCall"]["args"]["cell"], "a");
        assert_eq!(parts[2]["raw"]["message"][0]["signature"], "sig");
        assert_eq!(parts[2]["raw"]["message"][0]["thinking"], "hm");
        assert_eq!(r["candidates"][0]["finishReason"], "tool_use");
    }

    #[test]
    fn openai_stream_joins_tool_call_pieces() {
        let sse = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hi \"}}]}\r\n\r\n\
            data: {\"choices\":[{\"delta\":{\"content\":\"there\"}}]}\n\n\
            data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"run_drc\",\"arguments\":\"{\\\"ce\"}}]}}]}\n\n\
            data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ll\\\":\\\"b\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
            data: [DONE]\n\n";
        let mut acc = OpenAiAcc::default();
        let text = feed(sse, |v, f| acc.chunk(&v, f));
        assert_eq!(text, "Hi there");
        let r = from_openai(&acc.finish(), "openai").unwrap();
        let parts = &r["candidates"][0]["content"]["parts"];
        assert_eq!(parts[0]["text"], "Hi there");
        assert_eq!(parts[1]["functionCall"]["id"], "c1");
        assert_eq!(parts[1]["functionCall"]["args"]["cell"], "b");
    }

    #[test]
    fn gemini_stream_merges_text_and_keeps_signatures() {
        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"think\",\"thought\":true}]}}]}\n\n\
            data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}]}\n\n\
            data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"}]}}]}\n\n\
            data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"run_drc\",\"args\":{}},\"thoughtSignature\":\"s\"}]},\"finishReason\":\"STOP\"}]}\n\n";
        let mut acc = GeminiAcc::default();
        let text = feed(sse, |v, f| acc.chunk(&v, f));
        assert_eq!(text, "Hello");
        let r = acc.finish();
        let parts = r["candidates"][0]["content"]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1]["text"], "Hello");
        assert_eq!(parts[2]["thoughtSignature"], "s");
        assert_eq!(r["candidates"][0]["finishReason"], "STOP");
    }

    #[test]
    fn stream_abort_is_cancelled() {
        let mut acc = OpenAiAcc::default();
        let r = read_sse("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n".as_bytes(), &mut |v| {
            acc.chunk(&v, &mut |_| false)
        });
        assert_eq!(r, Err(CANCELLED.to_string()));
    }
}
