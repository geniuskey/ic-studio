//! The course's figures (`docs/images/*.svg`) drawn in the app. They are animated with CSS
//! keyframes, which resvg does not run, so [`snapshot`] evaluates the animations at a time
//! and writes the animated values into the SVG as plain attributes before it is rendered.
//! The text in a figure is translated through `i18n_ko::figures` in the Korean UI.
//!
//! The CSS supported is what the figures use: class, id, tag and descendant selectors,
//! `animation` and its longhands (several per element), `@keyframes` on opacity, fill,
//! stroke, stroke-dashoffset and transform, the timing functions `linear`, `ease…`,
//! `cubic-bezier()` and `steps()`, and `transform-origin` with `transform-box`.

use std::collections::HashMap;

type Decls = Vec<(String, String)>;

/// An element's animated values at a time: its style properties and transform matrix.
type Values<'a> = (roxmltree::Node<'a, 'a>, Vec<(String, String)>, Option<String>);

/// Translates a figure text (`None`: keep it).
pub type Translate<'a> = &'a dyn Fn(&str) -> Option<&'static str>;

struct Rule {
    selectors: Vec<Vec<Compound>>,
    decls: Decls,
}

/// `tag.class#id`; `None` parts match anything. Pseudo-classes and attribute selectors are
/// not supported: a compound with one never matches.
#[derive(Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    unsupported: bool,
}

#[derive(Default)]
struct Css {
    rules: Vec<Rule>,
    keyframes: HashMap<String, Vec<(f64, Decls)>>,
}

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        rest = rest[at + 2..].find("*/").map_or("", |e| &rest[at + 2 + e + 2..]);
    }
    out.push_str(rest);
    out
}

/// The text up to the `}` that closes a block whose `{` was just read, and the rest after it.
fn block(s: &str) -> (&str, &str) {
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (&s[..i], &s[i + 1..]);
                }
            }
            _ => {}
        }
    }
    (s, "")
}

fn parse_decls(s: &str) -> Decls {
    split_top(s, ';')
        .into_iter()
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let v = v.trim().trim_end_matches("!important").trim();
            Some((k.trim().to_ascii_lowercase(), v.to_string()))
        })
        .collect()
}

fn parse_compound(s: &str) -> Compound {
    let mut c = Compound::default();
    let mut cur = String::new();
    let mut kind = 't';
    let flush = |kind: char, cur: &mut String, c: &mut Compound| {
        let v = std::mem::take(cur);
        match kind {
            't' if !v.is_empty() && v != "*" => c.tag = Some(v),
            '.' => c.classes.push(v),
            '#' => c.id = Some(v),
            _ => {}
        }
    };
    for ch in s.chars() {
        match ch {
            '.' | '#' => {
                flush(kind, &mut cur, &mut c);
                kind = ch;
            }
            ':' | '[' => {
                c.unsupported = true;
                return c;
            }
            ch => cur.push(ch),
        }
    }
    flush(kind, &mut cur, &mut c);
    c
}

fn parse_css(src: &str) -> Css {
    let src = strip_comments(src);
    let mut css = Css::default();
    let mut rest = src.as_str();
    while let Some(open) = rest.find('{') {
        let prelude = rest[..open].trim();
        let (body, after) = block(&rest[open + 1..]);
        rest = after;
        if let Some(name) = prelude.strip_prefix("@keyframes") {
            let mut frames = vec![];
            let mut inner = body;
            while let Some(o) = inner.find('{') {
                let sel = inner[..o].trim();
                let (decls, after) = block(&inner[o + 1..]);
                inner = after;
                let decls = parse_decls(decls);
                for at in sel.split(',') {
                    let at = match at.trim() {
                        "from" => 0.0,
                        "to" => 100.0,
                        p => p.trim_end_matches('%').trim().parse().unwrap_or(0.0),
                    };
                    frames.push((at / 100.0, decls.clone()));
                }
            }
            frames.sort_by(|a: &(f64, Decls), b| a.0.total_cmp(&b.0));
            css.keyframes.insert(name.trim().to_string(), frames);
        } else if prelude.starts_with('@') {
            // @media (the reduced-motion switch) and the like: not for a still picture.
        } else {
            let selectors =
                prelude.split(',').map(|s| s.split_whitespace().filter(|p| *p != ">").map(parse_compound).collect()).collect();
            css.rules.push(Rule { selectors, decls: parse_decls(body) });
        }
    }
    css
}

/// Split on `sep` outside parentheses.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = vec![];
    let (mut depth, mut start) = (0, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty()).collect()
}

fn matches_compound(c: &Compound, n: roxmltree::Node) -> bool {
    if c.unsupported || !n.is_element() {
        return false;
    }
    if c.tag.as_deref().is_some_and(|t| t != n.tag_name().name()) {
        return false;
    }
    if c.id.as_deref().is_some_and(|id| Some(id) != n.attribute("id")) {
        return false;
    }
    let classes: Vec<&str> = n.attribute("class").unwrap_or("").split_whitespace().collect();
    c.classes.iter().all(|k| classes.contains(&k.as_str()))
}

fn matches(sel: &[Compound], n: roxmltree::Node) -> bool {
    let Some((last, rest)) = sel.split_last() else { return false };
    if !matches_compound(last, n) {
        return false;
    }
    let mut anc = n.parent_element();
    for c in rest.iter().rev() {
        loop {
            let Some(a) = anc else { return false };
            anc = a.parent_element();
            if matches_compound(c, a) {
                break;
            }
        }
    }
    true
}

// ---------- animations ----------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Timing {
    Bezier(f64, f64, f64, f64),
    /// Steps and the jump term: 0 jump-end, 1 jump-start, 2 jump-none, 3 jump-both.
    Steps(u32, u8),
}

impl Timing {
    fn parse(s: &str) -> Option<Timing> {
        let s = s.trim();
        Some(match s {
            "linear" => Timing::Bezier(0.0, 0.0, 1.0, 1.0),
            "ease" => Timing::Bezier(0.25, 0.1, 0.25, 1.0),
            "ease-in" => Timing::Bezier(0.42, 0.0, 1.0, 1.0),
            "ease-out" => Timing::Bezier(0.0, 0.0, 0.58, 1.0),
            "ease-in-out" => Timing::Bezier(0.42, 0.0, 0.58, 1.0),
            "step-start" => Timing::Steps(1, 1),
            "step-end" => Timing::Steps(1, 0),
            _ => {
                let (f, args) = s.strip_suffix(')')?.split_once('(')?;
                let a: Vec<&str> = args.split(',').map(str::trim).collect();
                match f.trim() {
                    "cubic-bezier" if a.len() == 4 => {
                        let n: Vec<f64> = a.iter().filter_map(|x| x.parse().ok()).collect();
                        (n.len() == 4).then(|| Timing::Bezier(n[0], n[1], n[2], n[3]))?
                    }
                    "steps" => {
                        let jump = match a.get(1).copied().unwrap_or("end") {
                            "start" | "jump-start" => 1,
                            "jump-none" => 2,
                            "jump-both" => 3,
                            _ => 0,
                        };
                        Timing::Steps(a[0].parse().ok()?, jump)
                    }
                    _ => return None,
                }
            }
        })
    }

    fn apply(self, x: f64) -> f64 {
        match self {
            Timing::Bezier(x1, y1, x2, y2) => {
                let bez = |t: f64, a: f64, b: f64| 3.0 * (1.0 - t) * (1.0 - t) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t;
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    if bez(mid, x1, x2) < x {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                bez(0.5 * (lo + hi), y1, y2)
            }
            Timing::Steps(n, jump) => {
                let n = n.max(1) as f64;
                let k = (x * n).floor();
                match jump {
                    1 => ((k + 1.0) / n).min(1.0),
                    2 if n > 1.0 => (k / (n - 1.0)).min(1.0),
                    3 => (k + 1.0) / (n + 1.0),
                    _ => k / n,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Anim {
    name: String,
    duration: f64,
    delay: f64,
    /// `f64::INFINITY` for `infinite`.
    iterations: f64,
    timing: Timing,
    alternate: bool,
    backwards: bool,
    forwards: bool,
}

fn seconds(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return ms.parse::<f64>().ok().map(|v| v / 1000.0);
    }
    s.strip_suffix('s')?.parse().ok()
}

/// The animations of an element from its cascaded declarations (the `animation` shorthand
/// resets the longhands, as in CSS).
fn animations(decls: &Decls) -> Vec<Anim> {
    let mut long: HashMap<&str, Vec<String>> = HashMap::new();
    for (k, v) in decls {
        if k == "animation" {
            let mut layers: Vec<HashMap<&str, String>> = vec![];
            for layer in split_top(v, ',') {
                let mut m = HashMap::new();
                let mut times = 0;
                for tok in split_top(layer, ' ') {
                    if let Some(_s) = seconds(tok) {
                        m.insert(if times == 0 { "animation-duration" } else { "animation-delay" }, tok.to_string());
                        times += 1;
                    } else if Timing::parse(tok).is_some() {
                        m.insert("animation-timing-function", tok.to_string());
                    } else if tok == "infinite" || tok.parse::<f64>().is_ok() {
                        m.insert("animation-iteration-count", tok.to_string());
                    } else if matches!(tok, "normal" | "reverse" | "alternate" | "alternate-reverse") {
                        m.insert("animation-direction", tok.to_string());
                    } else if matches!(tok, "none" | "forwards" | "backwards" | "both") && !m.contains_key("animation-fill-mode")
                    {
                        m.insert("animation-fill-mode", tok.to_string());
                    } else if !matches!(tok, "running" | "paused") {
                        m.insert("animation-name", tok.to_string());
                    }
                }
                layers.push(m);
            }
            for key in [
                "animation-name",
                "animation-duration",
                "animation-delay",
                "animation-timing-function",
                "animation-iteration-count",
                "animation-direction",
                "animation-fill-mode",
            ] {
                long.insert(key, layers.iter().map(|l| l.get(key).cloned().unwrap_or_default()).collect());
            }
        } else if let Some(key) = [
            "animation-name",
            "animation-duration",
            "animation-delay",
            "animation-timing-function",
            "animation-iteration-count",
            "animation-direction",
            "animation-fill-mode",
        ]
        .into_iter()
        .find(|key| key == k)
        {
            long.insert(key, split_top(v, ',').into_iter().map(str::to_string).collect());
        }
    }
    let names = long.get("animation-name").cloned().unwrap_or_default();
    let get = |key: &str, i: usize| -> String {
        let l = long.get(key).cloned().unwrap_or_default();
        if l.is_empty() { String::new() } else { l[i % l.len()].clone() }
    };
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_empty() && *n != "none")
        .map(|(i, name)| {
            let fill = get("animation-fill-mode", i);
            let dir = get("animation-direction", i);
            Anim {
                name: name.clone(),
                duration: seconds(&get("animation-duration", i)).unwrap_or(0.0),
                delay: seconds(&get("animation-delay", i)).unwrap_or(0.0),
                iterations: match get("animation-iteration-count", i).as_str() {
                    "infinite" => f64::INFINITY,
                    n => n.parse().unwrap_or(1.0),
                },
                timing: Timing::parse(&get("animation-timing-function", i)).unwrap_or(Timing::Bezier(0.25, 0.1, 0.25, 1.0)),
                alternate: dir.starts_with("alternate"),
                backwards: fill == "backwards" || fill == "both",
                forwards: fill == "forwards" || fill == "both",
            }
        })
        .collect()
}

impl Anim {
    /// The keyframe progress (0..1) at time `t`, `None` when the animation has no effect.
    fn progress(&self, t: f64) -> Option<f64> {
        let tt = t - self.delay;
        if tt < 0.0 {
            return self.backwards.then_some(0.0);
        }
        if self.duration <= 0.0 {
            return self.forwards.then_some(1.0);
        }
        let cycles = tt / self.duration;
        if cycles >= self.iterations {
            if !self.forwards {
                return None;
            }
            let last = (self.iterations.ceil() - 1.0).max(0.0);
            let end = self.iterations - last;
            let odd = self.alternate && last as u64 % 2 == 1;
            return Some(if odd { 1.0 - end } else { end });
        }
        let k = cycles.floor();
        let p = cycles - k;
        Some(if self.alternate && k as u64 % 2 == 1 { 1.0 - p } else { p })
    }
}

// ---------- values ----------

fn color(s: &str) -> Option<[f64; 3]> {
    let s = s.trim();
    let hex = s.strip_prefix('#')?;
    let d = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok().map(|v| if n == 1 { v * 17 } else { v } as f64);
    match hex.len() {
        3 => Some([d(0, 1)?, d(1, 1)?, d(2, 1)?]),
        6 => Some([d(0, 2)?, d(2, 2)?, d(4, 2)?]),
        _ => None,
    }
}

/// A number with an optional unit (`12px`, `-3`, `90deg`, `50%`).
fn number(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let end = s.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e'))).unwrap_or(s.len());
    let end = if s[..end].ends_with('e') { end - 1 } else { end };
    Some((s[..end].parse().ok()?, &s[end..]))
}

/// The functions of a transform list: name and arguments (with their units).
fn transform_list(s: &str) -> Vec<(String, Vec<(f64, String)>)> {
    let mut out = vec![];
    let mut rest = s.trim();
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().to_string();
        let close = rest[open..].find(')').map_or(rest.len(), |c| open + c);
        let args = rest[open + 1..close]
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|a| !a.is_empty())
            .filter_map(|a| number(a).map(|(v, u)| (v, u.to_string())))
            .collect();
        out.push((name, args));
        rest = rest.get(close + 1..).unwrap_or("");
    }
    out
}

fn identity_args(name: &str, n: usize) -> Vec<(f64, String)> {
    let one = name.starts_with("scale");
    vec![(if one { 1.0 } else { 0.0 }, String::new()); n]
}

fn show_transform(list: &[(String, Vec<(f64, String)>)]) -> String {
    list.iter()
        .map(|(n, a)| format!("{n}({})", a.iter().map(|(v, u)| format!("{}{u}", num(*v))).collect::<Vec<_>>().join(", ")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The value of `prop` a fraction `p` of the way from `a` to `b`.
fn interpolate(prop: &str, a: &str, b: &str, p: f64) -> String {
    let lerp = |x: f64, y: f64| x + (y - x) * p;
    if prop == "transform" {
        let (mut ta, mut tb) = (transform_list(a), transform_list(b));
        if ta.is_empty() {
            ta = tb.iter().map(|(n, args)| (n.clone(), identity_args(n, args.len()))).collect();
        }
        if tb.is_empty() {
            tb = ta.iter().map(|(n, args)| (n.clone(), identity_args(n, args.len()))).collect();
        }
        let same = ta.len() == tb.len() && ta.iter().zip(&tb).all(|(x, y)| x.0 == y.0);
        if !same {
            return if p < 0.5 { a } else { b }.to_string();
        }
        let list: Vec<_> = ta
            .iter()
            .zip(&tb)
            .map(|((n, xa), (_, xb))| {
                let len = xa.len().max(xb.len());
                let get = |v: &Vec<(f64, String)>, i: usize| v.get(i).cloned().unwrap_or_else(|| identity_args(n, 1)[0].clone());
                let args = (0..len).map(|i| {
                    let (va, ua) = get(xa, i);
                    let (vb, ub) = get(xb, i);
                    (lerp(va, vb), if ua.is_empty() { ub } else { ua })
                });
                (n.clone(), args.collect())
            })
            .collect();
        return show_transform(&list);
    }
    if let (Some(ca), Some(cb)) = (color(a), color(b)) {
        let c: Vec<u8> = (0..3).map(|i| lerp(ca[i], cb[i]).round().clamp(0.0, 255.0) as u8).collect();
        return format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    }
    if let (Some((va, ua)), Some((vb, ub))) = (number(a), number(b))
        && (ua.is_empty() || ub.is_empty() || ua == ub)
    {
        return format!("{}{}", num(lerp(va, vb)), if ua.is_empty() { ub } else { ua });
    }
    if p < 0.5 { a } else { b }.to_string()
}

fn default_value(prop: &str) -> &'static str {
    match prop {
        "opacity" | "fill-opacity" | "stroke-opacity" => "1",
        "stroke-dashoffset" => "0",
        "transform" => "none",
        "fill" => "#000000",
        _ => "",
    }
}

/// The value of `prop` at keyframe progress `p` of `frames`, from `under` where the
/// keyframes leave it out.
fn keyframe_value(frames: &[(f64, Decls)], prop: &str, p: f64, under: &str, timing: Timing) -> Option<String> {
    let mut pts: Vec<(f64, String, Timing)> = frames
        .iter()
        .filter_map(|(at, d)| {
            let v = d.iter().rev().find(|(k, _)| k == prop)?.1.clone();
            let tf = d
                .iter()
                .rev()
                .find(|(k, _)| k == "animation-timing-function")
                .and_then(|(_, v)| Timing::parse(v))
                .unwrap_or(timing);
            Some((*at, v, tf))
        })
        .collect();
    if pts.is_empty() {
        return None;
    }
    if pts[0].0 > 0.0 {
        pts.insert(0, (0.0, under.to_string(), timing));
    }
    if pts.last().unwrap().0 < 1.0 {
        pts.push((1.0, under.to_string(), timing));
    }
    let k = pts.iter().rposition(|(at, _, _)| *at <= p).unwrap_or(0);
    if k + 1 >= pts.len() {
        return Some(pts[k].1.clone());
    }
    let (a, b) = (&pts[k], &pts[k + 1]);
    let span = b.0 - a.0;
    let local = if span > 0.0 { ((p - a.0) / span).clamp(0.0, 1.0) } else { 1.0 };
    Some(interpolate(prop, &a.1, &b.1, a.2.apply(local)))
}

// ---------- geometry ----------

type Matrix = [f64; 6];

fn mul(a: Matrix, b: Matrix) -> Matrix {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

const ID: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn angle(v: f64, unit: &str) -> f64 {
    match unit {
        "rad" => v,
        "turn" => v * std::f64::consts::TAU,
        _ => v.to_radians(),
    }
}

/// The matrix of a CSS or SVG transform list.
fn matrix(s: &str) -> Matrix {
    let mut m = ID;
    for (name, a) in transform_list(s) {
        let v = |i: usize| a.get(i).map_or(0.0, |x| x.0);
        let f: Matrix = match name.as_str() {
            "translate" => [1.0, 0.0, 0.0, 1.0, v(0), v(1)],
            "translateX" => [1.0, 0.0, 0.0, 1.0, v(0), 0.0],
            "translateY" => [1.0, 0.0, 0.0, 1.0, 0.0, v(0)],
            "scale" => {
                let sy = if a.len() > 1 { v(1) } else { v(0) };
                [v(0), 0.0, 0.0, sy, 0.0, 0.0]
            }
            "scaleX" => [v(0), 0.0, 0.0, 1.0, 0.0, 0.0],
            "scaleY" => [1.0, 0.0, 0.0, v(0), 0.0, 0.0],
            "rotate" => {
                let r = angle(v(0), a.first().map_or("", |x| x.1.as_str()));
                let rot = [r.cos(), r.sin(), -r.sin(), r.cos(), 0.0, 0.0];
                if a.len() == 3 {
                    // SVG rotate(angle cx cy)
                    mul(mul([1.0, 0.0, 0.0, 1.0, v(1), v(2)], rot), [1.0, 0.0, 0.0, 1.0, -v(1), -v(2)])
                } else {
                    rot
                }
            }
            "matrix" if a.len() == 6 => [v(0), v(1), v(2), v(3), v(4), v(5)],
            _ => ID,
        };
        m = mul(m, f);
    }
    m
}

#[derive(Clone, Copy)]
struct BBox {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl BBox {
    fn point(x: f64, y: f64) -> BBox {
        BBox { x0: x, y0: y, x1: x, y1: y }
    }
    fn union(a: Option<BBox>, b: BBox) -> BBox {
        match a {
            None => b,
            Some(a) => BBox { x0: a.x0.min(b.x0), y0: a.y0.min(b.y0), x1: a.x1.max(b.x1), y1: a.y1.max(b.y1) },
        }
    }
    fn transformed(self, m: Matrix) -> BBox {
        let mut out: Option<BBox> = None;
        for (x, y) in [(self.x0, self.y0), (self.x1, self.y0), (self.x0, self.y1), (self.x1, self.y1)] {
            out = Some(BBox::union(out, BBox::point(m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])));
        }
        out.unwrap()
    }
}

/// The points of a path's outline and control polygon.
fn path_points(d: &str) -> Vec<(f64, f64)> {
    let mut toks: Vec<String> = vec![];
    let mut cur = String::new();
    for c in d.chars() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            toks.push(c.to_string());
        } else if c == ',' || c.is_whitespace() || (c == '-' && !cur.is_empty() && !cur.ends_with(['e', 'E'])) {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            if c == '-' {
                cur.push(c);
            }
        } else if c == '.' && cur.contains('.') {
            toks.push(std::mem::take(&mut cur));
            cur.push(c);
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    let mut pts = vec![];
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    let mut cmd = 'M';
    let mut nums: Vec<f64> = vec![];
    let run = |cmd: char, nums: &[f64], x: &mut f64, y: &mut f64, sx: &mut f64, sy: &mut f64, pts: &mut Vec<(f64, f64)>| {
        let rel = cmd.is_ascii_lowercase();
        let (bx, by) = if rel { (*x, *y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' | 'L' | 'T' => {
                *x = bx + nums[0];
                *y = by + nums[1];
                if cmd.eq_ignore_ascii_case(&'M') {
                    (*sx, *sy) = (*x, *y);
                }
            }
            'H' => *x = bx + nums[0],
            'V' => *y = by + nums[0],
            'C' => {
                pts.push((bx + nums[0], by + nums[1]));
                pts.push((bx + nums[2], by + nums[3]));
                *x = bx + nums[4];
                *y = by + nums[5];
            }
            'S' | 'Q' => {
                pts.push((bx + nums[0], by + nums[1]));
                *x = bx + nums[2];
                *y = by + nums[3];
            }
            'A' => {
                *x = bx + nums[5];
                *y = by + nums[6];
            }
            'Z' => (*x, *y) = (*sx, *sy),
            _ => {}
        }
        pts.push((*x, *y));
    };
    let arity = |c: char| match c.to_ascii_uppercase() {
        'M' | 'L' | 'T' => 2,
        'H' | 'V' => 1,
        'C' => 6,
        'S' | 'Q' => 4,
        'A' => 7,
        _ => 0,
    };
    for tok in toks {
        if let Some(c) = tok.chars().next().filter(|c| c.is_ascii_alphabetic() && tok.len() == 1) {
            cmd = c;
            nums.clear();
            if arity(c) == 0 {
                run(c, &[], &mut x, &mut y, &mut sx, &mut sy, &mut pts);
            }
            continue;
        }
        if let Ok(v) = tok.parse() {
            nums.push(v);
        }
        if arity(cmd) > 0 && nums.len() == arity(cmd) {
            run(cmd, &nums, &mut x, &mut y, &mut sx, &mut sy, &mut pts);
            nums.clear();
            // Coordinates after a moveto are linetos.
            cmd = match cmd {
                'M' => 'L',
                'm' => 'l',
                c => c,
            };
        }
    }
    pts
}

/// The bounding box of an element in its own coordinates (children with their transforms).
fn bbox(n: roxmltree::Node) -> Option<BBox> {
    let f = |k: &str| n.attribute(k).and_then(|v| number(v)).map_or(0.0, |x| x.0);
    let pts = |list: Vec<(f64, f64)>| list.into_iter().fold(None, |b, (x, y)| Some(BBox::union(b, BBox::point(x, y))));
    match n.tag_name().name() {
        "rect" | "image" | "use" => Some(BBox { x0: f("x"), y0: f("y"), x1: f("x") + f("width"), y1: f("y") + f("height") }),
        "circle" => Some(BBox { x0: f("cx") - f("r"), y0: f("cy") - f("r"), x1: f("cx") + f("r"), y1: f("cy") + f("r") }),
        "ellipse" => Some(BBox { x0: f("cx") - f("rx"), y0: f("cy") - f("ry"), x1: f("cx") + f("rx"), y1: f("cy") + f("ry") }),
        "line" => pts(vec![(f("x1"), f("y1")), (f("x2"), f("y2"))]),
        "polyline" | "polygon" => {
            let v: Vec<f64> = n
                .attribute("points")
                .unwrap_or("")
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter_map(|s| s.parse().ok())
                .collect();
            pts(v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect())
        }
        "path" => pts(path_points(n.attribute("d").unwrap_or(""))),
        "text" => {
            let size = 12.0;
            let len =
                n.descendants().filter(|c| c.is_text()).map(|c| c.text().unwrap_or("").chars().count()).sum::<usize>() as f64;
            Some(BBox { x0: f("x"), y0: f("y") - size, x1: f("x") + 0.55 * size * len, y1: f("y") })
        }
        "g" | "svg" | "a" => n.children().filter(|c| c.is_element()).fold(None, |b, c| match bbox(c) {
            Some(cb) => Some(BBox::union(b, cb.transformed(c.attribute("transform").map_or(ID, matrix)))),
            None => b,
        }),
        _ => None,
    }
}

/// `transform-origin` in user units: against the element's box (`fill-box`) or the view box.
fn origin(s: &str, frame: BBox) -> (f64, f64) {
    let words: Vec<&str> = s.split_whitespace().collect();
    let axis = |w: &str, horizontal: bool| -> f64 {
        let (lo, len) = if horizontal { (frame.x0, frame.x1 - frame.x0) } else { (frame.y0, frame.y1 - frame.y0) };
        match w {
            "left" | "top" => lo,
            "center" => lo + len / 2.0,
            "right" | "bottom" => lo + len,
            w => match number(w) {
                Some((v, "%")) => lo + len * v / 100.0,
                Some((v, _)) => lo + v,
                None => lo,
            },
        }
    };
    let (mut h, mut v) = (words.first().copied().unwrap_or("0"), words.get(1).copied().unwrap_or("center"));
    if words.len() == 1 && matches!(h, "top" | "bottom") {
        (h, v) = ("center", h);
    } else if words.len() == 1 && !matches!(h, "left" | "right" | "center") {
        v = "center";
    }
    if matches!(h, "top" | "bottom") || matches!(v, "left" | "right") {
        std::mem::swap(&mut h, &mut v);
    }
    (axis(h, true), axis(v, false))
}

// ---------- snapshot ----------

/// What a figure needs to be drawn at any time: its source and the animated elements.
pub struct Figure {
    src: String,
    css: Css,
    /// The length of the animation loop, in seconds (0 for a still figure).
    pub period: f64,
    /// A time showing the most of the figure (most elements visible).
    pub poster: f64,
}

/// The animated (or CSS-transformed) elements: their node index, cascaded declarations and
/// animations.
fn animated<'a>(doc: &'a roxmltree::Document, css: &Css) -> Vec<(roxmltree::Node<'a, 'a>, Decls, Vec<Anim>)> {
    let mut out = vec![];
    for n in doc.descendants().filter(|n| n.is_element()) {
        let mut decls: Decls = vec![];
        for r in &css.rules {
            if r.selectors.iter().any(|s| matches(s, n)) {
                decls.extend(r.decls.iter().cloned());
            }
        }
        if let Some(st) = n.attribute("style") {
            decls.extend(parse_decls(st));
        }
        let anims = animations(&decls);
        let transformed = decls.iter().any(|(k, _)| k == "transform");
        if !anims.is_empty() || transformed {
            out.push((n, decls, anims));
        }
    }
    out
}

impl Figure {
    pub fn new(src: &str) -> Figure {
        let css = roxmltree::Document::parse(src)
            .map(|doc| {
                let style: String = doc.descendants().filter(|n| n.has_tag_name("style")).filter_map(|n| n.text()).collect();
                parse_css(&style)
            })
            .unwrap_or_default();
        let mut fig = Figure { src: src.to_string(), css, period: 0.0, poster: 0.0 };
        let Ok(doc) = roxmltree::Document::parse(src) else { return fig };
        let els = animated(&doc, &fig.css);
        let period = els
            .iter()
            .flat_map(|(_, _, a)| a.iter())
            .map(|a| if a.iterations.is_finite() { a.delay + a.duration * a.iterations } else { a.delay + a.duration })
            .fold(0.0, f64::max);
        let mut poster = 0.0;
        if period > 0.0 {
            // Sample the loop and keep the time where the figure shows the most.
            let mut best = f64::MIN;
            for k in 0..48 {
                let t = period * (k as f64 + 0.5) / 48.0;
                let shown: f64 = fig
                    .values_at(&doc, &els, t)
                    .iter()
                    .filter_map(|(_, props, _)| {
                        props.iter().find(|(k, _)| k == "opacity").and_then(|(_, v)| v.parse::<f64>().ok())
                    })
                    .sum();
                if shown > best + 1e-9 {
                    (best, poster) = (shown, t);
                }
            }
        }
        (fig.period, fig.poster) = (period, poster);
        fig
    }

    /// The animated values of each element at time `t`: (node, style properties, transform).
    fn values_at<'a>(
        &self,
        doc: &roxmltree::Document,
        els: &[(roxmltree::Node<'a, 'a>, Decls, Vec<Anim>)],
        t: f64,
    ) -> Vec<Values<'a>> {
        let view = doc
            .root_element()
            .attribute("viewBox")
            .map(|v| v.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|s| s.parse::<f64>().ok()).collect::<Vec<_>>())
            .filter(|v| v.len() == 4)
            .map_or(BBox { x0: 0.0, y0: 0.0, x1: 100.0, y1: 100.0 }, |v| BBox {
                x0: v[0],
                y0: v[1],
                x1: v[0] + v[2],
                y1: v[1] + v[3],
            });
        els.iter()
            .map(|(n, decls, anims)| {
                let base = |prop: &str| -> String {
                    decls
                        .iter()
                        .rev()
                        .find(|(k, _)| k == prop)
                        .map(|(_, v)| v.clone())
                        .or_else(|| n.attribute(prop).map(str::to_string))
                        .unwrap_or_else(|| default_value(prop).to_string())
                };
                let mut props: Vec<(String, String)> = vec![];
                let mut transform = decls.iter().rev().find(|(k, _)| k == "transform").map(|(_, v)| v.clone());
                for a in anims {
                    let (Some(frames), Some(p)) = (self.css.keyframes.get(&a.name), a.progress(t)) else { continue };
                    for prop in ["opacity", "fill", "stroke", "stroke-dashoffset", "fill-opacity", "stroke-opacity", "transform"]
                    {
                        let under =
                            if prop == "transform" { transform.clone().unwrap_or_else(|| "none".into()) } else { base(prop) };
                        if let Some(v) = keyframe_value(frames, prop, p, &under, a.timing) {
                            if prop == "transform" {
                                transform = Some(v);
                            } else {
                                props.retain(|(k, _)| k != prop);
                                props.push((prop.to_string(), v));
                            }
                        }
                    }
                }
                let transform = transform.map(|tr| {
                    let fill_box =
                        decls.iter().rev().find(|(k, _)| k == "transform-box").is_some_and(|(_, v)| v.contains("fill-box"));
                    let frame = if fill_box { bbox(*n).unwrap_or(view) } else { view };
                    let o = decls.iter().rev().find(|(k, _)| k == "transform-origin").map(|(_, v)| v.as_str()).unwrap_or("0 0");
                    let (ox, oy) = origin(o, if fill_box { frame } else { BBox { x0: 0.0, y0: 0.0, ..view } });
                    let m = mul(mul([1.0, 0.0, 0.0, 1.0, ox, oy], matrix(&tr)), [1.0, 0.0, 0.0, 1.0, -ox, -oy]);
                    format!("matrix({})", m.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "))
                });
                (*n, props, transform)
            })
            .collect()
    }

    /// The figure as a still SVG at time `t` (seconds into the loop; `None` keeps it animated,
    /// for a browser), its text translated by `tr`.
    pub fn snapshot(&self, t: Option<f64>, tr: Translate) -> String {
        let Ok(doc) = roxmltree::Document::parse(&self.src) else { return self.src.clone() };
        let els = if t.is_some() { animated(&doc, &self.css) } else { vec![] };
        let mut edits: Vec<(usize, usize, String)> = vec![];
        for (n, mut props, transform) in self.values_at(&doc, &els, t.unwrap_or(0.0)) {
            let tag_end = n.range().start + 1 + n.tag_name().name().len();
            if transform.is_some() {
                // The origin is in the matrix; resvg would apply the stylesheet's again.
                props.push(("transform-origin".into(), "0 0".into()));
            }
            if !props.is_empty() {
                let add: String = props.iter().map(|(k, v)| format!("{k}:{v};")).collect();
                match n.attribute_node("style") {
                    Some(a) => {
                        let r = a.range_value();
                        edits.push((r.start, r.end, format!("{};{add}", a.value())));
                    }
                    None => edits.push((tag_end, tag_end, format!(" style=\"{add}\""))),
                }
            }
            if let Some(m) = transform {
                match n.attribute_node("transform") {
                    Some(a) => {
                        let r = a.range_value();
                        edits.push((r.start, r.end, m));
                    }
                    None => edits.push((tag_end, tag_end, format!(" transform=\"{m}\""))),
                }
            }
        }
        for n in doc.descendants().filter(|n| n.is_text()) {
            let in_text = n.parent_element().is_some_and(|p| p.has_tag_name("text") || p.has_tag_name("tspan"));
            let raw = n.text().unwrap_or("");
            if let (true, Some(ko)) = (in_text, tr(raw.trim())) {
                let r = n.range();
                let lead = &raw[..raw.len() - raw.trim_start().len()];
                let trail = &raw[raw.trim_end().len()..];
                edits.push((r.start, r.end, format!("{lead}{}{trail}", xml_escape(ko))));
            }
        }
        edits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut out = self.src.clone();
        for (s, e, text) in edits {
            out.replace_range(s..e, &text);
        }
        out
    }
}

/// A number for the SVG, rounded to 1e-4 (no float noise in the output).
fn num(v: f64) -> String {
    let r = (v * 1e4).round() / 1e4;
    if r == 0.0 { "0".into() } else { format!("{r}") }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Every text of a figure a reader sees (trimmed text nodes of `<text>` and `<tspan>`).
pub fn texts(src: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(src) else { return vec![] };
    doc.descendants()
        .filter(|n| n.is_text() && n.parent_element().is_some_and(|p| p.has_tag_name("text") || p.has_tag_name("tspan")))
        .map(|n| n.text().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The fonts figures are drawn with: the app's own (proportional and monospace) and the
/// system CJK font for Korean, as the families the figures ask for.
pub fn fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    use resvg::usvg::fontdb;
    static DB: std::sync::OnceLock<std::sync::Arc<fontdb::Database>> = std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        let defs = eframe::egui::FontDefinitions::default();
        let family = |name: &str, db: &mut fontdb::Database| -> Option<String> {
            let data = defs.font_data.get(name)?;
            let bytes: Vec<u8> = (*data.font).as_ref().to_vec();
            let ids = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(bytes)));
            db.face(*ids.first()?).and_then(|f| f.families.first()).map(|(n, _)| n.clone())
        };
        let sans = family("Ubuntu-Light", &mut db);
        let mono = family("Hack", &mut db);
        if let Some(bytes) = crate::theme::cjk_font_bytes() {
            db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(bytes)));
        }
        if let Some(s) = &sans {
            db.set_sans_serif_family(s.clone());
            db.set_serif_family(s.clone());
        }
        if let Some(m) = &mono {
            db.set_monospace_family(m.clone());
        }
        std::sync::Arc::new(db)
    })
    .clone()
}

/// Rasterise an SVG at `scale` (1 = its own pixel size).
pub fn render(svg: &str, scale: f32) -> Option<eframe::egui::ColorImage> {
    use resvg::{tiny_skia, usvg};
    let opt = usvg::Options { fontdb: fontdb(), font_family: "sans-serif".into(), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &opt).ok()?;
    let size = tree.size();
    let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
    let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    Some(eframe::egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pm.data()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50">
<style>
 .a { animation-iteration-count: infinite; animation-fill-mode: both; }
 .s0, .s1 { animation-duration: 8s; }
 .s0 { animation-name: s0; } .s1 { animation-name: s1; }
 @keyframes s0 { 0%, 40% { opacity: 1; } 50%, 100% { opacity: 0; } }
 @keyframes s1 { 0%, 40% { opacity: 0; } 50%, 100% { opacity: 1; } }
 .grow { transform-box: fill-box; transform-origin: 50% 100%; animation: g 4s linear infinite; }
 @keyframes g { from { transform: scaleY(0); } to { transform: scaleY(1); } }
 @media (prefers-reduced-motion: reduce) { * { animation: none !important; } }
</style>
<text class="s0 a" x="1" y="10">first</text><text class="s1 a" x="1" y="20">second</text>
<rect class="grow" x="10" y="20" width="10" height="20"/>
</svg>"#;

    #[test]
    fn keyframes_at_a_time() {
        let f = Figure::new(SVG);
        assert_eq!(f.period, 8.0);
        let none = |_: &str| None;
        let at1 = f.snapshot(Some(1.0), &none);
        assert!(at1.contains(r#"<text style="opacity:1;" class="s0 a""#), "{at1}");
        assert!(at1.contains(r#"<text style="opacity:0;" class="s1 a""#), "{at1}");
        let at6 = f.snapshot(Some(6.0), &none);
        assert!(at6.contains(r#"style="opacity:0;" class="s0 a""#));
        // Half way through the 4 s growth at t = 2 (and 6): scaled 0.5 about the bottom edge.
        assert!(at6.contains(r#"<rect transform="matrix(1 0 0 0.5 0 20)" style="transform-origin:0 0;""#), "{at6}");
        let ko = |s: &str| (s == "second").then_some("둘째 <2>");
        assert!(f.snapshot(Some(6.0), &ko).contains(">둘째 &lt;2&gt;</text>"));
    }

    #[test]
    fn timing_functions() {
        let lin = Timing::parse("linear").unwrap();
        assert!((lin.apply(0.3) - 0.3).abs() < 1e-6);
        let ease = Timing::parse("ease-in-out").unwrap();
        assert!(ease.apply(0.25) < 0.25 && (ease.apply(0.5) - 0.5).abs() < 1e-3);
        let steps = Timing::parse("steps(2, jump-none)").unwrap();
        assert_eq!((steps.apply(0.2), steps.apply(0.7)), (0.0, 1.0));
        assert_eq!(Timing::parse("steps(4)").unwrap().apply(0.6), 0.5);
        let a = &animations(&parse_decls("animation: blink .7s steps(2, jump-none) infinite"))[0];
        assert_eq!((a.name.as_str(), a.duration, a.iterations), ("blink", 0.7, f64::INFINITY));
        let delayed = &animations(&parse_decls("animation: p 2s 1s both"))[0];
        assert_eq!(delayed.progress(0.5), Some(0.0));
        assert_eq!(delayed.progress(2.0), Some(0.5));
        assert_eq!(delayed.progress(10.0), Some(1.0));
        assert_eq!(interpolate("fill", "#000000", "#ffffff", 0.5), "#808080");
        assert_eq!(interpolate("transform", "translate(0px, 10px)", "translate(10px, 20px)", 0.5), "translate(5px, 15px)");
        assert_eq!(interpolate("transform", "none", "scale(2)", 0.5), "scale(1.5)");
    }

    #[test]
    fn path_box() {
        let p = path_points("M10 10 h20 v5 l-5 5 Z M0,0 C1 2 3 4 5 6");
        let xs: Vec<f64> = p.iter().map(|p| p.0).collect();
        assert!(xs.contains(&30.0) && xs.contains(&25.0) && xs.contains(&5.0));
    }
}
