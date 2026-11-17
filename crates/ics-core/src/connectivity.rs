//! Schematic connectivity extraction: turns pins, wires and labels into named nets.

use std::collections::{BTreeMap, HashMap};

use crate::geom::{Pt, on_segment};
use crate::schematic::{DeviceKind, Schematic};

struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new() -> Self {
        Self { parent: vec![] }
    }
    fn make(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.parent.len() - 1
    }
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.parent[r] != r {
            r = self.parent[r];
        }
        let mut c = x;
        while self.parent[c] != r {
            let n = self.parent[c];
            self.parent[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b] = a;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Connectivity {
    /// (instance id, pin name) -> net name
    pub pin_nets: HashMap<(u64, &'static str), String>,
    /// net name -> wire ids belonging to it
    pub net_wires: BTreeMap<String, Vec<u64>>,
    /// wire id -> net name
    pub wire_net: HashMap<u64, String>,
    /// Points where three or more connections meet (rendered as junction dots).
    pub junctions: Vec<Pt>,
    /// Pins that touch nothing else (for "floating pin" markers).
    pub unconnected_pins: Vec<Pt>,
    /// Warnings found while extracting (e.g. shorted labels).
    pub warnings: Vec<String>,
}

impl Connectivity {
    pub fn net_of(&self, inst: u64, pin: &str) -> Option<&str> {
        self.pin_nets.iter().find(|((i, p), _)| *i == inst && *p == pin).map(|(_, n)| n.as_str())
    }

    pub fn nets(&self) -> Vec<String> {
        let mut v: Vec<String> = self.pin_nets.values().cloned().collect();
        v.sort();
        v.dedup();
        v
    }
}

pub fn extract(sch: &Schematic) -> Connectivity {
    let mut dsu = Dsu::new();
    let mut point_node: HashMap<Pt, usize> = HashMap::new();
    let mut degree: HashMap<Pt, u32> = HashMap::new();
    let mut node = |p: Pt, dsu: &mut Dsu| *point_node.entry(p).or_insert_with(|| dsu.make());

    // Collect all connection points.
    let mut pin_points: Vec<(u64, &'static str, Pt, DeviceKind)> = vec![];
    for inst in &sch.instances {
        for (name, p) in inst.pin_positions() {
            pin_points.push((inst.id, name, p, inst.kind));
            node(p, &mut dsu);
            *degree.entry(p).or_default() += 1;
        }
    }
    for w in &sch.wires {
        let a = node(w.a, &mut dsu);
        let b = node(w.b, &mut dsu);
        dsu.union(a, b);
        *degree.entry(w.a).or_default() += 1;
        *degree.entry(w.b).or_default() += 1;
    }
    for l in &sch.labels {
        node(l.pos, &mut dsu);
    }

    // T-junctions: any connection point lying on a wire's interior joins that wire.
    let points: Vec<Pt> = point_node.keys().copied().collect();
    for w in &sch.wires {
        let wn = point_node[&w.a];
        for &p in &points {
            if p != w.a && p != w.b && on_segment(p, w.a, w.b) {
                dsu.union(wn, point_node[&p]);
                *degree.entry(p).or_default() += 2;
            }
        }
    }

    // Name the nets. Priority: ground > supply symbols > labels > auto.
    let mut root_name: HashMap<usize, (u8, String)> = HashMap::new();
    let mut warnings = vec![];
    let mut assign = |root: usize, prio: u8, name: String, warnings: &mut Vec<String>| match root_name.get(&root) {
        Some((p, existing)) if *p <= prio => {
            if *existing != name && *p == prio && prio > 0 {
                warnings.push(format!("Net names '{existing}' and '{name}' are shorted together"));
            }
        }
        _ => {
            root_name.insert(root, (prio, name));
        }
    };
    for &(id, _, p, kind) in &pin_points {
        let root = dsu.find(point_node[&p]);
        match kind {
            DeviceKind::Ground => assign(root, 0, "0".into(), &mut warnings),
            DeviceKind::Supply => {
                let net = sch.instance(id).map(|i| i.param("net").trim().to_string()).unwrap_or_default();
                let net = if net.is_empty() { "vdd".to_string() } else { net };
                assign(root, 1, net, &mut warnings)
            }
            _ => {}
        }
    }
    for l in &sch.labels {
        let root = dsu.find(point_node[&l.pos]);
        let name = l.name.trim();
        if !name.is_empty() {
            assign(root, 2, name.to_string(), &mut warnings);
        }
    }

    let mut auto = 0;
    let mut auto_names: HashMap<usize, String> = HashMap::new();
    let mut name_of = |root: usize| -> String {
        if let Some((_, n)) = root_name.get(&root) {
            return n.clone();
        }
        auto_names
            .entry(root)
            .or_insert_with(|| {
                auto += 1;
                format!("net{auto}")
            })
            .clone()
    };

    let mut conn = Connectivity { warnings, ..Default::default() };
    for &(id, pin, p, kind) in &pin_points {
        let root = dsu.find(point_node[&p]);
        conn.pin_nets.insert((id, pin), name_of(root));
        if degree.get(&p).copied().unwrap_or(0) <= 1 && !matches!(kind, DeviceKind::Ground | DeviceKind::Supply) {
            // Only flag pins without any wire / other pin / label.
            let labelled = sch.labels.iter().any(|l| l.pos == p);
            if !labelled {
                conn.unconnected_pins.push(p);
            }
        }
    }
    // A MOS without a body pin has its body on the net its `bulk` parameter names, matched
    // to the schematic's named nets ignoring case, with gnd / gnd! meaning ground.
    let mut named: Vec<&str> = root_name.values().map(|(_, n)| n.as_str()).collect();
    named.sort();
    named.dedup();
    for inst in sch.instances.iter().filter(|i| i.kind.implicit_bulk()) {
        let bulk = inst.param("bulk").trim();
        let bulk = if bulk.is_empty() { inst.kind.default_bulk() } else { bulk };
        conn.pin_nets.insert((inst.id, "b"), resolve_bulk(bulk, &named));
    }
    for w in &sch.wires {
        let root = dsu.find(point_node[&w.a]);
        let n = name_of(root);
        conn.net_wires.entry(n.clone()).or_default().push(w.id);
        conn.wire_net.insert(w.id, n);
    }
    conn.junctions = degree.into_iter().filter(|(_, d)| *d >= 3).map(|(p, _)| p).collect();
    conn.junctions.sort();
    conn
}

/// The net a `bulk` parameter names: a named net of the schematic with that name (exact, else
/// ignoring case), ground for `0` / `gnd` / `gnd!`, else the name itself.
pub fn resolve_bulk(bulk: &str, named: &[&str]) -> String {
    if let Some(n) = named.iter().find(|n| **n == bulk).or_else(|| named.iter().find(|n| n.eq_ignore_ascii_case(bulk))) {
        return n.to_string();
    }
    if bulk == "0" || bulk.eq_ignore_ascii_case("gnd") || bulk.eq_ignore_ascii_case("gnd!") {
        return "0".into();
    }
    bulk.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Orient;

    #[test]
    fn rc_divider() {
        let mut s = Schematic::default();
        let r = s.add_instance(DeviceKind::Resistor, Pt::new(0, 0), Orient::default());
        let g = s.add_instance(DeviceKind::Ground, Pt::new(0, 6), Orient::default());
        let c = s.add_instance(DeviceKind::Capacitor, Pt::new(0, 4), Orient::default());
        let _ = g;
        // R.n at (0,2), C.p at (0,2) -> directly connected; C.n at (0,6) is ground.
        s.add_label("out", Pt::new(0, 2));
        let conn = extract(&s);
        assert_eq!(conn.net_of(r, "n"), Some("out"));
        assert_eq!(conn.net_of(c, "p"), Some("out"));
        assert_eq!(conn.net_of(c, "n"), Some("0"));
    }

    #[test]
    fn three_terminal_mos_body_is_its_bulk_net() {
        let mut s = Schematic::default();
        let n = s.add_instance(DeviceKind::Nmos3, Pt::new(0, 0), Orient::default());
        let p = s.add_instance(DeviceKind::Pmos3, Pt::new(10, 0), Orient::default());
        assert_eq!(s.instance(n).unwrap().pin_positions().len(), 3);
        s.instance_mut(p).unwrap().params.insert("bulk".into(), "vpwr".into());
        let conn = extract(&s);
        assert_eq!(conn.net_of(n, "b"), Some("0"));
        assert_eq!(conn.net_of(p, "b"), Some("vpwr"));
    }

    #[test]
    fn body_pin_can_be_hidden_and_shown() {
        let mut s = Schematic::default();
        let m = s.add_instance(DeviceKind::Nmos, Pt::new(0, 0), Orient::default());
        let b = s.instance(m).unwrap().pin_positions()[3].1;
        s.add_label("vsub", b);
        assert!(s.set_body_pin(m, false, None).unwrap());
        assert_eq!(s.instance(m).unwrap().kind, DeviceKind::Nmos3);
        assert_eq!(extract(&s).net_of(m, "b"), Some("vsub"));
        assert!(!s.set_body_pin(m, false, None).unwrap());
        assert!(s.set_body_pin(m, true, None).unwrap());
        assert_eq!(s.instance(m).unwrap().kind, DeviceKind::Nmos);
        assert!(!s.instance(m).unwrap().params.contains_key("bulk"));
        assert_eq!(extract(&s).net_of(m, "b"), Some("vsub"), "the label is still on the pin");

        // The wires to a hidden body go, up to the pin they lead to.
        let n = s.add_instance(DeviceKind::Nmos, Pt::new(40, 0), Orient::default());
        let pins = s.instance(n).unwrap().pin_positions();
        let (src, b) = (pins[2].1, pins[3].1);
        let corner = Pt::new(b.x, src.y);
        s.add_wire(b, corner);
        s.add_wire(corner, src);
        s.add_wire(src, Pt::new(src.x, src.y + 3));
        s.add_instance(DeviceKind::Ground, Pt::new(src.x, src.y + 3), Orient::default());
        let wires = s.wires.len();
        s.set_body_pin(n, false, None).unwrap();
        assert_eq!(s.instance(n).unwrap().param("bulk"), "0");
        assert_eq!(s.wires.len(), wires - 2, "the source wire to ground stays");

        // An unwired body becomes the default rail.
        let p = s.add_instance(DeviceKind::Pmos, Pt::new(20, 0), Orient::default());
        s.set_body_pin(p, false, Some("avdd")).unwrap();
        assert_eq!(s.instance(p).unwrap().param("bulk"), "avdd");

        // Showing the pin again keeps the body on its net through a label.
        s.set_body_pin(p, true, None).unwrap();
        assert_eq!(extract(&s).net_of(p, "b"), Some("avdd"));

        // A body on an unnamed net cannot be hidden.
        let mut s = Schematic::default();
        let m1 = s.add_instance(DeviceKind::Nmos, Pt::new(0, 0), Orient::default());
        let m2 = s.add_instance(DeviceKind::Nmos, Pt::new(20, 0), Orient::default());
        let b1 = s.instance(m1).unwrap().pin_positions()[3].1;
        let s2 = s.instance(m2).unwrap().pin_positions()[2].1;
        s.add_wire(b1, s2);
        assert!(s.set_body_pin(m1, false, None).unwrap_err().contains("no name"));
        assert_eq!(s.instance(m1).unwrap().kind, DeviceKind::Nmos);
    }

    #[test]
    fn bulk_names_match_the_schematic_nets() {
        let mut s = Schematic::default();
        let p = s.add_instance(DeviceKind::Pmos3, Pt::new(0, 0), Orient::default());
        let n = s.add_instance(DeviceKind::Nmos3, Pt::new(20, 0), Orient::default());
        s.add_label("VDD", Pt::new(40, 0));
        s.instance_mut(n).unwrap().params.insert("bulk".into(), "gnd!".into());
        let conn = extract(&s);
        assert_eq!(conn.net_of(p, "b"), Some("VDD"), "vdd matches the VDD label");
        assert_eq!(conn.net_of(n, "b"), Some("0"), "gnd! is ground");
    }

    #[test]
    fn t_junction() {
        let mut s = Schematic::default();
        let r1 = s.add_instance(DeviceKind::Resistor, Pt::new(0, 0), Orient::default());
        s.add_wire(Pt::new(0, 2), Pt::new(10, 2));
        // Second resistor whose top pin sits on the middle of the wire.
        let r2 = s.add_instance(DeviceKind::Resistor, Pt::new(5, 4), Orient::default());
        let conn = extract(&s);
        assert_eq!(conn.net_of(r1, "n"), conn.net_of(r2, "p"));
        assert!(conn.junctions.contains(&Pt::new(5, 2)));
    }
}
