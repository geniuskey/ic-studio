//! `ics pcell-skill`: a saved PCell Designer master exported as SKILL.

use std::process::Command;

use ics_core::pcell::PCell;
use ics_core::pcell_design::Template;
use ics_core::project::Project;

#[test]
fn exports_a_designer_master() {
    let dir = std::env::temp_dir().join(format!("ics-pcell-skill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = Project::new("t");
    let drawn = PCell::mos(false, 1000, 180, 2).generate(&p.tech).unwrap();
    let mut t = Template::from_layout("pc_nmos", &drawn);
    t.suggest_contact_fills(&p.tech);
    t.suggest_width(&p.tech).unwrap();
    p.publish_template("work", t).unwrap();
    p.save_as(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ics"))
        .args(["pcell-skill", dir.to_str().unwrap(), "work/pc_nmos", "--lib", "vlib"])
        .output()
        .expect("run ics");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("pcDefinePCell(\n  list(ddGetObj(\"vlib\") \"pc_nmos\" \"layout\")"), "{text}");
    assert!(text.contains("(w 1.0)"));
    // A cell that is not a designer master is refused.
    let bad = Command::new(env!("CARGO_BIN_EXE_ics")).args(["pcell-skill", dir.to_str().unwrap(), "work/nope"]).output().unwrap();
    assert!(!bad.status.success());
    std::fs::remove_dir_all(&dir).ok();
}
