//! UI language. English is the source language: every UI string is written in English and
//! [`t`] looks up its translation for the chosen language, falling back to the English text.
//! Korean needs a system CJK font (see `theme::setup_fonts`); without one the app stays in
//! English, which is why the Korean table is exempt from the bundled-font glyph check.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Ko,
}

impl Lang {
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ko => "한국어",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static CJK: AtomicU8 = AtomicU8::new(0);

/// Record whether a font with Hangul was loaded (Korean is only offered then).
pub fn set_cjk_font(found: bool) {
    CJK.store(found as u8, Ordering::Relaxed);
}

pub fn korean_available() -> bool {
    CJK.load(Ordering::Relaxed) == 1
}

pub fn set(lang: Lang) {
    let lang = if lang == Lang::Ko && !korean_available() { Lang::En } else { lang };
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::Ko } else { Lang::En }
}

fn ko() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| crate::i18n_ko::all().copied().collect())
}

/// The translation of an English UI string (the string itself when there is none).
pub fn t(en: &str) -> &str {
    match current() {
        Lang::En => en,
        Lang::Ko => ko().get(en).copied().unwrap_or(en),
    }
}

/// [`t`] for a template with `{}` placeholders, filled with `args` in order.
pub fn tf(template: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = t(template);
    for a in args {
        match rest.split_once("{}") {
            Some((head, tail)) => {
                out.push_str(head);
                out.push_str(a);
                rest = tail;
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Translate the known sentences inside a longer text (advice assembled from several
/// sentences, multi-paragraph notes).
pub fn t_text(en: &str) -> String {
    if current() == Lang::En {
        return en.to_string();
    }
    if let Some(v) = ko().get(en) {
        return v.to_string();
    }
    for (k, v) in crate::i18n_ko::templates() {
        if let Some(args) = match_template(k, en) {
            let mut out = String::new();
            let mut rest = *v;
            for a in &args {
                if let Some((head, tail)) = rest.split_once("{}") {
                    out.push_str(head);
                    // An argument can be a message of its own ("simulation failed: {}").
                    if a.len() < en.len() {
                        out.push_str(&t_text(a))
                    } else {
                        out.push_str(a)
                    }
                    rest = tail;
                }
            }
            out.push_str(rest);
            return out;
        }
    }
    let mut s = en.to_string();
    for (k, v) in crate::i18n_ko::SENTENCES {
        if s.contains(k) {
            s = s.replace(k, v);
        }
    }
    s
}

/// The texts filling the `{}` of `template` when `text` is that template filled in.
fn match_template<'a>(template: &str, text: &'a str) -> Option<Vec<&'a str>> {
    let parts: Vec<&str> = template.split("{}").collect();
    if parts.len() < 2 {
        return None;
    }
    let mut rest = text.strip_prefix(parts[0])?;
    let mut args = vec![];
    for (k, part) in parts.iter().enumerate().skip(1) {
        let last = k == parts.len() - 1;
        let at = if last {
            if !rest.ends_with(part) {
                return None;
            }
            rest.len() - part.len()
        } else {
            rest.find(part)?
        };
        args.push(&rest[..at]);
        rest = &rest[at + part.len()..];
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_and_templates() {
        set_cjk_font(true);
        set(Lang::Ko);
        assert_eq!(t("Specs (targets)"), "스펙 (목표)");
        assert_eq!(t("no such string"), "no such string");
        assert_eq!(tf("Derive a new design from {}", &["ota5t"]), "ota5t에서 새 설계 파생");
        assert_eq!(t_text("Too slow: raise it."), "Too slow: raise it.");
        assert_eq!(t_text("routed 4 net(s) with 7 connection(s), 16 via(s)"), "넷 4개 배선, 연결 7개, 비아 16개");
        assert_eq!(
            t_text("simulation failed: transient failed at t=1n: singular matrix"),
            "시뮬레이션 실패: 과도 해석이 t=1n에서 실패했습니다: singular matrix"
        );
        assert_eq!(t_text("metal1 width < 0.23 µm"), "metal1 폭 < 0.23 µm");
        set(Lang::En);
        assert_eq!(tf("Derive a new design from {}", &["ota5t"]), "Derive a new design from ota5t");
        let mut seen = std::collections::HashSet::new();
        let dups: Vec<&str> = crate::i18n_ko::all().map(|(k, _)| *k).filter(|k| !seen.insert(*k)).collect();
        assert!(dups.is_empty(), "duplicate keys in the Korean tables: {dups:?}");
        for (en, ko) in crate::i18n_ko::all().chain(crate::i18n_ko::templates()) {
            assert_eq!(en.matches("{}").count(), ko.matches("{}").count(), "placeholders of '{en}'");
        }
    }

    /// Every source file of the app, with its path relative to `src/`.
    fn sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                    out.push((rel, std::fs::read_to_string(&p).unwrap()));
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = vec![];
        walk(&root, &root, &mut out);
        out.retain(|(f, _)| !f.starts_with("i18n"));
        out.sort();
        out
    }

    /// A Rust string literal starting at `s[0] == '"'` (or a raw `r"`/`r#"`): its contents
    /// (escapes kept) and its length in the source.
    fn literal(s: &str) -> Option<(&str, usize)> {
        let b = s.as_bytes();
        if b.first() == Some(&b'r') {
            let hashes = b[1..].iter().take_while(|c| **c == b'#').count();
            let open = 1 + hashes;
            if b.get(open) != Some(&b'"') {
                return None;
            }
            let close = format!("\"{}", "#".repeat(hashes));
            let end = s[open + 1..].find(&close)? + open + 1;
            return Some((&s[open + 1..end], end + close.len()));
        }
        if b.first() != Some(&b'"') {
            return None;
        }
        let mut i = 1;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'"' => return Some((&s[1..i], i + 1)),
                _ => i += 1,
            }
        }
        None
    }

    /// The top-level arguments of the call whose `(` is at `s[0]`.
    fn args(s: &str) -> Vec<&str> {
        let b = s.as_bytes();
        let (mut depth, mut start, mut i, mut out) = (0i32, 1, 0, vec![]);
        while i < b.len() {
            match b[i] {
                b'"' => {
                    i += literal(&s[i..]).map_or(1, |(_, n)| n);
                    continue;
                }
                b'\'' if b.get(i + 2) == Some(&b'\'') || (b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'')) => {
                    i += if b[i + 1] == b'\\' { 4 } else { 3 };
                    continue;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        out.push(s[start..i].trim());
                        return out;
                    }
                }
                b',' if depth == 1 => {
                    out.push(s[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        out
    }

    /// Text a reader sees that is still English: words, not just symbols, units, numbers,
    /// acronyms (DRC, LVS, GDSII) or format placeholders.
    fn needs_translation(text: &str) -> bool {
        let stripped: String = {
            let mut out = String::new();
            let mut depth = 0;
            for c in text.chars() {
                match c {
                    '{' => depth += 1,
                    '}' if depth > 0 => depth -= 1,
                    _ if depth == 0 => out.push(c),
                    _ => {}
                }
            }
            out
        };
        stripped
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|w| w.len() >= 2 && w.chars().any(|c| c.is_ascii_lowercase()) && !NOT_WORDS.contains(&w))
    }

    /// Lower-case letter runs that are units, SI-prefixed quantities or code, not English.
    const NOT_WORDS: &[&str] = &[
        "nm", "um", "mm", "ns", "ps", "fs", "us", "ms", "mV", "uV", "mA", "uA", "nA", "pA", "fF", "pF", "nF", "uF", "dB", "dBm",
        "Hz", "kHz", "MHz", "GHz", "kOhm", "Ohm", "ohm", "sq", "eV", "px", "dbu", "Vth", "Vdd", "vdd", "vss", "gnd", "Id", "Vgs",
        "Vds", "Vbs", "gm", "gds", "Ids", "Cox", "Vt", "kT", "rms", "Vm", "tpd", "tr", "tf", "Ctrl", "Cmd", "Alt", "Shift",
        "Esc", "Tab", "Enter", "Del", "Fn", "Wh", "ln", "log", "sin", "cos", "exp", "sqrt", "abs", "min", "max", "avg", "dx",
        "dy",
    ];

    /// Calls whose string arguments are shown to the user.
    const SINKS: &[&str] = &[
        ".label(",
        ".button(",
        ".small_button(",
        ".heading(",
        ".selectable_label(",
        ".selectable_value(",
        ".radio_value(",
        ".radio(",
        ".checkbox(",
        ".menu_button(",
        ".on_hover_text(",
        ".on_disabled_hover_text(",
        ".hint_text(",
        ".text(",
        ".selected_text(",
        ".toggle_value(",
        ".link(",
        ".hyperlink_to(",
        ".monospace(",
        ".strong(",
        ".weak(",
        ".small(",
        "RichText::new(",
        "Button::new(",
        "Checkbox::new(",
        "SelectableLabel::new(",
        "Label::new(",
        "Window::new(",
        "CollapsingHeader::new(",
        "section(",
        "menu_item(",
        "hint(",
        "ui.collapsing(",
    ];

    /// The English UI text still shown as is: string literals passed straight to egui
    /// widgets, not through `t`/`tf`. A line ending in `// i18n: ignore` is skipped, as is
    /// test code.
    fn untranslated() -> Vec<String> {
        let mut out = vec![];
        for (file, src) in sources() {
            let src = match src.find("#[cfg(test)]") {
                Some(at) => &src[..at],
                None => &src[..],
            };
            for sink in SINKS {
                let mut from = 0;
                while let Some(at) = src[from..].find(sink) {
                    let at = from + at;
                    from = at + sink.len();
                    let prev = src[..at].chars().next_back().unwrap_or(' ');
                    if !sink.starts_with('.') && (prev.is_alphanumeric() || prev == '_') {
                        continue;
                    }
                    let line_no = src[..at].matches('\n').count() + 1;
                    let line = src.lines().nth(line_no - 1).unwrap_or("");
                    if line.trim_end().ends_with("// i18n: ignore") || line.trim_start().starts_with("//") {
                        continue;
                    }
                    for a in args(&src[at + sink.len() - 1..]) {
                        let a = a.trim_start_matches('&');
                        let a = a.strip_prefix("format!(").unwrap_or(a);
                        let Some((text, _)) = literal(a) else { continue };
                        if needs_translation(text) {
                            out.push(format!("{file}:{line_no}: \"{text}\""));
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Every UI string reaches the user in both languages: no English literal goes to a
    /// widget without `t`/`tf`, and every literal given to `t`/`tf` has a Korean entry.
    #[test]
    fn every_ui_string_is_translated() {
        let bare = untranslated();
        let keys: std::collections::HashSet<&str> = crate::i18n_ko::all().map(|(k, _)| *k).collect();
        let mut missing = vec![];
        for (file, src) in sources() {
            let src = match src.find("#[cfg(test)]") {
                Some(at) => &src[..at],
                None => &src[..],
            };
            for call in ["t(", "tf(", "t_text("] {
                let mut from = 0;
                while let Some(at) = src[from..].find(call) {
                    let at = from + at;
                    from = at + call.len();
                    let prev = src[..at].chars().next_back().unwrap_or(' ');
                    if prev.is_alphanumeric() || prev == '_' || prev == '.' || prev == ':' && !src[..at].ends_with("i18n::") {
                        continue;
                    }
                    let Some((text, _)) = literal(&src[from..]) else { continue };
                    let text = text.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\");
                    let text =
                        text.split("\\\n").enumerate().map(|(i, p)| if i == 0 { p } else { p.trim_start() }).collect::<String>();
                    if call != "t_text(" && !keys.contains(text.as_str()) {
                        let line_no = src[..at].matches('\n').count() + 1;
                        missing.push(format!("{file}:{line_no}: \"{text}\""));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            bare.is_empty() && missing.is_empty(),
            "{} UI string(s) not passed through t()/tf():\n{}\n\n{} t()/tf() string(s) without a Korean entry:\n{}",
            bare.len(),
            bare.join("\n"),
            missing.len(),
            missing.join("\n")
        );
    }

    /// The core crates whose messages reach the user (errors, check results, advice).
    const CORE_SOURCES: &[(&str, &str)] = &[
        ("ics-core/lvs.rs", include_str!("../../ics-core/src/lvs.rs")),
        ("ics-core/sch_check.rs", include_str!("../../ics-core/src/sch_check.rs")),
        ("ics-core/derive.rs", include_str!("../../ics-core/src/derive.rs")),
        ("ics-core/drc.rs", include_str!("../../ics-core/src/drc.rs")),
        ("ics-core/netlist.rs", include_str!("../../ics-core/src/netlist.rs")),
        ("ics-core/eco.rs", include_str!("../../ics-core/src/eco.rs")),
        ("ics-sim/derive.rs", include_str!("../../ics-sim/src/derive.rs")),
        ("ics-sim/engine.rs", include_str!("../../ics-sim/src/engine.rs")),
        ("ics-sim/frontend.rs", include_str!("../../ics-sim/src/frontend.rs")),
        ("ics-sim/lib.rs", include_str!("../../ics-sim/src/lib.rs")),
        ("ics-sim/calc.rs", include_str!("../../ics-sim/src/calc.rs")),
        ("ics-sim/optimize.rs", include_str!("../../ics-sim/src/optimize.rs")),
        ("ics-sim/verify.rs", include_str!("../../ics-sim/src/verify.rs")),
    ];

    /// A Rust format string as `t_text` sees the text it makes: every `{…}` argument as `{}`.
    fn as_template(lit: &str) -> String {
        let lit = lit.replace("\\\"", "\"").replace("\\n", "\n").replace("\\\\", "\\");
        let lit: String = lit.split("\\\n").enumerate().map(|(i, p)| if i == 0 { p } else { p.trim_start() }).collect();
        let mut out = String::new();
        let mut chars = lit.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push('}');
                }
                '{' => {
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                    out.push_str("{}");
                }
                c => out.push(c),
            }
        }
        out
    }

    /// The messages of the core crates (`format!`, `Err(..)`, `message: ..` literals with words
    /// in them), as templates.
    fn core_messages() -> Vec<(String, String)> {
        let mut out = vec![];
        for (file, src) in CORE_SOURCES {
            let src = src.split("#[cfg(test)]").next().unwrap_or(src);
            // Text between `// i18n: off` and `// i18n: on` (generated decks and netlists) is
            // not shown to the user as a message.
            let mut kept = String::new();
            let mut rest = src;
            while let Some(off) = rest.find("// i18n: off") {
                kept.push_str(&rest[..off]);
                let after = &rest[off..];
                let on = after.find("// i18n: on").unwrap_or(after.len());
                // Keep the line count, so line numbers stay right.
                kept.extend(after[..on].chars().filter(|c| *c == '\n'));
                rest = &after[on..];
            }
            kept.push_str(rest);
            let src = kept.as_str();
            for lead in ["format!(", "Err(", "message: ", "bail!("] {
                let mut from = 0;
                while let Some(at) = src[from..].find(lead) {
                    let at = from + at + lead.len();
                    from = at;
                    let rest = src[at..].trim_start();
                    let Some((lit, _)) = literal(rest) else { continue };
                    let line_no = src[..at].matches('\n').count() + 1;
                    // The marker ends the literal's line or the line above (rustfmt may wrap).
                    let marked =
                        |n: usize| n >= 1 && src.lines().nth(n - 1).is_some_and(|l| l.trim_end().ends_with("// i18n: ignore"));
                    if marked(line_no) || marked(line_no - 1) {
                        continue;
                    }
                    let tpl = as_template(lit);
                    let words = tpl
                        .replace("{}", " ")
                        .split(|c: char| !c.is_ascii_alphabetic())
                        .filter(|w| w.len() >= 3 && w.chars().any(|c| c.is_ascii_lowercase()))
                        .count();
                    if words >= 1 {
                        out.push((format!("{file}:{line_no}"), tpl));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out.dedup_by(|a, b| a.1 == b.1);
        out
    }

    /// Messages made by the core crates are shown in Korean too: each has a template (or,
    /// without arguments, a sentence) in the Korean tables.
    #[test]
    fn core_messages_are_translated() {
        let keys: std::collections::HashSet<&str> =
            crate::i18n_ko::all().chain(crate::i18n_ko::templates()).chain(crate::i18n_ko::SENTENCES).map(|(k, _)| *k).collect();
        let missing: Vec<String> = core_messages()
            .into_iter()
            .filter(|(_, t)| !keys.contains(t.as_str()))
            .map(|(at, t)| format!("{at}: {t:?}"))
            .collect();
        assert!(missing.is_empty(), "{} core message(s) without Korean:\n{}", missing.len(), missing.join("\n"));
    }
}
