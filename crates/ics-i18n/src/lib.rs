//! English/Korean text of IC Studio, shared by the desktop app and the `ics` command line.
//! English is the source language: every message is written in English and [`t`] looks up
//! its translation for the chosen language, falling back to the English text. The Korean
//! tables ([`ko`]) are keyed by the English text; messages with arguments by their format
//! string with `{}` for each argument.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

pub mod ko;

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

    /// The language a locale or language name stands for (`ko`, `ko-KR`, `ko_KR.UTF-8`,
    /// `korean`, `en`, …); `None` when it is neither.
    pub fn parse(name: &str) -> Option<Lang> {
        let n = name.trim().to_ascii_lowercase();
        if n.starts_with("ko") {
            Some(Lang::Ko)
        } else if n.starts_with("en") || n == "c" || n == "posix" {
            Some(Lang::En)
        } else {
            None
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Choose the language of every following [`t`], [`tf`] and [`t_text`].
pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::Ko } else { Lang::En }
}

fn whole() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| ko::all().copied().collect())
}

/// Every entry with `{}` (the template tables and whole strings with arguments), the most
/// specific first: the one with the most fixed text wins ("LVS: {} difference(s)" before
/// "{} difference(s)").
fn templated() -> &'static [(&'static str, &'static str)] {
    static LIST: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut v: Vec<_> = ko::templates().chain(ko::all().filter(|(k, _)| k.contains("{}"))).copied().collect();
        v.sort_by_key(|(k, _)| std::cmp::Reverse(k.len() - 2 * k.matches("{}").count()));
        v
    })
}

fn templates() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| ko::templates().chain(ko::all()).copied().collect())
}

/// The translation of an English string (the string itself when there is none).
pub fn t(en: &str) -> &str {
    match current() {
        Lang::En => en,
        Lang::Ko => whole().get(en).copied().unwrap_or(en),
    }
}

/// [`t`] for a template with `{}` placeholders, filled with `args` in order.
pub fn tf(template: &str, args: &[&str]) -> String {
    let ko = match current() {
        Lang::En => template,
        Lang::Ko => templates().get(template).copied().unwrap_or(template),
    };
    fill(ko, args.iter().map(|a| a.to_string()))
}

fn fill(template: &str, args: impl IntoIterator<Item = String>) -> String {
    let mut out = String::new();
    let mut rest = template;
    for a in args {
        match rest.split_once("{}") {
            Some((head, tail)) => {
                out.push_str(head);
                out.push_str(&a);
                rest = tail;
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Fill the translation of `template` with the arguments of `en` (the text `template` made),
/// each translated itself when it is a message of its own ("simulation failed: {}").
fn translate_filled(en: &str, template: &str, ko: &str) -> Option<String> {
    let args = match_template(template, en)?;
    Some(fill(ko, args.into_iter().map(|a| if a.len() < en.len() { t_text(a) } else { a.to_string() })))
}

/// Translate the known sentences inside a longer text (advice assembled from several
/// sentences, multi-paragraph notes).
pub fn t_text(en: &str) -> String {
    if current() == Lang::En {
        return en.to_string();
    }
    if let Some(v) = whole().get(en) {
        return v.to_string();
    }
    for (k, v) in templated() {
        if let Some(out) = translate_filled(en, k, v) {
            return out;
        }
    }
    let mut s = en.to_string();
    for (k, v) in ko::SENTENCES {
        if s.contains(k) {
            s = s.replace(k, v);
        }
    }
    s
}

/// The translation of `text`, made from the format string `template` (written with `{}`
/// for each argument): the Korean template filled with the arguments found in `text`, each
/// translated when it is a message itself. Unlike [`t_text`] it never guesses at other
/// templates, so text without an entry for `template` (data, JSON, netlists) stays as is.
pub fn t_formatted(template: &str, text: &str) -> String {
    if current() == Lang::En || !template.chars().any(|c| c.is_alphabetic()) {
        return text.to_string();
    }
    let Some(ko) = templates().get(template) else { return text.to_string() };
    if !template.contains("{}") {
        return ko.to_string();
    }
    translate_filled(text, template, ko).unwrap_or_else(|| text.to_string())
}

/// A Rust format string as the text it makes is matched: every `{…}` argument as `{}`,
/// `{{` and `}}` as the braces they print.
pub fn as_template(format: &str) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
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
    fn formatted_and_locales() {
        set(Lang::Ko);
        assert_eq!(t_formatted("{}", "{\"a\": 1}"), "{\"a\": 1}");
        assert_eq!(t_formatted("unknown subcircuit '{}'", "unknown subcircuit 'x'"), "알 수 없는 서브회로 'x'");
        assert_eq!(t_formatted("no entry {}", "no entry 3"), "no entry 3");
        assert_eq!(t_text("LVS: 2 difference(s)"), "LVS: 차이 2개");
        set(Lang::En);
        assert_eq!(t_formatted("unknown subcircuit '{}'", "unknown subcircuit 'x'"), "unknown subcircuit 'x'");
        assert_eq!(as_template("a {x:>9.3} {{b}} {}"), "a {} {b} {}");
        assert_eq!(Lang::parse("ko_KR.UTF-8"), Some(Lang::Ko));
        assert_eq!(Lang::parse("en-US"), Some(Lang::En));
        assert_eq!(Lang::parse("de"), None);
    }
}
