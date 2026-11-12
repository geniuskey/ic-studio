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
