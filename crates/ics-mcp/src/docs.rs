//! The IC Studio user manual (`docs/`, English and Korean) embedded at build time, split into
//! sections and searched by keyword, for the `docs_search` and `docs_read` tools.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::{Value, json};

include!(concat!(env!("OUT_DIR"), "/docs_index.rs"));

/// Characters of section text returned at most.
const MAX_TEXT: usize = 16_000;

#[derive(Debug)]
pub struct Section {
    /// Page key, e.g. `pcells` or `ko/pcells`.
    pub page: &'static str,
    /// The page's `#` title.
    pub title: String,
    pub heading: String,
    /// 1 for the page title, 2 for `##`, ...
    pub level: usize,
    /// mdBook anchor of the heading.
    pub anchor: String,
    pub text: String,
    lower: String,
    lower_heading: String,
}

/// `ko` for pages under `ko/`, else `en`.
pub fn lang_of(page: &str) -> &'static str {
    if page.starts_with("ko/") { "ko" } else { "en" }
}

/// mdBook's heading id: lowercase, letters, digits, `-` and `_`, spaces as `-`.
pub fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// Text without SEO comments and image lines.
fn clean(line: &str) -> Option<&str> {
    let t = line.trim_start();
    (!(t.starts_with("<!--") || t.starts_with("![") || t.starts_with("<img"))).then_some(line)
}

fn split(page: &'static str, text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = vec![];
    let mut title = String::new();
    let mut cur: Option<(String, usize, String)> = None;
    let mut fence = false;
    let push = |cur: Option<(String, usize, String)>, title: &str, out: &mut Vec<Section>| {
        if let Some((heading, level, body)) = cur {
            let text = body.trim().to_string();
            out.push(Section {
                page,
                title: title.to_string(),
                anchor: anchor(&heading),
                lower: text.to_lowercase(),
                lower_heading: heading.to_lowercase(),
                heading,
                level,
                text,
            });
        }
    };
    for line in text.lines().filter_map(clean) {
        if line.trim_start().starts_with("```") {
            fence = !fence;
        }
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if !fence && (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            let heading = line[hashes..].trim().to_string();
            if hashes == 1 && title.is_empty() {
                title = heading.clone();
            }
            push(cur.take(), &title, &mut out);
            cur = Some((heading, hashes, String::new()));
            continue;
        }
        let c = cur.get_or_insert_with(|| (page.to_string(), 1, String::new()));
        c.2.push_str(line);
        c.2.push('\n');
    }
    push(cur, &title, &mut out);
    out
}

pub fn sections() -> &'static [Section] {
    static S: OnceLock<Vec<Section>> = OnceLock::new();
    S.get_or_init(|| PAGES.iter().flat_map(|(p, t)| split(p, t)).collect())
}

fn is_hangul(c: char) -> bool {
    ('\u{ac00}'..='\u{d7a3}').contains(&c) || ('\u{3131}'..='\u{318e}').contains(&c)
}

/// Query words, lowercase; very common English words dropped.
fn words(q: &str) -> Vec<String> {
    const STOP: [&str; 40] = [
        "the",
        "a",
        "an",
        "to",
        "of",
        "in",
        "on",
        "for",
        "and",
        "or",
        "is",
        "do",
        "does",
        "how",
        "i",
        "can",
        "what",
        "where",
        "with",
        "my",
        "it",
        "be",
        "use",
        "ic",
        "studio",
        "방법",
        "방법은",
        "어떻게",
        "하는",
        "하려면",
        "하나요",
        "어디",
        "어디서",
        "무엇",
        "있나요",
        "되나요",
        "알려줘",
        "알려주세요",
        "싶어",
        "싶어요",
    ];
    let mut seen = BTreeSet::new();
    q.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .map(|w| w.trim_matches(|c| c == '-' || c == '.'))
        .filter(|w| w.chars().count() >= 2 || w.chars().any(is_hangul))
        .filter(|w| !STOP.contains(w))
        .filter(|w| seen.insert(w.to_string()))
        .map(str::to_string)
        .collect()
}

/// Forms of a word to look for, the preferred first: Korean words without trailing particles
/// and endings first ("레이아웃에서" as "레이아웃"), English ones as given, then without a
/// plural or -ing.
fn forms(w: &str) -> Vec<String> {
    let chars: Vec<char> = w.chars().collect();
    let mut out = vec![w.to_string()];
    if chars.iter().any(|&c| is_hangul(c)) {
        // Shortest first: the stem is what other sentences share.
        for cut in (1..=2).rev() {
            if chars.len() > cut + 1 {
                out.insert(out.len() - 1, chars[..chars.len() - cut].iter().collect());
            }
        }
    } else {
        for suf in ["ing", "es", "s"] {
            if let Some(s) = w.strip_suffix(suf).filter(|s| s.len() >= 3) {
                out.push(s.to_string());
                break;
            }
        }
    }
    out
}

fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() { 0 } else { hay.matches(needle).count() }
}

/// Up to `width` characters of `text` around the first match of a form of the words.
fn snippet(text: &str, lower: &str, terms: &[Vec<String>], width: usize) -> String {
    let at = terms.iter().flatten().filter_map(|f| lower.find(f.as_str())).min().unwrap_or(0);
    // `lower` and `text` can differ in byte length; map through character counts.
    let ci = lower[..at].chars().count();
    let start = ci.saturating_sub(width / 3);
    let s: String = text.chars().skip(start).take(width).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let pre = if start > 0 { "…" } else { "" };
    let post = if text.chars().count() > start + width { "…" } else { "" };
    format!("{pre}{s}{post}")
}

/// The best sections for `query` in language `lang` (`en`, `ko`, `all`; `None`: Korean when
/// the query has Hangul).
pub fn search(query: &str, lang: Option<&str>, max: usize) -> Value {
    let lang = lang.unwrap_or(if query.chars().any(is_hangul) { "ko" } else { "en" });
    let pool: Vec<&Section> = sections().iter().filter(|s| lang == "all" || lang_of(s.page) == lang).collect();
    let terms: Vec<Vec<String>> = words(query).iter().map(|w| forms(w)).collect();
    if terms.is_empty() {
        return json!({"query": query, "results": [], "note": "no search words; try menu names, tool names or terms"});
    }
    let n = pool.len().max(1) as f64;
    // Per word, the longest form found anywhere, and its inverse document frequency.
    let picked: Vec<(String, f64)> = terms
        .iter()
        .filter_map(|fs| {
            fs.iter().find_map(|f| {
                let df = pool.iter().filter(|s| s.lower.contains(f.as_str()) || s.lower_heading.contains(f.as_str())).count();
                (df > 0).then(|| (f.clone(), (n / (df as f64 + 1.0)).ln() + 1.0))
            })
        })
        .collect();
    // BM25 over the section text, plus a boost for words in the heading or page title.
    let avg = pool.iter().map(|s| s.text.len()).sum::<usize>() as f64 / n;
    let (k1, b) = (1.2, 0.75);
    let mut scored: Vec<(f64, &Section)> = pool
        .iter()
        .filter_map(|s| {
            let norm = k1 * (1.0 - b + b * s.text.len() as f64 / avg.max(1.0));
            let mut score = 0.0;
            let mut hit = 0;
            for (f, idf) in &picked {
                let tf = count(&s.lower, f) as f64;
                let th = (count(&s.lower_heading, f).min(1) + count(&s.title.to_lowercase(), f).min(1)) as f64;
                if tf + th > 0.0 {
                    hit += 1;
                    score += idf * (tf * (k1 + 1.0) / (tf + norm) + 1.5 * th);
                }
            }
            // Sections matching more of the words rank first.
            (hit > 0).then(|| {
                let coverage = hit as f64 / picked.len().max(1) as f64;
                (score * coverage * coverage, *s)
            })
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let results: Vec<Value> = scored
        .iter()
        .take(max.clamp(1, 20))
        .map(|(_, s)| {
            json!({
                "page": s.page,
                "page_title": s.title,
                "section": s.heading,
                "anchor": s.anchor,
                "snippet": snippet(&s.text, &s.lower, &terms, 280),
            })
        })
        .collect();
    let mut out = json!({"query": query, "lang": lang, "results": results});
    if results_empty(&out) {
        out["note"] = json!(
            "nothing found; try other words (menu or tool names), the other language, or docs_read without a page for the list of pages"
        );
    }
    out
}

fn results_empty(v: &Value) -> bool {
    v["results"].as_array().is_none_or(Vec::is_empty)
}

/// Normalise a page argument: `pcells`, `pcells.md`, `docs/ko/pcells.md`, `ko\pcells`.
fn page_key(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    let p = p.strip_prefix("docs/").unwrap_or(&p);
    let p = p.split('#').next().unwrap_or(p);
    p.strip_suffix(".md").unwrap_or(p).trim_matches('/').to_string()
}

/// The pages of a language with their titles.
pub fn pages(lang: &str) -> Value {
    let list: Vec<Value> = sections()
        .iter()
        .filter(|s| s.level == 1 && (lang == "all" || lang_of(s.page) == lang) && !s.page.ends_with("SUMMARY"))
        .map(|s| json!({"page": s.page, "title": s.heading}))
        .collect();
    json!({"lang": lang, "pages": list})
}

/// A page (whole, or its outline when long) or one section with its subsections.
pub fn read(page: &str, section: Option<&str>) -> Result<Value, String> {
    let key = page_key(page);
    let all: Vec<&Section> = sections().iter().filter(|s| s.page == key).collect();
    if all.is_empty() {
        return Err(format!("no manual page '{page}'; docs_read without a page lists them"));
    }
    let outline =
        || all.iter().filter(|s| s.level <= 3).map(|s| format!("{} {}", "#".repeat(s.level), s.heading)).collect::<Vec<_>>();
    let join = |part: &[&Section]| {
        part.iter().map(|s| format!("{} {}\n\n{}", "#".repeat(s.level), s.heading, s.text)).collect::<Vec<_>>().join("\n\n")
    };
    let (start, end) = match section {
        None => (0, all.len()),
        Some(want) => {
            let w = want.trim().trim_start_matches('#').trim().to_lowercase();
            let wa = anchor(&w);
            let i = all
                .iter()
                .position(|s| s.lower_heading == w || s.anchor == wa)
                .or_else(|| all.iter().position(|s| s.lower_heading.contains(&w)))
                .ok_or_else(|| format!("page '{key}' has no section '{want}'; sections: {}", outline().join(" | ")))?;
            let lvl = all[i].level;
            let end = all[i + 1..].iter().position(|s| s.level <= lvl).map_or(all.len(), |k| i + 1 + k);
            (i, end)
        }
    };
    let text = join(&all[start..end]);
    let mut out = json!({"page": key, "title": all[0].title, "lang": lang_of(&key)});
    if section.is_some() {
        out["section"] = json!(all[start].heading);
    }
    if text.chars().count() > MAX_TEXT {
        let cut: String = text.chars().take(MAX_TEXT).collect();
        out["text"] = json!(cut);
        out["truncated"] = json!(true);
        out["outline"] = json!(outline());
        out["note"] = json!("text cut short: read one section at a time (section = a heading from outline)");
    } else {
        out["text"] = json!(text);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_is_embedded_in_both_languages() {
        let keys: Vec<&str> = PAGES.iter().map(|p| p.0).collect();
        for k in ["pcells", "ko/pcells", "layout", "learn/glossary", "shortcuts"] {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }
        assert!(sections().iter().any(|s| s.page == "pcells" && s.heading == "PCell Designer" && s.anchor == "pcell-designer"));
        assert_eq!(anchor("PCell 배치와 변경"), "pcell-배치와-변경");
        assert_eq!(anchor("Placing and changing PCells"), "placing-and-changing-pcells");
    }

    #[test]
    fn finds_sections_in_english_and_korean() {
        let r = search("how do I make a pcell from a drawn layout", None, 5);
        assert_eq!(r["lang"], "en");
        assert!(r["results"].as_array().unwrap().iter().any(|x| x["page"] == "pcells"), "{r}");
        let r = search("레이아웃에서 PCell 배치하는 방법", None, 5);
        assert_eq!(r["lang"], "ko");
        let res = r["results"].as_array().unwrap();
        assert!(res.iter().all(|x| x["page"].as_str().unwrap().starts_with("ko/")), "{r}");
        assert!(res.iter().any(|x| x["page"] == "ko/pcells"), "{r}");
        let r = search("stretch shortcut", Some("en"), 8);
        assert!(r["results"].as_array().unwrap().iter().any(|x| x["page"] == "shortcuts" || x["page"] == "layout"), "{r}");
        assert!(results_empty(&search("zzqqxx", None, 5)));
    }

    #[test]
    fn reads_pages_and_sections() {
        let s = read("docs/pcells.md", Some("pcell designer")).unwrap();
        let t = s["text"].as_str().unwrap();
        assert!(t.starts_with("## PCell Designer"), "{t}");
        assert!(t.contains("### Saving and placing") && !t.contains("## Your own device generators"));
        let k = read("ko/pcells", Some("pcell-designer")).unwrap();
        assert!(k["text"].as_str().unwrap().contains("저장과 배치"));
        assert!(read("pcells", Some("nope")).unwrap_err().contains("sections:"));
        assert!(read("nope", None).is_err());
        let p = pages("ko");
        assert!(p["pages"].as_array().unwrap().iter().any(|x| x["page"] == "ko/pcells"));
        assert!(!read("shortcuts", None).unwrap()["text"].as_str().unwrap().is_empty());
    }
}
