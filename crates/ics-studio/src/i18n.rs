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
    MAP.get_or_init(|| crate::i18n_ko::KO.iter().copied().collect())
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
    for (k, v) in crate::i18n_ko::TEMPLATES {
        if let Some(args) = match_template(k, en) {
            let mut out = String::new();
            let mut rest = *v;
            for a in &args {
                if let Some((head, tail)) = rest.split_once("{}") {
                    out.push_str(head);
                    out.push_str(a);
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
        set(Lang::En);
        assert_eq!(tf("Derive a new design from {}", &["ota5t"]), "Derive a new design from ota5t");
        let keys: Vec<&str> = crate::i18n_ko::KO.iter().map(|(k, _)| *k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "duplicate keys in the Korean table");
        for (en, ko) in crate::i18n_ko::KO.iter().chain(crate::i18n_ko::TEMPLATES) {
            assert_eq!(en.matches("{}").count(), ko.matches("{}").count(), "placeholders of '{en}'");
        }
    }
}
