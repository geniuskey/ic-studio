//! Schematic rule checks ("Check and Save"): connectivity, naming and parameter problems found
//! before netlisting.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::connectivity::{Connectivity, extract};
use crate::geom::{Pt, on_segment};
use crate::netlist::spice_name;
use crate::schematic::{DeviceKind, Instance, Schematic};
use crate::units::parse_spice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub at: Option<Pt>,
    /// Ids of the instances involved.
    pub instances: Vec<u64>,
    pub net: Option<String>,
}

impl Finding {
    fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self { severity, code, message: message.into(), at: None, instances: vec![], net: None }
    }

    fn at(mut self, p: Pt) -> Self {
        self.at = Some(p);
        self
    }

    fn inst(mut self, ids: impl IntoIterator<Item = u64>) -> Self {
        self.instances.extend(ids);
        self
    }

    fn net(mut self, net: &str) -> Self {
        self.net = Some(net.to_string());
        self
    }
}

struct Pin<'a> {
    inst: &'a Instance,
    name: &'static str,
    at: Pt,
    net: &'a str,
}

fn is_device(kind: DeviceKind) -> bool {
    kind.spice_prefix().is_some()
}

fn is_mos(kind: DeviceKind) -> bool {
    matches!(kind, DeviceKind::Nmos | DeviceKind::Pmos)
}

fn is_ground(net: &str) -> bool {
    net == "0" || net.eq_ignore_ascii_case("gnd")
}

fn same_net(a: &str, b: &str) -> bool {
    a == b || (is_ground(a) && is_ground(b))
}

struct Ctx<'a> {
    sch: &'a Schematic,
    conn: &'a Connectivity,
    pins: Vec<Pin<'a>>,
    /// Net of each label that touches something.
    label_nets: Vec<(&'a str, Pt, &'a str)>,
    out: Vec<Finding>,
}

impl<'a> Ctx<'a> {
    fn new(sch: &'a Schematic, conn: &'a Connectivity) -> Self {
        let pins: Vec<Pin> = sch
            .instances
            .iter()
            .flat_map(|inst| {
                inst.pin_positions().into_iter().map(move |(name, at)| Pin {
                    inst,
                    name,
                    at,
                    net: conn.pin_nets.get(&(inst.id, name)).map_or("", String::as_str),
                })
            })
            .collect();
        let net_at = |p: Pt| -> Option<&'a str> {
            if let Some(pin) = pins.iter().find(|pin| pin.at == p) {
                return Some(pin.net);
            }
            let w = sch.wires.iter().find(|w| on_segment(p, w.a, w.b))?;
            conn.wire_net.get(&w.id).map(String::as_str)
        };
        let label_nets = sch
            .labels
            .iter()
            .filter(|l| !l.name.trim().is_empty())
            .filter_map(|l| Some((l.name.trim(), l.pos, net_at(l.pos)?)))
            .collect();
        Self { sch, conn, pins, label_nets, out: vec![] }
    }

    fn push(&mut self, f: Finding) {
        self.out.push(f);
    }

    fn pins_of(&self, id: u64) -> Vec<&Pin<'a>> {
        self.pins.iter().filter(|p| p.inst.id == id).collect()
    }

    fn devices(&self) -> impl Iterator<Item = &'a Instance> + use<'a> {
        self.sch.instances.iter().filter(|i| is_device(i.kind))
    }

    /// Nets touched by device pins, with those pins.
    fn device_nets(&self) -> BTreeMap<&'a str, Vec<&Pin<'a>>> {
        let mut m: BTreeMap<&str, Vec<&Pin>> = BTreeMap::new();
        for p in &self.pins {
            if is_device(p.inst.kind) {
                m.entry(p.net).or_default().push(p);
            }
        }
        m
    }

    /// Nets named by a label, ground or supply symbol.
    fn named(&self, net: &str) -> bool {
        is_ground(net)
            || self.label_nets.iter().any(|(_, _, n)| *n == net)
            || self.pins.iter().any(|p| p.net == net && !is_device(p.inst.kind))
    }

    fn bulk_floating(&self, inst: &Instance) -> bool {
        self.pins.iter().any(|p| p.inst.id == inst.id && p.name == "b" && self.conn.unconnected_pins.contains(&p.at))
    }
}

/// Run every schematic check. Errors come first, then findings are ordered by position.
pub fn check(sch: &Schematic) -> Vec<Finding> {
    let conn = extract(sch);
    let mut cx = Ctx::new(sch, &conn);
    unconnected_pins(&mut cx);
    floating_nets(&mut cx);
    dangling_wires(&mut cx);
    names(&mut cx);
    params(&mut cx);
    let has_ground = ground(&mut cx);
    shorted_devices(&mut cx);
    vsource_loops(&mut cx);
    label_conflicts(&mut cx);
    overlapping(&mut cx);
    if has_ground {
        dc_paths(&mut cx);
    }
    let mut out = cx.out;
    out.sort_by(|a, b| {
        (a.severity, a.at.map(|p| (p.y, p.x)), a.code, &a.message).cmp(&(
            b.severity,
            b.at.map(|p| (p.y, p.x)),
            b.code,
            &b.message,
        ))
    });
    out
}

fn unconnected_pins(cx: &mut Ctx) {
    let mut found = vec![];
    for p in cx.pins.iter().filter(|p| is_device(p.inst.kind) && cx.conn.unconnected_pins.contains(&p.at)) {
        let f = if is_mos(p.inst.kind) && p.name == "b" {
            Finding::new(Severity::Info, "unconnected-pin", format!("{}: bulk pin is unconnected (tied to source)", p.inst.name))
        } else {
            Finding::new(Severity::Warning, "unconnected-pin", format!("{}: pin '{}' is unconnected", p.inst.name, p.name))
        };
        found.push(f.at(p.at).inst([p.inst.id]).net(p.net));
    }
    cx.out.extend(found);
}

fn floating_nets(cx: &mut Ctx) {
    let mut found = vec![];
    for (net, pins) in cx.device_nets() {
        if let [p] = pins[..]
            && !cx.conn.unconnected_pins.contains(&p.at)
            && !cx.named(net)
        {
            let msg = format!("Net '{net}' only connects to {} pin '{}'", p.inst.name, p.name);
            found.push(Finding::new(Severity::Warning, "floating-net", msg).at(p.at).inst([p.inst.id]).net(net));
        }
    }
    cx.out.extend(found);
}

fn dangling_wires(cx: &mut Ctx) {
    let sch = cx.sch;
    for w in &sch.wires {
        for e in [w.a, w.b] {
            let touches = cx.pins.iter().any(|p| p.at == e)
                || sch.labels.iter().any(|l| l.pos == e)
                || sch.wires.iter().any(|o| o.id != w.id && on_segment(e, o.a, o.b));
            if !touches {
                let mut f =
                    Finding::new(Severity::Warning, "dangling-wire", format!("Wire end at ({}, {}) is not connected", e.x, e.y))
                        .at(e);
                f.net = cx.conn.wire_net.get(&w.id).cloned();
                cx.push(f);
            }
        }
    }
}

fn bad_name_char(c: char) -> bool {
    !c.is_ascii_graphic() || "()=,;{}[]\"'".contains(c)
}

fn names(cx: &mut Ctx) {
    let mut groups: BTreeMap<String, Vec<&Instance>> = BTreeMap::new();
    for inst in cx.devices() {
        let name = inst.name.trim();
        if name.is_empty() {
            cx.push(
                Finding::new(Severity::Error, "empty-name", format!("{} has no name", inst.kind.label()))
                    .at(inst.pos)
                    .inst([inst.id]),
            );
            continue;
        }
        if inst.name.chars().any(bad_name_char) {
            let msg = format!("Instance name '{}' contains characters SPICE cannot take", inst.name);
            cx.push(Finding::new(Severity::Error, "bad-name", msg).at(inst.pos).inst([inst.id]));
        }
        groups.entry(spice_name(inst).to_ascii_lowercase()).or_default().push(inst);
    }
    for group in groups.values().filter(|g| g.len() > 1) {
        let names: Vec<&str> = group.iter().map(|i| i.name.as_str()).collect();
        let msg = format!("Duplicate instance name: {}", names.join(", "));
        cx.push(Finding::new(Severity::Error, "duplicate-name", msg).at(group[0].pos).inst(group.iter().map(|i| i.id)));
    }
}

fn is_param_ref(v: &str) -> bool {
    (v.len() > 1 && v.starts_with('{') && v.ends_with('}')) || (v.len() > 1 && v.starts_with('\'') && v.ends_with('\''))
}

fn value_ok(v: &str) -> bool {
    is_param_ref(v) || parse_spice(v).is_some()
}

fn params(cx: &mut Ctx) {
    let mut found = vec![];
    for inst in cx.devices() {
        let finding = |code, msg: String| Finding::new(Severity::Error, code, msg).at(inst.pos).inst([inst.id]);
        let mut value = |key: &str, required: bool| {
            let v = inst.param(key).trim();
            if v.is_empty() {
                if required {
                    found.push(finding("missing-param", format!("{}: parameter '{key}' is empty", inst.name)));
                }
            } else if !value_ok(v) {
                found.push(finding("invalid-value", format!("{}: '{key}' value '{v}' is not a number", inst.name)));
            }
        };
        match inst.kind {
            DeviceKind::Resistor => value("r", true),
            DeviceKind::Capacitor => value("c", true),
            DeviceKind::Inductor => value("l", true),
            DeviceKind::Nmos | DeviceKind::Pmos => {
                for key in ["w", "l"] {
                    value(key, true);
                }
                for key in ["m", "ad", "as", "pd", "ps"] {
                    value(key, false);
                }
            }
            DeviceKind::VSource | DeviceKind::ISource => {
                value("dc", false);
                let ac = inst.param("ac").split_whitespace().next().unwrap_or("");
                if !ac.is_empty() && !value_ok(ac) {
                    found.push(finding("invalid-value", format!("{}: 'ac' value '{ac}' is not a number", inst.name)));
                }
                if ["dc", "ac", "tran"].iter().all(|k| inst.param(k).trim().is_empty()) {
                    found.push(finding("missing-param", format!("{}: source has no dc, ac or tran value", inst.name)));
                }
            }
            DeviceKind::Diode | DeviceKind::Ground | DeviceKind::Supply => {}
        }
        if (is_mos(inst.kind) || inst.kind == DeviceKind::Diode) && inst.param("model").trim().is_empty() {
            found.push(finding("missing-param", format!("{}: parameter 'model' is empty", inst.name)));
        }
    }
    cx.out.extend(found);
}

/// Reports `no-ground` and returns whether the schematic has a ground net.
fn ground(cx: &mut Ctx) -> bool {
    if cx.devices().next().is_none() {
        return false;
    }
    let grounded = cx.pins.iter().any(|p| is_ground(p.net)) || cx.label_nets.iter().any(|(n, _, _)| is_ground(n));
    if !grounded {
        cx.push(Finding::new(Severity::Error, "no-ground", "Schematic has devices but no ground"));
    }
    grounded
}

fn two_pin(kind: DeviceKind) -> bool {
    matches!(
        kind,
        DeviceKind::Resistor
            | DeviceKind::Capacitor
            | DeviceKind::Inductor
            | DeviceKind::Diode
            | DeviceKind::VSource
            | DeviceKind::ISource
    )
}

fn shorted_devices(cx: &mut Ctx) {
    let mut found = vec![];
    for inst in cx.devices().filter(|i| two_pin(i.kind)) {
        if let [a, b] = cx.pins_of(inst.id)[..]
            && same_net(a.net, b.net)
        {
            let sev =
                if matches!(inst.kind, DeviceKind::VSource | DeviceKind::ISource) { Severity::Error } else { Severity::Warning };
            let msg = format!("{}: both pins are on net '{}'", inst.name, a.net);
            found.push(Finding::new(sev, "shorted-device", msg).at(inst.pos).inst([inst.id]).net(a.net));
        }
    }
    cx.out.extend(found);
}

fn canonical(net: &str) -> &str {
    if is_ground(net) { "0" } else { net }
}

struct NetSets {
    index: HashMap<String, usize>,
    parent: Vec<usize>,
}

impl NetSets {
    fn new() -> Self {
        Self { index: HashMap::new(), parent: vec![] }
    }

    fn id(&mut self, net: &str) -> usize {
        let n = self.parent.len();
        let i = *self.index.entry(canonical(net).to_string()).or_insert(n);
        if i == n {
            self.parent.push(n);
        }
        i
    }

    fn find(&mut self, net: &str) -> usize {
        let mut r = self.id(net);
        while self.parent[r] != r {
            self.parent[r] = self.parent[self.parent[r]];
            r = self.parent[r];
        }
        r
    }

    /// Returns false if the nets were already joined.
    fn union(&mut self, a: &str, b: &str) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        self.parent[b] = a;
        a != b
    }
}

fn vsource_loops(cx: &mut Ctx) {
    let mut sources: Vec<(&Instance, &str, &str)> = cx
        .devices()
        .filter(|i| i.kind == DeviceKind::VSource)
        .filter_map(|i| match cx.pins_of(i.id)[..] {
            [a, b] if !same_net(a.net, b.net) => Some((i, a.net, b.net)),
            _ => None,
        })
        .collect();
    sources.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    let mut pairs: HashMap<(&str, &str), &Instance> = HashMap::new();
    let mut sets = NetSets::new();
    let mut found = vec![];
    for (inst, a, b) in sources {
        let (ca, cb) = (canonical(a), canonical(b));
        let key = if ca <= cb { (ca, cb) } else { (cb, ca) };
        if let Some(other) = pairs.get(&key) {
            let msg = format!("{} and {} are both connected across '{a}' and '{b}'", other.name, inst.name);
            found.push(Finding::new(Severity::Error, "parallel-vsource", msg).at(inst.pos).inst([other.id, inst.id]));
        } else if !sets.union(a, b) {
            let msg = format!("{} closes a loop of voltage sources", inst.name);
            found.push(Finding::new(Severity::Error, "vsource-loop", msg).at(inst.pos).inst([inst.id]));
        }
        pairs.entry(key).or_insert(inst);
    }
    cx.out.extend(found);
}

fn label_conflicts(cx: &mut Ctx) {
    let mut by_net: BTreeMap<&str, (Pt, BTreeSet<&str>)> = BTreeMap::new();
    for &(name, pos, net) in &cx.label_nets {
        by_net.entry(net).or_insert((pos, BTreeSet::new())).1.insert(name);
    }
    let mut found = vec![];
    for (net, (pos, labels)) in by_net {
        let msg = if labels.len() > 1 {
            let list: Vec<String> = labels.iter().map(|l| format!("'{l}'")).collect();
            format!("Net carries labels {}; they are merged as '{net}'", list.join(", "))
        } else if let Some(l) = labels.first()
            && !same_net(l, net)
        {
            format!("Label '{l}' is overridden by net name '{net}'")
        } else {
            continue;
        };
        found.push(Finding::new(Severity::Warning, "label-conflict", msg).at(pos).net(net));
    }
    cx.out.extend(found);
}

fn overlapping(cx: &mut Ctx) {
    let mut groups: BTreeMap<(Pt, DeviceKind), Vec<&Instance>> = BTreeMap::new();
    for inst in &cx.sch.instances {
        groups.entry((inst.pos, inst.kind)).or_default().push(inst);
    }
    for ((pos, _), group) in groups.into_iter().filter(|(_, g)| g.len() > 1) {
        let names: Vec<&str> = group.iter().map(|i| i.name.as_str()).collect();
        let msg = format!("Instances {} are placed on top of each other", names.join(", "));
        cx.push(Finding::new(Severity::Warning, "overlapping-instances", msg).at(pos).inst(group.iter().map(|i| i.id)));
    }
}

/// Nets that have no DC path to ground through anything but capacitors and current sources.
fn dc_paths(cx: &mut Ctx) {
    let mut sets = NetSets::new();
    for inst in cx.devices() {
        let pins = cx.pins_of(inst.id);
        let net = |name: &str| pins.iter().find(|p| p.name == name).map_or("", |p| p.net);
        match inst.kind {
            DeviceKind::Resistor | DeviceKind::Inductor | DeviceKind::VSource => {
                sets.union(net("p"), net("n"));
            }
            DeviceKind::Diode => {
                sets.union(net("a"), net("k"));
            }
            DeviceKind::Nmos | DeviceKind::Pmos => {
                sets.union(net("d"), net("s"));
                if !cx.bulk_floating(inst) {
                    sets.union(net("s"), net("b"));
                }
            }
            DeviceKind::Capacitor | DeviceKind::ISource | DeviceKind::Ground | DeviceKind::Supply => {}
        }
    }
    let gnd = sets.find("0");
    let mut found = vec![];
    for (net, pins) in cx.device_nets() {
        if pins.len() <= 1 && !cx.named(net) {
            continue;
        }
        if pins.iter().all(|p| is_mos(p.inst.kind) && p.name == "b" && cx.bulk_floating(p.inst)) {
            continue;
        }
        if sets.find(net) != gnd {
            let ids: BTreeSet<u64> = pins.iter().map(|p| p.inst.id).collect();
            let msg = format!("Net '{net}' has no DC path to ground");
            found.push(Finding::new(Severity::Info, "isolated-net", msg).at(pins[0].at).inst(ids).net(net));
        }
    }
    cx.out.extend(found);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Orient;

    const O: Orient = Orient { rot: 0, mirror: false };

    fn set(s: &mut Schematic, id: u64, kv: &[(&str, &str)]) {
        let inst = s.instance_mut(id).unwrap();
        for (k, v) in kv {
            inst.params.insert((*k).into(), (*v).into());
        }
    }

    fn codes(s: &Schematic) -> Vec<(Severity, &'static str)> {
        check(s).into_iter().map(|f| (f.severity, f.code)).collect()
    }

    fn has(s: &Schematic, sev: Severity, code: &str) -> bool {
        check(s).iter().any(|f| f.severity == sev && f.code == code)
    }

    /// V1 driving R1 into ground: V1 at (0,0) spans (0,-2)..(0,2), R1 at (4,0).
    fn clean() -> Schematic {
        let mut s = Schematic::default();
        s.add_instance(DeviceKind::VSource, Pt::new(0, 0), O);
        s.add_instance(DeviceKind::Resistor, Pt::new(4, 0), O);
        s.add_wire(Pt::new(0, -2), Pt::new(4, -2));
        s.add_wire(Pt::new(0, 2), Pt::new(4, 2));
        s.add_instance(DeviceKind::Ground, Pt::new(0, 2), O);
        s.add_label("in", Pt::new(2, -2));
        s
    }

    #[test]
    fn clean_schematic_has_no_findings() {
        assert_eq!(codes(&clean()), []);
    }

    #[test]
    fn unconnected_pins() {
        let mut s = clean();
        let m = s.add_instance(DeviceKind::Nmos, Pt::new(20, 0), O);
        let f = check(&s);
        let pins: Vec<_> = f.iter().filter(|f| f.code == "unconnected-pin").collect();
        assert_eq!(pins.len(), 4);
        assert!(pins.iter().all(|f| f.instances == [m]));
        assert_eq!(pins.iter().filter(|f| f.severity == Severity::Info).count(), 1);
    }

    #[test]
    fn floating_net() {
        let mut s = clean();
        s.add_instance(DeviceKind::Capacitor, Pt::new(20, 0), O);
        s.add_wire(Pt::new(20, -2), Pt::new(20, -6));
        s.add_wire(Pt::new(20, 2), Pt::new(4, 2));
        assert!(has(&s, Severity::Warning, "floating-net"));
        assert!(has(&s, Severity::Warning, "dangling-wire"));
    }

    #[test]
    fn dangling_wire() {
        let mut s = clean();
        s.add_wire(Pt::new(2, -2), Pt::new(2, -8));
        let f: Vec<_> = check(&s).into_iter().filter(|f| f.code == "dangling-wire").collect();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].at, Some(Pt::new(2, -8)));
        assert_eq!(f[0].net.as_deref(), Some("in"));
    }

    #[test]
    fn duplicate_names() {
        let mut s = clean();
        let r = s.add_instance(DeviceKind::Resistor, Pt::new(8, 0), O);
        s.add_wire(Pt::new(4, -2), Pt::new(8, -2));
        s.add_wire(Pt::new(4, 2), Pt::new(8, 2));
        s.instance_mut(r).unwrap().name = "r1".into();
        let f: Vec<_> = check(&s).into_iter().filter(|f| f.code == "duplicate-name").collect();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert_eq!(f[0].instances.len(), 2);
    }

    #[test]
    fn empty_and_bad_names() {
        let mut s = clean();
        let r = s.instances[1].id;
        s.instance_mut(r).unwrap().name = " ".into();
        assert!(has(&s, Severity::Error, "empty-name"));
        s.instance_mut(r).unwrap().name = "R 1".into();
        assert!(has(&s, Severity::Error, "bad-name"));
        s.instance_mut(r).unwrap().name = "R(1)".into();
        assert!(has(&s, Severity::Error, "bad-name"));
    }

    #[test]
    fn missing_and_invalid_params() {
        let mut s = clean();
        let (v, r) = (s.instances[0].id, s.instances[1].id);
        set(&mut s, r, &[("r", "")]);
        assert!(has(&s, Severity::Error, "missing-param"));
        set(&mut s, r, &[("r", "abc")]);
        assert!(has(&s, Severity::Error, "invalid-value"));
        set(&mut s, r, &[("r", "{rload}")]);
        assert_eq!(codes(&s), []);
        set(&mut s, v, &[("dc", ""), ("ac", ""), ("tran", "")]);
        assert!(has(&s, Severity::Error, "missing-param"));
        set(&mut s, v, &[("ac", "1 90")]);
        assert_eq!(codes(&s), []);
    }

    #[test]
    fn mos_params() {
        let mut s = clean();
        let m = s.add_instance(DeviceKind::Nmos, Pt::new(20, 0), O);
        set(&mut s, m, &[("w", ""), ("l", "x"), ("model", "")]);
        let f: Vec<_> = check(&s).into_iter().filter(|f| f.severity == Severity::Error).map(|f| f.code).collect();
        assert_eq!(f.iter().filter(|c| **c == "missing-param").count(), 2, "{f:?}");
        assert!(f.contains(&"invalid-value"));
    }

    #[test]
    fn no_ground() {
        let mut s = clean();
        s.instances.retain(|i| i.kind != DeviceKind::Ground);
        assert!(has(&s, Severity::Error, "no-ground"));
        s.add_label("gnd", Pt::new(2, 2));
        assert!(!has(&s, Severity::Error, "no-ground"));
        assert!(!has(&Schematic::default(), Severity::Error, "no-ground"));
    }

    #[test]
    fn shorted_devices() {
        let mut s = clean();
        let r = s.add_instance(DeviceKind::Resistor, Pt::new(8, 0), O);
        s.add_wire(Pt::new(8, -2), Pt::new(10, -2));
        s.add_wire(Pt::new(10, -2), Pt::new(10, 2));
        s.add_wire(Pt::new(10, 2), Pt::new(8, 2));
        s.add_wire(Pt::new(4, 2), Pt::new(8, 2));
        let f = check(&s);
        assert!(f.iter().any(|f| f.code == "shorted-device" && f.severity == Severity::Warning && f.instances == [r]));
        let v = s.instances[0].id;
        s.wires.push(crate::schematic::Wire { id: 999, a: Pt::new(0, -2), b: Pt::new(0, 2) });
        let f = check(&s);
        assert!(f.iter().any(|f| f.code == "shorted-device" && f.severity == Severity::Error && f.instances == [v]));
    }

    #[test]
    fn parallel_and_looped_sources() {
        let mut s = clean();
        s.add_instance(DeviceKind::VSource, Pt::new(-4, 0), O);
        s.add_wire(Pt::new(-4, -2), Pt::new(0, -2));
        s.add_wire(Pt::new(-4, 2), Pt::new(0, 2));
        assert!(has(&s, Severity::Error, "parallel-vsource"));

        let mut s = clean();
        s.add_instance(DeviceKind::VSource, Pt::new(-4, -4), O);
        s.add_instance(DeviceKind::VSource, Pt::new(-4, 0), O);
        s.add_wire(Pt::new(-4, -6), Pt::new(0, -6));
        s.add_wire(Pt::new(0, -6), Pt::new(0, -2));
        s.add_wire(Pt::new(-4, 2), Pt::new(0, 2));
        let f = check(&s);
        assert!(f.iter().any(|f| f.code == "vsource-loop"), "{f:?}");
        assert!(!f.iter().any(|f| f.code == "parallel-vsource"));
    }

    #[test]
    fn label_conflict() {
        let mut s = clean();
        s.add_label("in2", Pt::new(4, -2));
        let f: Vec<_> = check(&s).into_iter().filter(|f| f.code == "label-conflict").collect();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);

        let mut s = clean();
        s.add_label("out", Pt::new(2, 2));
        assert!(has(&s, Severity::Warning, "label-conflict"));
        let mut s = clean();
        s.add_label("gnd", Pt::new(2, 2));
        assert!(!has(&s, Severity::Warning, "label-conflict"));
    }

    #[test]
    fn overlapping_instances() {
        let mut s = clean();
        s.add_instance(DeviceKind::Resistor, Pt::new(4, 0), O);
        let f = check(&s);
        assert!(f.iter().any(|f| f.code == "overlapping-instances" && f.instances.len() == 2));
    }

    #[test]
    fn isolated_net() {
        let mut s = clean();
        let c = s.add_instance(DeviceKind::Capacitor, Pt::new(8, 0), O);
        let r = s.add_instance(DeviceKind::Resistor, Pt::new(8, 4), O);
        s.add_wire(Pt::new(4, -2), Pt::new(8, -2));
        s.add_wire(Pt::new(8, 6), Pt::new(4, 6));
        s.add_wire(Pt::new(4, 6), Pt::new(4, 2));
        let f: Vec<_> = check(&s).into_iter().filter(|f| f.code == "isolated-net").collect();
        assert!(f.is_empty(), "{f:?}");
        s.instances.retain(|i| i.id != r);
        s.add_instance(DeviceKind::Capacitor, Pt::new(8, 4), O);
        let f: Vec<_> = check(&s).into_iter().filter(|f| f.code == "isolated-net").collect();
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].severity, Severity::Info);
        assert!(f[0].instances.contains(&c));
    }

    #[test]
    fn errors_sort_first() {
        let mut s = clean();
        s.add_wire(Pt::new(2, -2), Pt::new(2, -8));
        let r = s.instances[1].id;
        set(&mut s, r, &[("r", "")]);
        let f = check(&s);
        assert_eq!(f[0].severity, Severity::Error);
        assert!(f.windows(2).all(|w| w[0].severity <= w[1].severity));
    }

    #[test]
    fn demo_cells_have_no_errors() {
        let p = crate::examples::demo_project();
        let mut checked = 0;
        for (r, cell) in p.all_cells() {
            let Some(s) = &cell.schematic else { continue };
            let errors: Vec<_> = check(s).into_iter().filter(|f| f.severity == Severity::Error).collect();
            assert!(errors.is_empty(), "{}: {errors:#?}", r.cell);
            checked += 1;
        }
        assert!(checked > 5);
    }
}
