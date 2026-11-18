//! Embed the user manual (`docs/**/*.md`) so `docs_search` and `docs_read` work in every build.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let docs = manifest.join("../../docs");
    println!("cargo:rerun-if-changed={}", docs.display());
    let mut files = vec![];
    walk(&docs, &mut files);
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    files.sort();
    let mut s = String::from(
        "/// Manual pages: (path under docs/ without .md, using '/', text).\npub static PAGES: &[(&str, &str)] = &[\n",
    );
    for f in &files {
        let rel = f.strip_prefix(&docs).expect("under docs");
        let key: Vec<String> =
            rel.with_extension("").components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let abs = f.canonicalize().unwrap_or_else(|_| f.clone());
        let _ = writeln!(s, "    ({:?}, include_str!({:?})),", key.join("/"), abs.display().to_string());
    }
    s.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("docs_index.rs");
    std::fs::write(out, s).expect("write docs index");
}
