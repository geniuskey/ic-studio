//! Language of `ics`: its messages, help and the core crates' errors in English or Korean
//! (tables in `ics-i18n`). The language comes from `--lang`, else `ICS_LANG`, else the
//! system locale. `--json` output, netlists, decks and other data are never translated;
//! neither is the MCP server (`ics mcp`), whose readers are programs.

use ics_i18n::{Lang, t};

/// Pick the language from the command line, the environment and the system locale.
pub fn init(args: &[std::ffi::OsString]) {
    ics_i18n::set(choose(args, std::env::var("ICS_LANG").ok(), sys_locale::get_locale()));
}

fn choose(args: &[std::ffi::OsString], env: Option<String>, locale: Option<String>) -> Lang {
    let mut from_args = None;
    let mut it = args.iter().skip(1).map(|a| a.to_string_lossy());
    while let Some(a) = it.next() {
        if a == "--" {
            break;
        }
        if a == "--lang" {
            from_args = it.next().map(|v| v.to_string());
        } else if let Some(v) = a.strip_prefix("--lang=") {
            from_args = Some(v.to_string());
        }
    }
    [from_args, env, locale].into_iter().flatten().find_map(|n| Lang::parse(&n)).unwrap_or(Lang::En)
}

/// Text printed by `println!`/`eprintln!`/`print!` from the format string `format`: its
/// translation when the table has `format`, else the text as is.
pub fn line(format: &str, text: String) -> String {
    if ics_i18n::current() == Lang::En {
        return text;
    }
    ics_i18n::t_formatted(&ics_i18n::as_template(format), &text)
}

/// An error message for the user: `run`'s errors are mostly the core crates' messages.
pub fn error(e: &str) -> String {
    ics_i18n::tf("error: {}", &[&ics_i18n::t_text(e)])
}

/// The help of every command and argument in the chosen language.
pub fn localize(mut cmd: clap::Command) -> clap::Command {
    if ics_i18n::current() == Lang::En {
        return cmd;
    }
    cmd.build();
    localize_cmd(cmd)
}

fn tr(s: &str) -> String {
    t(s).to_string()
}

fn localize_cmd(mut cmd: clap::Command) -> clap::Command {
    if let Some(a) = cmd.get_about().map(|s| tr(&s.to_string())) {
        cmd = cmd.about(a);
    }
    if let Some(a) = cmd.get_long_about().map(|s| tr(&s.to_string())) {
        cmd = cmd.long_about(a);
    }
    cmd = cmd
        .help_template(format!("{{before-help}}{{about-with-newline}}\n{} {{usage}}\n\n{{all-args}}{{after-help}}", t("Usage:")))
        .subcommand_help_heading(t("Commands"))
        .mut_args(|a| {
            let heading = if a.is_positional() { t("Arguments") } else { t("Options") };
            let mut a = a.help_heading(heading);
            if let Some(h) = a.get_help().map(|s| tr(&s.to_string())) {
                a = a.help(h);
            }
            if let Some(h) = a.get_long_help().map(|s| tr(&s.to_string())) {
                a = a.long_help(h);
            }
            a
        })
        .mut_subcommands(localize_cmd);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_order() {
        let args = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let ko = Some("ko-KR".to_string());
        let en = Some("en_US.UTF-8".to_string());
        assert_eq!(choose(&args(&["ics", "--lang", "en", "sim"]), ko.clone(), ko.clone()), Lang::En);
        assert_eq!(choose(&args(&["ics", "--lang=ko"]), None, en.clone()), Lang::Ko);
        assert_eq!(choose(&args(&["ics", "sim"]), ko.clone(), en.clone()), Lang::Ko);
        assert_eq!(choose(&args(&["ics", "sim"]), Some("xx".into()), ko.clone()), Lang::Ko);
        assert_eq!(choose(&args(&["ics", "sim"]), None, None), Lang::En);
        assert_eq!(choose(&args(&["ics", "--", "--lang", "ko"]), None, en), Lang::En);
    }

    /// A Rust string literal at the start of `s` (escapes kept).
    fn literal(s: &str) -> Option<&str> {
        let b = s.as_bytes();
        if b.first() != Some(&b'"') {
            return None;
        }
        let mut i = 1;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'"' => return Some(&s[1..i]),
                _ => i += 1,
            }
        }
        None
    }

    /// The literal's text as the program sees it (common escapes, `\` line continuations).
    fn unescape(lit: &str) -> String {
        let lit: String = lit.split("\\\n").enumerate().map(|(i, p)| if i == 0 { p } else { p.trim_start() }).collect();
        lit.replace("\\\"", "\"").replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\")
    }

    fn has_words(template: &str) -> bool {
        template
            .replace("{}", " ")
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|w| w.len() >= 3 && w.chars().any(|c| c.is_ascii_lowercase()))
    }

    /// The messages of `main.rs` (printed, formatted or returned as errors) as templates. A
    /// literal on a line ending in `// i18n: ignore` (or the line below one) is skipped, as is
    /// text between `// i18n: off` and `// i18n: on` (generated files, data).
    fn messages() -> Vec<(usize, String)> {
        let src = include_str!("main.rs").replace("\r\n", "\n");
        let mut kept = String::new();
        let mut rest = src.as_str();
        while let Some(off) = rest.find("// i18n: off") {
            kept.push_str(&rest[..off]);
            let after = &rest[off..];
            let on = after.find("// i18n: on").unwrap_or(after.len());
            kept.extend(after[..on].chars().filter(|c| *c == '\n'));
            rest = &after[on..];
        }
        kept.push_str(rest);
        let src = kept.as_str();
        let lines: Vec<&str> = src.lines().collect();
        let marked = |n: usize| n >= 1 && lines.get(n - 1).is_some_and(|l| l.trim_end().ends_with("// i18n: ignore"));
        let mut out = vec![];
        for lead in ["println!(", "print!(", "format!(", "Err("] {
            let mut from = 0;
            while let Some(at) = src[from..].find(lead) {
                let at = from + at + lead.len();
                from = at;
                let Some(lit) = literal(src[at..].trim_start()) else { continue };
                let line_no = src[..at].matches('\n').count() + 1;
                if marked(line_no) || marked(line_no - 1) {
                    continue;
                }
                let tpl = ics_i18n::as_template(&unescape(lit));
                if has_words(&tpl) {
                    out.push((line_no, tpl));
                }
            }
        }
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out.dedup_by(|a, b| a.1 == b.1);
        out.sort();
        out
    }

    /// Every message of `ics` has Korean: its template is in the tables.
    #[test]
    fn every_message_is_translated() {
        let keys: std::collections::HashSet<&str> =
            ics_i18n::ko::all().chain(ics_i18n::ko::templates()).map(|(k, _)| *k).collect();
        let missing: Vec<String> = messages()
            .into_iter()
            .filter(|(_, t)| !keys.contains(t.as_str()))
            .map(|(n, t)| format!("main.rs:{n}: {t:?}"))
            .collect();
        assert!(missing.is_empty(), "{} message(s) of ics without Korean:\n{}", missing.len(), missing.join("\n"));
    }

    /// The help texts of every command and argument, as clap shows them.
    fn help_texts() -> Vec<String> {
        fn walk(cmd: &clap::Command, out: &mut Vec<String>) {
            out.extend(cmd.get_about().map(|s| s.to_string()));
            out.extend(cmd.get_long_about().map(|s| s.to_string()));
            for a in cmd.get_arguments() {
                out.extend(a.get_help().map(|s| s.to_string()));
                out.extend(a.get_long_help().map(|s| s.to_string()));
            }
            for c in cmd.get_subcommands() {
                walk(c, out);
            }
        }
        let mut cmd = <crate::Cli as clap::CommandFactory>::command();
        cmd.build();
        let mut out = vec!["Usage:".to_string(), "Commands".into(), "Arguments".into(), "Options".into()];
        walk(&cmd, &mut out);
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn every_help_text_is_translated() {
        let missing: Vec<String> = help_texts().into_iter().filter(|h| !ics_i18n::ko::all().any(|(k, _)| k == h)).collect();
        assert!(missing.is_empty(), "{} help text(s) of ics without Korean:\n{}", missing.len(), missing.join("\n---\n"));
    }
}
